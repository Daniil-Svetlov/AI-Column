// Распознавание речи (STT). Провайдеры:
//   openai     — OpenAI-совместимый /v1/audio/transcriptions (speaches, faster-whisper-server, vLLM, LocalAI, OpenAI, Groq…)
//   whispercpp — родной сервер whisper.cpp (POST /inference)
//   mock       — для тестов: возвращает заранее заданный текст
import { encodeWav } from '../audio/wav.js';

const trimSlash = (s) => String(s || '').replace(/\/+$/, '');

async function postForm(url, form, { apiKey, signal }) {
  const headers = {};
  if (apiKey && apiKey !== 'none') headers.Authorization = `Bearer ${apiKey}`;
  const res = await fetch(url, { method: 'POST', body: form, headers, signal });
  const body = await res.text();
  if (!res.ok) throw new Error(`STT HTTP ${res.status}: ${body.slice(0, 300)}`);
  try {
    return JSON.parse(body);
  } catch {
    return { text: body };
  }
}

export const mockSttQueue = [];

/**
 * @param {object} cfg  speech.stt из конфига
 * @param {{hints?: string[]}} opts  hints — имена агентов, чтобы Whisper писал их правильно
 */
export function createStt(cfg, { hints = [] } = {}) {
  const base = trimSlash(cfg.base_url);
  const prompt = cfg.prompt ?? (hints.length ? `${hints.join(', ')}.` : undefined);
  // На тишине Whisper иногда просто повторяет подсказку — это не речь.
  const norm = (s) => String(s ?? '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
  const notEcho = (text) => (prompt && norm(text) === norm(prompt) ? '' : text);

  const withTimeout = (signal) => (signal ? AbortSignal.any([signal, AbortSignal.timeout(cfg.timeout_ms)]) : AbortSignal.timeout(cfg.timeout_ms));

  if (cfg.provider === 'mock') {
    return {
      name: 'mock',
      async transcribe() {
        return mockSttQueue.length ? mockSttQueue.shift() : (cfg.mock_text ?? '');
      },
    };
  }

  if (cfg.provider === 'whispercpp') {
    return {
      name: `whispercpp(${base})`,
      async transcribe(samples, sampleRate, { signal } = {}) {
        const form = new FormData();
        form.append('file', new Blob([encodeWav(samples, sampleRate)], { type: 'audio/wav' }), 'speech.wav');
        form.append('response_format', 'json');
        form.append('temperature', '0.0');
        if (cfg.language) form.append('language', cfg.language);
        if (prompt) form.append('prompt', prompt);
        const out = await postForm(`${base}${cfg.path ?? '/inference'}`, form, { apiKey: cfg.api_key, signal: withTimeout(signal) });
        return notEcho(String(out.text ?? '').trim());
      },
    };
  }

  // openai-совместимый
  return {
    name: `openai(${base}, ${cfg.model})`,
    async transcribe(samples, sampleRate, { signal } = {}) {
      const form = new FormData();
      form.append('file', new Blob([encodeWav(samples, sampleRate)], { type: 'audio/wav' }), 'speech.wav');
      form.append('model', cfg.model ?? 'whisper-1');
      form.append('response_format', 'json');
      if (cfg.language) form.append('language', cfg.language);
      if (prompt) form.append('prompt', prompt);
      if (cfg.temperature !== undefined) form.append('temperature', String(cfg.temperature));
      const out = await postForm(`${base}/audio/transcriptions`, form, { apiKey: cfg.api_key, signal: withTimeout(signal) });
      return notEcho(String(out.text ?? '').trim());
    },
  };
}

/** Whisper на тишине любит «галлюцинировать» такие фразы — считаем их пустым распознаванием. */
const HALLUCINATIONS = [
  /^продолжение следует\.?\.?\.?$/i,
  /^субтитры (?:сделал|делал|создавал)/i,
  /^редактор субтитров/i,
  /^спасибо за просмотр/i,
  /^подписывайтесь на канал/i,
  /^thank you\.?$/i,
  /^\.+$/,
];

export function isLikelyHallucination(text) {
  const t = String(text || '').trim();
  return !t || HALLUCINATIONS.some((re) => re.test(t));
}
