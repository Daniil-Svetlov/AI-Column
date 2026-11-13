//! Сквозные проверки: «колонка» по WebSocket ↔ сервер с тестовыми STT/TTS и агентами.
mod common;

use common::{device, device_with, speech_pcm, start_server};
use serde_json::{Value, json};

fn broken_and_echo() -> Value {
    json!({
        "router": {"default": "auto", "default_agent": "broken", "fallback": ["broken", "echo"]},
        "agents": [
            {"id": "broken", "name": "Сломанный", "provider": "openai", "base_url": "http://127.0.0.1:9/v1", "model": "x", "aliases": ["сломанный"]},
            {"id": "echo", "name": "Эхо", "provider": "echo", "aliases": ["эхо"], "delay_ms": 1},
        ],
    })
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn api(base: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut rb = http().request(method.parse().unwrap(), format!("{base}{path}")).bearer_auth("test-token");
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn voice_turn_with_fallback_agent() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-1").await;
    let welcome = dev.find("welcome").unwrap().clone();
    assert_eq!(welcome["agent"], "auto");
    let ids: Vec<_> = welcome["agents"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["broken", "echo"]);

    srv.app.stt.mock_push("Как дела у колонки?");
    dev.send(json!({"type": "listen_start"}));
    for c in speech_pcm(1.0, 16000).chunks(640) {
        dev.send_audio(c);
    }
    dev.send(json!({"type": "listen_stop"}));
    dev.wait_turn_end().await;

    assert_eq!(dev.find("transcript").unwrap()["text"], "Как дела у колонки?");
    let agents: Vec<(String, String)> =
        dev.all("agent").iter().map(|a| (a["id"].as_str().unwrap().into(), a["reason"].as_str().unwrap().into())).collect();
    assert_eq!(agents, [("broken".into(), "default".into()), ("echo".into(), "fallback".into())]);
    assert!(dev.find("reply_end").unwrap()["text"].as_str().unwrap().contains("Ты сказал: Как дела у колонки?"));
    let types = dev.types();
    let pos = |t: &str| types.iter().position(|x| x == t).unwrap();
    assert!(pos("audio_start") < pos("audio_end"));
    assert_eq!(dev.find("audio_start").unwrap()["rate"], 16000);
    let bytes: usize = dev.audio.iter().map(Vec::len).sum();
    assert!(bytes > 16000, "мало звука: {bytes} байт");
    assert!(dev.audio.iter().all(|b| b.len() <= 4096));

    // Сломанный агент теперь помечен недоступным — следующий ход идёт сразу в echo.
    dev.clear();
    dev.send(json!({"type": "text", "text": "Ещё раз"}));
    dev.wait_turn_end().await;
    let ids: Vec<_> = dev.all("agent").iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["echo"]);
    dev.close();
    srv.app.close().await;
}

#[tokio::test]
async fn tool_from_agent_changes_volume_and_persists() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-2").await;
    dev.send(json!({"type": "text", "text": "Эхо, поставь громкость 35"}));
    let vol = dev.wait_type("set_volume").await;
    assert_eq!(vol["value"], 35);
    dev.wait_turn_end().await;
    assert!(dev.find("reply_end").unwrap()["text"].as_str().unwrap().starts_with("Громкость 35%"));
    dev.close();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Настройка сохранилась и вернётся при переподключении.
    let again = device(&srv.ws_url, "e2e-2").await;
    assert_eq!(again.find("welcome").unwrap()["volume"], 35);
    again.close();

    let (_, devices) = api(&srv.base, "GET", "/api/devices", None).await;
    assert!(devices.as_array().unwrap().iter().any(|d| d["id"] == "e2e-2" && d["volume"] == 35));
    let (_, hist) = api(&srv.base, "GET", "/api/history?limit=100", None).await;
    assert!(hist.as_array().unwrap().iter().any(|t| t["tools"].as_array().unwrap().iter().any(|c| c["name"] == "set_volume")));
    srv.app.close().await;
}

#[tokio::test]
async fn voice_switch_without_llm() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-3").await;
    dev.send(json!({"type": "text", "text": "Переключись на эхо"}));
    assert_eq!(dev.wait_type("agent_selected").await["agent"], "echo");
    dev.wait_turn_end().await;
    assert_eq!(dev.find("reply_end").unwrap()["text"], "Эхо на связи.");
    assert!(dev.find("agent").is_none());
    srv.app.close().await;
}

