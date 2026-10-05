// STT/TTS-клиенты против поддельных серверов (формат запросов как у Piper, speaches/OpenAI, whisper.cpp).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createStt } from '../src/speech/stt.js';
import { createTts } from '../src/speech/tts.js';
import { encodeWav } from '../src/audio/wav.js';
import { fakeHttp } from './helpers.js';

const tone = (n) => Int16Array.from({ length: n }, (_, i) => Math.round(Math.sin(i / 5) * 5000));

test('Piper: POST /synthesize с JSON, WAV 22050 → PCM нужной частоты', async () => {
  const fake = await fakeHttp((req, res, body) => {
    if (req.url !== '/synthesize') { res.writeHead(404); return res.end(); }
    assert.equal(body.text, 'Привет');
    assert.equal(body.voice, 'ru_RU-irina-medium');
    assert.ok(Math.abs(body.length_scale - 1 / 1.25) < 1e-9);
    res.writeHead(200, { 'Content-Type': 'audio/wav' });
    res.end(encodeWav(tone(22050, 22050), 22050));
  });
  try {
    const tts = createTts({ provider: 'piper', base_url: fake.url, voice: 'ru_RU-irina-medium', speed: 1.25, timeout_ms: 5000 });
    const pcm = await tts.synthesize('Привет', 24000);
    assert.ok(Math.abs(pcm.length - 24000) <= 1);
  } finally {
    await fake.close();
  }
});

test('Piper старой версии: при 404 на /synthesize пробуем POST /', async () => {
  const fake = await fakeHttp((req, res) => {
    if (req.url === '/') {
      res.writeHead(200, { 'Content-Type': 'audio/wav' });
      return res.end(encodeWav(tone(1600, 16000), 16000));
    }
    res.writeHead(404);
    return res.end('not found');
  });
  try {
    const tts = createTts({ provider: 'piper', base_url: fake.url, timeout_ms: 5000 });
    assert.equal((await tts.synthesize('Тест', 16000)).length, 1600);
    assert.deepEqual(fake.requests.map((r) => r.url), ['/synthesize', '/']);
  } finally {
    await fake.close();
  }
});

test('OpenAI-совместимый TTS: /audio/speech, формат wav и сырой pcm', async () => {
  const fake = await fakeHttp((req, res, body) => {
    assert.equal(req.url, '/v1/audio/speech');
    assert.equal(req.headers.authorization, 'Bearer k');
    res.writeHead(200);
    if (body.response_format === 'pcm') return res.end(Buffer.from(tone(2400, 24000).buffer));
    return res.end(encodeWav(tone(2400, 24000), 24000));
  });
  try {
    const wav = createTts({ provider: 'openai', base_url: `${fake.url}/v1`, api_key: 'k', model: 'tts-1', voice: 'alloy', timeout_ms: 5000 });
    assert.equal((await wav.synthesize('a', 24000)).length, 2400);
    const pcm = createTts({ provider: 'openai', base_url: `${fake.url}/v1`, api_key: 'k', response_format: 'pcm', timeout_ms: 5000 });
    assert.equal((await pcm.synthesize('a', 12000)).length, 1200);
    assert.equal(fake.requests[0].body.input, 'a');
  } finally {
    await fake.close();
  }
});

test('OpenAI-совместимый STT: multipart с WAV, языком и подсказкой имён агентов', async () => {
  const fake = await fakeHttp((req, res, body) => {
    assert.equal(req.url, '/v1/audio/transcriptions');
    assert.match(req.headers['content-type'], /multipart\/form-data/);
    assert.match(body, /name="model"\r\n\r\nSystran\/faster-whisper-medium/);
    assert.match(body, /name="language"\r\n\r\nru/);
    assert.match(body, /name="prompt"\r\n\r\nClaude, Qwen\./);
    assert.match(body, /RIFF/);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ text: ' Привет, колонка! ' }));
  });
  try {
    const stt = createStt({ provider: 'openai', base_url: `${fake.url}/v1`, model: 'Systran/faster-whisper-medium', language: 'ru', timeout_ms: 5000 }, { hints: ['Claude', 'Qwen'] });
    assert.equal(await stt.transcribe(tone(16000, 16000), 16000), 'Привет, колонка!');
  } finally {
    await fake.close();
  }
});

test('если Whisper вернул саму подсказку (тишина) — считаем, что ничего не сказано', async () => {
  const fake = await fakeHttp((req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ text: 'Claude, Qwen' }));
  });
  try {
    const stt = createStt({ provider: 'openai', base_url: fake.url, model: 'm', timeout_ms: 5000 }, { hints: ['Claude', 'Qwen'] });
    assert.equal(await stt.transcribe(tone(1600), 16000), '');
  } finally {
    await fake.close();
  }
});

test('whisper.cpp: POST /inference', async () => {
  const fake = await fakeHttp((req, res, body) => {
    assert.equal(req.url, '/inference');
    assert.match(body, /name="response_format"\r\n\r\njson/);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ text: 'Который час?' }));
  });
  try {
    const stt = createStt({ provider: 'whispercpp', base_url: fake.url, language: 'ru', timeout_ms: 5000 });
    assert.equal(await stt.transcribe(tone(8000, 16000), 16000), 'Который час?');
  } finally {
    await fake.close();
  }
});

test('ошибка STT-сервера превращается в понятное исключение', async () => {
  const fake = await fakeHttp((req, res) => {
    res.writeHead(500);
    res.end('model not loaded');
  });
  try {
    const stt = createStt({ provider: 'openai', base_url: fake.url, model: 'm', timeout_ms: 5000 });
    await assert.rejects(stt.transcribe(tone(100, 16000), 16000), /STT HTTP 500: model not loaded/);
  } finally {
    await fake.close();
  }
});
