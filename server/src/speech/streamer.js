// Потоковая озвучка: предложения → TTS (с опережением) → PCM-кадры на колонку с контролем темпа.
import { int16ToBuffer } from '../audio/wav.js';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export class SpeechStreamer {
  /**
   * @param {object} p
   * @param {{synthesize: Function}} p.tts
   * @param {import('../session.js').DeviceSession} p.session
   * @param {number} p.rate          частота PCM для колонки
   * @param {AbortSignal} p.signal
   * @param {number} [p.lookahead]   сколько предложений синтезировать заранее
   * @param {number} [p.aheadSec]    насколько секунд можно обгонять воспроизведение
   * @param {number} [p.chunkBytes]  размер бинарного кадра
   */
  constructor({ tts, session, rate, signal, logger, lookahead = 2, aheadSec = 3, chunkBytes = 4096, onFirstAudio, turnId }) {
    Object.assign(this, { tts, session, rate, signal, log: logger, lookahead, aheadSec, chunkBytes, onFirstAudio, turnId });
    this.items = [];
    this.cursor = 0;
    this.loop = null;
    this.started = false;
    this.sentSamples = 0;
    this.t0 = 0;
    this.errors = [];
  }

  get hasAudio() {
    return this.started;
  }

  say(text) {
    if (!text || this.signal?.aborted) return;
    this.items.push({ text, pcm: null });
    this._ensureStarted(this.cursor);
    if (!this.loop) this.loop = this._run();
  }

  _ensureStarted(from) {
    const end = Math.min(this.items.length, from + 1 + this.lookahead);
    for (let i = from; i < end; i++) {
      const it = this.items[i];
      if (it.pcm) continue;
      it.pcm = this.tts.synthesize(it.text, this.rate, { signal: this.signal }).catch((e) => {
        if (!this.signal?.aborted) {
          this.errors.push(e);
          this.log?.warn(`TTS не смог озвучить «${it.text.slice(0, 40)}»: ${e.message}`);
        }
        return null;
      });
    }
  }

  async _run() {
    try {
      while (this.cursor < this.items.length) {
        if (this.signal?.aborted) return;
        this._ensureStarted(this.cursor);
        const pcm = await this.items[this.cursor].pcm;
        this.items[this.cursor].pcm = Promise.resolve(null); // освобождаем память
        if (pcm?.length && !this.signal?.aborted) await this._send(pcm);
        this.cursor++;
      }
    } finally {
      this.loop = null;
    }
  }

  async _send(pcm) {
    const ws = this.session.ws;
    if (!this.started) {
      this.started = true;
      this.t0 = Date.now();
      this.session.send({ type: 'audio_start', rate: this.rate, format: 'pcm16', turn: this.turnId });
      this.session.state = 'speaking';
      this.onFirstAudio?.();
    }
    const bytes = int16ToBuffer(pcm);
    for (let off = 0; off < bytes.length; off += this.chunkBytes) {
      for (;;) {
        if (this.signal?.aborted || !this.session.isOpen) return;
        const ahead = this.sentSamples / this.rate - (Date.now() - this.t0) / 1000;
        if (ahead < -0.25) this.t0 = Date.now() - (this.sentSamples / this.rate) * 1000; // колонка «голодала» — переякоримся
        if (ahead < this.aheadSec && (ws?.bufferedAmount ?? 0) < 256 * 1024) break;
        await sleep(30);
      }
      const chunk = bytes.subarray(off, Math.min(bytes.length, off + this.chunkBytes));
      this.session.sendAudio(chunk);
      this.sentSamples += chunk.length / 2;
      this.session.speakingUntil = this.t0 + (this.sentSamples / this.rate) * 1000 + 500;
    }
  }

  /** Дождаться, пока всё сказанное уйдёт на колонку; закрыть аудиопоток. */
  async finish() {
    while (this.loop) await this.loop;
    if (this.started && !this.signal?.aborted) this.session.send({ type: 'audio_end', turn: this.turnId, seconds: +(this.sentSamples / this.rate).toFixed(2) });
  }
}
