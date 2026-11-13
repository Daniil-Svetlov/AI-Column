//! Провайдеры против поддельных серверов: стриминг, tools, think-блоки, ошибки, классификатор.
mod common;

use std::sync::Arc;

use ai_column_server::agents::{ErrorKind, Msg, Registry, Router, RunParams, ToolContext, Toolbox};
use ai_column_server::config::{Config, normalize_config};
use ai_column_server::logger::Logger;
use ai_column_server::session::DeviceSession;
use common::{FakeResp, fake_http};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn make(agent: Value) -> (Arc<Registry>, Toolbox, Arc<Config>, String) {
    let id = agent["id"].as_str().unwrap().to_string();
    let cfg = Arc::new(normalize_config(&json!({"agents": [agent], "tools": {"enabled": ["datetime", "volume"]}})).unwrap());
    let reg = Arc::new(Registry::new(&cfg.agents, Logger::silent()));
    let tb = Toolbox::new(cfg.clone(), Logger::silent());
    (reg, tb, cfg, id)
}

fn chunk(delta: Value, finish: Option<&str>) -> (Option<&'static str>, String) {
    (None, json!({"id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "qwen3:8b",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}).to_string())
}

fn done() -> (Option<&'static str>, String) {
    (None, "[DONE]".into())
}

#[tokio::test]
async fn openai_stream_think_tools_second_step() {
    let fake = fake_http(|r| {
        if r.path == "/v1/models" {
            return FakeResp::Json(200, json!({"object": "list", "data": [{"id": "qwen3:8b", "object": "model"}]}));
        }
        let has_tool = r.json()["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
        if !has_tool {
            return FakeResp::Sse(vec![
                chunk(json!({"role": "assistant", "content": "<think>пользователь хочет время"}), None),
                chunk(json!({"content": "</think>\n\nСекунду."}), None),
                chunk(json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "get_datetime", "arguments": ""}}]}), None),
                chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]}), None),
                chunk(json!({}), Some("tool_calls")),
                done(),
            ]);
        }
        FakeResp::Sse(vec![chunk(json!({"content": "Сейчас "}), None), chunk(json!({"content": "полдень."}), None), chunk(json!({}), Some("stop")), done()])
    })
    .await;
    let (reg, tb, _, id) =
        make(json!({"id": "qwen", "provider": "openai", "base_url": format!("{}/v1", fake.url), "model": "qwen3:8b", "no_think": true}));
    let agent = reg.get(&id).unwrap();
    assert_eq!(agent.health().await.ok, Some(true));

    let mut ctx = ToolContext::new("qwen");
    ctx.registry = Some(reg.clone());
    let history = [Msg::new("assistant", "лишнее"), Msg::new("user", "Привет"), Msg::new("assistant", "Привет!")];
    let mut streamed = String::new();
    let res = agent
        .run_guarded(
            RunParams { system: "Ты помощник.", history: &history, user_text: "Который час?", toolbox: Some(&tb), ctx: Some(&ctx), max_tokens: None },
            &mut |d: &str| streamed.push_str(d),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(res.text, "Секунду. Сейчас полдень.");
    assert_eq!(streamed, res.text);
    assert_eq!(res.tool_calls[0].name, "get_datetime");
    assert!(res.tool_calls[0].result.starts_with("Сейчас "), "{}", res.tool_calls[0].result);

    let chats: Vec<_> = fake.requests().into_iter().filter(|r| r.path == "/v1/chat/completions").collect();
    assert_eq!(chats.len(), 2);
    let first = chats[0].json();
    assert_eq!(first["stream"], true);
    assert!(first["messages"][0]["content"].as_str().unwrap().ends_with("/no_think"));
    let roles: Vec<_> = first["messages"].as_array().unwrap()[1..].iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user"], "история нормализована");
    assert!(first["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "get_datetime"));
    let second = chats[1].json();
    let msgs = second["messages"].as_array().unwrap();
    assert_eq!(msgs.iter().find(|m| m["role"] == "tool").unwrap()["tool_call_id"], "call_1");
    assert_eq!(msgs.iter().find(|m| m.get("tool_calls").is_some()).unwrap()["content"], "Секунду.");
}

#[tokio::test]
async fn openai_without_tools_support_retries() {
    let fake = fake_http(|r| {
        if r.json().get("tools").is_some() {
            return FakeResp::Json(400, json!({"error": {"message": "model does not support tools"}}));
        }
        FakeResp::Sse(vec![chunk(json!({"content": "Ок."}), None), chunk(json!({}), Some("stop")), done()])
    })
    .await;
    let (reg, tb, _, id) = make(json!({"id": "q", "provider": "openai", "base_url": format!("{}/v1", fake.url), "model": "m"}));
    let agent = reg.get(&id).unwrap();
    let mut ctx = ToolContext::new("q");
    ctx.registry = Some(reg.clone());
    let res = agent
        .run_guarded(
            RunParams { system: "s", history: &[], user_text: "x", toolbox: Some(&tb), ctx: Some(&ctx), max_tokens: None },
            &mut |_: &str| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(res.text, "Ок.");
    assert!(!agent.tools_supported());
}

#[tokio::test]
async fn openai_health_reports_missing_model() {
    let fake = fake_http(|_| FakeResp::Json(200, json!({"object": "list", "data": [{"id": "llama3:8b", "object": "model"}]}))).await;
    let (reg, _, _, id) = make(json!({"id": "q", "provider": "openai", "base_url": format!("{}/v1", fake.url), "model": "qwen3:8b"}));
    let st = reg.get(&id).unwrap().health().await;
    assert_eq!(st.ok, Some(false));
    assert!(st.error.unwrap().contains("нет модели «qwen3:8b»"));
}

#[tokio::test]
async fn unreachable_server_is_connection_error() {
    let (reg, _, _, id) = make(json!({"id": "q", "provider": "openai", "base_url": "http://127.0.0.1:9/v1", "model": "m"}));
    let e = reg
        .get(&id)
        .unwrap()
        .run_guarded(
            RunParams { system: "s", history: &[], user_text: "x", toolbox: None, ctx: None, max_tokens: None },
            &mut |_: &str| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Connection);
}

#[tokio::test]
async fn first_token_timeout() {
    let fake = fake_http(|_| FakeResp::Hang).await;
    let (reg, _, _, id) =
        make(json!({"id": "q", "provider": "openai", "base_url": format!("{}/v1", fake.url), "model": "m", "first_token_timeout_ms": 150}));
    let e = reg
        .get(&id)
        .unwrap()
        .run_guarded(
            RunParams { system: "s", history: &[], user_text: "x", toolbox: None, ctx: None, max_tokens: None },
            &mut |_: &str| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Timeout);
    assert!(e.message.contains("не начал отвечать"), "{}", e.message);
}

fn anthropic_events(blocks: Vec<Value>, stop: &str) -> Vec<(Option<&'static str>, String)> {
    let mut ev: Vec<(Option<&'static str>, String)> = vec![(Some("message_start"), json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-test", "content": [], "stop_reason": null, "usage": {"input_tokens": 5, "output_tokens": 1}}}).to_string())];
    for (i, b) in blocks.iter().enumerate() {
        if b["type"] == "text" {
            ev.push((Some("content_block_start"), json!({"type": "content_block_start", "index": i, "content_block": {"type": "text", "text": ""}}).to_string()));
            for p in b["parts"].as_array().unwrap() {
                ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": p}}).to_string()));
            }
        } else {
            ev.push((Some("content_block_start"), json!({"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": b["id"], "name": b["name"], "input": {}}}).to_string()));
            ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": b["input"].to_string()}}).to_string()));
        }
        ev.push((Some("content_block_stop"), json!({"type": "content_block_stop", "index": i}).to_string()));
    }
    ev.push((Some("message_delta"), json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 10}}).to_string()));
    ev.push((Some("message_stop"), json!({"type": "message_stop"}).to_string()));
    ev
}

