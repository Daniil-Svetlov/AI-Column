// Базовый класс агента. Конкретные провайдеры: anthropic.js, openai.js, echo.js.

export const MAX_TOOL_STEPS = 5;

export class AgentError extends Error {
  /**
   * @param {string} message
   * @param {{agentId?: string, cause?: unknown, kind?: 'timeout'|'connection'|'auth'|'http'|'aborted'|'other'}} opts
   */
  constructor(message, { agentId, cause, kind = 'other' } = {}) {
    super(message, { cause });
    this.agentId = agentId;
    this.kind = kind;
  }
  get aborted() {
    return this.kind === 'aborted';
  }
}

/** Склеивает историю так, чтобы роли чередовались и первой была реплика пользователя. */
export function normalizeHistory(history, userText) {
  const msgs = [];
  for (const m of history || []) {
    if (!m?.content || (m.role !== 'user' && m.role !== 'assistant')) continue;
    if (msgs.length && msgs[msgs.length - 1].role === m.role) {
      msgs[msgs.length - 1] = { role: m.role, content: `${msgs[msgs.length - 1].content}\n${m.content}` };
    } else {
      msgs.push({ role: m.role, content: String(m.content) });
    }
  }
  while (msgs.length && msgs[0].role !== 'user') msgs.shift();
  if (msgs.length && msgs[msgs.length - 1].role === 'user') {
    msgs[msgs.length - 1] = { role: 'user', content: `${msgs[msgs.length - 1].content}\n${userText}` };
  } else {
    msgs.push({ role: 'user', content: userText });
  }
  return msgs;
}

/** Классифицирует ошибку SDK/сети в AgentError. */
export function toAgentError(err, agentId, signal) {
  if (err instanceof AgentError) return err;
  const name = err?.constructor?.name || err?.name || '';
  const status = err?.status;
  if (signal?.aborted || name === 'APIUserAbortError' || name === 'AbortError') {
    const reason = signal?.reason;
    if (reason instanceof AgentError) return reason;
    return new AgentError('запрос отменён', { agentId, cause: err, kind: 'aborted' });
  }
  if (name === 'APIConnectionTimeoutError' || name === 'TimeoutError') {
    return new AgentError('агент не ответил вовремя', { agentId, cause: err, kind: 'timeout' });
  }
  if (name === 'APIConnectionError' || err?.code === 'ECONNREFUSED' || err?.cause?.code === 'ECONNREFUSED') {
    return new AgentError('сервер агента недоступен', { agentId, cause: err, kind: 'connection' });
  }
  if (status === 401 || status === 403) {
    return new AgentError('неверный API-ключ или нет доступа', { agentId, cause: err, kind: 'auth' });
  }
  if (status) return new AgentError(`ошибка HTTP ${status}: ${err.message}`, { agentId, cause: err, kind: 'http' });
  return new AgentError(err?.message || String(err), { agentId, cause: err });
}

export class Agent {
  constructor(cfg, { logger } = {}) {
    this.cfg = cfg;
    this.id = cfg.id;
    this.name = cfg.name;
    this.color = cfg.color;
    this.description = cfg.description || '';
    this.aliases = cfg.aliases || [];
    this.provider = cfg.provider;
    this.model = cfg.model;
    this.log = logger;
    this.toolsEnabled = cfg.tools !== false;
    this.status = { ok: null, error: null, checkedAt: 0, latencyMs: null };
  }

  get available() {
    return this.status.ok !== false;
  }

  info() {
    return {
      id: this.id,
      name: this.name,
      color: this.color,
      provider: this.provider,
      model: this.model,
      description: this.description,
      aliases: this.aliases,
      available: this.available,
      status: this.status,
    };
  }

  /**
   * Один ход разговора со стримингом текста и вызовом инструментов.
   * @returns {Promise<{text: string, toolCalls: Array<{name: string, args: object, result: string}>}>}
   */
  // eslint-disable-next-line no-unused-vars
  async run({ system, history, userText, toolbox, toolContext, onText, signal }) {
    throw new Error('not implemented');
  }

  /** Короткий ответ без стриминга и инструментов (для классификатора и ask_agent). */
  async complete({ system, userText, maxTokens = 400, signal }) {
    let text = '';
    const res = await this.runGuarded({ system, history: [], userText, toolbox: null, toolContext: null, onText: (d) => { text += d; }, signal, maxTokens });
    return (res?.text ?? text).trim();
  }

  // eslint-disable-next-line no-unused-vars
  async probe(signal) {
    return { ok: true };
  }

  async health() {
    const t0 = Date.now();
    try {
      const r = await this.probe(AbortSignal.timeout(5000));
      this.status = { ok: !!r.ok, error: r.ok ? null : r.error || 'недоступен', checkedAt: Date.now(), latencyMs: Date.now() - t0 };
    } catch (e) {
      const err = toAgentError(e, this.id);
      this.status = { ok: false, error: err.message, checkedAt: Date.now(), latencyMs: Date.now() - t0 };
    }
    return this.status;
  }

  /**
   * Обёртка с таймаутами: «первый токен» и общий. Если ответ не начался вовремя — AgentError(kind=timeout),
   * чтобы роутер мог переключиться на запасного агента.
   */
  async runGuarded(params) {
    const { signal: outer } = params;
    const ctrl = new AbortController();
    const signal = outer ? AbortSignal.any([outer, ctrl.signal]) : ctrl.signal;
    let started = false;
    const firstTimer = setTimeout(() => {
      if (!started) ctrl.abort(new AgentError(`${this.name} не начал отвечать за ${Math.round(this.cfg.first_token_timeout_ms / 1000)} с`, { agentId: this.id, kind: 'timeout' }));
    }, this.cfg.first_token_timeout_ms);
    const totalTimer = setTimeout(() => {
      ctrl.abort(new AgentError(`${this.name}: превышено время ответа`, { agentId: this.id, kind: 'timeout' }));
    }, this.cfg.timeout_ms);
    const markStarted = () => {
      if (!started) {
        started = true;
        clearTimeout(firstTimer);
      }
    };
    try {
      return await this.run({
        ...params,
        signal,
        markStarted,
        onText: (d) => {
          if (d) markStarted();
          params.onText?.(d);
        },
      });
    } catch (e) {
      throw toAgentError(e, this.id, signal);
    } finally {
      clearTimeout(firstTimer);
      clearTimeout(totalTimer);
    }
  }
}
