// Состояние одной подключённой колонки.
import WebSocket from 'ws';

export class DeviceSession {
  constructor({ ws, deviceId, store, logger, defaults = {} }) {
    this.ws = ws;
    this.deviceId = deviceId;
    this.store = store;
    this.log = logger;
    this.connectedAt = Date.now();
    this.fw = null;
    this.micRate = 16000;
    this.playRate = defaults.playRate ?? 24000;
    this.volume = defaults.volume ?? 60;
    this.agent = defaults.agent ?? 'auto';
    this.battery = null;
    this.rssi = null;
    this.state = 'idle';
    this.history = [];
    this.lastTurnAt = 0;
    this.utterance = null; // { chunks: Buffer[], bytes, startedAt, agent }
    this.turn = null; // { id, ctrl, promise }
    this.turnSeq = 0;
    this.lastAgentUsed = null;
    this.speakingUntil = 0; // когда колонка доиграет текущий ответ (оценка)
  }

  get isOpen() {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  send(obj) {
    if (this.isOpen) this.ws.send(JSON.stringify(obj));
  }

  sendAudio(buf) {
    if (this.isOpen) this.ws.send(buf, { binary: true });
  }

  setState(state) {
    this.state = state;
    this.send({ type: 'state', state });
  }

  setVolume(v, { notify = true } = {}) {
    this.volume = Math.max(0, Math.min(100, Math.round(v)));
    if (notify) this.send({ type: 'set_volume', value: this.volume });
    this.store?.upsertDevice({ id: this.deviceId, volume: this.volume });
  }

  setAgent(id, { notify = true } = {}) {
    this.agent = id;
    if (notify) this.send({ type: 'agent_selected', agent: id });
    this.store?.upsertDevice({ id: this.deviceId, agent: id });
  }

  setLights({ mode, color, brightness }) {
    this.send({ type: 'led', mode, ...(color ? { color } : {}), ...(brightness ? { brightness } : {}) });
  }

  info() {
    return {
      id: this.deviceId,
      online: this.isOpen,
      fw: this.fw,
      state: this.state === 'speaking' && Date.now() > this.speakingUntil ? 'idle' : this.state,
      agent: this.agent,
      lastAgent: this.lastAgentUsed,
      volume: this.volume,
      battery: this.battery,
      rssi: this.rssi,
      connectedAt: this.connectedAt,
      historyTurns: Math.floor(this.history.length / 2),
    };
  }
}
