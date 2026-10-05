// Выбор агента для реплики: голосовое обращение → выбор на колонке → правила → классификатор → по умолчанию.
// Плюс порядок запасных агентов, если выбранный не ответил.
import { normalize } from '../text/clean.js';

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const END = '(?=$|[\\s,.:;!?…—-])';
const TAIL = '[\\s,.:;!?…—-]*';
const AUTO_WORDS = '(?:авто(?:матически(?:й|е))?(?:\\s+(?:режим|выбор))?|автовыбор|auto)';

export class Router {
  constructor({ registry, routerConfig, logger }) {
    this.registry = registry;
    this.cfg = routerConfig;
    this.log = logger;
    this._build();
  }

  _build() {
    const byWord = new Map();
    for (const a of this.registry.list()) {
      for (const w of [a.id, a.name, ...a.aliases]) {
        const k = normalize(w).trim();
        if (k) byWord.set(k, a.id);
      }
    }
    this.aliasToId = byWord;
    const alt = [...byWord.keys()].sort((x, y) => y.length - x.length).map(escapeRe).join('|');
    this.reAddress = new RegExp(`^\\s*(?:(?:эй|ой|окей|ок|ok|hey|слушай|спроси|спросите|спросить|пусть|а|ну)[\\s,]+)?(${alt})${END}${TAIL}`, 'iu');
    this.reSwitch = new RegExp(
      `^\\s*(?:переключи(?:сь)?|перейди|смени(?:\\s+агента)?|выбери|поставь|давай)\\s+(?:на\\s+|агента\\s+)?(${alt}|${AUTO_WORDS})${END}${TAIL}$`,
      'iu',
    );
    this.reReset = /^\s*(?:новый разговор|начн(?:и|ем) (?:заново|сначала|новый разговор)|забудь (?:все|разговор|контекст)|сбрось (?:контекст|разговор|историю)|очисти (?:историю|контекст))/iu;
  }

  /**
   * @returns {{kind:'reset'} | {kind:'switch', agentId:string} | {kind:'address', agentId:string, text:string} | null}
   */
  parse(text) {
    const n = normalize(text);
    if (this.reReset.test(n)) return { kind: 'reset' };
    if (!this.cfg.voice_prefix) return null;
    const sw = n.match(this.reSwitch);
    if (sw) {
      const word = sw[1].trim();
      const agentId = this.aliasToId.get(word) ?? 'auto';
      return { kind: 'switch', agentId };
    }
    const m = n.match(this.reAddress);
    if (m) {
      const agentId = this.aliasToId.get(m[1].trim());
      const rest = text.slice(m[0].length).trim(); // normalize не меняет длину строки
      if (!rest) return { kind: 'switch', agentId };
      return { kind: 'address', agentId, text: rest };
    }
    return null;
  }

  _available(id) {
    const a = this.registry.get(id);
    return !!a && a.available;
  }

  _defaultAgent() {
    const order = [this.cfg.default_agent, ...this.cfg.fallback, ...this.registry.ids()].filter(Boolean);
    return order.find((id) => this._available(id)) ?? this.cfg.default_agent ?? this.registry.ids()[0];
  }

  async _classify(text, signal) {
    const c = this.cfg.classifier;
    if (!c) return null;
    const judge = this.registry.get(c.agent);
    if (!judge || !judge.available) return null;
    const options = this.registry.list().filter((a) => a.available);
    if (options.length < 2) return null;
    const list = options.map((a) => `- ${a.id}: ${a.description || a.name}`).join('\n');
    const system = `Ты диспетчер голосового помощника. Выбери, какой агент лучше ответит на запрос.\nАгенты:\n${list}\nОтветь одним словом — id агента, без пояснений.`;
    const t0 = Date.now();
    try {
      const timeout = AbortSignal.timeout(c.timeout_ms);
      const out = await judge.complete({ system, userText: text, maxTokens: 24, signal: signal ? AbortSignal.any([signal, timeout]) : timeout });
      const n = normalize(out);
      const hit = options.find((a) => n.includes(a.id.toLowerCase()))
        ?? options.find((a) => [a.name, ...a.aliases].some((w) => n.includes(normalize(w))));
      this.log?.debug(`классификатор (${judge.id}) за ${Date.now() - t0} мс: «${out}» → ${hit?.id ?? '—'}`);
      return hit?.id ?? null;
    } catch (e) {
      this.log?.warn(`классификатор не ответил (${e.message}) — беру агента по умолчанию`);
      return null;
    }
  }

  /**
   * @param {{text:string, deviceAgent?:string, signal?:AbortSignal}} p
   * @returns {Promise<{agentId:string, text:string, reason:string}>}
   */
  async choose({ text, deviceAgent = 'auto', signal }) {
    const parsed = this.parse(text);
    if (parsed?.kind === 'address') return { agentId: parsed.agentId, text: parsed.text, reason: 'voice' };
    if (deviceAgent && deviceAgent !== 'auto' && this.registry.has(deviceAgent)) {
      return { agentId: deviceAgent, text, reason: 'device' };
    }
    const n = normalize(text);
    for (const rule of this.cfg.rules) {
      if (rule.keywords.some((k) => n.includes(k)) && this._available(rule.agent)) {
        return { agentId: rule.agent, text, reason: 'rule' };
      }
    }
    const picked = await this._classify(text, signal);
    if (picked) return { agentId: picked, text, reason: 'classifier' };
    return { agentId: this._defaultAgent(), text, reason: 'default' };
  }

  /** Порядок попыток: выбранный агент, затем запасные (недоступные по health-check пропускаем). */
  candidates(primaryId) {
    const order = [primaryId, ...this.cfg.fallback].filter((id, i, arr) => id && arr.indexOf(id) === i && this.registry.has(id));
    const avail = order.filter((id) => this._available(id));
    if (!avail.length) return order.length ? order : [primaryId];
    return avail;
  }
}
