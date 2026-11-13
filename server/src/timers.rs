//! Таймеры колонок. Живут на сервере: когда время выходит — колонка говорит об этом.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::logger::Logger;

#[derive(Clone, Debug, Serialize)]
pub struct TimerInfo {
    pub id: u64,
    pub label: String,
    pub seconds: u64,
    #[serde(rename = "dueAt")]
    pub due_at: i64,
}

struct Entry {
    info: TimerInfo,
    handle: tokio::task::AbortHandle,
}

/// Сработавший таймер: (id колонки, таймер).
pub type Fired = (String, TimerInfo);

pub struct TimerManager {
    timers: Mutex<HashMap<String, HashMap<u64, Entry>>>,
    seq: AtomicU64,
    fire_tx: mpsc::UnboundedSender<Fired>,
    log: Logger,
}

impl TimerManager {
    pub fn new(log: Logger) -> (Arc<TimerManager>, mpsc::UnboundedReceiver<Fired>) {
        let (fire_tx, rx) = mpsc::unbounded_channel();
        (Arc::new(TimerManager { timers: Mutex::new(HashMap::new()), seq: AtomicU64::new(0), fire_tx, log }), rx)
    }

    pub fn add(self: &Arc<Self>, device_id: &str, seconds: u64, label: &str) -> u64 {
        let id = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let me = Arc::downgrade(self);
        let dev = device_id.to_string();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            if let Some(m) = me.upgrade() {
                m.fire(&dev, id);
            }
        });
        let info = TimerInfo {
            id,
            label: label.to_string(),
            seconds,
            due_at: chrono::Utc::now().timestamp_millis() + seconds as i64 * 1000,
        };
        self.timers.lock().unwrap().entry(device_id.to_string()).or_default().insert(id, Entry { info, handle: task.abort_handle() });
        self.log.info(format!("таймер #{id} для {device_id}: {seconds} с «{label}»"));
        id
    }

    fn fire(&self, device_id: &str, id: u64) {
        let entry = self.timers.lock().unwrap().get_mut(device_id).and_then(|m| m.remove(&id));
        if let Some(e) = entry {
            let _ = self.fire_tx.send((device_id.to_string(), e.info));
        }
    }

    pub fn list(&self, device_id: &str) -> Vec<TimerInfo> {
        self.timers.lock().unwrap().get(device_id).map(|m| m.values().map(|e| e.info.clone()).collect()).unwrap_or_default()
    }

    pub fn cancel_all(&self, device_id: &str) -> usize {
        let mut t = self.timers.lock().unwrap();
        let Some(m) = t.get_mut(device_id) else { return 0 };
        for e in m.values() {
            e.handle.abort();
        }
        let n = m.len();
        m.clear();
        n
    }

    pub fn stop(&self) {
        let ids: Vec<String> = self.timers.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.cancel_all(&id);
        }
    }
}
