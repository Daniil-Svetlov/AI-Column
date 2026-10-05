// Пересэмплирование моно PCM16. Для речи хватает линейной интерполяции;
// при понижении частоты сначала сглаживаем, чтобы не было алиасинга.

function lowpassBox(samples, ratio) {
  const win = Math.max(1, Math.round(ratio));
  if (win <= 1) return samples;
  const out = new Int16Array(samples.length);
  let acc = 0;
  for (let i = 0; i < samples.length; i++) {
    acc += samples[i];
    if (i >= win) acc -= samples[i - win];
    out[i] = Math.round(acc / Math.min(i + 1, win));
  }
  return out;
}

export function resample(samples, fromRate, toRate) {
  if (!samples.length || fromRate === toRate) return samples;
  const ratio = fromRate / toRate;
  const src = ratio > 1.15 ? lowpassBox(samples, ratio) : samples;
  const outLen = Math.max(1, Math.floor(samples.length / ratio));
  const out = new Int16Array(outLen);
  for (let i = 0; i < outLen; i++) {
    const pos = i * ratio;
    const i0 = Math.floor(pos);
    const i1 = Math.min(i0 + 1, src.length - 1);
    const frac = pos - i0;
    out[i] = Math.round(src[i0] * (1 - frac) + src[i1] * frac);
  }
  return out;
}

/** Длительность в секундах. */
export const durationSec = (samples, rate) => samples.length / rate;

/** RMS (0…1) — для отладки и проверки «тишины». */
export function rms(samples) {
  if (!samples.length) return 0;
  let s = 0;
  for (let i = 0; i < samples.length; i++) s += samples[i] * samples[i];
  return Math.sqrt(s / samples.length) / 32768;
}

/**
 * Подтягивает громкость фразы перед распознаванием: пик → targetPeak, но не больше maxGain.
 * Тихие микрофоны (INMP441 без усиления) Whisper распознаёт хуже.
 */
export function normalizePeak(samples, { targetPeak = 0.7, maxGain = 8 } = {}) {
  let peak = 0;
  for (let i = 0; i < samples.length; i++) {
    const a = Math.abs(samples[i]);
    if (a > peak) peak = a;
  }
  if (peak < 50) return samples; // тишина — усиливать только шум
  const gain = Math.min(maxGain, (targetPeak * 32767) / peak);
  if (gain <= 1.05) return samples;
  const out = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    const v = Math.round(samples[i] * gain);
    out[i] = v > 32767 ? 32767 : v < -32768 ? -32768 : v;
  }
  return out;
}
