// REST API и веб-панель.
import path from 'node:path';
import crypto from 'node:crypto';
import express from 'express';

export function tokenOk(expected, got) {
  if (!expected) return true;
  if (!got) return false;
  const a = Buffer.from(String(expected));
  const b = Buffer.from(String(got));
  return a.length === b.length && crypto.timingSafeEqual(a, b);
}

export function extractToken(req) {
  const h = req.headers.authorization || '';
  if (h.startsWith('Bearer ')) return h.slice(7).trim();
  const url = new URL(req.url, 'http://x');
  return url.searchParams.get('token') || req.headers['x-auth-token'] || null;
}

function publicConfig(cfg) {
  const strip = (o) => Object.fromEntries(Object.entries(o).filter(([k]) => !/key|token|secret|password/i.test(k)));
  return {
    assistant: { name: cfg.assistant.name, history_turns: cfg.assistant.history_turns },
    router: cfg.router,
    speech: { stt: strip(cfg.speech.stt), tts: strip(cfg.speech.tts), play_rate: cfg.speech.play_rate },
    tools: cfg.tools.enabled,
  };
}

export function createHttpApp({ config, hub, assistant, registry, store, stt, tts, version, logger }) {
  const app = express();
  app.disable('x-powered-by');
  app.use(express.json({ limit: '256kb' }));

  app.get('/api/health', (req, res) => {
    res.json({
      ok: true,
      version,
      uptime: Math.round(process.uptime()),
      auth: !!config.server.auth_token,
      stt: stt.name,
      tts: tts.name,
      store: store.kind,
      devices: hub.list().length,
      agents: registry.list().map((a) => ({ id: a.id, available: a.available })),
    });
  });

  app.use('/api', (req, res, next) => {
    if (tokenOk(config.server.auth_token, extractToken(req))) return next();
    res.status(401).json({ error: 'нужен токен: заголовок Authorization: Bearer <AUTH_TOKEN>' });
  });

  app.get('/api/config', (req, res) => res.json(publicConfig(config)));

  app.get('/api/agents', (req, res) => res.json(registry.info()));
  app.post('/api/agents/check', async (req, res) => res.json(await registry.checkAll()));

  app.get('/api/devices', (req, res) => {
    const online = new Map(hub.list().map((d) => [d.id, d]));
    const all = store.listDevices().map((d) => online.get(d.id) ?? { id: d.id, online: false, fw: d.fw, agent: d.agent, volume: d.volume, lastSeen: d.last_seen });
    for (const d of online.values()) if (!all.some((x) => x.id === d.id)) all.push(d);
    res.json(all);
  });

  const withDevice = (handler) => async (req, res) => {
    const s = hub.get(req.params.id);
    if (!s) return res.status(404).json({ error: 'колонка не подключена' });
    return handler(s, req, res);
  };

  app.post('/api/devices/:id/agent', withDevice((s, req, res) => {
    const { agent } = req.body ?? {};
    if (agent !== 'auto' && !registry.has(agent)) return res.status(400).json({ error: `нет агента ${agent}` });
    s.setAgent(agent);
    res.json(s.info());
  }));
  app.post('/api/devices/:id/volume', withDevice((s, req, res) => {
    const v = Number(req.body?.volume);
    if (!Number.isFinite(v)) return res.status(400).json({ error: 'volume: число 0–100' });
    s.setVolume(v);
    res.json(s.info());
  }));
  app.post('/api/devices/:id/say', withDevice((s, req, res) => {
    const text = String(req.body?.text ?? '').trim();
    if (!text) return res.status(400).json({ error: 'text пустой' });
    assistant.announce(s, text, { alert: 'message' });
    res.json({ ok: true });
  }));
  app.post('/api/devices/:id/ask', withDevice((s, req, res) => {
    const text = String(req.body?.text ?? '').trim();
    if (!text) return res.status(400).json({ error: 'text пустой' });
    assistant.handleText(s, text, { agent: req.body?.agent });
    res.json({ ok: true });
  }));
  app.post('/api/devices/:id/reset', withDevice((s, req, res) => {
    s.history = [];
    res.json({ ok: true });
  }));

  app.get('/api/history', (req, res) => {
    const limit = Math.min(500, Number(req.query.limit) || 50);
    res.json(store.listTurns({ deviceId: req.query.device || undefined, limit, before: Number(req.query.before) || undefined }));
  });

  app.post('/api/ask', async (req, res) => {
    const text = String(req.body?.text ?? '').trim();
    if (!text) return res.status(400).json({ error: 'text пустой' });
    const signal = AbortSignal.timeout(120000);
    res.json(await assistant.ask({ text, agent: req.body?.agent, signal }));
  });

  const pub = path.resolve(config._baseDir ?? process.cwd(), config.server.public_dir);
  app.use(express.static(pub));

  app.use((err, req, res, _next) => {
    logger.error(`HTTP ${req.method} ${req.url}: ${err.stack || err.message}`);
    res.status(500).json({ error: err.message });
  });

  return app;
}
