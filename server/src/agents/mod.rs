//! Агенты: общий тип, ошибки, таймауты, реестр. Провайдеры: anthropic.rs, openai.rs, echo.rs.
mod anthropic;
mod echo;
mod openai;
pub mod router;
mod sse;
pub mod tools;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::AgentCfg;
use crate::logger::Logger;
pub use router::Router;
pub use tools::{ToolContext, Toolbox};

pub const MAX_TOOL_STEPS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Timeout,
    Connection,
    Auth,
    Http,
    Aborted,
    Other,
}

#[derive(Clone, Debug)]
pub struct AgentError {
    pub message: String,
    pub kind: ErrorKind,
}

impl AgentError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        AgentError { message: message.into(), kind }
    }
    pub fn aborted() -> Self {
        Self::new(ErrorKind::Aborted, "запрос отменён")
    }
    pub fn is_aborted(&self) -> bool {
        self.kind == ErrorKind::Aborted
    }

    /// Ошибка сети/HTTP-клиента → понятная AgentError.
    pub fn from_reqwest(e: &reqwest::Error) -> Self {
        if e.is_timeout() {
            Self::new(ErrorKind::Timeout, "агент не ответил вовремя")
        } else if e.is_connect() {
            Self::new(ErrorKind::Connection, "сервер агента недоступен")
        } else if let Some(s) = e.status() {
            Self::from_status(s.as_u16(), &e.to_string())
        } else {
            Self::new(ErrorKind::Other, e.to_string())
        }
    }

    pub fn from_status(status: u16, body: &str) -> Self {
        let msg = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| body.chars().take(300).collect());
        if status == 401 || status == 403 {
            Self::new(ErrorKind::Auth, "неверный API-ключ или нет доступа")
        } else {
            Self::new(ErrorKind::Http, format!("ошибка HTTP {status}: {msg}"))
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AgentError {}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AgentStatus {
    pub ok: Option<bool>,
    pub error: Option<String>,
    #[serde(rename = "checkedAt")]
    pub checked_at: i64,
    #[serde(rename = "latencyMs")]
    pub latency_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub args: Value,
    pub result: String,
}

#[derive(Clone, Debug, Default)]
pub struct RunResult {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

impl Msg {
    pub fn new(role: &str, content: &str) -> Msg {
        Msg { role: role.into(), content: content.into() }
    }
}

/// Склеивает историю так, чтобы роли чередовались и первой была реплика пользователя.
pub fn normalize_history(history: &[Msg], user_text: &str) -> Vec<Msg> {
    let mut msgs: Vec<Msg> = vec![];
    for m in history {
        if m.content.is_empty() || (m.role != "user" && m.role != "assistant") {
            continue;
        }
        match msgs.last_mut() {
            Some(last) if last.role == m.role => last.content = format!("{}\n{}", last.content, m.content),
            _ => msgs.push(m.clone()),
        }
    }
    while msgs.first().is_some_and(|m| m.role != "user") {
        msgs.remove(0);
    }
    match msgs.last_mut() {
        Some(last) if last.role == "user" => last.content = format!("{}\n{}", last.content, user_text),
        _ => msgs.push(Msg::new("user", user_text)),
    }
    msgs
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Что нужно агенту для одного хода.
pub struct RunParams<'a> {
    pub system: &'a str,
    pub history: &'a [Msg],
    pub user_text: &'a str,
    pub toolbox: Option<&'a Toolbox>,
    pub ctx: Option<&'a ToolContext>,
    pub max_tokens: Option<u32>,
}

/// Куда агент отдаёт текст по мере генерации.
pub struct Output<'a> {
    on_text: &'a mut (dyn FnMut(&str) + Send),
    started: &'a AtomicBool,
}

impl Output<'_> {
    pub fn text(&mut self, d: &str) {
        if !d.is_empty() {
            self.started.store(true, Ordering::Relaxed);
        }
        (self.on_text)(d);
    }
    /// Агент начал отвечать (например, вызовом инструмента) — таймаут «первого токена» снимается.
    pub fn mark_started(&self) {
        self.started.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Anthropic,
    OpenAi,
    Echo,
}

pub struct Agent {
    pub cfg: AgentCfg,
    provider: Provider,
    status: Mutex<AgentStatus>,
    /// OpenAI-совместимые: выключается сам, если сервер не умеет tools.
    tools_supported: AtomicBool,
    http: reqwest::Client,
    log: Logger,
}

impl Agent {
    pub fn new(cfg: AgentCfg, log: Logger) -> Agent {
        let provider = match cfg.provider.as_str() {
            "anthropic" => Provider::Anthropic,
            "openai" => Provider::OpenAi,
            _ => Provider::Echo,
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP-клиент");
        Agent { cfg, provider, status: Mutex::new(AgentStatus::default()), tools_supported: AtomicBool::new(true), http, log }
    }

    pub fn id(&self) -> &str {
        &self.cfg.id
    }
    pub fn name(&self) -> &str {
        &self.cfg.name
    }
    pub fn color(&self) -> &str {
        &self.cfg.color
    }
    pub fn description(&self) -> &str {
        &self.cfg.description
    }
    pub fn aliases(&self) -> &[String] {
        &self.cfg.aliases
    }
    pub fn tools_enabled(&self) -> bool {
        self.cfg.tools
    }
    pub fn tools_supported(&self) -> bool {
        self.tools_supported.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> AgentStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn set_status(&self, s: AgentStatus) {
        *self.status.lock().unwrap() = s;
    }

    /// Пометить недоступным после неудачного запроса (до следующей проверки здоровья).
    pub fn mark_down(&self, error: &str) {
        let mut st = self.status.lock().unwrap();
        st.ok = Some(false);
        st.error = Some(error.to_string());
        st.checked_at = now_ms();
    }

    pub fn available(&self) -> bool {
        self.status.lock().unwrap().ok != Some(false)
    }

    pub fn info(&self) -> Value {
        serde_json::json!({
            "id": self.cfg.id,
            "name": self.cfg.name,
            "color": self.cfg.color,
            "provider": self.cfg.provider,
            "model": self.cfg.model,
            "description": self.cfg.description,
            "aliases": self.cfg.aliases,
            "available": self.available(),
            "status": self.status(),
        })
    }

    async fn run(&self, p: &RunParams<'_>, out: &mut Output<'_>) -> Result<RunResult, AgentError> {
        match self.provider {
            Provider::Anthropic => anthropic::run(self, p, out).await,
            Provider::OpenAi => openai::run(self, p, out).await,
            Provider::Echo => echo::run(self, p, out).await,
        }
    }

    /// Один ход со стримингом и инструментами — с таймаутами «первого токена» и общим.
    /// Если ответ не начался вовремя — AgentError(Timeout), чтобы роутер переключился на запасного агента.
    pub async fn run_guarded(
        &self,
        p: RunParams<'_>,
        on_text: &mut (dyn FnMut(&str) + Send),
        cancel: &CancellationToken,
    ) -> Result<RunResult, AgentError> {
        let started = AtomicBool::new(false);
        let mut out = Output { on_text, started: &started };
        let first = Duration::from_millis(self.cfg.first_token_timeout_ms);
        let first_timer = async {
            tokio::time::sleep(first).await;
            if started.load(Ordering::Relaxed) {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AgentError::aborted()),
            r = self.run(&p, &mut out) => r,
            _ = first_timer => Err(AgentError::new(
                ErrorKind::Timeout,
                format!("{} не начал отвечать за {} с", self.cfg.name, (first.as_millis() as f64 / 1000.0).round()),
            )),
            _ = tokio::time::sleep(Duration::from_millis(self.cfg.timeout_ms)) => Err(AgentError::new(
                ErrorKind::Timeout,
                format!("{}: превышено время ответа", self.cfg.name),
            )),
        }
    }

    /// Короткий ответ без инструментов (классификатор, ask_agent).
    pub async fn complete(
        &self,
        system: &str,
        user_text: &str,
        max_tokens: u32,
        cancel: &CancellationToken,
    ) -> Result<String, AgentError> {
        let mut text = String::new();
        let res = self
            .run_guarded(
                RunParams { system, history: &[], user_text, toolbox: None, ctx: None, max_tokens: Some(max_tokens) },
                &mut |d: &str| text.push_str(d),
                cancel,
            )
            .await?;
        Ok(if res.text.is_empty() { text } else { res.text }.trim().to_string())
    }

    async fn probe(&self) -> Result<(), String> {
        match self.provider {
            Provider::Anthropic => anthropic::probe(self).await,
            Provider::OpenAi => openai::probe(self).await,
            Provider::Echo => Ok(()),
        }
    }

    pub async fn health(&self) -> AgentStatus {
        let t0 = Instant::now();
        let r = match tokio::time::timeout(Duration::from_secs(5), self.probe()).await {
            Ok(r) => r,
            Err(_) => Err("агент не ответил вовремя".to_string()),
        };
        let st = AgentStatus {
            ok: Some(r.is_ok()),
            error: r.err(),
            checked_at: now_ms(),
            latency_ms: Some(t0.elapsed().as_millis() as i64),
        };
        self.set_status(st.clone());
        st
    }
}

/// Реестр агентов: создаёт их из конфига и периодически проверяет доступность.
pub struct Registry {
    agents: Vec<Arc<Agent>>,
    log: Logger,
    changed: tokio::sync::watch::Sender<u64>,
}

impl Registry {
    pub fn new(cfgs: &[AgentCfg], log: Logger) -> Registry {
        let agents = cfgs.iter().map(|c| Arc::new(Agent::new(c.clone(), log.child(&c.id)))).collect();
        Registry { agents, log, changed: tokio::sync::watch::channel(0).0 }
    }

    pub fn get(&self, id: &str) -> Option<Arc<Agent>> {
        self.agents.iter().find(|a| a.id() == id).cloned()
    }

    pub fn has(&self, id: &str) -> bool {
        self.agents.iter().any(|a| a.id() == id)
    }

    pub fn list(&self) -> &[Arc<Agent>] {
        &self.agents
    }

    pub fn ids(&self) -> Vec<String> {
        self.agents.iter().map(|a| a.id().to_string()).collect()
    }

    pub fn info(&self) -> Vec<Value> {
        self.agents.iter().map(|a| a.info()).collect()
    }

    /// Подписка на смену доступности агентов.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub async fn check_all(&self) -> Vec<Value> {
        let before: Vec<Option<bool>> = self.agents.iter().map(|a| a.status().ok).collect();
        let results = futures_util::future::join_all(self.agents.iter().map(|a| a.health())).await;
        for (a, st) in self.agents.iter().zip(results) {
            if st.ok == Some(true) {
                self.log.debug(format!("{}: доступен ({} мс)", a.id(), st.latency_ms.unwrap_or(0)));
            } else {
                self.log.warn(format!("{}: недоступен — {}", a.id(), st.error.unwrap_or_default()));
            }
        }
        let after: Vec<Option<bool>> = self.agents.iter().map(|a| a.status().ok).collect();
        if before != after {
            self.changed.send_modify(|v| *v += 1);
        }
        self.info()
    }

    /// Фоновая проверка доступности; останавливается по токену.
    pub fn start_health_checks(self: &Arc<Self>, every: Duration, stop: CancellationToken) {
        let reg = self.clone();
        tokio::spawn(async move {
            loop {
                reg.check_all().await;
                tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = tokio::time::sleep(every) => {}
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_normalized() {
        let h = [Msg::new("assistant", "лишнее"), Msg::new("user", "Привет"), Msg::new("user", "ещё"), Msg::new("assistant", "Привет!")];
        let out = normalize_history(&h, "Который час?");
        assert_eq!(out, [Msg::new("user", "Привет\nещё"), Msg::new("assistant", "Привет!"), Msg::new("user", "Который час?")]);
    }
}
