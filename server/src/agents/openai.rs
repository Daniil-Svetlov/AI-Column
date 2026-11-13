//! Любой OpenAI-совместимый сервер: Ollama, vLLM, llama.cpp, LM Studio, LocalAI,
//! а также облачные API (Moonshot/Kimi, OpenRouter, DeepSeek, OpenAI…).
use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use super::sse::SseReader;
use super::{Agent, AgentError, MAX_TOOL_STEPS, Output, RunParams, RunResult, ToolCall, normalize_history};
use crate::text::ThinkFilter;

fn base(agent: &Agent) -> String {
    agent.cfg.base_url.as_deref().unwrap_or("").trim_end_matches('/').to_string()
}

fn with_auth(agent: &Agent, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match agent.cfg.api_key.as_deref() {
        Some(k) if k != "none" => rb.bearer_auth(k),
        _ => rb,
    }
}

pub async fn probe(agent: &Agent) -> Result<(), String> {
    let resp = with_auth(agent, agent.http.get(format!("{}/models", base(agent))))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| AgentError::from_reqwest(&e).message)?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(AgentError::from_status(status, &body).message);
    }
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let ids: Vec<String> = v["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).take(500).collect())
        .unwrap_or_default();
    let model = agent.cfg.model.as_deref().unwrap_or("");
    let found = ids
        .iter()
        .any(|id| id == model || id.rsplit('/').next() == Some(model) || *id == format!("{model}:latest"));
    if !ids.is_empty() && !found {
        let shown = ids.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
        return Err(format!("на сервере нет модели «{model}» (есть: {shown}{})", if ids.len() > 6 { "…" } else { "" }));
    }
    Ok(())
}

async fn create(agent: &Agent, body: &Value) -> Result<reqwest::Response, (u16, String, Option<AgentError>)> {
    let resp = with_auth(agent, agent.http.post(format!("{}/chat/completions", base(agent))))
        .json(body)
        .send()
        .await
        .map_err(|e| (0, String::new(), Some(AgentError::from_reqwest(&e))))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err((status, text, None));
    }
    Ok(resp)
}

#[derive(Default, Clone)]
struct PendingCall {
    id: String,
    name: String,
    args: String,
}

