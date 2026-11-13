//! Хранилище: устройства (настройки колонок) и история реплик.
//! По умолчанию SQLite; если базу не открыть — память (с предупреждением).
use std::collections::HashMap;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::agents::ToolCall;
use crate::logger::Logger;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS devices (
  id TEXT PRIMARY KEY, name TEXT, fw TEXT, agent TEXT, volume INTEGER, last_seen INTEGER, created_at INTEGER
);
CREATE TABLE IF NOT EXISTS turns (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT, ts INTEGER NOT NULL, agent_id TEXT, route_reason TEXT, user_text TEXT, reply_text TEXT,
  tools TEXT, stt_ms INTEGER, first_token_ms INTEGER, first_audio_ms INTEGER, total_ms INTEGER, error TEXT
);
CREATE INDEX IF NOT EXISTS idx_turns_device_ts ON turns(device_id, ts DESC);
";

#[derive(Clone, Debug, Default, Serialize)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub fw: Option<String>,
    pub agent: Option<String>,
    pub volume: Option<i64>,
    pub last_seen: i64,
    pub created_at: i64,
}

/// Изменения для upsert_device: None — не трогать поле.
#[derive(Default)]
pub struct DeviceUpdate<'a> {
    pub id: &'a str,
    pub fw: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub volume: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Turn {
    pub id: i64,
    pub device_id: String,
    pub ts: i64,
    pub agent_id: Option<String>,
    pub route_reason: Option<String>,
    pub user_text: Option<String>,
    pub reply_text: Option<String>,
    pub tools: Value,
    pub stt_ms: Option<i64>,
    pub first_token_ms: Option<i64>,
    pub first_audio_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub error: Option<String>,
}

impl Turn {
    pub fn new(device_id: &str) -> Turn {
        Turn { device_id: device_id.into(), ts: now(), tools: Value::Array(vec![]), ..Default::default() }
    }
    pub fn tools(mut self, calls: &[ToolCall]) -> Turn {
        self.tools = serde_json::to_value(calls).unwrap_or_default();
        self
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

struct Memory {
    devices: HashMap<String, Device>,
    turns: Vec<Turn>,
    seq: i64,
}

enum Inner {
    Memory(Memory),
    Sqlite(Connection),
}

pub struct Store {
    inner: Mutex<Inner>,
}

fn row_to_device(r: &rusqlite::Row) -> rusqlite::Result<Device> {
    Ok(Device {
        id: r.get("id")?,
        name: r.get("name")?,
        fw: r.get("fw")?,
        agent: r.get("agent")?,
        volume: r.get("volume")?,
        last_seen: r.get::<_, Option<i64>>("last_seen")?.unwrap_or(0),
        created_at: r.get::<_, Option<i64>>("created_at")?.unwrap_or(0),
    })
}

fn row_to_turn(r: &rusqlite::Row) -> rusqlite::Result<Turn> {
    let tools: Option<String> = r.get("tools")?;
    Ok(Turn {
        id: r.get("id")?,
        device_id: r.get::<_, Option<String>>("device_id")?.unwrap_or_default(),
        ts: r.get("ts")?,
        agent_id: r.get("agent_id")?,
        route_reason: r.get("route_reason")?,
        user_text: r.get("user_text")?,
        reply_text: r.get("reply_text")?,
        tools: tools.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Array(vec![])),
        stt_ms: r.get("stt_ms")?,
        first_token_ms: r.get("first_token_ms")?,
        first_audio_ms: r.get("first_audio_ms")?,
        total_ms: r.get("total_ms")?,
        error: r.get("error")?,
    })
}

impl Store {
    pub fn memory() -> Store {
        Store { inner: Mutex::new(Inner::Memory(Memory { devices: HashMap::new(), turns: vec![], seq: 0 })) }
    }

