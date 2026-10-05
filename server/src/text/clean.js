// Очистка текста перед озвучкой и показом на экране колонки.

/** Убирает markdown, ссылки, эмодзи — то, что синтезатор прочитает криво. */
export function cleanForSpeech(text) {
  let t = String(text ?? '');
  t = t.replace(/```[\s\S]*?(```|$)/g, ' '); // блоки кода не читаем вслух
  t = t.replace(/`([^`]*)`/g, '$1');
  t = t.replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1');
  t = t.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1');
  t = t.replace(/https?:\/\/\S+/g, 'ссылка');
  t = t.replace(/^\s{0,3}#{1,6}\s+/gm, '');
  t = t.replace(/^\s*(?:[-*+•]|\d+[.)])\s+/gm, '');
  t = t.replace(/(\*\*|__|\*|~~)(?=\S)([\s\S]*?\S)\1/g, '$2');
  t = t.replace(/[*#_~|<>]/g, ' ');
  t = t.replace(/[\p{Extended_Pictographic}\u{1F1E6}-\u{1F1FF}\u{FE0F}\u{200D}]/gu, '');
  t = t.replace(/\s*₽/g, ' рублей').replace(/[„“”]/g, '"');
  t = t.replace(/\s+/g, ' ').trim();
  return t;
}

/** Нормализация для сравнения: нижний регистр, ё→е. Длина строки не меняется. */
export function normalize(text) {
  return String(text ?? '').toLowerCase().replace(/ё/g, 'е');
}

/** Потоковый фильтр, вырезающий <think>…</think> (Qwen3, DeepSeek-R1 и т.п.). */
export class ThinkFilter {
  constructor(open = '<think>', close = '</think>') {
    this.open = open;
    this.close = close;
    this.inThink = false;
    this.carry = '';
    this.started = false;
  }

  static _partialSuffix(s, tag) {
    for (let n = Math.min(tag.length - 1, s.length); n > 0; n--) {
      if (tag.startsWith(s.slice(-n))) return n;
    }
    return 0;
  }

  push(text) {
    let s = this.carry + text;
    this.carry = '';
    let out = '';
    for (;;) {
      if (!this.inThink) {
        const i = s.indexOf(this.open);
        if (i >= 0) {
          out += s.slice(0, i);
          s = s.slice(i + this.open.length);
          this.inThink = true;
          continue;
        }
        const keep = ThinkFilter._partialSuffix(s, this.open);
        out += s.slice(0, s.length - keep);
        this.carry = s.slice(s.length - keep);
        break;
      } else {
        const i = s.indexOf(this.close);
        if (i >= 0) {
          s = s.slice(i + this.close.length);
          this.inThink = false;
          continue;
        }
        const keep = ThinkFilter._partialSuffix(s, this.close);
        this.carry = s.slice(s.length - keep);
        break;
      }
    }
    if (!this.started) {
      out = out.replace(/^\s+/, '');
      if (out) this.started = true;
    }
    return out;
  }

  flush() {
    const rest = this.inThink ? '' : this.carry;
    this.carry = '';
    return this.started ? rest : rest.replace(/^\s+/, '');
  }
}
