// Реестр агентов: создаёт их из конфига и периодически проверяет доступность.
import { AnthropicAgent } from './anthropic.js';
import { OpenAICompatAgent } from './openai.js';
import { EchoAgent } from './echo.js';

const CLASSES = {
  anthropic: AnthropicAgent,
  openai: OpenAICompatAgent,
  echo: EchoAgent,
};

export class AgentRegistry {
  constructor(agentConfigs, { logger } = {}) {
    this.log = logger;
    this.agents = new Map();
    for (const cfg of agentConfigs) {
      const Cls = CLASSES[cfg.provider];
      this.agents.set(cfg.id, new Cls(cfg, { logger: logger?.child?.(cfg.id) ?? logger }));
    }
    this.listeners = new Set();
    this._timer = null;
  }

  get(id) {
    return this.agents.get(id) || null;
  }

  has(id) {
    return this.agents.has(id);
  }

  list() {
    return [...this.agents.values()];
  }

  ids() {
    return [...this.agents.keys()];
  }

  info() {
    return this.list().map((a) => a.info());
  }

  onChange(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  async checkAll() {
    const before = JSON.stringify(this.list().map((a) => [a.id, a.status.ok]));
    await Promise.all(this.list().map(async (a) => {
      const st = await a.health();
      if (st.ok) this.log?.debug(`${a.id}: доступен (${st.latencyMs} мс)`);
      else this.log?.warn(`${a.id}: недоступен — ${st.error}`);
    }));
    const after = JSON.stringify(this.list().map((a) => [a.id, a.status.ok]));
    if (before !== after) for (const fn of this.listeners) fn(this.info());
    return this.info();
  }

  startHealthChecks(intervalMs = 60000) {
    this.stopHealthChecks();
    this.checkAll().catch(() => {});
    this._timer = setInterval(() => this.checkAll().catch(() => {}), intervalMs);
    this._timer.unref?.();
  }

  stopHealthChecks() {
    if (this._timer) clearInterval(this._timer);
    this._timer = null;
  }
}
