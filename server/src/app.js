// Сборка сервера: конфиг → агенты → речь → WebSocket + REST.
import http from 'node:http';
import os from 'node:os';
import fs from 'node:fs';
import { WebSocketServer } from 'ws';
import { createLogger } from './logger.js';
import { createStore } from './db.js';
import { AgentRegistry } from './agents/registry.js';
import { Router } from './agents/router.js';
import { Toolbox, humanDuration } from './agents/tools.js';
import { createStt } from './speech/stt.js';
import { createTts } from './speech/tts.js';
import { TimerManager } from './timers.js';
import { Assistant } from './assistant.js';
import { DeviceHub } from './hub.js';
import { createHttpApp, extractToken, tokenOk } from './http.js';

const pkg = JSON.parse(fs.readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

/**
 * @param {object} p
 * @param {object} p.config — результат loadConfig()/normalizeConfig()
 * @param {object} [p.logger]
 * @param {boolean} [p.healthChecks=true]
 * @param {typeof fetch} [p.fetchImpl] — для тестов инструментов
 */
export async function createServer({ config, logger, healthChecks = true, fetchImpl } = {}) {
  const log = logger ?? createLogger(config.server.log_level, 'server');
  const store = await createStore(config.server.db_path, log);
  const registry = new AgentRegistry(config.agents, { logger: log.child('agent') });
  const router = new Router({ registry, routerConfig: config.router, logger: log.child('router') });
  const toolbox = new Toolbox({ config, fetchImpl, logger: log.child('tools') });
  const hints = [...new Set(registry.list().flatMap((a) => [a.name, ...a.aliases.filter((w) => /[а-яё]/i.test(w)).slice(0, 1)]))];
  const stt = createStt(config.speech.stt, { hints });
  const tts = createTts(config.speech.tts);

  let hub;
  const timers = new TimerManager({
    logger: log.child('timer'),
    onFire: (deviceId, t) => {
      const s = hub?.get(deviceId);
      const text = `Время вышло${t.label ? `: ${t.label}` : ''}! Прошло ${humanDuration(t.seconds)}.`;
      if (s) assistant.announce(s, text, { alert: 'timer' });
      else log.warn(`таймер для ${deviceId} сработал, но колонка не в сети`);
    },
  });
  const assistant = new Assistant({ config, registry, router, toolbox, stt, tts, store, timers, logger: log.child('turn') });
  hub = new DeviceHub({ assistant, registry, store, config, logger: log.child('ws') });

  const app = createHttpApp({ config, hub, assistant, registry, store, stt, tts, version: pkg.version, logger: log.child('http') });
  const server = http.createServer(app);
  const wss = new WebSocketServer({ noServer: true, maxPayload: 1024 * 1024 });

  server.on('upgrade', (req, socket, head) => {
    const url = new URL(req.url, 'http://x');
    if (url.pathname !== '/ws') {
      socket.destroy();
      return;
    }
    if (!tokenOk(config.server.auth_token, extractToken(req))) {
      log.warn(`отклонено подключение без верного токена с ${req.socket.remoteAddress}`);
      socket.write('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n');
      socket.destroy();
      return;
    }
    const deviceId = (url.searchParams.get('id') || `column-${req.socket.remoteAddress}`).replace(/[^\w.:-]/g, '_').slice(0, 64);
    wss.handleUpgrade(req, socket, head, (ws) => hub.connect(ws, { deviceId, remote: req.socket.remoteAddress }));
  });

  let bonjour = null;

  async function listen(port = config.server.port, host = config.server.host) {
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(port, host, () => {
        server.off('error', reject);
        resolve();
      });
    });
    const actual = server.address().port;
    log.info(`AI-Column ${pkg.version}: http://${host === '0.0.0.0' ? 'localhost' : host}:${actual}  (колонки: ws://<ip>:${actual}/ws)`);
    log.info(`STT: ${stt.name} | TTS: ${tts.name} | история: ${store.kind}`);
    log.info(`агенты: ${registry.list().map((a) => `${a.id} (${a.provider}:${a.model ?? '-'})`).join(', ')}`);
    if (!config.server.auth_token) log.warn('AUTH_TOKEN не задан — сервер открыт для всех в сети. Задай его в .env!');
    if (healthChecks) registry.startHealthChecks(60000);
    if (config.server.mdns) {
      try {
        const mod = await import('bonjour-service');
        const Bonjour = mod.Bonjour ?? mod.default?.Bonjour ?? mod.default;
        bonjour = new Bonjour();
        bonjour.publish({ name: `AI-Column ${os.hostname()}`.slice(0, 60), type: 'aicolumn', port: actual, txt: { path: '/ws', v: pkg.version } });
        log.info('mDNS: объявлен сервис _aicolumn._tcp — колонка найдёт сервер сама');
      } catch (e) {
        log.warn(`mDNS не запустился (${e.message}) — укажи адрес сервера в прошивке вручную`);
      }
    }
    return actual;
  }

  async function close() {
    registry.stopHealthChecks();
    timers.stop();
    hub.closeAll();
    try {
      bonjour?.unpublishAll();
      bonjour?.destroy();
    } catch { /* ignore */ }
    wss.close();
    await new Promise((r) => server.close(() => r()));
    server.closeAllConnections?.();
    store.close();
  }

  return { server, app, hub, assistant, registry, router, toolbox, store, stt, tts, timers, listen, close, log };
}
