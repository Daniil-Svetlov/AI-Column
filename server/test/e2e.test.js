// Сквозные проверки: «колонка» по WebSocket ↔ сервер с тестовыми STT/TTS и агентами.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import WebSocket from 'ws';
import { startTestServer, deviceClient, waitTurnEnd, speechPcm } from './helpers.js';
import { mockSttQueue } from '../src/speech/stt.js';

let srv;
before(async () => {
  srv = await startTestServer({
    router: { default: 'auto', default_agent: 'broken', fallback: ['broken', 'echo'] },
    agents: [
      { id: 'broken', name: 'Сломанный', provider: 'openai', base_url: 'http://127.0.0.1:9/v1', model: 'x', aliases: ['сломанный'] },
      { id: 'echo', name: 'Эхо', provider: 'echo', aliases: ['эхо'], delay_ms: 1 },
    ],
  });
});
after(async () => {
  await srv.app.close();
});

const auth = { Authorization: 'Bearer test-token', 'Content-Type': 'application/json' };

test('голосовой ход: звук → распознавание → запасной агент → текст и звук', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-1' });
  const welcome = dev.find('welcome');
  assert.equal(welcome.agent, 'auto');
  assert.deepEqual(welcome.agents.map((a) => a.id), ['broken', 'echo']);

  mockSttQueue.push('Как дела у колонки?');
  dev.send({ type: 'listen_start' });
  const pcm = speechPcm(1);
  for (let off = 0; off < pcm.length; off += 640) dev.sendAudio(pcm.subarray(off, off + 640));
  dev.send({ type: 'listen_stop' });
  await waitTurnEnd(dev);

  const types = dev.types();
  assert.equal(dev.find('transcript').text, 'Как дела у колонки?');
  const agents = dev.messages.filter((m) => m.type === 'agent');
  assert.deepEqual(agents.map((a) => [a.id, a.reason]), [['broken', 'default'], ['echo', 'fallback']]);
  assert.match(dev.find('reply_end').text, /Ты сказал: Как дела у колонки\?/);
  assert.ok(types.indexOf('audio_start') < types.indexOf('audio_end'));
  assert.equal(dev.find('audio_start').rate, 16000);
  const bytes = dev.audio.reduce((n, b) => n + b.length, 0);
  assert.ok(bytes > 16000, `мало звука: ${bytes} байт`);
  assert.ok(dev.audio.every((b) => b.length <= 4096));

  // Сломанный агент теперь помечен недоступным — следующий ход идёт сразу в echo.
  dev.clear();
  dev.send({ type: 'text', text: 'Ещё раз' });
  await waitTurnEnd(dev);
  assert.deepEqual(dev.messages.filter((m) => m.type === 'agent').map((a) => a.id), ['echo']);
  dev.close();
});

test('инструмент из ответа агента меняет громкость колонки', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-2' });
  dev.send({ type: 'text', text: 'Эхо, поставь громкость 35' });
  const vol = await dev.waitFor('set_volume');
  assert.equal(vol.value, 35);
  await waitTurnEnd(dev);
  assert.match(dev.find('reply_end').text, /^Громкость 35%/);
  dev.close();

  // Настройка сохранилась и вернётся при переподключении.
  const again = await deviceClient(srv.wsUrl, { id: 'e2e-2' });
  assert.equal(again.find('welcome').volume, 35);
  again.close();
});

test('«Переключись на эхо» меняет агента колонки без вызова LLM', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-3' });
  dev.send({ type: 'text', text: 'Переключись на эхо' });
  const sel = await dev.waitFor('agent_selected');
  assert.equal(sel.agent, 'echo');
  await waitTurnEnd(dev);
  assert.equal(dev.find('reply_end').text, 'Эхо на связи.');
  assert.equal(dev.find('agent'), undefined);
  dev.close();
});

test('слишком короткая фраза и тишина (галлюцинация Whisper)', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-4' });
  dev.send({ type: 'listen_start' });
  dev.sendAudio(speechPcm(0.1));
  dev.send({ type: 'listen_stop' });
  const err = await dev.waitFor('error');
  assert.equal(err.message, 'Слишком коротко');
  await dev.waitFor((m) => m.type === 'state' && m.state === 'idle');

  dev.clear();
  mockSttQueue.push('Продолжение следует...');
  dev.send({ type: 'listen_start' });
  dev.sendAudio(speechPcm(1));
  dev.send({ type: 'listen_stop' });
  await waitTurnEnd(dev);
  assert.equal(dev.find('transcript').text, '');
  assert.match(dev.find('reply_end').text, /Не расслышал/);
  dev.close();
});

