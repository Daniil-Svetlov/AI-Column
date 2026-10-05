// Инструменты, которые агенты могут вызывать (function calling).
// Описания нейтральные (JSON Schema) — провайдеры сами переводят их в свой формат.

const COLOR_NAMES = {
  красный: '#ff0000', red: '#ff0000',
  оранжевый: '#ff6600', orange: '#ff6600',
  желтый: '#ffcc00', жёлтый: '#ffcc00', yellow: '#ffcc00',
  зеленый: '#00ff00', зелёный: '#00ff00', green: '#00ff00',
  голубой: '#00ccff', cyan: '#00ccff',
  синий: '#0033ff', blue: '#0033ff',
  фиолетовый: '#8800ff', purple: '#8800ff',
  розовый: '#ff3399', pink: '#ff3399',
  белый: '#ffffff', white: '#ffffff',
  теплый: '#ffb060', тёплый: '#ffb060', warm: '#ffb060',
};

const WEATHER_CODES = {
  0: 'ясно', 1: 'преимущественно ясно', 2: 'переменная облачность', 3: 'пасмурно',
  45: 'туман', 48: 'изморозь и туман', 51: 'слабая морось', 53: 'морось', 55: 'сильная морось',
  56: 'ледяная морось', 57: 'сильная ледяная морось', 61: 'небольшой дождь', 63: 'дождь', 65: 'сильный дождь',
  66: 'ледяной дождь', 67: 'сильный ледяной дождь', 71: 'небольшой снег', 73: 'снег', 75: 'сильный снег',
  77: 'снежная крупа', 80: 'ливень', 81: 'сильный ливень', 82: 'очень сильный ливень',
  85: 'снегопад', 86: 'сильный снегопад', 95: 'гроза', 96: 'гроза с градом', 99: 'сильная гроза с градом',
};

