//! Подключения колонок по WebSocket. Протокол описан в docs/API.md.
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::agents::Registry;
use crate::assistant::Assistant;
use crate::config::Config;
use crate::logger::Logger;
use crate::session::{DeviceSession, Outgoing, SessionDefaults};
use crate::store::{DeviceUpdate, Store};

pub const PROTOCOL_VERSION: u32 = 1;

pub struct DeviceHub {
    assistant: Arc<Assistant>,
    registry: Arc<Registry>,
    store: Arc<Store>,
    config: Arc<Config>,
    log: Logger,
    sessions: Mutex<HashMap<String, Arc<DeviceSession>>>,
}

fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

impl DeviceHub {
    pub fn new(assistant: Arc<Assistant>, registry: Arc<Registry>, store: Arc<Store>, config: Arc<Config>, log: Logger) -> Arc<DeviceHub> {
        let hub = Arc::new(DeviceHub { assistant, registry, store, config, log, sessions: Mutex::new(HashMap::new()) });
        // Сменилась доступность агентов — сообщить колонкам.
        let weak = Arc::downgrade(&hub);
        let mut rx = hub.registry.subscribe();
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let Some(h) = weak.upgrade() else { break };
                h.broadcast_agents();
            }
        });
        hub
    }

    pub fn agents_for_device(&self) -> Vec<Value> {
        self.registry
            .list()
            .iter()
            .map(|a| json!({"id": a.id(), "name": a.name(), "color": a.color(), "available": a.available()}))
            .collect()
    }

    pub fn broadcast_agents(&self) {
        let msg = json!({"type": "agents", "agents": self.agents_for_device()});
        for s in self.sessions.lock().unwrap().values() {
            s.send(msg.clone());
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<DeviceSession>> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Value> {
        self.sessions.lock().unwrap().values().map(|s| s.info()).collect()
    }

    /// Обслуживает WebSocket-подключение колонки до его закрытия.
    pub async fn serve(self: Arc<Self>, ws: WebSocket, device_id: String, remote: String) {
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
        let queued = Arc::new(AtomicUsize::new(0));

        if let Some(old) = self.sessions.lock().unwrap().remove(&device_id) {
            self.log.info(format!("[{device_id}] переподключение — закрываю старое соединение"));
            self.assistant.cancel_turn(&old, "переподключение");
            old.close(4000, "replaced");
        }
        let saved = self.store.get_device(&device_id);
        let mut agent = saved.as_ref().and_then(|d| d.agent.clone()).unwrap_or_else(|| self.config.router.default.clone());
        if agent != "auto" && !self.registry.has(&agent) {
            agent = "auto".into();
        }
        let volume = saved.as_ref().and_then(|d| d.volume).unwrap_or(60).clamp(0, 100) as u8;
        let session = Arc::new(DeviceSession::new(
            &device_id,
            tx,
            queued.clone(),
            Some(self.store.clone()),
            SessionDefaults { agent, volume, play_rate: self.config.speech.play_rate },
        ));
        self.sessions.lock().unwrap().insert(device_id.clone(), session.clone());
        self.log.info(format!("[{device_id}] подключилась ({remote})"));

        // Писатель: всё, что сессия отправляет, — в сокет.
        let q = queued.clone();
        let writer = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                let r = match m {
                    Outgoing::Text(t) => sink.send(Message::Text(t.into())).await,
                    Outgoing::Binary(b) => {
                        let n = b.len();
                        let r = sink.send(Message::Binary(b.into())).await;
                        q.fetch_sub(n, Ordering::Relaxed);
                        r
                    }
                    Outgoing::Close(code, reason) => {
                        let _ = sink.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                        break;
                    }
                };
                if r.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let mut close_code: Option<u16> = None;
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(Message::Binary(b)) => self.on_audio(&session, &b),
                Ok(Message::Text(t)) => match serde_json::from_str::<Value>(&t) {
                    Ok(v) => self.on_json(&session, &v),
                    Err(e) => self.log.warn(format!("[{device_id}] плохое сообщение: {e}")),
                },
                Ok(Message::Close(f)) => {
                    close_code = f.map(|f| f.code);
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    self.log.warn(format!("[{device_id}] ошибка сокета: {e}"));
                    break;
                }
            }
        }
        session.mark_closed();
        self.assistant.cancel_turn(&session, "колонка отключилась");
        {
            let mut map = self.sessions.lock().unwrap();
            if map.get(&device_id).is_some_and(|s| Arc::ptr_eq(s, &session)) {
                map.remove(&device_id);
            }
        }
        writer.abort();
        self.log.info(format!(
            "[{device_id}] отключилась ({})",
            close_code.map(|c| c.to_string()).unwrap_or_else(|| "без кода".into())
        ));
    }

    fn welcome(&self, s: &DeviceSession) {
        let (agent, volume, play_rate) = {
            let st = s.state();
            (st.agent.clone(), st.volume, st.play_rate)
        };
        s.send(json!({
            "type": "welcome",
            "protocol": PROTOCOL_VERSION,
            "assistant": self.config.assistant.name,
            "agents": self.agents_for_device(),
            "agent": agent,
            "volume": volume,
            "play_rate": play_rate,
            "max_utterance_seconds": self.config.assistant.max_utterance_seconds,
        }));
    }

    fn on_audio(&self, s: &Arc<DeviceSession>, data: &[u8]) {
        let max_bytes = self.config.assistant.max_utterance_seconds as usize * s.state().mic_rate as usize * 2;
        let full = {
            let mut st = s.state();
            let Some(u) = st.utterance.as_mut() else { return }; // звук вне режима прослушивания игнорируем
            u.extend_from_slice(data);
            u.len() >= max_bytes
        };
        if full {
            s.send(json!({"type": "stop_listening", "reason": "max_duration"}));
            self.finish_utterance(s);
        }
    }

    fn finish_utterance(&self, s: &Arc<DeviceSession>) {
        let (pcm, rate) = {
            let mut st = s.state();
            (st.utterance.take(), st.mic_rate)
        };
        if let Some(pcm) = pcm {
            self.assistant.handle_utterance(s, pcm, rate);
        }
    }

    fn on_json(&self, s: &Arc<DeviceSession>, msg: &Value) {
        let id = s.device_id();
        match msg["type"].as_str().unwrap_or("") {
            "hello" => {
                {
                    let mut st = s.state();
                    st.fw = msg["fw"].as_str().map(str::to_string);
                    if let Some(r) = msg["mic_rate"].as_u64().filter(|r| *r >= 8000) {
                        st.mic_rate = r as u32;
                    }
                    if let Some(r) = msg["play_rate"].as_u64().filter(|r| *r >= 8000) {
                        st.play_rate = r as u32;
                    }
                    if let Some(b) = finite(&msg["battery"]) {
                        st.battery = Some(b);
                    }
                    // колонка — главная по своей громкости
                    if let Some(v) = finite(&msg["volume"]) {
                        st.volume = v.round().clamp(0.0, 100.0) as u8;
                    }
                }
                let (fw, agent, volume, mic, play) = {
                    let st = s.state();
                    (st.fw.clone(), st.agent.clone(), st.volume, st.mic_rate, st.play_rate)
                };
                self.store.upsert_device(DeviceUpdate { id, fw: fw.as_deref(), agent: Some(&agent), volume: Some(volume as i64) });
                self.log.info(format!(
                    "[{id}] hello: прошивка {}, микрофон {mic} Гц, динамик {play} Гц",
                    fw.as_deref().unwrap_or("?")
                ));
                self.welcome(s);
            }
            "listen_start" => {
                self.assistant.cancel_turn(s, "начали говорить заново");
                let mut st = s.state();
                st.utterance = Some(Vec::with_capacity(64 * 1024));
                st.state = "listening".into();
            }
            "listen_stop" => self.finish_utterance(s),
            "cancel" => {
                s.state().utterance = None;
                self.assistant.cancel_turn(s, "отменено на колонке");
                s.set_state("idle");
            }
            "set_agent" => {
                let a = msg["agent"].as_str().unwrap_or("");
                if a == "auto" || self.registry.has(a) {
                    s.set_agent(a, false);
                }
            }
            "set_volume" => {
                if let Some(v) = finite(&msg["value"]) {
                    s.set_volume(v.round().clamp(0.0, 100.0) as u8, false);
                }
            }
            "new_conversation" => {
                s.state().history.clear();
                self.log.info(format!("[{id}] новый разговор"));
            }
            "status" => {
                let mut st = s.state();
                if let Some(b) = finite(&msg["battery"]) {
                    st.battery = Some(b);
                }
                if let Some(r) = finite(&msg["rssi"]) {
                    st.rssi = Some(r);
                }
                if let Some(v) = finite(&msg["volume"]) {
                    st.volume = v.round().clamp(0.0, 100.0) as u8;
                }
            }
            "text" => {
                if let Some(t) = msg["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
                    let agent = msg["agent"].as_str().map(str::to_string);
                    self.assistant.handle_text(s, t, agent);
                }
            }
            "ping" => s.send(json!({"type": "pong", "t": msg["t"]})),
            other => self.log.debug(format!("[{id}] неизвестный тип сообщения: {other}")),
        }
    }

    pub fn close_all(&self) {
        let sessions: Vec<_> = self.sessions.lock().unwrap().drain().map(|(_, s)| s).collect();
        for s in sessions {
            self.assistant.cancel_turn(&s, "сервер останавливается");
            s.close(1001, "server shutdown");
        }
    }
}
