// Таймеры колонок. Живут на сервере: когда время выходит — колонка говорит об этом.
export class TimerManager {
  constructor({ onFire, logger } = {}) {
    this.onFire = onFire;
    this.log = logger;
    this.timers = new Map(); // deviceId -> Map(id -> {id, label, dueAt, handle})
    this.seq = 0;
  }

  add(deviceId, seconds, label = '') {
    const id = ++this.seq;
    const dueAt = Date.now() + seconds * 1000;
    const handle = setTimeout(() => this._fire(deviceId, id), seconds * 1000);
    handle.unref?.();
    if (!this.timers.has(deviceId)) this.timers.set(deviceId, new Map());
    this.timers.get(deviceId).set(id, { id, label, seconds, dueAt, handle });
    this.log?.info(`таймер #${id} для ${deviceId}: ${seconds} с «${label}»`);
    return id;
  }

  _fire(deviceId, id) {
    const t = this.timers.get(deviceId)?.get(id);
    if (!t) return;
    this.timers.get(deviceId).delete(id);
    this.onFire?.(deviceId, t);
  }

  list(deviceId) {
    return [...(this.timers.get(deviceId)?.values() ?? [])].map(({ handle, ...t }) => ({ ...t, leftSec: Math.max(0, Math.round((t.dueAt - Date.now()) / 1000)) }));
  }

  cancelAll(deviceId) {
    const m = this.timers.get(deviceId);
    if (!m) return 0;
    for (const t of m.values()) clearTimeout(t.handle);
    const n = m.size;
    m.clear();
    return n;
  }

  stop() {
    for (const id of this.timers.keys()) this.cancelAll(id);
  }
}
