// Нарезка потокового ответа LLM на предложения для синтеза речи.
// Первое предложение отдаём как можно раньше (быстрый старт речи),
// дальше склеиваем короткие, чтобы не дёргать TTS на каждое «Да.».

const CLOSERS = '"\'»)]”’';
const ENDERS = '.!?…';

export class SentenceSplitter {
  constructor({ minChars = 40, maxChars = 240 } = {}) {
    this.minChars = minChars;
    this.maxChars = maxChars;
    this.buf = '';
    this.emitted = 0;
  }

  /** Находит конец предложения, начиная с from. Возвращает индекс (не включая) или -1, если ещё рано решать. */
  _findBoundary(from) {
    const s = this.buf;
    for (let i = from; i < s.length; i++) {
      const ch = s[i];
      if (ch === '\n') return i + 1;
      if (!ENDERS.includes(ch)) continue;
      let j = i + 1;
      while (j < s.length && (ENDERS.includes(s[j]) || CLOSERS.includes(s[j]))) j++;
      if (j >= s.length) return -1; // не знаем, что дальше
      if (!/\s/.test(s[j])) continue; // «3.5», «т.е.» без пробела
      let k = j;
      while (k < s.length && /\s/.test(s[k])) k++;
      if (k >= s.length) return -1; // ждём следующий непробельный символ
      const next = s[k];
      // Строчная буква после точки — скорее сокращение («т. е.», «и т. д. и»), не конец.
      if (ch === '.' && /\p{Ll}/u.test(next)) continue;
      return j;
    }
    return -1;
  }

  _splitLong() {
    const s = this.buf.slice(0, this.maxChars);
    for (const sep of ['; ', ': ', ', ', ' — ', ' - ']) {
      const idx = s.lastIndexOf(sep);
      if (idx > this.maxChars * 0.4) return idx + sep.length;
    }
    const sp = s.lastIndexOf(' ');
    return sp > 0 ? sp + 1 : this.maxChars;
  }

  push(text) {
    this.buf += text;
    const out = [];
    let searchFrom = 0;
    for (;;) {
      const b = this._findBoundary(searchFrom);
      if (b < 0) {
        if (this.buf.length > this.maxChars) {
          const cut = this._splitLong();
          this._emit(this.buf.slice(0, cut), out);
          this.buf = this.buf.slice(cut);
          searchFrom = 0;
          continue;
        }
        break;
      }
      const candidate = this.buf.slice(0, b).trim();
      const need = this.emitted === 0 ? 1 : this.minChars;
      if (candidate.length < need && b < this.buf.length) {
        searchFrom = b; // слишком коротко — приклеим следующее предложение
        continue;
      }
      this._emit(this.buf.slice(0, b), out);
      this.buf = this.buf.slice(b);
      searchFrom = 0;
    }
    return out;
  }

  flush() {
    const out = [];
    this._emit(this.buf, out);
    this.buf = '';
    return out;
  }

  _emit(piece, out) {
    const t = piece.replace(/\s+/g, ' ').trim();
    if (t) {
      out.push(t);
      this.emitted++;
    }
  }
}