#[tokio::test]
async fn anthropic_stream_tool_use_then_answer() {
    let fake = fake_http(|r| {
        assert_eq!(r.header("x-api-key"), "sk-test");
        assert_eq!(r.header("anthropic-version"), "2026-10-21");
        let body = r.json();
        let last = body["messages"].as_array().unwrap().last().unwrap().clone();
        if last["content"][0]["type"] == "tool_result" {
            return FakeResp::Sse(anthropic_events(vec![json!({"type": "text", "parts": ["Поставил ", "громкость."]})], "end_turn"));
        }
        FakeResp::Sse(anthropic_events(
            vec![json!({"type": "text", "parts": ["Хорошо."]}), json!({"type": "tool_use", "id": "toolu_1", "name": "set_volume", "input": {"level": 25}})],
            "tool_use",
        ))
    })
    .await;
    let (reg, tb, _, id) =
        make(json!({"id": "claude", "provider": "anthropic", "base_url": fake.url, "model": "claude-test", "api_key": "sk-test"}));
    let (session, _rx) = DeviceSession::detached("dev", 50);
    let mut ctx = ToolContext::new("claude");
    ctx.registry = Some(reg.clone());
    ctx.session = Some(session.clone());
    let mut streamed = String::new();
    let res = reg
        .get(&id)
        .unwrap()
        .run_guarded(
            RunParams { system: "sys", history: &[], user_text: "Сделай потише", toolbox: Some(&tb), ctx: Some(&ctx), max_tokens: None },
            &mut |d: &str| streamed.push_str(d),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(res.text, "Хорошо. Поставил громкость.");
    assert_eq!(streamed, res.text);
    assert_eq!(session.volume(), 25);
    let reqs: Vec<_> = fake.requests().into_iter().filter(|r| r.path == "/v1/messages").collect();
    assert_eq!(reqs.len(), 2);
    let first = reqs[0].json();
    assert_eq!(first["system"], "sys");
    assert!(first.get("temperature").is_none());
    assert!(first["tools"].as_array().unwrap().iter().any(|t| t["name"] == "set_volume" && t.get("input_schema").is_some()));
    let second = reqs[1].json();
    let asst = &second["messages"][1];
    assert_eq!(asst["role"], "assistant");
    assert_eq!(asst["content"][1]["input"], json!({"level": 25}), "блок tool_use вернулся в историю с разобранным input");
    let tr = &second["messages"][2]["content"][0];
    assert_eq!((tr["type"].as_str(), tr["tool_use_id"].as_str(), tr["content"].as_str()), (Some("tool_result"), Some("toolu_1"), Some("Громкость 25%")));
}

#[tokio::test]
async fn anthropic_refusal_fallback_on_new_models() {
    let fake = fake_http(|r| {
        assert_eq!(r.header("anthropic-beta"), "server-side-fallback-2026-07-01");
        assert_eq!(r.json()["fallbacks"], "default");
        FakeResp::Sse(anthropic_events(vec![json!({"type": "text", "parts": ["Да."]})], "end_turn"))
    })
    .await;
    let (reg, _, _, id) =
        make(json!({"id": "claude", "provider": "anthropic", "base_url": fake.url, "model": "claude-opus-5-5", "api_key": "k"}));
    let out = reg.get(&id).unwrap().complete("s", "x", 10, &CancellationToken::new()).await.unwrap();
    assert_eq!(out, "Да.");
}

#[tokio::test]
async fn anthropic_without_key_is_auth_error() {
    let (reg, _, _, id) = make(json!({"id": "claude", "provider": "anthropic", "model": "m", "api_key": ""}));
    let agent = reg.get(&id).unwrap();
    assert_eq!(agent.health().await.ok, Some(false));
    let e = agent.complete("s", "x", 10, &CancellationToken::new()).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Auth);
}

