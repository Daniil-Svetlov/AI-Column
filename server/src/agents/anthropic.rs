//! Claude через Anthropic Messages API: потоковый ответ (SSE) + инструменты.
//! Официального SDK для Rust нет, поэтому запросы собираются вручную.
use std::collections::HashMap;

use serde_json::{Value, json};

use super::sse::SseReader;
use super::{Agent, AgentError, ErrorKind, MAX_TOOL_STEPS, Output, RunParams, RunResult, ToolCall, normalize_history};

const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const DEFAULT_BASE: &str = "https://api.anthropic.com";

fn base(agent: &Agent) -> String {
    agent.cfg.base_url.as_deref().unwrap_or(DEFAULT_BASE).trim_end_matches('/').to_string()
}

/// Серверный запасной вариант при отказе модели (stop_reason=refusal) поддерживают модели 5-го поколения.
fn wants_refusal_fallback(agent: &Agent) -> bool {
    let model = agent.cfg.model.as_deref().unwrap_or("");
    agent.cfg.refusal_fallback.unwrap_or_else(|| {
        ["claude-fable-5", "claude-opus-5", "claude-sonnet-5-5", "claude-mythos-5"].iter().any(|p| model.starts_with(p))
    })
}

fn request(agent: &Agent, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let mut rb = agent
        .http
        .request(method, format!("{}{path}", base(agent)))
        .header("x-api-key", agent.cfg.api_key.as_deref().unwrap_or(""))
        .header("anthropic-version", API_VERSION);
    if wants_refusal_fallback(agent) && path.starts_with("/v1/messages") {
        rb = rb.header("anthropic-beta", FALLBACK_BETA);
    }
    rb
}

pub async fn probe(agent: &Agent) -> Result<(), String> {
    if agent.cfg.api_key.is_none() {
        return Err("не задан api_key (ANTHROPIC_API_KEY)".into());
    }
    let resp = request(agent, reqwest::Method::GET, "/v1/models?limit=1")
        .send()
        .await
        .map_err(|e| AgentError::from_reqwest(&e).message)?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let body = resp.text().await.unwrap_or_default();
        return Err(AgentError::from_status(status, &body).message);
    }
    Ok(())
}

pub async fn run(agent: &Agent, p: &RunParams<'_>, out: &mut Output<'_>) -> Result<RunResult, AgentError> {
    if agent.cfg.api_key.is_none() {
        return Err(AgentError::new(ErrorKind::Auth, "не задан ANTHROPIC_API_KEY"));
    }
    let mut messages: Vec<Value> = normalize_history(p.history, p.user_text)
        .into_iter()
        .map(|m| json!({"role": m.role, "content": m.content}))
        .collect();
    let toolbox = p.toolbox.filter(|_| agent.tools_enabled());
    let tools: Vec<Value> = match (toolbox, p.ctx) {
        (Some(tb), Some(ctx)) => tb
            .definitions(ctx)
            .into_iter()
            .map(|d| json!({"name": d.name, "description": d.description, "input_schema": d.parameters}))
            .collect(),
        _ => vec![],
    };

    let mut full = String::new();
    let mut pending_space = false;
    let mut tool_calls = vec![];

    for _step in 0..MAX_TOOL_STEPS {
        let mut body = json!({
            "model": agent.cfg.model,
            "max_tokens": p.max_tokens.unwrap_or(agent.cfg.max_tokens),
            "system": p.system,
            "messages": messages,
            "stream": true,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.clone());
        }
        if wants_refusal_fallback(agent) {
            body["fallbacks"] = json!("default");
        }
        let resp = request(agent, reqwest::Method::POST, "/v1/messages")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::from_reqwest(&e))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            return Err(AgentError::from_status(status, &text));
        }

        // Собираем блоки ответа целиком (text, tool_use, thinking…), чтобы вернуть их в историю как есть.
        let mut blocks: Vec<Value> = vec![];
        let mut partial_json: HashMap<usize, String> = HashMap::new();
        let mut stop_reason = String::new();
        let mut sse = SseReader::new(resp);
        while let Some(ev) = sse.next().await {
            let ev = ev.map_err(|e| AgentError::from_reqwest(&e))?;
            let Ok(data) = serde_json::from_str::<Value>(&ev.data) else { continue };
            let idx = data["index"].as_u64().unwrap_or(0) as usize;
            match data["type"].as_str().unwrap_or("") {
                "content_block_start" => {
                    let block = data["content_block"].clone();
                    if block["type"] == "tool_use" {
                        out.mark_started();
                    }
                    if blocks.len() <= idx {
                        blocks.resize(idx + 1, Value::Null);
                    }
                    blocks[idx] = block;
                }
                "content_block_delta" => {
                    let delta = &data["delta"];
                    let Some(block) = blocks.get_mut(idx) else { continue };
                    match delta["type"].as_str().unwrap_or("") {
                        "text_delta" => {
                            let t = delta["text"].as_str().unwrap_or("");
                            if t.is_empty() {
                                continue;
                            }
                            if pending_space && !full.is_empty() && !full.ends_with(char::is_whitespace) {
                                full.push(' ');
                                out.text(" ");
                            }
                            pending_space = false;
                            full.push_str(t);
                            out.text(t);
                            let prev = block["text"].as_str().unwrap_or("").to_string();
                            block["text"] = json!(prev + t);
                        }
                        "input_json_delta" => {
                            partial_json.entry(idx).or_default().push_str(delta["partial_json"].as_str().unwrap_or(""));
                        }
                        "thinking_delta" => {
                            let prev = block["thinking"].as_str().unwrap_or("").to_string();
                            block["thinking"] = json!(prev + delta["thinking"].as_str().unwrap_or(""));
                        }
                        "signature_delta" => {
                            block["signature"] = delta["signature"].clone();
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    if let (Some(block), Some(js)) = (blocks.get_mut(idx), partial_json.remove(&idx)) {
                        block["input"] = if js.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&js).unwrap_or_else(|_| json!({}))
                        };
                    }
                }
                "message_delta" => {
                    if let Some(r) = data["delta"]["stop_reason"].as_str() {
                        stop_reason = r.to_string();
                    }
                }
                "error" => {
                    let msg = data["error"]["message"].as_str().unwrap_or("ошибка потока").to_string();
                    return Err(AgentError::new(ErrorKind::Http, msg));
                }
                _ => {}
            }
        }
        blocks.retain(|b| !b.is_null());

        let uses: Vec<Value> = blocks.iter().filter(|b| b["type"] == "tool_use").cloned().collect();
        let (Some(tb), Some(ctx)) = (toolbox, p.ctx) else { break };
        if stop_reason != "tool_use" || uses.is_empty() {
            break;
        }
        messages.push(json!({"role": "assistant", "content": blocks}));
        let mut results = vec![];
        for u in uses {
            let name = u["name"].as_str().unwrap_or("").to_string();
            let args = if u["input"].is_object() { u["input"].clone() } else { json!({}) };
            let result = tb.execute(&name, &args, ctx).await;
            results.push(json!({"type": "tool_result", "tool_use_id": u["id"], "content": result}));
            tool_calls.push(ToolCall { name, args, result });
        }
        messages.push(json!({"role": "user", "content": results}));
        pending_space = true;
    }
    Ok(RunResult { text: full, tool_calls })
}
