// Любой OpenAI-совместимый сервер: Ollama, vLLM, llama.cpp, LM Studio, LocalAI,
// а также облачные API (Moonshot/Kimi, OpenRouter, DeepSeek, OpenAI…).
import OpenAI from 'openai';
import { Agent, MAX_TOOL_STEPS, normalizeHistory } from './base.js';
import { ThinkFilter } from '../text/clean.js';

function safeJson(s) {
  if (!s || !String(s).trim()) return {};
  try {
    return JSON.parse(s);
  } catch {
    return {};
  }
}

export class OpenAICompatAgent extends Agent {
  constructor(cfg, deps) {
    super(cfg, deps);
    this.client = new OpenAI({
      apiKey: cfg.api_key || 'none',
      baseURL: cfg.base_url,
      maxRetries: 0,
      timeout: cfg.timeout_ms,
    });
    this.toolsSupported = true; // выключится сам, если сервер не умеет tools
  }

  async probe(signal) {
    const ids = [];
    for await (const m of this.client.models.list({ signal, timeout: 5000 })) {
      ids.push(m.id);
      if (ids.length > 500) break;
    }
    if (ids.length && !ids.some((id) => id === this.model || id.split('/').pop() === this.model || id === `${this.model}:latest`)) {
      return { ok: false, error: `на сервере нет модели «${this.model}» (есть: ${ids.slice(0, 6).join(', ')}${ids.length > 6 ? '…' : ''})` };
    }
    return { ok: true };
  }

  _systemPrompt(system) {
    if (!this.cfg.no_think) return system;
    return `${system}\n/no_think`;
  }

  async _create(body, signal) {
    return this.client.chat.completions.create(body, { signal });
  }

  async run({ system, history, userText, toolbox, toolContext, onText, signal, markStarted, maxTokens }) {
    const messages = [{ role: 'system', content: this._systemPrompt(system) }, ...normalizeHistory(history, userText)];
    const defs = toolbox && this.toolsEnabled ? toolbox.definitions(toolContext) : [];
    const tools = defs.map((d) => ({ type: 'function', function: { name: d.name, description: d.description, parameters: d.parameters } }));
    const filter = new ThinkFilter();
    let fullText = '';
    let pendingSpace = false;
    const toolCalls = [];

    let stepText = '';
    const emit = (raw) => {
      const visible = filter.push(raw);
      if (!visible) return;
      stepText += visible;
      if (pendingSpace && fullText && !/\s$/.test(fullText) && !/^\s/.test(visible)) {
        fullText += ' ';
        onText(' ');
      }
      pendingSpace = false;
      fullText += visible;
      onText(visible);
    };

    for (let step = 0; step < MAX_TOOL_STEPS; step++) {
      const body = {
        model: this.model,
        messages,
        stream: true,
        [this.cfg.max_tokens_param || 'max_tokens']: maxTokens || this.cfg.max_tokens,
        ...(this.cfg.extra_body || {}),
      };
      if (this.cfg.temperature !== undefined) body.temperature = this.cfg.temperature;
      const useTools = tools.length > 0 && this.toolsSupported;
      if (useTools) body.tools = tools;

      let stream;
      try {
        stream = await this._create(body, signal);
      } catch (e) {
        // Сервер не поддерживает function calling — повторяем без инструментов.
        if (useTools && (e?.status === 400 || e?.status === 422) && /tool|function/i.test(String(e?.message))) {
          this.log?.warn(`${this.id}: сервер не принял tools (${e.message}) — работаю без инструментов`);
          this.toolsSupported = false;
          delete body.tools;
          stream = await this._create(body, signal);
        } else {
          throw e;
        }
      }

      const acc = [];
      let finish = null;
      stepText = '';
      for await (const chunk of stream) {
        const choice = chunk?.choices?.[0];
        if (!choice) continue;
        const d = choice.delta || {};
        if (typeof d.content === 'string' && d.content) emit(d.content);
        if (Array.isArray(d.tool_calls)) {
          markStarted?.();
          for (const tc of d.tool_calls) {
            const idx = tc.index ?? acc.length;
            acc[idx] ||= { id: '', name: '', args: '' };
            if (tc.id) acc[idx].id = tc.id;
            if (tc.function?.name) acc[idx].name = tc.function.name;
            if (tc.function?.arguments) acc[idx].args += tc.function.arguments;
          }
        }
        if (choice.finish_reason) finish = choice.finish_reason;
      }
      const tail = filter.flush();
      if (tail) {
        fullText += tail;
        stepText += tail;
        onText(tail);
      }

      const calls = acc.filter((c) => c && c.name);
      if (!calls.length || !toolbox) break;
      if (finish && finish !== 'tool_calls' && finish !== 'function_call' && finish !== 'stop') break;

      calls.forEach((c, i) => { c.id ||= `call_${step}_${i}`; });
      messages.push({
        role: 'assistant',
        content: stepText || null,
        tool_calls: calls.map((c) => ({ id: c.id, type: 'function', function: { name: c.name, arguments: c.args || '{}' } })),
      });
      for (const c of calls) {
        const args = safeJson(c.args);
        const result = await toolbox.execute(c.name, args, toolContext);
        toolCalls.push({ name: c.name, args, result });
        messages.push({ role: 'tool', tool_call_id: c.id, content: String(result) });
      }
      pendingSpace = true;
    }
    return { text: fullText, toolCalls };
  }
}
