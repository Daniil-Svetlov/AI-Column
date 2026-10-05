// Подключения колонок по WebSocket. Протокол описан в docs/API.md.
import { DeviceSession } from './session.js';

export const PROTOCOL_VERSION = 1;

export class DeviceHub {
  constructor({ assistant, registry, store, config, logger }) {
    Object.assign(this, { assistant, registry, store, config, log: logger });
    this.sessions = new Map();
    registry.onChange(() => this.broadcastAgents());
  }

  agentsForDevice() {
    return this.registry.list().map((a) => ({ id: a.id, name: a.name, color: a.color, available: a.available }));
  }

  broadcastAgents() {
    const agents = this.agentsForDevice();
    for (const s of this.sessions.values()) s.send({ type: 'agents', agents });
  }

  get(id) {
    return this.sessions.get(id) ?? null;
  }

  list() {
    return [...this.sessions.values()].map((s) => s.info());
  }

  /** Новое WebSocket-подключение колонки. */
  connect(ws, { deviceId, remote }) {
    const old = this.sessions.get(deviceId);
    if (old) {
      this.log.info(`[${deviceId}] переподключение — закрываю старое соединение`);
      this.assistant.cancelTurn(old, 'переподключение');
      old.ws.close(4000, 'replaced');
    }
    const saved = this.store.getDevice(deviceId);
    const session = new DeviceSession({
      ws,
      deviceId,
      store: this.store,
      logger: this.log,
      defaults: {
        agent: saved?.agent ?? this.config.router.default,
        volume: saved?.volume ?? 60,
        playRate: this.config.speech.play_rate,
      },
    });
    if (session.agent !== 'auto' && !this.registry.has(session.agent)) session.agent = 'auto';
    this.sessions.set(deviceId, session);
    this.log.info(`[${deviceId}] подключилась (${remote})`);

    ws.on('message', (data, isBinary) => {
      try {
        if (isBinary) this._onAudio(session, data);
        else this._onJson(session, JSON.parse(data.toString('utf8')));
      } catch (e) {
        this.log.warn(`[${deviceId}] плохое сообщение: ${e.message}`);
      }
    });
    ws.on('close', (code) => {
      this.assistant.cancelTurn(session, 'колонка отключилась');
      if (this.sessions.get(deviceId) === session) this.sessions.delete(deviceId);
      this.log.info(`[${deviceId}] отключилась (${code})`);
    });
    ws.on('error', (e) => this.log.warn(`[${deviceId}] ошибка сокета: ${e.message}`));
    return session;
  }

  _welcome(session) {
    session.send({
      type: 'welcome',
      protocol: PROTOCOL_VERSION,
      assistant: this.config.assistant.name,
      agents: this.agentsForDevice(),
      agent: session.agent,
      volume: session.volume,
      play_rate: session.playRate,
      max_utterance_seconds: this.config.assistant.max_utterance_seconds,
    });
  }

  _onAudio(session, data) {
    const u = session.utterance;
    if (!u) return; // звук вне режима прослушивания игнорируем
    u.chunks.push(Buffer.from(data));
    u.bytes += data.length;
    const maxBytes = this.config.assistant.max_utterance_seconds * session.micRate * 2;
    if (u.bytes >= maxBytes) {
      session.send({ type: 'stop_listening', reason: 'max_duration' });
      this._finishUtterance(session);
    }
  }

  _finishUtterance(session) {
    const u = session.utterance;
    session.utterance = null;
    if (!u) return;
    const pcm = Buffer.concat(u.chunks, u.bytes);
    this.assistant.handleUtterance(session, pcm, session.micRate);
  }

  _onJson(session, msg) {
    switch (msg.type) {
      case 'hello': {
        session.fw = msg.fw ?? null;
        if (Number.isInteger(msg.mic_rate) && msg.mic_rate >= 8000) session.micRate = msg.mic_rate;
        if (Number.isInteger(msg.play_rate) && msg.play_rate >= 8000) session.playRate = msg.play_rate;
        if (Number.isFinite(msg.battery)) session.battery = msg.battery;
        if (Number.isFinite(msg.volume)) session.volume = Math.max(0, Math.min(100, Math.round(msg.volume))); // колонка — главная по своей громкости
        this.store.upsertDevice({ id: session.deviceId, fw: session.fw, agent: session.agent, volume: session.volume });
        this.log.info(`[${session.deviceId}] hello: прошивка ${session.fw}, микрофон ${session.micRate} Гц, динамик ${session.playRate} Гц`);
        this._welcome(session);
        break;
      }
      case 'listen_start': {
        this.assistant.cancelTurn(session, 'начали говорить заново');
        session.utterance = { chunks: [], bytes: 0, startedAt: Date.now() };
        session.state = 'listening';
        break;
      }
      case 'listen_stop':
        this._finishUtterance(session);
        break;
      case 'cancel':
        session.utterance = null;
        this.assistant.cancelTurn(session, 'отменено на колонке');
        session.setState('idle');
        break;
      case 'set_agent':
        if (msg.agent === 'auto' || this.registry.has(msg.agent)) session.setAgent(msg.agent, { notify: false });
        break;
      case 'set_volume':
        if (Number.isFinite(msg.value)) session.setVolume(msg.value, { notify: false });
        break;
      case 'new_conversation':
        session.history = [];
        this.log.info(`[${session.deviceId}] новый разговор`);
        break;
      case 'status':
        if (Number.isFinite(msg.battery)) session.battery = msg.battery;
        if (Number.isFinite(msg.rssi)) session.rssi = msg.rssi;
        if (Number.isFinite(msg.volume)) session.volume = msg.volume;
        break;
      case 'text':
        if (typeof msg.text === 'string' && msg.text.trim()) this.assistant.handleText(session, msg.text.trim(), { agent: msg.agent });
        break;
      case 'ping':
        session.send({ type: 'pong', t: msg.t ?? null });
        break;
      default:
        this.log.debug(`[${session.deviceId}] неизвестный тип сообщения: ${msg.type}`);
    }
  }

  closeAll() {
    for (const s of this.sessions.values()) {
      this.assistant.cancelTurn(s, 'сервер останавливается');
      s.ws.close(1001, 'server shutdown');
    }
    this.sessions.clear();
  }
}