pub async fn run(agent: &Agent, p: &RunParams<'_>, out: &mut Output<'_>) -> Result<RunResult, AgentError> {
    let system = if agent.cfg.no_think { format!("{}\n/no_think", p.system) } else { p.system.to_string() };
    let mut messages: Vec<Value> = vec![json!({"role": "system", "content": system})];
    messages.extend(normalize_history(p.history, p.user_text).into_iter().map(|m| json!({"role": m.role, "content": m.content})));
    let toolbox = p.toolbox.filter(|_| agent.tools_enabled());
    let tools: Vec<Value> = match (toolbox, p.ctx) {
        (Some(tb), Some(ctx)) => tb
            .definitions(ctx)
            .into_iter()
            .map(|d| json!({"type": "function", "function": {"name": d.name, "description": d.description, "parameters": d.parameters}}))
            .collect(),
        _ => vec![],
    };
    let mut filter = ThinkFilter::new();
    let mut full = String::new();
    let mut pending_space = false;
    let mut tool_calls = vec![];

    for step in 0..MAX_TOOL_STEPS {
        let mut body = json!({"model": agent.cfg.model, "messages": messages, "stream": true});
        let param = agent.cfg.max_tokens_param.as_deref().unwrap_or("max_tokens");
        body[param] = json!(p.max_tokens.unwrap_or(agent.cfg.max_tokens));
        if let Some(Value::Object(extra)) = &agent.cfg.extra_body {
            for (k, v) in extra {
                body[k] = v.clone();
            }
        }
        if let Some(t) = agent.cfg.temperature {
            body["temperature"] = json!(t);
        }
        let use_tools = !tools.is_empty() && agent.tools_supported();
        if use_tools {
            body["tools"] = Value::Array(tools.clone());
        }

        let resp = match create(agent, &body).await {
            Ok(r) => r,
            Err((status, text, None)) if use_tools && (status == 400 || status == 422) && {
                let l = text.to_lowercase();
                l.contains("tool") || l.contains("function")
            } =>
            {
                // Сервер не поддерживает function calling — повторяем без инструментов.
                agent.log.warn(format!("{}: сервер не принял tools ({}) — работаю без инструментов", agent.id(), AgentError::from_status(status, &text)));
                agent.tools_supported.store(false, Ordering::Relaxed);
                body.as_object_mut().unwrap().remove("tools");
                match create(agent, &body).await {
                    Ok(r) => r,
                    Err((_, _, Some(e))) => return Err(e),
                    Err((s, t, None)) => return Err(AgentError::from_status(s, &t)),
                }
            }
            Err((_, _, Some(e))) => return Err(e),
            Err((s, t, None)) => return Err(AgentError::from_status(s, &t)),
        };

        let mut acc: Vec<PendingCall> = vec![];
        let mut finish: Option<String> = None;
        let mut step_text = String::new();
        let mut sse = SseReader::new(resp);
        while let Some(ev) = sse.next().await {
            let ev = ev.map_err(|e| AgentError::from_reqwest(&e))?;
            if ev.data.trim() == "[DONE]" {
                break;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(&ev.data) else { continue };
            let Some(choice) = chunk["choices"].get(0) else { continue };
            let d = &choice["delta"];
            if let Some(content) = d["content"].as_str().filter(|s| !s.is_empty()) {
                let visible = filter.push(content);
                if !visible.is_empty() {
                    step_text.push_str(&visible);
                    if pending_space && !full.is_empty() && !full.ends_with(char::is_whitespace) && !visible.starts_with(char::is_whitespace) {
                        full.push(' ');
                        out.text(" ");
                    }
                    pending_space = false;
                    full.push_str(&visible);
                    out.text(&visible);
                }
            }
            if let Some(calls) = d["tool_calls"].as_array() {
                out.mark_started();
                for tc in calls {
                    let idx = tc["index"].as_u64().map(|i| i as usize).unwrap_or(acc.len());
                    if acc.len() <= idx {
                        acc.resize(idx + 1, PendingCall::default());
                    }
                    let c = &mut acc[idx];
                    if let Some(id) = tc["id"].as_str().filter(|s| !s.is_empty()) {
                        c.id = id.into();
                    }
                    if let Some(n) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                        c.name = n.into();
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        c.args.push_str(a);
                    }
                }
            }
            if let Some(f) = choice["finish_reason"].as_str() {
                finish = Some(f.to_string());
            }
        }
        let tail = filter.flush();
        if !tail.is_empty() {
            full.push_str(&tail);
            step_text.push_str(&tail);
            out.text(&tail);
        }

        let mut calls: Vec<PendingCall> = acc.into_iter().filter(|c| !c.name.is_empty()).collect();
        let (Some(tb), Some(ctx)) = (toolbox, p.ctx) else { break };
        if calls.is_empty() {
            break;
        }
        if let Some(f) = &finish {
            if !["tool_calls", "function_call", "stop"].contains(&f.as_str()) {
                break;
            }
        }
        for (i, c) in calls.iter_mut().enumerate() {
            if c.id.is_empty() {
                c.id = format!("call_{step}_{i}");
            }
        }
        messages.push(json!({
            "role": "assistant",
            "content": if step_text.is_empty() { Value::Null } else { json!(step_text) },
            "tool_calls": calls.iter().map(|c| json!({
                "id": c.id, "type": "function",
                "function": {"name": c.name, "arguments": if c.args.is_empty() { "{}" } else { &c.args }},
            })).collect::<Vec<_>>(),
        }));
        for c in calls {
            let args: Value = serde_json::from_str(&c.args).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
            let result = tb.execute(&c.name, &args, ctx).await;
            messages.push(json!({"role": "tool", "tool_call_id": c.id, "content": result}));
            tool_calls.push(ToolCall { name: c.name, args, result });
        }
        pending_space = true;
    }
    Ok(RunResult { text: full, tool_calls })
}
