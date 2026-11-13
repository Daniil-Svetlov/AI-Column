//! Общие утилиты для интеграционных тестов.
#![allow(dead_code)]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai_column_server::app::App;
use ai_column_server::config::{Config, normalize_config};
use ai_column_server::logger::Logger;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

// ─────────────────────────────────────────── поддельный HTTP-сервер провайдера

#[derive(Clone, Debug)]
pub struct FakeReq {
    pub method: String,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl FakeReq {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> String {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }
}

pub enum FakeResp {
    Json(u16, Value),
    Bytes(u16, Vec<u8>),
    Text(u16, String),
    Sse(Vec<(Option<&'static str>, String)>),
    Hang,
}

type Handler = Arc<dyn Fn(&FakeReq) -> FakeResp + Send + Sync>;

pub struct Fake {
    pub url: String,
    pub requests: Arc<Mutex<Vec<FakeReq>>>,
}

impl Fake {
    pub fn requests(&self) -> Vec<FakeReq> {
        self.requests.lock().unwrap().clone()
    }
}

async fn fake_handler(State((h, reqs)): State<(Handler, Arc<Mutex<Vec<FakeReq>>>)>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let r = FakeReq {
        method: parts.method.to_string(),
        path: parts.uri.path_and_query().map(|p| p.to_string()).unwrap_or_default(),
        headers: parts.headers,
        body,
    };
    reqs.lock().unwrap().push(r.clone());
    let resp = h(&r);
    let build = |status: u16, ct: &str, body: Body| {
        Response::builder().status(StatusCode::from_u16(status).unwrap()).header("content-type", ct).body(body).unwrap()
    };
    match resp {
        FakeResp::Json(s, v) => build(s, "application/json", Body::from(v.to_string())),
        FakeResp::Bytes(s, b) => build(s, "application/octet-stream", Body::from(b)),
        FakeResp::Text(s, t) => build(s, "text/plain", Body::from(t)),
        FakeResp::Sse(events) => {
            let mut out = String::new();
            for (ev, data) in events {
                if let Some(e) = ev {
                    out += &format!("event: {e}\n");
                }
                out += &format!("data: {data}\n\n");
            }
            build(200, "text/event-stream", Body::from(out))
        }
        FakeResp::Hang => {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

pub async fn fake_http(h: impl Fn(&FakeReq) -> FakeResp + Send + Sync + 'static) -> Fake {
    let requests = Arc::new(Mutex::new(vec![]));
    let handler: Handler = Arc::new(h);
    let app = axum::Router::new().fallback(fake_handler).with_state((handler, requests.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Fake { url, requests }
}

// ─────────────────────────────────────────── тестовый сервер AI-Column

pub fn merge(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                if k == "agents" || !b.contains_key(k) || !v.is_object() {
                    b.insert(k.clone(), v.clone());
                } else {
                    merge(b.get_mut(k).unwrap(), v);
                }
            }
        }
        (b, e) => *b = e.clone(),
    }
}

pub fn test_config(overrides: Value) -> Config {
    let mut base = json!({
        "server": {"port": 0, "host": "127.0.0.1", "db_path": ":memory:", "mdns": false, "log_level": "silent", "auth_token": "test-token"},
        "speech": {"stt": {"provider": "mock"}, "tts": {"provider": "mock"}, "play_rate": 16000},
        "router": {"default": "auto", "default_agent": "echo", "fallback": ["echo"]},
        "agents": [{"id": "echo", "name": "Эхо", "provider": "echo", "aliases": ["эхо"], "delay_ms": 1}],
    });
    merge(&mut base, &overrides);
    let mut cfg = normalize_config(&base).unwrap();
    cfg.base_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    cfg
}

pub struct TestServer {
    pub app: Arc<App>,
    pub base: String,
    pub ws_url: String,
}

pub async fn start_server(overrides: Value) -> TestServer {
    let app = App::build(test_config(overrides), Logger::silent());
    let addr = app.listen("127.0.0.1", 0, false).await.unwrap();
    TestServer { app, base: format!("http://{addr}"), ws_url: format!("ws://{addr}/ws") }
}

// ─────────────────────────────────────────── «колонка» по WebSocket

pub struct Device {
    pub messages: Vec<Value>,
    pub audio: Vec<Vec<u8>>,
    rx: mpsc::UnboundedReceiver<Message>,
    tx: mpsc::UnboundedSender<Message>,
}

impl Device {
    pub fn send(&self, v: Value) {
        self.tx.send(Message::Text(v.to_string().into())).unwrap();
    }
    pub fn send_audio(&self, b: &[u8]) {
        self.tx.send(Message::Binary(b.to_vec().into())).unwrap();
    }
    pub fn types(&self) -> Vec<String> {
        self.messages.iter().map(|m| m["type"].as_str().unwrap_or("").to_string()).collect()
    }
    pub fn find(&self, t: &str) -> Option<&Value> {
        self.messages.iter().find(|m| m["type"] == t)
    }
    pub fn all(&self, t: &str) -> Vec<&Value> {
        self.messages.iter().filter(|m| m["type"] == t).collect()
    }
    pub fn clear(&mut self) {
        self.messages.clear();
        self.audio.clear();
    }
    pub fn close(&self) {
        let _ = self.tx.send(Message::Close(None));
    }

    pub async fn wait_for(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        self.wait_for_ms(pred, 5000).await
    }

    pub async fn wait_for_ms(&mut self, pred: impl Fn(&Value) -> bool, ms: u64) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            if let Some(m) = self.messages.iter().find(|m| pred(m)) {
                return m.clone();
            }
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(Message::Text(t))) => self.messages.push(serde_json::from_str(&t).unwrap()),
                Ok(Some(Message::Binary(b))) => {
                    self.messages.push(json!({"type": "__audio", "bytes": b.len()}));
                    self.audio.push(b.to_vec());
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("сокет закрыт; пришло: {:?}", self.types()),
                Err(_) => panic!("не дождался условия; пришло: {:?}", self.types()),
            }
        }
    }

    pub async fn wait_type(&mut self, t: &str) -> Value {
        let t = t.to_string();
        self.wait_for(move |m| m["type"] == t.as_str()).await
    }

    /// Ждёт конца ответа: reply_end и (если был звук) audio_end.
    pub async fn wait_turn_end(&mut self) {
        self.wait_for_ms(|m| m["type"] == "reply_end", 8000).await;
        if self.find("audio_start").is_some() {
            self.wait_for_ms(|m| m["type"] == "audio_end", 8000).await;
        }
    }
}

pub async fn device(ws_url: &str, id: &str) -> Device {
    device_with(ws_url, id, "test-token").await.unwrap()
}

pub async fn device_with(ws_url: &str, id: &str, token: &str) -> Result<Device, u16> {
    let url = format!("{ws_url}?id={id}&token={token}");
    let (ws, _) = match tokio_tungstenite::connect_async(&url).await {
        Ok(c) => c,
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => return Err(r.status().as_u16()),
        Err(e) => panic!("{e}"),
    };
    let (mut sink, mut stream) = ws.split();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    tokio::spawn(async move {
        while let Some(Ok(m)) = stream.next().await {
            if in_tx.send(m).is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
    });
    let mut d = Device { messages: vec![], audio: vec![], rx: in_rx, tx: out_tx };
    d.send(json!({"type": "hello", "fw": "test", "mic_rate": 16000, "play_rate": 16000}));
    d.wait_type("welcome").await;
    Ok(d)
}

pub fn speech_pcm(seconds: f64, rate: u32) -> Vec<u8> {
    let n = (seconds * rate as f64).round() as usize;
    (0..n)
        .flat_map(|i| (((2.0 * std::f64::consts::PI * 300.0 * i as f64 / rate as f64).sin() * 8000.0).round() as i16).to_le_bytes())
        .collect()
}

pub fn tone(n: usize) -> Vec<i16> {
    (0..n).map(|i| ((i as f64 / 5.0).sin() * 5000.0).round() as i16).collect()
}