fn classifier_setup(fake_url: &str, timeout_ms: u64) -> Router {
    let cfg = normalize_config(&json!({
        "agents": [
            {"id": "claude", "name": "Claude", "provider": "echo", "aliases": ["клод"], "description": "сложное"},
            {"id": "qwen", "name": "Qwen", "provider": "openai", "base_url": format!("{fake_url}/v1"), "model": "m", "description": "быстрое"},
        ],
        "router": {"default_agent": "qwen", "classifier": {"agent": "qwen", "timeout_ms": timeout_ms}},
    }))
    .unwrap();
    let reg = Arc::new(Registry::new(&cfg.agents, Logger::silent()));
    Router::new(reg, cfg.router, Logger::silent())
}

#[tokio::test]
async fn classifier_picks_agent() {
    let fake = fake_http(|r| {
        assert!(r.json()["messages"][0]["content"].as_str().unwrap().contains("claude: сложное"));
        FakeResp::Sse(vec![chunk(json!({"content": "claude"}), Some("stop")), done()])
    })
    .await;
    let router = classifier_setup(&fake.url, 1000);
    let r = router.choose("Докажи теорему Ферма", "auto", &CancellationToken::new()).await;
    assert_eq!((r.agent_id.as_str(), r.reason), ("claude", "classifier"));
}

#[tokio::test]
async fn classifier_hang_falls_back_to_default() {
    let fake = fake_http(|_| FakeResp::Hang).await;
    let router = classifier_setup(&fake.url, 50);
    let r = router.choose("что-нибудь", "auto", &CancellationToken::new()).await;
    assert_eq!((r.agent_id.as_str(), r.reason), ("qwen", "default"));
}
