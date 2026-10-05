// Загрузка и проверка config.yaml. Секреты берутся из окружения через ${VAR} / ${VAR:-default}.
import fs from 'node:fs';
import path from 'node:path';
import YAML from 'yaml';

export const PROVIDERS = ['anthropic', 'openai', 'echo'];
export const STT_PROVIDERS = ['openai', 'whispercpp', 'mock'];
export const TTS_PROVIDERS = ['piper', 'openai', 'mock'];
export const ALL_TOOLS = ['datetime', 'volume', 'lights', 'timer', 'weather', 'ask_agent', 'switch_agent'];

const DEFAULT_PERSONA = `Ты — голосовой помощник умной колонки по имени {assistant}. Сейчас отвечаешь ты, агент {agent}.
Твой ответ будет озвучен синтезатором речи, поэтому:
- говори по-русски, коротко и по делу: обычно одно-три предложения;
- никакого markdown, списков, таблиц, эмодзи и ссылок;
- числа, даты и единицы пиши словами там, где так проще произнести;
- если вопрос сложный, дай суть и предложи рассказать подробнее.`;

export const DEFAULTS = {
  server: {
    host: '0.0.0.0',
    port: 8080,
    auth_token: '',
    db_path: './data/ai-column.db',
    mdns: true,
    timezone: 'Europe/Moscow',
    log_level: 'info',
    public_dir: './public',
  },
  assistant: {
    name: 'Джарвис',
    language: 'ru',
    persona: DEFAULT_PERSONA,
    history_turns: 8,
    idle_reset_minutes: 15,
    max_utterance_seconds: 20,
  },
  router: {
    default: 'auto',
    default_agent: null,
    voice_prefix: true,
    fallback: [],
    classifier: null,
    rules: [],
  },
  speech: {
    stt: { provider: 'mock', language: 'ru', timeout_ms: 20000 },
    tts: { provider: 'mock', timeout_ms: 20000, speed: 1.0 },
    play_rate: 24000,
  },
  tools: {
    enabled: [...ALL_TOOLS],
    weather: { default_city: 'Москва' },
  },
  agents: [],
};

const AGENT_DEFAULTS = {
  color: '#888888',
  aliases: [],
  description: '',
  tools: true,
  max_tokens: 700,
  temperature: undefined,
  timeout_ms: 60000,
  first_token_timeout_ms: 20000,
  system: '',
  no_think: false,
  extra_body: undefined,
};

/** Подставляет ${VAR} и ${VAR:-default} во все строки объекта. */
export function substituteEnv(value, env = process.env, key = '') {
  if (typeof value === 'string') {
    const whole = value.match(/^\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}$/);
    const replaced = value.replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}/g, (_, name, def) => {
      const v = env[name];
      return v !== undefined && v !== '' ? v : (def ?? '');
    });
    // Целое значение вида "${PORT:-8080}" превращаем в число/булево, если похоже
    // (но не секреты: токен «0123» должен остаться строкой).
    if (whole && !/token|key|secret|password/i.test(key)) {
      if (/^-?\d+(\.\d+)?$/.test(replaced)) return Number(replaced);
      if (replaced === 'true') return true;
      if (replaced === 'false') return false;
    }
    return replaced;
  }
  if (Array.isArray(value)) return value.map((v) => substituteEnv(v, env, key));
  if (value && typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = substituteEnv(v, env, k);
    return out;
  }
  return value;
}

function isPlainObject(v) {
  return v && typeof v === 'object' && !Array.isArray(v);
}

function deepMerge(base, extra) {
  if (!isPlainObject(base) || !isPlainObject(extra)) return extra === undefined ? base : extra;
  const out = { ...base };
  for (const [k, v] of Object.entries(extra)) {
    out[k] = isPlainObject(v) && isPlainObject(base[k]) ? deepMerge(base[k], v) : v;
  }
  return out;
}

export class ConfigError extends Error {}

