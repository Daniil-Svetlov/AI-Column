// WAV: разбор (PCM 8/16/24/32 бит, float32, WAVE_FORMAT_EXTENSIBLE) и запись PCM16 mono.

export class WavError extends Error {}

/**
 * Разбирает WAV и возвращает моно Int16Array.
 * Терпим к «потоковым» WAV, где размер data = 0 или 0xFFFFFFFF (так отдают некоторые TTS).
 */
export function parseWav(input) {
  const buf = Buffer.isBuffer(input) ? input : Buffer.from(input);
  if (buf.length < 12 || buf.toString('ascii', 0, 4) !== 'RIFF' || buf.toString('ascii', 8, 12) !== 'WAVE') {
    throw new WavError('не WAV (нет заголовка RIFF/WAVE)');
  }
  let pos = 12;
  let fmt = null;
  let dataStart = -1;
  let dataLen = 0;
  while (pos + 8 <= buf.length) {
    const id = buf.toString('ascii', pos, pos + 4);
    let size = buf.readUInt32LE(pos + 4);
    const body = pos + 8;
    if (id === 'fmt ') {
      let format = buf.readUInt16LE(body);
      const channels = buf.readUInt16LE(body + 2);
      const sampleRate = buf.readUInt32LE(body + 4);
      const bitsPerSample = buf.readUInt16LE(body + 14);
      if (format === 0xfffe && size >= 26) format = buf.readUInt16LE(body + 24); // SubFormat GUID: первые 2 байта
      fmt = { format, channels, sampleRate, bitsPerSample };
    } else if (id === 'data') {
      dataStart = body;
      if (size === 0 || size === 0xffffffff || body + size > buf.length) size = buf.length - body;
      dataLen = size;
      break;
    }
    pos = body + size + (size % 2);
  }
  if (!fmt) throw new WavError('в WAV нет блока fmt');
  if (dataStart < 0) throw new WavError('в WAV нет блока data');
  const { format, channels, sampleRate, bitsPerSample } = fmt;
  if (!channels || !sampleRate) throw new WavError('битый заголовок WAV');
  const bytesPer = bitsPerSample / 8;
  const frameBytes = bytesPer * channels;
  const frames = Math.floor(dataLen / frameBytes);
  const out = new Int16Array(frames);

  const read = (() => {
    if (format === 1 && bitsPerSample === 16) return (o) => buf.readInt16LE(o) / 32768;
    if (format === 1 && bitsPerSample === 8) return (o) => (buf.readUInt8(o) - 128) / 128;
    if (format === 1 && bitsPerSample === 24) return (o) => buf.readIntLE(o, 3) / 8388608;
    if (format === 1 && bitsPerSample === 32) return (o) => buf.readInt32LE(o) / 2147483648;
    if (format === 3 && bitsPerSample === 32) return (o) => buf.readFloatLE(o);
    if (format === 3 && bitsPerSample === 64) return (o) => buf.readDoubleLE(o);
    throw new WavError(`неподдерживаемый формат WAV: format=${format}, bits=${bitsPerSample}`);
  })();

  for (let i = 0; i < frames; i++) {
    let acc = 0;
    const base = dataStart + i * frameBytes;
    for (let c = 0; c < channels; c++) acc += read(base + c * bytesPer);
    const v = Math.round((acc / channels) * 32767);
    out[i] = v > 32767 ? 32767 : v < -32768 ? -32768 : v;
  }
  return { sampleRate, channels, bitsPerSample, samples: out };
}

/** Кодирует моно PCM16 в WAV. */
export function encodeWav(samples, sampleRate) {
  const data = Buffer.from(samples.buffer, samples.byteOffset, samples.byteLength);
  const header = Buffer.alloc(44);
  header.write('RIFF', 0, 'ascii');
  header.writeUInt32LE(36 + data.length, 4);
  header.write('WAVE', 8, 'ascii');
  header.write('fmt ', 12, 'ascii');
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(sampleRate, 24);
  header.writeUInt32LE(sampleRate * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write('data', 36, 'ascii');
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

/** Buffer (PCM16LE) → Int16Array (с копированием, чтобы не зависеть от выравнивания). */
export function pcmBufferToInt16(buf) {
  const even = buf.length - (buf.length % 2);
  const out = new Int16Array(even / 2);
  for (let i = 0; i < out.length; i++) out[i] = buf.readInt16LE(i * 2);
  return out;
}

export function int16ToBuffer(samples) {
  return Buffer.from(samples.buffer, samples.byteOffset, samples.byteLength);
}