#[tokio::test]
async fn too_short_and_whisper_hallucination() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-4").await;
    dev.send(json!({"type": "listen_start"}));
    dev.send_audio(&speech_pcm(0.1, 16000));
    dev.send(json!({"type": "listen_stop"}));
    assert_eq!(dev.wait_type("error").await["message"], "Слишком коротко");
    dev.wait_for(|m| m["type"] == "state" && m["state"] == "idle").await;

    dev.clear();
    srv.app.stt.mock_push("Продолжение следует...");
    dev.send(json!({"type": "listen_start"}));
    dev.send_audio(&speech_pcm(1.0, 16000));
    dev.send(json!({"type": "listen_stop"}));
    dev.wait_turn_end().await;
    assert_eq!(dev.find("transcript").unwrap()["text"], "");
    assert!(dev.find("reply_end").unwrap()["text"].as_str().unwrap().contains("Не расслышал"));
    srv.app.close().await;
}

#[tokio::test]
async fn interrupt_cancels_current_answer() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-5").await;
    dev.send(json!({"type": "text", "text": "Эхо, расскажи очень длинную историю про колонку, микрофоны, динамики и агентов"}));
    dev.wait_type("reply_delta").await;
    dev.send(json!({"type": "cancel"}));
    dev.wait_for(|m| m["type"] == "state" && m["state"] == "idle").await;
    dev.clear();
    dev.send(json!({"type": "text", "text": "Эхо, короткий вопрос"}));
    dev.wait_turn_end().await;
    assert!(dev.find("reply_end").unwrap()["text"].as_str().unwrap().contains("короткий вопрос"));
    srv.app.close().await;
}

#[tokio::test]
async fn websocket_without_token_rejected() {
    let srv = start_server(broken_and_echo()).await;
    assert_eq!(device_with(&srv.ws_url, "x", "").await.err(), Some(401));
    srv.app.close().await;
}

#[tokio::test]
async fn rest_health_auth_ask_history() {
    let srv = start_server(broken_and_echo()).await;
    let health: Value = http().get(format!("{}/api/health", srv.base)).send().await.unwrap().json().await.unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["stt"], "mock");

    assert_eq!(http().get(format!("{}/api/agents", srv.base)).send().await.unwrap().status().as_u16(), 401);
    let (_, agents) = api(&srv.base, "GET", "/api/agents", None).await;
    let ids: Vec<_> = agents.as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["broken", "echo"]);

    let (_, ask) = api(&srv.base, "POST", "/api/ask", Some(json!({"text": "Эхо, привет из браузера"}))).await;
    assert_eq!(ask["agent"], "echo");
    assert_eq!(ask["reason"], "voice");
    assert!(ask["reply"].as_str().unwrap().contains("привет из браузера"));

    let (_, hist) = api(&srv.base, "GET", "/api/history?limit=100", None).await;
    assert!(hist.as_array().unwrap().iter().any(|t| t["device_id"] == "web" && t["agent_id"] == "echo"));

    // пустое тело без JSON — как у кнопки «проверить агентов» в панели
    let r = http().post(format!("{}/api/agents/check", srv.base)).bearer_auth("test-token").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);

    // веб-панель раздаётся из public/
    let index = http().get(format!("{}/", srv.base)).send().await.unwrap();
    assert_eq!(index.status().as_u16(), 200);
    assert!(index.text().await.unwrap().contains("<html"));
    srv.app.close().await;
}

#[tokio::test]
async fn rest_message_and_agent_switch_from_panel() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-6").await;
    let (s, _) = api(&srv.base, "POST", "/api/devices/e2e-6/agent", Some(json!({"agent": "echo"}))).await;
    assert_eq!(s, 200);
    assert_eq!(dev.wait_type("agent_selected").await["agent"], "echo");

    let (s, _) = api(&srv.base, "POST", "/api/devices/e2e-6/say", Some(json!({"text": "Ужин готов!"}))).await;
    assert_eq!(s, 200);
    assert_eq!(dev.wait_type("alert").await["text"], "Ужин готов!");
    dev.wait_turn_end().await;
    assert_eq!(dev.find("reply_end").unwrap()["text"], "Ужин готов!");

    let (s, _) = api(&srv.base, "POST", "/api/devices/nope/say", Some(json!({"text": "x"}))).await;
    assert_eq!(s, 404);
    srv.app.close().await;
}

#[tokio::test]
async fn timer_fires_and_device_announces() {
    let srv = start_server(broken_and_echo()).await;
    let mut dev = device(&srv.ws_url, "e2e-7").await;
    let session = srv.app.hub.get("e2e-7").unwrap();
    let mut ctx = ai_column_server::agents::ToolContext::new("echo");
    ctx.session = Some(session);
    ctx.timers = Some(srv.app.timers.clone());
    ctx.registry = Some(srv.app.registry.clone());
    let res = srv.app.toolbox.execute("set_timer", &json!({"seconds": 1, "label": "чай"}), &ctx).await;
    assert!(res.contains("Таймер «чай» поставлен на 1 секунду"), "{res}");
    let alert = dev.wait_for_ms(|m| m["type"] == "alert", 4000).await;
    assert_eq!(alert["kind"], "timer");
    assert!(alert["text"].as_str().unwrap().contains("чай"));
    srv.app.close().await;
}