/** Проверяет конфиг, заполняет значения по умолчанию. Бросает ConfigError с понятным текстом. */
export function normalizeConfig(raw) {
  const cfg = deepMerge(structuredClone(DEFAULTS), raw ?? {});
  const errors = [];

  if (!Array.isArray(cfg.agents) || cfg.agents.length === 0) {
    errors.push('agents: нужен хотя бы один агент');
    cfg.agents = [];
  }
  const ids = new Set();
  cfg.agents = cfg.agents.map((a, i) => {
    const agent = { ...AGENT_DEFAULTS, ...a };
    const where = `agents[${i}]${a?.id ? ` (${a.id})` : ''}`;
    if (!agent.id || !/^[a-z0-9_-]+$/i.test(agent.id)) errors.push(`${where}: id обязателен (латиница, цифры, - и _)`);
    if (ids.has(agent.id)) errors.push(`${where}: повторяющийся id`);
    if (agent.id === 'auto') errors.push(`${where}: id "auto" зарезервирован`);
    ids.add(agent.id);
    if (!PROVIDERS.includes(agent.provider)) errors.push(`${where}: provider должен быть одним из ${PROVIDERS.join(', ')}`);
    if (agent.provider !== 'echo' && !agent.model) errors.push(`${where}: не указан model`);
    if (agent.provider === 'openai' && !agent.base_url) errors.push(`${where}: для provider=openai нужен base_url`);
    agent.name = agent.name || agent.id;
    agent.aliases = (agent.aliases || []).map(String);
    if (agent.enabled === false) agent.disabled = true;
    return agent;
  }).filter((a) => !a.disabled);

  const known = new Set(cfg.agents.map((a) => a.id));
  const r = cfg.router;
  if (r.default !== 'auto' && !known.has(r.default)) errors.push(`router.default: нет агента "${r.default}"`);
  if (!r.default_agent) r.default_agent = cfg.agents[0]?.id ?? null;
  if (r.default_agent && !known.has(r.default_agent)) errors.push(`router.default_agent: нет агента "${r.default_agent}"`);
  r.fallback = (r.fallback || []).filter((id) => {
    if (!known.has(id)) errors.push(`router.fallback: нет агента "${id}"`);
    return known.has(id);
  });
  if (r.classifier) {
    if (typeof r.classifier === 'string') r.classifier = { agent: r.classifier };
    r.classifier = { timeout_ms: 2500, ...r.classifier };
    if (!known.has(r.classifier.agent)) errors.push(`router.classifier.agent: нет агента "${r.classifier.agent}"`);
  }
  r.rules = (r.rules || []).map((rule, i) => {
    if (!known.has(rule.agent)) errors.push(`router.rules[${i}]: нет агента "${rule.agent}"`);
    return { agent: rule.agent, keywords: (rule.keywords || []).map((k) => String(k).toLowerCase()) };
  });

  const { stt, tts } = cfg.speech;
  if (!STT_PROVIDERS.includes(stt.provider)) errors.push(`speech.stt.provider: одно из ${STT_PROVIDERS.join(', ')}`);
  if (!TTS_PROVIDERS.includes(tts.provider)) errors.push(`speech.tts.provider: одно из ${TTS_PROVIDERS.join(', ')}`);
  if (stt.provider !== 'mock' && !stt.base_url) errors.push('speech.stt.base_url: не указан');
  if (tts.provider !== 'mock' && !tts.base_url) errors.push('speech.tts.base_url: не указан');

  cfg.tools.enabled = (cfg.tools.enabled || []).filter((t) => {
    if (!ALL_TOOLS.includes(t)) errors.push(`tools.enabled: неизвестный инструмент "${t}" (есть: ${ALL_TOOLS.join(', ')})`);
    return ALL_TOOLS.includes(t);
  });

  cfg.server.port = Number(cfg.server.port);
  if (!Number.isInteger(cfg.server.port) || cfg.server.port < 0 || cfg.server.port > 65535) errors.push('server.port: некорректный порт');

  if (errors.length) throw new ConfigError('Ошибки в конфиге:\n  - ' + errors.join('\n  - '));
  return cfg;
}

/**
 * Читает YAML-конфиг. Порядок поиска: явный путь → $AI_COLUMN_CONFIG → config/config.yaml → config/config.example.yaml.
 */
export function loadConfig({ file, baseDir = process.cwd(), env = process.env } = {}) {
  const candidates = [file, env.AI_COLUMN_CONFIG, 'config/config.yaml', 'config/config.example.yaml']
    .filter(Boolean)
    .map((p) => path.resolve(baseDir, p));
  const found = candidates.find((p) => fs.existsSync(p));
  if (!found) throw new ConfigError(`Не найден конфиг. Искал: ${candidates.join(', ')}`);
  const raw = YAML.parse(fs.readFileSync(found, 'utf8')) ?? {};
  const cfg = normalizeConfig(substituteEnv(raw, env));
  cfg._file = found;
  cfg._baseDir = baseDir;
  return cfg;
}
