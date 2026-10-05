// Простой логгер без зависимостей: уровни, время, «область» (scope).
const LEVELS = { debug: 10, info: 20, warn: 30, error: 40, silent: 100 };

function ts() {
  const d = new Date();
  const p = (n, w = 2) => String(n).padStart(w, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

export function createLogger(level = 'info', scope = 'app', sink = console) {
  const min = LEVELS[level] ?? LEVELS.info;
  const make = (lvl, fn) => (...args) => {
    if (LEVELS[lvl] < min) return;
    fn.call(sink, `${ts()} ${lvl.toUpperCase().padEnd(5)} [${scope}]`, ...args);
  };
  return {
    level,
    debug: make('debug', sink.debug ?? sink.log),
    info: make('info', sink.log),
    warn: make('warn', sink.warn ?? sink.log),
    error: make('error', sink.error ?? sink.log),
    child: (sub) => createLogger(level, `${scope}:${sub}`, sink),
  };
}
