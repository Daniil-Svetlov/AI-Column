#!/usr/bin/env node
// Симулятор колонки: проверить сервер без железа.
//
//   node tools/simulate-device.js --text "Квен, который час?"
//   node tools/simulate-device.js --wav вопрос.wav --out ответ.wav
//
// Опции:
//   --url ws://localhost:8080/ws   адрес сервера
//   --token XXX                    AUTH_TOKEN (по умолчанию из .env)
//   --id sim-1                     id «колонки»
//   --agent qwen                   принудительно выбрать агента (только для --text)
//   --fast                         отправлять WAV быстрее реального времени
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import WebSocket from 'ws';
import { parseWav, encodeWav, pcmBufferToInt16, int16ToBuffer } from '../src/audio/wav.js';
import { resample } from '../src/audio/resample.js';

const here = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
try {
  process.loadEnvFile(path.join(here, '.env'));
} catch { /* нет .env */ }

function args(argv) {
  const out = {};
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('--')) continue;
    const key = a.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) out[key] = true;
    else {
      out[key] = next;
      i++;
    }
  }
  return out;
}

const opt = args(process.argv);
if (!opt.text && !opt.wav) {
  console.log('Нужно --text "вопрос" или --wav файл.wav (см. комментарий в начале файла)');
  process.exit(2);
}
const base = opt.url ?? `ws://localhost:${process.env.PORT || 8080}/ws`;
const url = new URL(base);
url.searchParams.set('id', opt.id ?? 'simulator');
const token = opt.token ?? process.env.AUTH_TOKEN;
if (token) url.searchParams.set('token', token);

const MIC_RATE = 16000;
const PLAY_RATE = 24000;
const t0 = Date.now();
const stamp = () => `${String(Date.now() - t0).padStart(5)} мс`;
const audio = [];
let gotAudioStart = false;
let gotAudioEnd = false;
let gotReplyEnd = false;
let replyText = '';

const ws = new WebSocket(url);
const timeout = setTimeout(() => {
  console.error('✖ сервер не ответил за 120 с');
  process.exit(1);
}, 120000);

function maybeDone() {
  if (!gotReplyEnd || (gotAudioStart && !gotAudioEnd)) return;
  clearTimeout(timeout);
  const pcm = audio.length ? pcmBufferToInt16(Buffer.concat(audio)) : new Int16Array(0);
  console.log(`\nОтвет: ${replyText.trim()}`);
  if (pcm.length) {
    const out = opt.out ?? 'reply.wav';
    fs.writeFileSync(out, encodeWav(pcm, PLAY_RATE));
    console.log(`Звук ответа: ${(pcm.length / PLAY_RATE).toFixed(1)} с → ${out}`);
  } else {
    console.log('Звука в ответе нет (проверь TTS).');
  }
  ws.close();
}

async function sendWav(file) {
  const { samples, sampleRate } = parseWav(fs.readFileSync(file));
  const pcm = int16ToBuffer(resample(samples, sampleRate, MIC_RATE));
  ws.send(JSON.stringify({ type: 'listen_start', mode: 'tap' }));
  const frame = (MIC_RATE / 50) * 2; // 20 мс
  for (let off = 0; off < pcm.length; off += frame) {
    ws.send(pcm.subarray(off, off + frame), { binary: true });
    if (!opt.fast) await new Promise((r) => setTimeout(r, 20));
  }
  ws.send(JSON.stringify({ type: 'listen_stop' }));
  console.log(`${stamp()}  → отправил ${(pcm.length / 2 / MIC_RATE).toFixed(1)} с звука`);
}

ws.on('open', () => {
  console.log(`${stamp()}  подключился к ${base}`);
  ws.send(JSON.stringify({ type: 'hello', fw: 'simulator', mic_rate: MIC_RATE, play_rate: PLAY_RATE, battery: 100 }));
});

ws.on('message', (data, isBinary) => {
  if (isBinary) {
    if (gotAudioStart && !gotAudioEnd) audio.push(Buffer.from(data));
    return;
  }
  const m = JSON.parse(data.toString());
  switch (m.type) {
    case 'welcome':
      console.log(`${stamp()}  сервер: ассистент «${m.assistant}», агенты: ${m.agents.map((a) => `${a.name}${a.available ? '' : ' (недоступен)'}`).join(', ')}; выбрано: ${m.agent}`);
      if (opt.wav) sendWav(opt.wav);
      else ws.send(JSON.stringify({ type: 'text', text: opt.text, agent: opt.agent }));
      break;
    case 'transcript':
      console.log(`${stamp()}  распознано: «${m.text}»`);
      break;
    case 'agent':
      console.log(`${stamp()}  отвечает: ${m.name} (${m.reason})`);
      break;
    case 'delegate':
      console.log(`${stamp()}  ${m.from} спрашивает ${m.name}…`);
      break;
    case 'reply_delta':
      replyText += m.text;
      process.stdout.write(`${stamp()}  текст: ${m.text}\n`);
      break;
    case 'audio_start':
      gotAudioStart = true;
      console.log(`${stamp()}  ♪ начало звука (${m.rate} Гц)`);
      break;
    case 'audio_end':
      gotAudioEnd = true;
      console.log(`${stamp()}  ♪ конец звука (${m.seconds} с)`);
      maybeDone();
      break;
    case 'reply_end':
      gotReplyEnd = true;
      if (!replyText) replyText = m.text;
      maybeDone();
      break;
    case 'error':
      console.log(`${stamp()}  ⚠ ошибка: ${m.message}`);
      break;
    case 'state':
      if (m.state === 'idle' && gotReplyEnd) maybeDone();
      break;
    default:
      console.log(`${stamp()}  ${JSON.stringify(m)}`);
  }
});

ws.on('unexpected-response', (req, res) => {
  console.error(`✖ сервер ответил ${res.statusCode} — проверь --token / AUTH_TOKEN`);
  process.exit(1);
});
ws.on('error', (e) => {
  console.error(`✖ ${e.message}`);
  process.exit(1);
});
ws.on('close', () => process.exit(0));
