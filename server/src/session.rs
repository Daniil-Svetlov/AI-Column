//! Состояние одной подключённой колонки.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::agents::Msg;
use crate::store::{DeviceUpdate, Store};

/// Что уходит в сокет колонки.
pub enum Outgoing {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

pub struct TurnHandle {
    pub id: u64,
    pub cancel: CancellationToken,
    pub done: watch::Receiver<bool>,
}

pub struct SessionState {
    pub fw: Option<String>,
    pub mic_rate: u32,
    pub play_rate: u32,
    pub volume: u8,
    pub agent: String,
    pub battery: Option<f64>,
    pub rssi: Option<f64>,
    pub state: String,
    pub history: Vec<Msg>,
    pub last_turn_at: i64,
    /// PCM16 текущей фразы (None — колонка не слушает).
    pub utterance: Option<Vec<u8>>,
    pub turn: Option<TurnHandle>,
    pub turn_seq: u64,
    pub last_agent_used: Option<String>,
    /// Когда колонка доиграет текущий ответ (оценка, мс).
    pub speaking_until: i64,
}

pub struct DeviceSession {
    device_id: String,
    tx: mpsc::UnboundedSender<Outgoing>,
    open: AtomicBool,
    /// Сколько байт ждёт отправки в сокет (аналог ws.bufferedAmount).
    queued: Arc<AtomicUsize>,
    store: Option<Arc<Store>>,
    connected_at: i64,
    st: Mutex<SessionState>,
}

pub struct SessionDefaults {
    pub agent: String,
    pub volume: u8,
    pub play_rate: u32,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl DeviceSession {
    pub fn new(
        device_id: &str,
        tx: mpsc::UnboundedSender<Outgoing>,
        queued: Arc<AtomicUsize>,
        store: Option<Arc<Store>>,
        d: SessionDefaults,
    ) -> DeviceSession {
        DeviceSession {
            device_id: device_id.into(),
            tx,
            open: AtomicBool::new(true),
            queued,
            store,
            connected_at: now(),
            st: Mutex::new(SessionState {
                fw: None,
                mic_rate: 16000,
                play_rate: d.play_rate,
                volume: d.volume,
                agent: d.agent,
                battery: None,
                rssi: None,
                state: "idle".into(),
                history: vec![],
                last_turn_at: 0,
                utterance: None,
                turn: None,
                turn_seq: 0,
                last_agent_used: None,
                speaking_until: 0,
            }),
        }
    }

    /// Сессия без сокета (тесты инструментов): сообщения уходят в возвращённый приёмник.
    pub fn detached(device_id: &str, volume: u8) -> (Arc<DeviceSession>, mpsc::UnboundedReceiver<Outgoing>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let s = DeviceSession::new(
            device_id,
            tx,
            Arc::new(AtomicUsize::new(0)),
            None,
            SessionDefaults { agent: "auto".into(), volume, play_rate: 24000 },
        );
        (Arc::new(s), rx)
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn state(&self) -> MutexGuard<'_, SessionState> {
        self.st.lock().unwrap()
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed) && !self.tx.is_closed()
    }

    pub fn mark_closed(&self) {
        self.open.store(false, Ordering::Relaxed);
    }

    pub fn buffered(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    pub fn send(&self, v: Value) {
        if self.is_open() {
            let _ = self.tx.send(Outgoing::Text(v.to_string()));
        }
    }

    pub fn send_audio(&self, bytes: Vec<u8>) {
        if self.is_open() {
            self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
            let _ = self.tx.send(Outgoing::Binary(bytes));
        }
    }

    pub fn close(&self, code: u16, reason: &str) {
        let _ = self.tx.send(Outgoing::Close(code, reason.into()));
        self.mark_closed();
    }

    pub fn set_state(&self, state: &str) {
        self.state().state = state.into();
        self.send(json!({"type": "state", "state": state}));
    }

    pub fn volume(&self) -> u8 {
        self.state().volume
    }

    pub fn play_rate(&self) -> u32 {
        self.state().play_rate
    }

    pub fn agent(&self) -> String {
        self.state().agent.clone()
    }

    pub fn set_volume(&self, v: u8, notify: bool) {
        let v = v.min(100);
        self.state().volume = v;
        if notify {
            self.send(json!({"type": "set_volume", "value": v}));
        }
        if let Some(s) = &self.store {
            s.upsert_device(DeviceUpdate { id: &self.device_id, volume: Some(v as i64), ..Default::default() });
        }
    }

    pub fn set_agent(&self, id: &str, notify: bool) {
        self.state().agent = id.into();
        if notify {
            self.send(json!({"type": "agent_selected", "agent": id}));
        }
        if let Some(s) = &self.store {
            s.upsert_device(DeviceUpdate { id: &self.device_id, agent: Some(id), ..Default::default() });
        }
    }

    pub fn set_lights(&self, mode: &str, color: Option<&str>, brightness: Option<u8>) {
        let mut m = json!({"type": "led", "mode": mode});
        if let Some(c) = color {
            m["color"] = json!(c);
        }
        if let Some(b) = brightness {
            m["brightness"] = json!(b);
        }
        self.send(m);
    }

    pub fn info(&self) -> Value {
        let online = self.is_open();
        let s = self.state();
        let state = if s.state == "speaking" && now() > s.speaking_until { "idle" } else { s.state.as_str() };
        json!({
            "id": self.device_id,
            "online": online,
            "fw": s.fw,
            "state": state,
            "agent": s.agent,
            "lastAgent": s.last_agent_used,
            "volume": s.volume,
            "battery": s.battery,
            "rssi": s.rssi,
            "connectedAt": self.connected_at,
            "historyTurns": s.history.len() / 2,
        })
    }
}
