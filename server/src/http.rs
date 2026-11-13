//! REST API, WebSocket-вход колонок и веб-панель.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router as AxumRouter;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

use crate::app::App;
use crate::config::public_config;
use crate::session::DeviceSession;

/// Сравнение токенов за постоянное время.
pub fn token_ok(expected: &str, got: Option<&str>) -> bool {
    if expected.is_empty() {
        return true;
    }
    let Some(got) = got else { return false };
    let (a, b) = (expected.as_bytes(), got.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn extract_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(h) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(t) = h.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("token=") {
                if !v.is_empty() {
                    return urlencoding::decode(v).ok().map(|s| s.into_owned());
                }
            }
        }
    }
    headers.get("x-auth-token").and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, axum::Json(json!({"error": msg.into()}))).into_response()
}

fn body_json(b: &Bytes) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

fn text_field(v: &Value) -> String {
    match &v["text"] {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

async fn auth(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    if token_ok(&app.config.server.auth_token, extract_token(req.headers(), req.uri().query()).as_deref()) {
        next.run(req).await
    } else {
        err(StatusCode::UNAUTHORIZED, "нужен токен: заголовок Authorization: Bearer <AUTH_TOKEN>")
    }
}

async fn health(State(app): State<Arc<App>>) -> Response {
    axum::Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": app.started.elapsed().as_secs(),
        "auth": !app.config.server.auth_token.is_empty(),
        "stt": app.stt.name(),
        "tts": app.tts.name(),
        "store": app.store.kind(),
        "devices": app.hub.list().len(),
        "agents": app.registry.list().iter().map(|a| json!({"id": a.id(), "available": a.available()})).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn config(State(app): State<Arc<App>>) -> Response {
    axum::Json(public_config(&app.config)).into_response()
}

async fn agents(State(app): State<Arc<App>>) -> Response {
    axum::Json(app.registry.info()).into_response()
}

async fn agents_check(State(app): State<Arc<App>>) -> Response {
    axum::Json(app.registry.check_all().await).into_response()
}

async fn devices(State(app): State<Arc<App>>) -> Response {
    let online: Vec<Value> = app.hub.list();
    let mut all: Vec<Value> = app
        .store
        .list_devices()
        .into_iter()
        .map(|d| {
            online.iter().find(|o| o["id"] == d.id.as_str()).cloned().unwrap_or_else(|| {
                json!({"id": d.id, "online": false, "fw": d.fw, "agent": d.agent, "volume": d.volume, "lastSeen": d.last_seen})
            })
        })
        .collect();
    for o in online {
        if !all.iter().any(|x| x["id"] == o["id"]) {
            all.push(o);
        }
    }
    axum::Json(all).into_response()
}

fn device(app: &App, id: &str) -> Result<Arc<DeviceSession>, Response> {
    app.hub.get(id).ok_or_else(|| err(StatusCode::NOT_FOUND, "колонка не подключена"))
}

async fn device_agent(State(app): State<Arc<App>>, Path(id): Path<String>, body: Bytes) -> Response {
    let s = match device(&app, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let b = body_json(&body);
    let agent = b["agent"].as_str().unwrap_or("");
    if agent != "auto" && !app.registry.has(agent) {
        return err(StatusCode::BAD_REQUEST, format!("нет агента {}", if agent.is_empty() { "undefined" } else { agent }));
    }
    s.set_agent(agent, true);
    axum::Json(s.info()).into_response()
}

async fn device_volume(State(app): State<Arc<App>>, Path(id): Path<String>, body: Bytes) -> Response {
    let s = match device(&app, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let b = body_json(&body);
    let v = b["volume"].as_f64().or_else(|| b["volume"].as_str().and_then(|s| s.parse().ok()));
    let Some(v) = v.filter(|v| v.is_finite()) else { return err(StatusCode::BAD_REQUEST, "volume: число 0–100") };
    s.set_volume(v.round().clamp(0.0, 100.0) as u8, true);
    axum::Json(s.info()).into_response()
}

async fn device_say(State(app): State<Arc<App>>, Path(id): Path<String>, body: Bytes) -> Response {
    let s = match device(&app, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let text = text_field(&body_json(&body));
    if text.is_empty() {
        return err(StatusCode::BAD_REQUEST, "text пустой");
    }
    let a = app.assistant.clone();
    tokio::spawn(async move { a.announce(&s, &text, Some("message")).await });
    axum::Json(json!({"ok": true})).into_response()
}

async fn device_ask(State(app): State<Arc<App>>, Path(id): Path<String>, body: Bytes) -> Response {
    let s = match device(&app, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let b = body_json(&body);
    let text = text_field(&b);
    if text.is_empty() {
        return err(StatusCode::BAD_REQUEST, "text пустой");
    }
    app.assistant.handle_text(&s, &text, b["agent"].as_str().map(str::to_string));
    axum::Json(json!({"ok": true})).into_response()
}

async fn device_reset(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    match device(&app, &id) {
        Ok(s) => {
            s.state().history.clear();
            axum::Json(json!({"ok": true})).into_response()
        }
        Err(r) => r,
    }
}

async fn history(State(app): State<Arc<App>>, Query(q): Query<HashMap<String, String>>) -> Response {
    let limit = q.get("limit").and_then(|v| v.parse::<usize>().ok()).filter(|v| *v > 0).unwrap_or(50).min(500);
    let before = q.get("before").and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0);
    let dev = q.get("device").map(String::as_str).filter(|d| !d.is_empty());
    axum::Json(app.store.list_turns(dev, limit, before)).into_response()
}

async fn ask(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let b = body_json(&body);
    let text = text_field(&b);
    if text.is_empty() {
        return err(StatusCode::BAD_REQUEST, "text пустой");
    }
    let cancel = CancellationToken::new();
    let guard = cancel.clone();
    let timeout = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        guard.cancel();
    });
    let r = app.assistant.ask(&text, b["agent"].as_str(), &cancel).await;
    timeout.abort();
    axum::Json(r).into_response()
}

async fn ws(
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let token = extract_token(&headers, None).or_else(|| q.get("token").cloned());
    if !token_ok(&app.config.server.auth_token, token.as_deref()) {
        app.log.warn(format!("отклонено подключение без верного токена с {}", addr.ip()));
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let raw = q.get("id").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| format!("column-{}", addr.ip()));
    let device_id: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "_.:-".contains(c) { c } else { '_' })
        .take(64)
        .collect();
    let hub = app.hub.clone();
    let remote = addr.ip().to_string();
    upgrade.max_message_size(1024 * 1024).on_upgrade(move |socket| hub.serve(socket, device_id, remote))
}

pub fn router(app: Arc<App>) -> AxumRouter {
    let public = app.config.base_dir.join(&app.config.server.public_dir);
    let api = AxumRouter::new()
        .route("/config", get(config))
        .route("/agents", get(agents))
        .route("/agents/check", post(agents_check))
        .route("/devices", get(devices))
        .route("/devices/{id}/agent", post(device_agent))
        .route("/devices/{id}/volume", post(device_volume))
        .route("/devices/{id}/say", post(device_say))
        .route("/devices/{id}/ask", post(device_ask))
        .route("/devices/{id}/reset", post(device_reset))
        .route("/history", get(history))
        .route("/ask", post(ask))
        .layer(middleware::from_fn_with_state(app.clone(), auth));
    AxumRouter::new()
        .route("/api/health", get(health))
        .nest("/api", api)
        .route("/ws", get(ws))
        .fallback_service(ServeDir::new(public))
        .with_state(app)
}
