// Хранилище: устройства (настройки колонок) и история реплик.
// По умолчанию SQLite (better-sqlite3); если модуль не собрался — память (с предупреждением).
import fs from 'node:fs';
import path from 'node:path';

const SCHEMA = `
CREATE TABLE IF NOT EXISTS devices (
  id TEXT PRIMARY KEY,
  name TEXT,
  fw TEXT,
  agent TEXT,
  volume INTEGER,
  last_seen INTEGER,
  created_at INTEGER
);
CREATE TABLE IF NOT EXISTS turns (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT,
  ts INTEGER NOT NULL,
  agent_id TEXT,
  route_reason TEXT,
  user_text TEXT,
  reply_text TEXT,
  tools TEXT,
  stt_ms INTEGER,
  first_token_ms INTEGER,
  first_audio_ms INTEGER,
  total_ms INTEGER,
  error TEXT
);
CREATE INDEX IF NOT EXISTS idx_turns_device_ts ON turns(device_id, ts DESC);
`;

const TURN_FIELDS = ['device_id', 'ts', 'agent_id', 'route_reason', 'user_text', 'reply_text', 'tools', 'stt_ms', 'first_token_ms', 'first_audio_ms', 'total_ms', 'error'];

function rowToTurn(r) {
  return { ...r, tools: r.tools ? JSON.parse(r.tools) : [] };
}

export class MemoryStore {
  constructor() {
    this.kind = 'memory';
    this.devices = new Map();
    this.turns = [];
    this.seq = 0;
  }
  upsertDevice(d) {
    const now = Date.now();
    const prev = this.devices.get(d.id) ?? { id: d.id, created_at: now };
    const next = { ...prev, ...Object.fromEntries(Object.entries(d).filter(([, v]) => v !== undefined)), last_seen: now };
    this.devices.set(d.id, next);
    return next;
  }
  getDevice(id) {
    return this.devices.get(id) ?? null;
  }
  listDevices() {
    return [...this.devices.values()].sort((a, b) => b.last_seen - a.last_seen);
  }
  saveTurn(t) {
    const row = { id: ++this.seq, ...Object.fromEntries(TURN_FIELDS.map((f) => [f, t[f] ?? null])) };
    row.tools = t.tools ? JSON.stringify(t.tools) : null;
    this.turns.push(row);
    if (this.turns.length > 5000) this.turns.shift();
    return row.id;
  }
  listTurns({ deviceId, limit = 50, before } = {}) {
    return this.turns
      .filter((t) => (!deviceId || t.device_id === deviceId) && (!before || t.id < before))
      .slice(-limit)
      .reverse()
      .map(rowToTurn);
  }
  close() {}
}

export class SqliteStore {
  constructor(db) {
    this.kind = 'sqlite';
    this.db = db;
    db.pragma('journal_mode = WAL');
    db.exec(SCHEMA);
    this.stmt = {
      getDevice: db.prepare('SELECT * FROM devices WHERE id = ?'),
      insertDevice: db.prepare('INSERT INTO devices (id, name, fw, agent, volume, last_seen, created_at) VALUES (@id, @name, @fw, @agent, @volume, @last_seen, @created_at)'),
      updateDevice: db.prepare('UPDATE devices SET name = COALESCE(@name, name), fw = COALESCE(@fw, fw), agent = COALESCE(@agent, agent), volume = COALESCE(@volume, volume), last_seen = @last_seen WHERE id = @id'),
      listDevices: db.prepare('SELECT * FROM devices ORDER BY last_seen DESC'),
      insertTurn: db.prepare(`INSERT INTO turns (${TURN_FIELDS.join(', ')}) VALUES (${TURN_FIELDS.map((f) => `@${f}`).join(', ')})`),
    };
  }
  upsertDevice(d) {
    const now = Date.now();
    const p = { id: d.id, name: d.name ?? null, fw: d.fw ?? null, agent: d.agent ?? null, volume: d.volume ?? null, last_seen: now, created_at: now };
    if (this.stmt.getDevice.get(d.id)) this.stmt.updateDevice.run(p);
    else this.stmt.insertDevice.run(p);
    return this.getDevice(d.id);
  }
  getDevice(id) {
    return this.stmt.getDevice.get(id) ?? null;
  }
  listDevices() {
    return this.stmt.listDevices.all();
  }
  saveTurn(t) {
    const p = Object.fromEntries(TURN_FIELDS.map((f) => [f, t[f] ?? null]));
    p.tools = t.tools ? JSON.stringify(t.tools) : null;
    return Number(this.stmt.insertTurn.run(p).lastInsertRowid);
  }
  listTurns({ deviceId, limit = 50, before } = {}) {
    const where = [];
    const args = [];
    if (deviceId) { where.push('device_id = ?'); args.push(deviceId); }
    if (before) { where.push('id < ?'); args.push(before); }
    const sql = `SELECT * FROM turns ${where.length ? `WHERE ${where.join(' AND ')}` : ''} ORDER BY id DESC LIMIT ?`;
    return this.db.prepare(sql).all(...args, Math.min(500, Math.max(1, limit))).map(rowToTurn);
  }
  close() {
    this.db.close();
  }
}

export async function createStore(dbPath, logger) {
  if (!dbPath || dbPath === ':memory:' || dbPath === 'memory') return new MemoryStore();
  try {
    const { default: Database } = await import('better-sqlite3');
    fs.mkdirSync(path.dirname(path.resolve(dbPath)), { recursive: true });
    return new SqliteStore(new Database(dbPath));
  } catch (e) {
    logger?.warn(`SQLite недоступен (${e.message}) — история будет храниться только в памяти`);
    return new MemoryStore();
  }
}
