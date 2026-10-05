// Превращает ответ TTS в моно PCM16 нужной частоты.
// WAV разбираем сами; всё остальное (mp3, ogg, flac…) — через ffmpeg, если он установлен.
import { spawn } from 'node:child_process';
import { parseWav, pcmBufferToInt16 } from './wav.js';
import { resample } from './resample.js';

function isWav(buf) {
  return buf.length >= 12 && buf.toString('ascii', 0, 4) === 'RIFF' && buf.toString('ascii', 8, 12) === 'WAVE';
}

function ffmpegToPcm(buf, rate, signal) {
  return new Promise((resolve, reject) => {
    const p = spawn('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-i', 'pipe:0', '-f', 's16le', '-ac', '1', '-ar', String(rate), 'pipe:1'], { signal });
    const chunks = [];
    let err = '';
    p.stdout.on('data', (c) => chunks.push(c));
    p.stderr.on('data', (c) => { err += c; });
    p.on('error', (e) => reject(e.code === 'ENOENT'
      ? new Error('аудио не в WAV, а ffmpeg не установлен — попроси TTS отдавать wav или поставь ffmpeg')
      : e));
    p.on('close', (code) => (code === 0 ? resolve(pcmBufferToInt16(Buffer.concat(chunks))) : reject(new Error(`ffmpeg: ${err.trim() || code}`))));
    p.stdin.on('error', () => {});
    p.stdin.end(buf);
  });
}

/**
 * @param {Buffer} buf  — WAV/MP3/… от TTS, либо сырой PCM16 при rawRate
 * @param {number} targetRate
 * @param {{rawRate?: number, signal?: AbortSignal}} opts
 * @returns {Promise<Int16Array>}
 */
export async function decodeToPcm(buf, targetRate, { rawRate, signal } = {}) {
  if (rawRate) return resample(pcmBufferToInt16(buf), rawRate, targetRate);
  if (isWav(buf)) {
    const { samples, sampleRate } = parseWav(buf);
    return resample(samples, sampleRate, targetRate);
  }
  return ffmpegToPcm(buf, targetRate, signal);
}
