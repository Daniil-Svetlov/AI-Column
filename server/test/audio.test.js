import { test } from 'node:test';
import assert from 'node:assert/strict';
import { encodeWav, parseWav, pcmBufferToInt16 } from '../src/audio/wav.js';
import { resample, rms } from '../src/audio/resample.js';
import { decodeToPcm } from '../src/audio/decode.js';

function sine(n, rate, freq = 440, amp = 10000) {
  const s = new Int16Array(n);
  for (let i = 0; i < n; i++) s[i] = Math.round(Math.sin((2 * Math.PI * freq * i) / rate) * amp);
  return s;
}

test('WAV PCM16: запись и чтение без потерь', () => {
  const s = sine(1000, 16000);
  const { samples, sampleRate, channels } = parseWav(encodeWav(s, 16000));
  assert.equal(sampleRate, 16000);
  assert.equal(channels, 1);
  assert.deepEqual(Array.from(samples), Array.from(s));
});

test('WAV float32 стерео → моно PCM16', () => {
  const frames = 100;
  const data = Buffer.alloc(frames * 8);
  for (let i = 0; i < frames; i++) {
    data.writeFloatLE(0.5, i * 8);
    data.writeFloatLE(-0.5, i * 8 + 4);
  }
  const h = Buffer.alloc(44);
  h.write('RIFF', 0); h.writeUInt32LE(36 + data.length, 4); h.write('WAVE', 8);
  h.write('fmt ', 12); h.writeUInt32LE(16, 16); h.writeUInt16LE(3, 20); h.writeUInt16LE(2, 22);
  h.writeUInt32LE(22050, 24); h.writeUInt32LE(22050 * 8, 28); h.writeUInt16LE(8, 32); h.writeUInt16LE(32, 34);
  h.write('data', 36); h.writeUInt32LE(data.length, 40);
  const { samples, sampleRate } = parseWav(Buffer.concat([h, data]));
  assert.equal(sampleRate, 22050);
  assert.equal(samples.length, frames);
  assert.ok(samples.every((v) => v === 0));
});

test('«потоковый» WAV с размером data = 0xFFFFFFFF читается целиком', () => {
  const wav = encodeWav(sine(500, 24000), 24000);
  wav.writeUInt32LE(0xffffffff, 40);
  assert.equal(parseWav(wav).samples.length, 500);
});

test('resample меняет длину пропорционально и сохраняет громкость', () => {
  const s = sine(24000, 24000, 300);
  const down = resample(s, 24000, 16000);
  assert.equal(down.length, 16000);
  assert.ok(Math.abs(rms(down) - rms(s)) < 0.02);
  const up = resample(sine(22050, 22050, 300), 22050, 24000);
  assert.ok(Math.abs(up.length - 24000) <= 1);
});

test('decodeToPcm: WAV и сырой PCM', async () => {
  const s = sine(2205, 22050);
  const a = await decodeToPcm(encodeWav(s, 22050), 22050);
  assert.equal(a.length, 2205);
  const raw = Buffer.from(s.buffer);
  const b = await decodeToPcm(raw, 11025, { rawRate: 22050 });
  assert.ok(Math.abs(b.length - 1102) <= 1);
  assert.deepEqual(Array.from(pcmBufferToInt16(raw)).slice(0, 5), Array.from(s).slice(0, 5));
});

test('normalizePeak усиливает тихую речь, но не шум и не громкую', async () => {
  const { normalizePeak } = await import('../src/audio/resample.js');
  const quiet = sine(1600, 16000, 300, 2000);
  const louder = normalizePeak(quiet);
  assert.ok(Math.max(...louder) > 15000 && Math.max(...louder) <= 32767);
  const silence = sine(1600, 16000, 300, 20);
  assert.equal(normalizePeak(silence), silence);
  const loud = sine(1600, 16000, 300, 30000);
  assert.equal(normalizePeak(loud), loud);
});