test('перебивание: новая фраза отменяет текущий ответ', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-5' });
  dev.send({ type: 'text', text: 'Эхо, расскажи очень длинную историю про колонку, микрофоны, динамики и агентов' });
  await dev.waitFor('reply_delta');
  dev.send({ type: 'cancel' });
  await dev.waitFor((m) => m.type === 'state' && m.state === 'idle');
  dev.clear();
  dev.send({ type: 'text', text: 'Эхо, короткий вопрос' });
  await waitTurnEnd(dev);
  assert.match(dev.find('reply_end').text, /короткий вопрос/);
  dev.close();
});

test('WebSocket без токена отклоняется', async () => {
  const ws = new WebSocket(`${srv.wsUrl}?id=x`);
  const code = await new Promise((resolve) => {
    ws.on('unexpected-response', (req, res) => resolve(res.statusCode));
    ws.on('open', () => resolve(101));
    ws.on('error', () => {});
  });
  assert.equal(code, 401);
});

test('REST: health без токена, остальное с токеном; ask и история', async () => {
  const health = await (await fetch(`${srv.base}/api/health`)).json();
  assert.equal(health.ok, true);
  assert.equal(health.stt, 'mock');

  assert.equal((await fetch(`${srv.base}/api/agents`)).status, 401);
  const agents = await (await fetch(`${srv.base}/api/agents`, { headers: auth })).json();
  assert.deepEqual(agents.map((a) => a.id), ['broken', 'echo']);

  const ask = await (await fetch(`${srv.base}/api/ask`, { method: 'POST', headers: auth, body: JSON.stringify({ text: 'Эхо, привет из браузера' }) })).json();
  assert.equal(ask.agent, 'echo');
  assert.equal(ask.reason, 'voice');
  assert.match(ask.reply, /привет из браузера/);

  const hist = await (await fetch(`${srv.base}/api/history?limit=100`, { headers: auth })).json();
  assert.ok(hist.length >= 4);
  assert.ok(hist.some((t) => t.device_id === 'web' && t.agent_id === 'echo'));
  assert.ok(hist.some((t) => t.tools.some((c) => c.name === 'set_volume')));

  const devices = await (await fetch(`${srv.base}/api/devices`, { headers: auth })).json();
  assert.ok(devices.some((d) => d.id === 'e2e-2' && d.volume === 35));
});

test('REST: сообщение на колонку и смена агента из панели', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-6' });
  let r = await fetch(`${srv.base}/api/devices/e2e-6/agent`, { method: 'POST', headers: auth, body: JSON.stringify({ agent: 'echo' }) });
  assert.equal(r.status, 200);
  assert.equal((await dev.waitFor('agent_selected')).agent, 'echo');

  r = await fetch(`${srv.base}/api/devices/e2e-6/say`, { method: 'POST', headers: auth, body: JSON.stringify({ text: 'Ужин готов!' }) });
  assert.equal(r.status, 200);
  assert.equal((await dev.waitFor('alert')).text, 'Ужин готов!');
  await waitTurnEnd(dev);
  assert.equal(dev.find('reply_end').text, 'Ужин готов!');

  r = await fetch(`${srv.base}/api/devices/nope/say`, { method: 'POST', headers: auth, body: JSON.stringify({ text: 'x' }) });
  assert.equal(r.status, 404);
  dev.close();
});

test('таймер срабатывает и колонка объявляет его', async () => {
  const dev = await deviceClient(srv.wsUrl, { id: 'e2e-7' });
  const session = srv.app.hub.get('e2e-7');
  const res = await srv.app.toolbox.execute('set_timer', { seconds: 1, label: 'чай' }, { session, timers: srv.app.timers, registry: srv.app.registry });
  assert.match(res, /Таймер «чай» поставлен на 1 секунду/);
  const alert = await dev.waitFor('alert', 4000);
  assert.equal(alert.kind, 'timer');
  assert.match(alert.text, /чай/);
  dev.close();
});
