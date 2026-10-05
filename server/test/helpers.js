// Общие утилиты для тестов.
import http from 'node:http';
import WebSocket from 'ws';
import { normalizeConfig } from '../src/config.js';
import { createServer } from '../src/app.js';
import { createLogger } from '../src/logger.js';

export const quietLogger = createLogger('silent');

export function testConfig(overrides = {}) {
  const base = {
    server: { port: 0, host: '127.0.0.1', db_path: ':memory:', mdns: false, log_level: 'silent', auth_token: 'test-token' },
    speech: { stt: { provider: 'mock' }, tts: { provider: 'mock' }, play_rate: 16000 },
    router: { default: 'auto', default_agent: 'echo', fallback: ['echo'] },
    agents: [{ id: 'echo', name: 'Эхо', provider: 'echo', aliases: ['эхо'], delay_ms: 1 }],
  };
  const merged = { ...base, ...overrides };
  for (const k of ['server', 'speech', 'router', 'assistant', 'tools']) {
    if (base[k] || overrides[k]) merged[k] = { ...(base[k] ?? {}), ...(overrides[k] ?? {}) };
  }
  return normalizeConfig(merged);
}

export async function startTestServer(cfgOverrides = {}, opts = {}) {
  const config = testConfig(cfgOverrides);
  const app = await createServer({ config, logger: quietLogger, healthChecks: false, ...opts });
  const port = await app.listen(0, '127.0.0.1');
  return { app, port, config, base: `http://127.0.0.1:${port}`, wsUrl: `ws://127.0.0.1:${port}/ws` };
}

/** WebSocket-клиент, который копит сообщения и умеет ждать нужное. */
export async function deviceClient(wsUrl, { id = 'test-dev', token = 'test-token', hello = {} } = {}) {
  const ws = new WebSocket(`${wsUrl}?id=${id}&token=${token}`);
  const messages = [];
  const audio = [];
  const waiters = [];
  ws.on('message', (data, isBinary) => {
    if (isBinary) {
      audio.push(Buffer.from(data));
      messages.push({ type: '__audio', bytes: data.length });
    } else {
      messages.push(JSON.parse(data.toString()));
    }
    for (const w of [...waiters]) w();
  });
  await new Promise((resolve, reject) => {
    ws.once('open', resolve);
    ws.once('error', reject);
    ws.once('unexpected-response', (req, res) => reject(new Error(`HTTP ${res.statusCode}`)));
  });
  const client = {
    ws,
    messages,
    audio,
    send: (obj) => ws.send(JSON.stringify(obj)),
    sendAudio: (buf) => ws.send(buf, { binary: true }),
    types: () => messages.map((m) => m.type),
    find: (type) => messages.find((m) => m.type === type),
    clear: () => { messages.length = 0; audio.length = 0; },
    waitFor(pred, timeoutMs = 5000) {
      const test = typeof pred === 'string' ? (m) => m.type === pred : pred;
      return new Promise((resolve, reject) => {
        const check = () => {
          const hit = messages.find(test);
          if (hit) {
            waiters.splice(waiters.indexOf(check), 1);
            clearTimeout(t);
            resolve(hit);
          }
        };
        const t = setTimeout(() => {
          waiters.splice(waiters.indexOf(check), 1);
          reject(new Error(`не дождался ${typeof pred === 'string' ? pred : 'условия'}; пришло: ${messages.map((m) => m.type).join(', ')}`));
        }, timeoutMs);
        waiters.push(check);
        check();
      });
    },
    close: () => ws.close(),
  };
  client.send({ type: 'hello', fw: 'test', mic_rate: 16000, play_rate: 16000, ...hello });
  await client.waitFor('welcome');
  return client;
}

/** Ждёт конца ответа: reply_end и (если был звук) audio_end. */
export async function waitTurnEnd(client, timeoutMs = 8000) {
  await client.waitFor('reply_end', timeoutMs);
  if (client.find('audio_start')) await client.waitFor('audio_end', timeoutMs);
}

/** Маленький HTTP-сервер для имитации API провайдеров. */
export async function fakeHttp(handler) {
  const requests = [];
  const server = http.createServer(async (req, res) => {
    let body = '';
    for await (const c of req) body += c;
    const parsed = body && req.headers['content-type']?.includes('json') ? JSON.parse(body) : body;
    requests.push({ method: req.method, url: req.url, headers: req.headers, body: parsed });
    await handler(req, res, parsed, requests);
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    requests,
    close: () => new Promise((r) => { server.closeAllConnections?.(); server.close(r); }),
  };
}

export function sse(res, events) {
  res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' });
  for (const e of events) {
    if (e.event) res.write(`event: ${e.event}\n`);
    res.write(`data: ${typeof e.data === 'string' ? e.data : JSON.stringify(e.data)}\n\n`);
  }
  res.end();
}

export function speechPcm(seconds = 1, rate = 16000) {
  const n = Math.round(seconds * rate);
  const buf = Buffer.alloc(n * 2);
  for (let i = 0; i < n; i++) buf.writeInt16LE(Math.round(Math.sin((2 * Math.PI * 300 * i) / rate) * 8000), i * 2);
  return buf;
}
