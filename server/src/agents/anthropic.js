// Claude через Anthropic Messages API (стриминг + инструменты).
import Anthropic from '@anthropic-ai/sdk';
import { Agent, AgentError, MAX_TOOL_STEPS, normalizeHistory } from './base.js';

export class AnthropicAgent extends Agent {
  constructor(cfg, deps) {
    super(cfg, deps);
    this.client = new Anthropic({
      apiKey: cfg.api_key || 'missing',
      baseURL: cfg.base_url || undefined,
      maxRetries: 1,
      timeout: cfg.timeout_ms,
    });
  }

  async probe(signal) {
    if (!this.cfg.api_key) return { ok: false, error: 'не задан api_key (ANTHROPIC_API_KEY)' };
    await this.client.models.list({ limit: 1 }, { signal });
    return { ok: true };
  }

  async run({ system, history, userText, toolbox, toolContext, onText, signal, markStarted, maxTokens }) {
    if (!this.cfg.api_key) throw new AgentError('не задан ANTHROPIC_API_KEY', { agentId: this.id, kind: 'auth' });
    const messages = normalizeHistory(history, userText);
    const defs = toolbox && this.toolsEnabled ? toolbox.definitions(toolContext) : [];
    const tools = defs.map((d) => ({ name: d.name, description: d.description, input_schema: d.parameters }));
    let fullText = '';
    let pendingSpace = false;
    const toolCalls = [];

    for (let step = 0; step < MAX_TOOL_STEPS; step++) {
      const params = {
        model: this.model,
        max_tokens: maxTokens || this.cfg.max_tokens,
        system,
        messages,
      };
      if (tools.length) params.tools = tools;
      if (this.cfg.temperature !== undefined) params.temperature = this.cfg.temperature;

      const stream = this.client.messages.stream(params, { signal });
      stream.on('text', (delta) => {
        if (!delta) return;
        if (pendingSpace && fullText && !/\s$/.test(fullText)) {
          fullText += ' ';
          onText(' ');
        }
        pendingSpace = false;
        fullText += delta;
        onText(delta);
      });
      stream.on('streamEvent', (ev) => {
        if (ev.type === 'content_block_start' && ev.content_block?.type === 'tool_use') markStarted?.();
      });
      const msg = await stream.finalMessage();

      const uses = msg.content.filter((b) => b.type === 'tool_use');
      if (msg.stop_reason !== 'tool_use' || !uses.length || !toolbox) break;

      messages.push({ role: 'assistant', content: msg.content });
      const results = [];
      for (const use of uses) {
        const result = await toolbox.execute(use.name, use.input ?? {}, toolContext);
        toolCalls.push({ name: use.name, args: use.input ?? {}, result });
        results.push({ type: 'tool_result', tool_use_id: use.id, content: String(result) });
      }
      messages.push({ role: 'user', content: results });
      pendingSpace = true;
    }
    return { text: fullText, toolCalls };
  }
}