    pub fn open(db_path: &str, log: &Logger) -> Store {
        if db_path.is_empty() || db_path == ":memory:" || db_path == "memory" {
            return Store::memory();
        }
        let open = || -> Result<Connection, String> {
            if let Some(dir) = std::path::Path::new(db_path).parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let db = Connection::open(db_path).map_err(|e| e.to_string())?;
            db.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
            db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
            Ok(db)
        };
        match open() {
            Ok(db) => Store { inner: Mutex::new(Inner::Sqlite(db)) },
            Err(e) => {
                log.warn(format!("SQLite недоступен ({e}) — история будет храниться только в памяти"));
                Store::memory()
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match &*self.inner.lock().unwrap() {
            Inner::Memory(_) => "memory",
            Inner::Sqlite(_) => "sqlite",
        }
    }

    pub fn upsert_device(&self, d: DeviceUpdate) {
        let now = now();
        match &mut *self.inner.lock().unwrap() {
            Inner::Memory(m) => {
                let e = m.devices.entry(d.id.to_string()).or_insert_with(|| Device { id: d.id.into(), created_at: now, ..Default::default() });
                if let Some(fw) = d.fw {
                    e.fw = Some(fw.into());
                }
                if let Some(a) = d.agent {
                    e.agent = Some(a.into());
                }
                if let Some(v) = d.volume {
                    e.volume = Some(v);
                }
                e.last_seen = now;
            }
            Inner::Sqlite(db) => {
                let r = db.execute(
                    "INSERT INTO devices (id, fw, agent, volume, last_seen, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                     ON CONFLICT(id) DO UPDATE SET fw = COALESCE(?2, fw), agent = COALESCE(?3, agent),
                       volume = COALESCE(?4, volume), last_seen = ?5",
                    params![d.id, d.fw, d.agent, d.volume, now],
                );
                if let Err(e) = r {
                    eprintln!("SQLite: {e}");
                }
            }
        }
    }

    pub fn get_device(&self, id: &str) -> Option<Device> {
        match &*self.inner.lock().unwrap() {
            Inner::Memory(m) => m.devices.get(id).cloned(),
            Inner::Sqlite(db) => db.query_row("SELECT * FROM devices WHERE id = ?1", [id], row_to_device).optional().ok().flatten(),
        }
    }

    pub fn list_devices(&self) -> Vec<Device> {
        match &*self.inner.lock().unwrap() {
            Inner::Memory(m) => {
                let mut v: Vec<Device> = m.devices.values().cloned().collect();
                v.sort_by_key(|d| std::cmp::Reverse(d.last_seen));
                v
            }
            Inner::Sqlite(db) => {
                let Ok(mut st) = db.prepare("SELECT * FROM devices ORDER BY last_seen DESC") else { return vec![] };
                st.query_map([], row_to_device).map(|rows| rows.flatten().collect()).unwrap_or_default()
            }
        }
    }

    pub fn save_turn(&self, mut t: Turn) -> i64 {
        match &mut *self.inner.lock().unwrap() {
            Inner::Memory(m) => {
                m.seq += 1;
                t.id = m.seq;
                m.turns.push(t);
                if m.turns.len() > 5000 {
                    m.turns.remove(0);
                }
                m.seq
            }
            Inner::Sqlite(db) => {
                let tools = serde_json::to_string(&t.tools).ok();
                let r = db.execute(
                    "INSERT INTO turns (device_id, ts, agent_id, route_reason, user_text, reply_text, tools, stt_ms,
                       first_token_ms, first_audio_ms, total_ms, error) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    params![
                        t.device_id, t.ts, t.agent_id, t.route_reason, t.user_text, t.reply_text, tools, t.stt_ms,
                        t.first_token_ms, t.first_audio_ms, t.total_ms, t.error
                    ],
                );
                match r {
                    Ok(_) => db.last_insert_rowid(),
                    Err(e) => {
                        eprintln!("SQLite: {e}");
                        0
                    }
                }
            }
        }
    }

    pub fn list_turns(&self, device_id: Option<&str>, limit: usize, before: Option<i64>) -> Vec<Turn> {
        let limit = limit.clamp(1, 500);
        match &*self.inner.lock().unwrap() {
            Inner::Memory(m) => m
                .turns
                .iter()
                .rev()
                .filter(|t| device_id.is_none_or(|d| t.device_id == d) && before.is_none_or(|b| t.id < b))
                .take(limit)
                .cloned()
                .collect(),
            Inner::Sqlite(db) => {
                let sql = "SELECT * FROM turns WHERE (?1 IS NULL OR device_id = ?1) AND (?2 IS NULL OR id < ?2) ORDER BY id DESC LIMIT ?3";
                let Ok(mut st) = db.prepare(sql) else { return vec![] };
                st.query_map(params![device_id, before, limit as i64], row_to_turn).map(|rows| rows.flatten().collect()).unwrap_or_default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(store: Store) {
        store.upsert_device(DeviceUpdate { id: "a", fw: Some("1"), volume: Some(40), ..Default::default() });
        store.upsert_device(DeviceUpdate { id: "a", agent: Some("qwen"), ..Default::default() });
        let d = store.get_device("a").unwrap();
        assert_eq!((d.fw.as_deref(), d.agent.as_deref(), d.volume), (Some("1"), Some("qwen"), Some(40)));
        for i in 0..3 {
            let mut t = Turn::new(if i == 1 { "b" } else { "a" });
            t.user_text = Some(format!("q{i}"));
            store.save_turn(t);
        }
        let all = store.list_turns(None, 50, None);
        assert_eq!(all.iter().map(|t| t.user_text.clone().unwrap()).collect::<Vec<_>>(), ["q2", "q1", "q0"]);
        assert_eq!(store.list_turns(Some("a"), 50, None).len(), 2);
        assert_eq!(store.list_turns(None, 50, Some(all[0].id)).len(), 2);
    }

    #[test]
    fn memory_store() {
        check(Store::memory());
    }

    #[test]
    fn sqlite_store() {
        let dir = std::env::temp_dir().join(format!("aic-test-{}", std::process::id()));
        let path = dir.join("t.db");
        check(Store::open(path.to_str().unwrap(), &Logger::silent()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
