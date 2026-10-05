// Синтез речи (TTS). Провайдеры:
//   piper  — HTTP-сервер Piper (python3 -m piper.http_server), POST /synthesize
//   openai — OpenAI-совместимый /v1/audio/speech (OpenAI, speaches, LocalAI, Kokoro-FastAPI, openedai-speech…)
//   mock   — для тестов: короткий тональный сигнал вместо речи
import { decodeToPcm } from '../audio/decode.js';

const trimSlash = (s) => String(s || '').replace(/\/+$/, '');

async function fetchAudio(url, init) {
  const res = await fetch(url, init);
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    const err = new Error(`TTS HTTP ${res.status}: ${body.slice(0, 300)}`);
    err.status = res.status;
    throw err;
  }
  return Buffer.from(await res.arrayBuffer());
}

function tone(text, rate) {
  const dur = Math.min(3, 0.25 + String(text).length * 0.03);
  const n = Math.round(dur * rate);
  const out = new Int16Array(n);
  for (let i = 0; i < n; i++) {
    const t = i / rate;
    const env = Math.min(1, t / 0.02, (dur - t) / 0.05);
    out[i] = Math.round(Math.sin(2 * Math.PI * 440 * t) * 6000 * Math.max(0, env));
  }
  return out;
}

export function createTts(cfg) {
  const base = trimSlash(cfg.base_url);
  const withTimeout = (signal) => (signal ? AbortSignal.any([signal, AbortSignal.timeout(cfg.timeout_ms)]) : AbortSignal.timeout(cfg.timeout_ms));
  const auth = cfg.api_key && cfg.api_key !== 'none' ? { Authorization: `Bearer ${cfg.api_key}` } : {};

  if (cfg.provider === 'mock') {
    return {
      name: 'mock',
      async synthesize(text, rate) {
        return tone(text, rate);
      },
    };
  }

  if (cfg.provider === 'piper') {
    let path = cfg.path ?? '/synthesize';
    return {
      name: `piper(${base}, ${cfg.voice ?? 'голос по умолчанию'})`,
      async synthesize(text, rate, { signal } = {}) {
        const body = { text };
        if (cfg.voice) body.voice = cfg.voice;
        if (cfg.speaker !== undefined) body.speaker = cfg.speaker;
        if (cfg.speed && cfg.speed !== 1) body.length_scale = 1 / cfg.speed;
        const init = () => ({ method: 'POST', headers: { 'Content-Type': 'application/json', ...auth }, body: JSON.stringify(body), signal: withTimeout(signal) });
        let wav;
        try {
          wav = await fetchAudio(`${base}${path}`, init());
        } catch (e) {
          // Старые версии piper http_server принимали POST на «/».
          if (e.status === 404 && path !== '/') {
            path = '/';
            wav = await fetchAudio(`${base}${path}`, init());
          } else throw e;
        }
        return decodeToPcm(wav, rate, { signal });
      },
    };
  }

  // openai-совместимый
  return {
    name: `openai(${base}, ${cfg.model}/${cfg.voice})`,
    async synthesize(text, rate, { signal } = {}) {
      const body = {
        model: cfg.model ?? 'tts-1',
        voice: cfg.voice ?? 'alloy',
        input: text,
        response_format: cfg.response_format ?? 'wav',
      };
      if (cfg.speed && cfg.speed !== 1) body.speed = cfg.speed;
      if (cfg.instructions) body.instructions = cfg.instructions;
      const audio = await fetchAudio(`${base}/audio/speech`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...auth },
        body: JSON.stringify(body),
        signal: withTimeout(signal),
      });
      // response_format=pcm у OpenAI — сырой PCM16 24 кГц без заголовка.
      return decodeToPcm(audio, rate, { signal, rawRate: body.response_format === 'pcm' ? (cfg.pcm_rate ?? 24000) : undefined });
    },
  };
}