export function parseColor(input) {
  if (!input) return null;
  const s = String(input).trim().toLowerCase();
  if (/^#?[0-9a-f]{6}$/.test(s)) return s.startsWith('#') ? s : `#${s}`;
  return COLOR_NAMES[s] ?? null;
}

function plural(n, one, few, many) {
  const n10 = n % 10;
  const n100 = n % 100;
  if (n10 === 1 && n100 !== 11) return one;
  if (n10 >= 2 && n10 <= 4 && (n100 < 10 || n100 >= 20)) return few;
  return many;
}

export function humanDuration(sec) {
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = sec % 60;
  const parts = [];
  if (h) parts.push(`${h} ${plural(h, 'час', 'часа', 'часов')}`);
  if (m) parts.push(`${m} ${plural(m, 'минуту', 'минуты', 'минут')}`);
  if (s || !parts.length) parts.push(`${s} ${plural(s, 'секунду', 'секунды', 'секунд')}`);
  return parts.join(' ');
}

export class Toolbox {
  /**
   * @param {{config: object, fetchImpl?: typeof fetch, logger?: object}} deps
   */
  constructor({ config, fetchImpl = fetch, logger } = {}) {
    this.config = config;
    this.enabled = new Set(config.tools.enabled);
    this.fetch = fetchImpl;
    this.log = logger;
  }

  /** Список инструментов для конкретного вызова (контекст влияет: глубина, текущий агент, наличие колонки). */
  definitions(ctx = {}) {
    const defs = [];
    const has = (t) => this.enabled.has(t);
    const others = (ctx.registry?.list() ?? []).filter((a) => a.id !== ctx.agentId);
    if (has('datetime')) {
      defs.push({
        name: 'get_datetime',
        description: 'Текущие дата, время и день недели. Вызывай, когда спрашивают про время или дату.',
        parameters: { type: 'object', properties: {}, additionalProperties: false },
      });
    }
    if (has('weather')) {
      defs.push({
        name: 'get_weather',
        description: 'Текущая погода и прогноз на сегодня-завтра. Если город не назван — город по умолчанию.',
        parameters: {
          type: 'object',
          properties: { city: { type: 'string', description: 'Город, например «Казань»' } },
          additionalProperties: false,
        },
      });
    }
    if (ctx.session) {
      if (has('volume')) {
        defs.push({
          name: 'set_volume',
          description: 'Изменить громкость колонки. Либо level (0–100), либо change (относительно, например -10 или +20).',
          parameters: {
            type: 'object',
            properties: {
              level: { type: 'integer', minimum: 0, maximum: 100 },
              change: { type: 'integer', minimum: -100, maximum: 100 },
            },
            additionalProperties: false,
          },
        });
      }
      if (has('lights')) {
        defs.push({
          name: 'set_lights',
          description: 'Управлять подсветкой колонки (светодиодная матрица).',
          parameters: {
            type: 'object',
            properties: {
              mode: { type: 'string', enum: ['off', 'solid', 'breathe', 'rainbow', 'auto'], description: 'auto — подсветка по состоянию колонки' },
              color: { type: 'string', description: 'Цвет: #RRGGBB или название (красный, синий, тёплый…)' },
              brightness: { type: 'integer', minimum: 1, maximum: 100 },
            },
            required: ['mode'],
            additionalProperties: false,
          },
        });
      }
      if (has('timer')) {
        defs.push({
          name: 'set_timer',
          description: 'Поставить таймер. Когда время выйдет, колонка скажет об этом вслух.',
          parameters: {
            type: 'object',
            properties: {
              seconds: { type: 'integer', minimum: 1, maximum: 86400 },
              label: { type: 'string', description: 'Для чего таймер, например «пицца»' },
            },
            required: ['seconds'],
            additionalProperties: false,
          },
        });
        defs.push({
          name: 'cancel_timers',
          description: 'Отменить все таймеры на колонке.',
          parameters: { type: 'object', properties: {}, additionalProperties: false },
        });
      }
      if (has('switch_agent') && ctx.registry) {
        defs.push({
          name: 'switch_agent',
          description: 'Переключить, какой агент отвечает на этой колонке дальше (auto — выбирать автоматически).',
          parameters: {
            type: 'object',
            properties: { agent: { type: 'string', enum: ['auto', ...ctx.registry.ids()] } },
            required: ['agent'],
            additionalProperties: false,
          },
        });
      }
    }
    if (has('ask_agent') && (ctx.depth ?? 0) === 0 && others.length) {
      defs.push({
        name: 'ask_agent',
        description: `Задать вопрос другому агенту и получить его ответ. Агенты: ${others.map((a) => `${a.id} — ${a.description || a.name}`).join('; ')}.`,
        parameters: {
          type: 'object',
          properties: {
            agent: { type: 'string', enum: others.map((a) => a.id) },
            question: { type: 'string' },
          },
          required: ['agent', 'question'],
          additionalProperties: false,
        },
      });
    }
    return defs;
  }

  async execute(name, args, ctx = {}) {
    const t0 = Date.now();
    try {
      const fn = this[`_tool_${name}`];
      if (!fn || !this.definitions(ctx).some((d) => d.name === name)) return `Ошибка: инструмент ${name} недоступен`;
      const res = await fn.call(this, args || {}, ctx);
      this.log?.info(`инструмент ${name}(${JSON.stringify(args)}) → ${String(res).slice(0, 120)} [${Date.now() - t0} мс]`);
      return res;
    } catch (e) {
      this.log?.warn(`инструмент ${name} упал: ${e.message}`);
      return `Ошибка инструмента ${name}: ${e.message}`;
    }
  }

  _tool_get_datetime() {
    const tz = this.config.server.timezone;
    const now = new Date();
    const date = new Intl.DateTimeFormat('ru-RU', { timeZone: tz, weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' }).format(now);
    const time = new Intl.DateTimeFormat('ru-RU', { timeZone: tz, hour: '2-digit', minute: '2-digit' }).format(now);
    return `Сейчас ${time}, ${date} (часовой пояс ${tz})`;
  }

  async _tool_get_weather({ city }, ctx) {
    const name = (city || this.config.tools.weather?.default_city || 'Москва').trim();
    const signal = ctx.signal ? AbortSignal.any([ctx.signal, AbortSignal.timeout(8000)]) : AbortSignal.timeout(8000);
    const geoUrl = `https://geocoding-api.open-meteo.com/v1/search?count=1&language=ru&name=${encodeURIComponent(name)}`;
    const geo = await (await this.fetch(geoUrl, { signal })).json();
    const place = geo?.results?.[0];
    if (!place) return `Не нашёл город «${name}»`;
    const url = 'https://api.open-meteo.com/v1/forecast'
      + `?latitude=${place.latitude}&longitude=${place.longitude}`
      + '&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m,relative_humidity_2m'
      + '&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code'
      + '&timezone=auto&forecast_days=2&wind_speed_unit=ms';
    const w = await (await this.fetch(url, { signal })).json();
    const c = w.current;
    const d = w.daily;
    const r = (x) => Math.round(x);
    const desc = (code) => WEATHER_CODES[code] ?? 'без осадков';
    return [
      `${place.name}${place.admin1 ? ` (${place.admin1})` : ''}: сейчас ${r(c.temperature_2m)}°C, ощущается как ${r(c.apparent_temperature)}°C, ${desc(c.weather_code)}, ветер ${r(c.wind_speed_10m)} м/с, влажность ${r(c.relative_humidity_2m)}%.`,
      `Сегодня от ${r(d.temperature_2m_min[0])} до ${r(d.temperature_2m_max[0])}°C, ${desc(d.weather_code[0])}, вероятность осадков ${d.precipitation_probability_max[0] ?? 0}%.`,
      `Завтра от ${r(d.temperature_2m_min[1])} до ${r(d.temperature_2m_max[1])}°C, ${desc(d.weather_code[1])}, вероятность осадков ${d.precipitation_probability_max[1] ?? 0}%.`,
    ].join(' ');
  }

  _tool_set_volume({ level, change }, ctx) {
    const s = ctx.session;
    let v = Number.isFinite(level) ? level : s.volume + (Number.isFinite(change) ? change : 0);
    v = Math.max(0, Math.min(100, Math.round(v)));
    s.setVolume(v);
    return `Громкость ${v}%`;
  }

  _tool_set_lights({ mode, color, brightness }, ctx) {
    const hex = parseColor(color);
    if (color && !hex) return `Не знаю цвет «${color}». Используй #RRGGBB или простое название.`;
    ctx.session.setLights({ mode, color: hex ?? undefined, brightness });
    return mode === 'off' ? 'Подсветка выключена' : `Подсветка: ${mode}${hex ? `, цвет ${hex}` : ''}`;
  }

  _tool_set_timer({ seconds, label }, ctx) {
    const sec = Math.max(1, Math.min(86400, Math.round(seconds)));
    ctx.timers.add(ctx.session.deviceId, sec, label || '');
    return `Таймер${label ? ` «${label}»` : ''} поставлен на ${humanDuration(sec)}`;
  }

  _tool_cancel_timers(_args, ctx) {
    const n = ctx.timers.cancelAll(ctx.session.deviceId);
    return n ? `Отменено таймеров: ${n}` : 'Активных таймеров нет';
  }

  _tool_switch_agent({ agent }, ctx) {
    if (agent !== 'auto' && !ctx.registry.has(agent)) return `Нет агента ${agent}`;
    ctx.session.setAgent(agent);
    const name = agent === 'auto' ? 'автовыбор' : ctx.registry.get(agent).name;
    return `Дальше отвечает: ${name}`;
  }

  async _tool_ask_agent({ agent, question }, ctx) {
    const target = ctx.registry.get(agent);
    if (!target) return `Нет агента ${agent}`;
    if (!target.available) return `${target.name} сейчас недоступен`;
    ctx.onDelegate?.(target);
    const system = `${ctx.baseSystem ?? ''}\nТебе задаёт вопрос другой агент (${ctx.agentName ?? ctx.agentId}). Ответь по существу, кратко, обычным текстом.`;
    const answer = await target.complete({ system, userText: question, maxTokens: 600, signal: ctx.signal });
    return `Ответ ${target.name}: ${answer}`;
  }
}
