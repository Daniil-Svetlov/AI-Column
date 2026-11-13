//! Агент-«эхо» без нейросети: проверить колонку, звук и инструменты, когда LLM ещё не поднята.
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::json;

use super::{Agent, AgentError, Output, RunParams, RunResult, ToolCall};
use crate::text::normalize;

static VOLUME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"громкость\D{0,20}(\d{1,3})").unwrap());
static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"который час|сколько времени").unwrap());

pub async fn run(agent: &Agent, p: &RunParams<'_>, out: &mut Output<'_>) -> Result<RunResult, AgentError> {
    let mut tool_calls = vec![];
    let n = normalize(p.user_text);
    let mut prefix = String::new();

    if let (Some(tb), Some(ctx)) = (p.toolbox.filter(|_| agent.tools_enabled()), p.ctx) {
        let names: Vec<String> = tb.definitions(ctx).into_iter().map(|d| d.name).collect();
        let has = |n: &str| names.iter().any(|x| x == n);
        if let Some(level) = VOLUME.captures(&n).and_then(|c| c[1].parse::<i64>().ok()).filter(|_| has("set_volume")) {
            out.mark_started();
            let args = json!({"level": level});
            let result = tb.execute("set_volume", &args, ctx).await;
            prefix = format!("{result}. ");
            tool_calls.push(ToolCall { name: "set_volume".into(), args, result });
        } else if TIME.is_match(&n) && has("get_datetime") {
            out.mark_started();
            let result = tb.execute("get_datetime", &json!({}), ctx).await;
            prefix = format!("{result}. ");
            tool_calls.push(ToolCall { name: "get_datetime".into(), args: json!({}), result });
        }
    }

    let reply = format!("{prefix}Я {}, тестовый агент. Ты сказал: {}", agent.name(), p.user_text);
    let delay = Duration::from_millis(agent.cfg.delay_ms.unwrap_or(15));
    let mut text = String::new();
    // Отдаём по словам вместе с пробелами — как настоящая модель в потоке.
    let mut word = String::new();
    let mut words = vec![];
    for ch in reply.chars() {
        if !word.is_empty() && ch.is_whitespace() != word.chars().last().unwrap().is_whitespace() {
            words.push(std::mem::take(&mut word));
        }
        word.push(ch);
    }
    if !word.is_empty() {
        words.push(word);
    }
    for w in words {
        tokio::time::sleep(delay).await;
        text.push_str(&w);
        out.text(&w);
    }
    Ok(RunResult { text, tool_calls })
}
