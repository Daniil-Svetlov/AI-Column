//! Загрузка и проверка config.yaml. Секреты берутся из окружения через ${VAR} / ${VAR:-default}.
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROVIDERS: &[&str] = &["anthropic", "openai", "echo"];
pub const STT_PROVIDERS: &[&str] = &["openai", "whispercpp", "mock"];
pub const TTS_PROVIDERS: &[&str] = &["piper", "openai", "mock"];
pub const ALL_TOOLS: &[&str] = &["datetime", "volume", "lights", "timer", "weather", "ask_agent", "switch_agent"];

pub const DEFAULT_PERSONA: &str = "Ты — голосовой помощник умной колонки по имени {assistant}. Сейчас отвечаешь ты, агент {agent}.
Твой ответ будет озвучен синтезатором речи, поэтому:
- говори по-русски, коротко и по делу: обычно одно-три предложения;
- никакого markdown, списков, таблиц, эмодзи и ссылок;
- числа, даты и единицы пиши словами там, где так проще произнести;
- если вопрос сложный, дай суть и предложи рассказать подробнее.";

// ───────────────────────────────────────────── «терпимые» десериализаторы
// После подстановки ${VAR} строка вроде "123" может превратиться в число — принимаем оба вида.
mod de {
    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    pub fn to_str(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(to_str(&Value::deserialize(d)?).unwrap_or_default())
    }

    pub fn opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Ok(to_str(&Value::deserialize(d)?).filter(|s| !s.is_empty()))
    }

    pub fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::Array(a) => a.iter().filter_map(to_str).collect(),
            Value::Null => vec![],
            other => to_str(&other).into_iter().collect(),
        })
    }

    pub fn number<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        let v = Value::deserialize(d)?;
        match &v {
            Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => Ok(s.trim().parse().unwrap_or(f64::NAN)),
            _ => Ok(f64::NAN),
        }
    }

    pub fn opt_number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match &v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
    }

    pub fn boolean<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::Bool(b) => b,
            Value::String(s) => !matches!(s.as_str(), "" | "false" | "0" | "no" | "off"),
            Value::Number(n) => n.as_f64() != Some(0.0),
            _ => false,
        })
    }
}

// ───────────────────────────────────────────── структуры

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerCfg {
    #[serde(deserialize_with = "de::string")]
    pub host: String,
    #[serde(deserialize_with = "de::number")]
    pub port: f64,
    #[serde(deserialize_with = "de::string")]
    pub auth_token: String,
    #[serde(deserialize_with = "de::string")]
    pub db_path: String,
    #[serde(deserialize_with = "de::boolean")]
    pub mdns: bool,
    #[serde(deserialize_with = "de::string")]
    pub timezone: String,
    #[serde(deserialize_with = "de::string")]
    pub log_level: String,
    #[serde(deserialize_with = "de::string")]
    pub public_dir: String,
}

impl Default for ServerCfg {
    fn default() -> Self {
        ServerCfg {
            host: "0.0.0.0".into(),
            port: 8080.0,
            auth_token: String::new(),
            db_path: "./data/ai-column.db".into(),
            mdns: true,
            timezone: "Europe/Moscow".into(),
            log_level: "info".into(),
            public_dir: "./public".into(),
        }
    }
}

impl ServerCfg {
    pub fn port(&self) -> u16 {
        self.port as u16
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantCfg {
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    #[serde(deserialize_with = "de::string")]
    pub language: String,
    #[serde(deserialize_with = "de::string")]
    pub persona: String,
    pub history_turns: usize,
    pub idle_reset_minutes: u64,
    pub max_utterance_seconds: u64,
}

impl Default for AssistantCfg {
    fn default() -> Self {
        AssistantCfg {
            name: "Джарвис".into(),
            language: "ru".into(),
            persona: DEFAULT_PERSONA.into(),
            history_turns: 8,
            idle_reset_minutes: 15,
            max_utterance_seconds: 20,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassifierCfg {
    #[serde(deserialize_with = "de::string")]
    pub agent: String,
    #[serde(default = "default_classifier_timeout")]
    pub timeout_ms: u64,
}

fn default_classifier_timeout() -> u64 {
    2500
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ClassifierRaw {
    Id(String),
    Full(ClassifierCfg),
}

fn de_classifier<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<ClassifierCfg>, D::Error> {
    Ok(match Option::<ClassifierRaw>::deserialize(d)? {
        None => None,
        Some(ClassifierRaw::Id(agent)) if agent.is_empty() => None,
        Some(ClassifierRaw::Id(agent)) => Some(ClassifierCfg { agent, timeout_ms: 2500 }),
        Some(ClassifierRaw::Full(c)) => Some(c),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuleCfg {
    #[serde(default, deserialize_with = "de::string")]
    pub agent: String,
    #[serde(default, deserialize_with = "de::strings")]
    pub keywords: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RouterCfg {
    #[serde(deserialize_with = "de::string")]
    pub default: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub default_agent: Option<String>,
    #[serde(deserialize_with = "de::boolean")]
    pub voice_prefix: bool,
    #[serde(deserialize_with = "de::strings")]
    pub fallback: Vec<String>,
    #[serde(deserialize_with = "de_classifier")]
    pub classifier: Option<ClassifierCfg>,
    pub rules: Vec<RuleCfg>,
}

impl Default for RouterCfg {
    fn default() -> Self {
        RouterCfg {
            default: "auto".into(),
            default_agent: None,
            voice_prefix: true,
            fallback: vec![],
            classifier: None,
            rules: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SttCfg {
    #[serde(deserialize_with = "de::string")]
    pub provider: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub base_url: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub api_key: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub model: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub language: Option<String>,
    pub timeout_ms: u64,
    #[serde(deserialize_with = "de::opt_string")]
    pub prompt: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub path: Option<String>,
    #[serde(deserialize_with = "de::opt_number")]
    pub temperature: Option<f64>,
    #[serde(deserialize_with = "de::opt_string")]
    pub mock_text: Option<String>,
}

impl Default for SttCfg {
    fn default() -> Self {
        SttCfg {
            provider: "mock".into(),
            base_url: None,
            api_key: None,
            model: None,
            language: Some("ru".into()),
            timeout_ms: 20000,
            prompt: None,
            path: None,
            temperature: None,
            mock_text: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsCfg {
    #[serde(deserialize_with = "de::string")]
    pub provider: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub base_url: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub api_key: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub model: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub voice: Option<String>,
    #[serde(deserialize_with = "de::number")]
    pub speed: f64,
    pub timeout_ms: u64,
    #[serde(deserialize_with = "de::opt_string")]
    pub path: Option<String>,
    pub speaker: Option<Value>,
    #[serde(deserialize_with = "de::opt_string")]
    pub response_format: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub instructions: Option<String>,
    pub pcm_rate: Option<u32>,
}

impl Default for TtsCfg {
    fn default() -> Self {
        TtsCfg {
            provider: "mock".into(),
            base_url: None,
            api_key: None,
            model: None,
            voice: None,
            speed: 1.0,
            timeout_ms: 20000,
            path: None,
            speaker: None,
            response_format: None,
            instructions: None,
            pcm_rate: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechCfg {
    pub stt: SttCfg,
    pub tts: TtsCfg,
    pub play_rate: u32,
}

impl Default for SpeechCfg {
    fn default() -> Self {
        SpeechCfg { stt: SttCfg::default(), tts: TtsCfg::default(), play_rate: 24000 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherCfg {
    #[serde(deserialize_with = "de::string")]
    pub default_city: String,
}

impl Default for WeatherCfg {
    fn default() -> Self {
        WeatherCfg { default_city: "Москва".into() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsCfg {
    #[serde(deserialize_with = "de::strings")]
    pub enabled: Vec<String>,
    pub weather: WeatherCfg,
}

impl Default for ToolsCfg {
    fn default() -> Self {
        ToolsCfg { enabled: ALL_TOOLS.iter().map(|s| s.to_string()).collect(), weather: WeatherCfg::default() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentCfg {
    #[serde(deserialize_with = "de::string")]
    pub id: String,
    #[serde(deserialize_with = "de::string")]
    pub name: String,
    #[serde(deserialize_with = "de::string")]
    pub color: String,
    #[serde(deserialize_with = "de::string")]
    pub provider: String,
    #[serde(deserialize_with = "de::opt_string")]
    pub model: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub base_url: Option<String>,
    #[serde(deserialize_with = "de::opt_string")]
    pub api_key: Option<String>,
    #[serde(deserialize_with = "de::strings")]
    pub aliases: Vec<String>,
    #[serde(deserialize_with = "de::string")]
    pub description: String,
    #[serde(deserialize_with = "de::boolean")]
    pub tools: bool,
    pub max_tokens: u32,
    /// Только для OpenAI-совместимых серверов: Anthropic API (версия 2026-10-21) не принимает temperature.
    #[serde(deserialize_with = "de::opt_number")]
    pub temperature: Option<f64>,
    pub timeout_ms: u64,
    pub first_token_timeout_ms: u64,
    #[serde(deserialize_with = "de::string")]
    pub system: String,
    #[serde(deserialize_with = "de::boolean")]
    pub no_think: bool,
    pub extra_body: Option<Value>,
    #[serde(deserialize_with = "de::opt_string")]
    pub max_tokens_param: Option<String>,
    pub delay_ms: Option<u64>,
    pub enabled: Option<bool>,
    /// Anthropic: при отказе модели (stop_reason=refusal) сервер API сам повторит запрос на запасной модели.
    pub refusal_fallback: Option<bool>,
}

impl Default for AgentCfg {
    fn default() -> Self {
        AgentCfg {
            id: String::new(),
            name: String::new(),
            color: "#888888".into(),
            provider: String::new(),
            model: None,
            base_url: None,
            api_key: None,
            aliases: vec![],
            description: String::new(),
            tools: true,
            max_tokens: 700,
            temperature: None,
            timeout_ms: 60000,
            first_token_timeout_ms: 20000,
            system: String::new(),
            no_think: false,
            extra_body: None,
            max_tokens_param: None,
            delay_ms: None,
            enabled: None,
            refusal_fallback: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    server: ServerCfg,
    assistant: AssistantCfg,
    router: RouterCfg,
    speech: SpeechCfg,
    tools: ToolsCfg,
    agents: Value,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub server: ServerCfg,
    pub assistant: AssistantCfg,
    pub router: RouterCfg,
    pub speech: SpeechCfg,
    pub tools: ToolsCfg,
    pub agents: Vec<AgentCfg>,
    /// Откуда прочитан конфиг (None — собран в коде, например в тестах).
    pub file: Option<PathBuf>,
    pub base_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

// ───────────────────────────────────────────── ${VAR}

static RE_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").unwrap());
static RE_WHOLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}$").unwrap());
static RE_SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)token|key|secret|password").unwrap());
static RE_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+(\.\d+)?$").unwrap());

/// Подставляет ${VAR} и ${VAR:-default} во все строки значения.
/// Целое значение вида "${PORT:-8080}" превращается в число/булево (кроме секретов: токен «0123» остаётся строкой).
pub fn substitute_env(value: &Value, env: &HashMap<String, String>, key: &str) -> Value {
    match value {
        Value::String(s) => {
            let replaced = RE_VAR
                .replace_all(s, |c: &regex::Captures| {
                    match env.get(&c[1]).filter(|v| !v.is_empty()) {
                        Some(v) => v.clone(),
                        None => c.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
                    }
                })
                .into_owned();
            if RE_WHOLE.is_match(s) && !RE_SECRET.is_match(key) {
                if RE_NUM.is_match(&replaced) {
                    if let Ok(i) = replaced.parse::<i64>() {
                        return Value::from(i);
                    }
                    if let Ok(f) = replaced.parse::<f64>() {
                        return Value::from(f);
                    }
                }
                if replaced == "true" {
                    return Value::Bool(true);
                }
                if replaced == "false" {
                    return Value::Bool(false);
                }
            }
            Value::String(replaced)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| substitute_env(v, env, key)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), substitute_env(v, env, k))).collect()),
        other => other.clone(),
    }
}

// ───────────────────────────────────────────── проверка

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Проверяет конфиг и заполняет значения по умолчанию. Все ошибки собираются в один ConfigError.
pub fn normalize_config(raw: &Value) -> Result<Config, ConfigError> {
    let raw = if raw.is_null() { Value::Object(Default::default()) } else { raw.clone() };
    let parsed: RawConfig =
        serde_json::from_value(raw).map_err(|e| ConfigError(format!("Ошибки в конфиге:\n  - {e}")))?;
    let mut errors: Vec<String> = vec![];

    let agent_values = match &parsed.agents {
        Value::Array(a) if !a.is_empty() => a.clone(),
        _ => {
            errors.push("agents: нужен хотя бы один агент".into());
            vec![]
        }
    };
    let mut ids = std::collections::HashSet::new();
    let mut agents = vec![];
    for (i, v) in agent_values.iter().enumerate() {
        let mut a: AgentCfg = match serde_json::from_value(v.clone()) {
            Ok(a) => a,
            Err(e) => {
                errors.push(format!("agents[{i}]: {e}"));
                continue;
            }
        };
        let where_ = if a.id.is_empty() { format!("agents[{i}]") } else { format!("agents[{i}] ({})", a.id) };
        if !valid_id(&a.id) {
            errors.push(format!("{where_}: id обязателен (латиница, цифры, - и _)"));
        }
        if ids.contains(&a.id) {
            errors.push(format!("{where_}: повторяющийся id"));
        }
        if a.id == "auto" {
            errors.push(format!("{where_}: id \"auto\" зарезервирован"));
        }
        ids.insert(a.id.clone());
        if !PROVIDERS.contains(&a.provider.as_str()) {
            errors.push(format!("{where_}: provider должен быть одним из {}", PROVIDERS.join(", ")));
        }
        if a.provider != "echo" && a.model.is_none() {
            errors.push(format!("{where_}: не указан model"));
        }
        if a.provider == "openai" && a.base_url.is_none() {
            errors.push(format!("{where_}: для provider=openai нужен base_url"));
        }
        if a.name.is_empty() {
            a.name = a.id.clone();
        }
        if a.enabled != Some(false) {
            agents.push(a);
        }
    }

    let known: std::collections::HashSet<String> = agents.iter().map(|a| a.id.clone()).collect();
    let mut router = parsed.router;
    if router.default != "auto" && !known.contains(&router.default) {
        errors.push(format!("router.default: нет агента \"{}\"", router.default));
    }
    if router.default_agent.is_none() {
        router.default_agent = agents.first().map(|a| a.id.clone());
    }
    if let Some(d) = &router.default_agent {
        if !known.contains(d) {
            errors.push(format!("router.default_agent: нет агента \"{d}\""));
        }
    }
    router.fallback.retain(|id| {
        if !known.contains(id) {
            errors.push(format!("router.fallback: нет агента \"{id}\""));
        }
        known.contains(id)
    });
    if let Some(c) = &router.classifier {
        if !known.contains(&c.agent) {
            errors.push(format!("router.classifier.agent: нет агента \"{}\"", c.agent));
        }
    }
    for (i, rule) in router.rules.iter_mut().enumerate() {
        if !known.contains(&rule.agent) {
            errors.push(format!("router.rules[{i}]: нет агента \"{}\"", rule.agent));
        }
        rule.keywords = rule.keywords.iter().map(|k| k.to_lowercase()).collect();
    }

    let speech = parsed.speech;
    if !STT_PROVIDERS.contains(&speech.stt.provider.as_str()) {
        errors.push(format!("speech.stt.provider: одно из {}", STT_PROVIDERS.join(", ")));
    }
    if !TTS_PROVIDERS.contains(&speech.tts.provider.as_str()) {
        errors.push(format!("speech.tts.provider: одно из {}", TTS_PROVIDERS.join(", ")));
    }
    if speech.stt.provider != "mock" && speech.stt.base_url.is_none() {
        errors.push("speech.stt.base_url: не указан".into());
    }
    if speech.tts.provider != "mock" && speech.tts.base_url.is_none() {
        errors.push("speech.tts.base_url: не указан".into());
    }

    let mut tools = parsed.tools;
    tools.enabled.retain(|t| {
        let ok = ALL_TOOLS.contains(&t.as_str());
        if !ok {
            errors.push(format!("tools.enabled: неизвестный инструмент \"{t}\" (есть: {})", ALL_TOOLS.join(", ")));
        }
        ok
    });

    let server = parsed.server;
    if !(server.port.fract() == 0.0 && (0.0..=65535.0).contains(&server.port)) {
        errors.push("server.port: некорректный порт".into());
    }
    if server.timezone.parse::<chrono_tz::Tz>().is_err() {
        errors.push(format!("server.timezone: неизвестный часовой пояс \"{}\"", server.timezone));
    }

    if !errors.is_empty() {
        return Err(ConfigError(format!("Ошибки в конфиге:\n  - {}", errors.join("\n  - "))));
    }
    Ok(Config {
        server,
        assistant: parsed.assistant,
        router,
        speech,
        tools,
        agents,
        file: None,
        base_dir: std::env::current_dir().unwrap_or_default(),
    })
}

/// Читает YAML-конфиг. Порядок поиска: явный путь → $AI_COLUMN_CONFIG → config/config.yaml → config/config.example.yaml.
pub fn load_config(file: Option<&str>, base_dir: &Path, env: &HashMap<String, String>) -> Result<Config, ConfigError> {
    let candidates: Vec<PathBuf> = [file.map(str::to_string), env.get("AI_COLUMN_CONFIG").cloned()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .chain(["config/config.yaml".to_string(), "config/config.example.yaml".to_string()])
        .map(|p| base_dir.join(p))
        .collect();
    let found = candidates.iter().find(|p| p.exists()).ok_or_else(|| {
        ConfigError(format!(
            "Не найден конфиг. Искал: {}",
            candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        ))
    })?;
    let text = std::fs::read_to_string(found).map_err(|e| ConfigError(format!("{}: {e}", found.display())))?;
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| ConfigError(format!("{}: ошибка YAML: {e}", found.display())))?;
    let json = serde_json::to_value(yaml).map_err(|e| ConfigError(format!("{}: {e}", found.display())))?;
    let mut cfg = normalize_config(&substitute_env(&json, env, ""))?;
    cfg.file = Some(found.clone());
    cfg.base_dir = base_dir.to_path_buf();
    Ok(cfg)
}

/// Конфиг без секретов — для /api/config.
pub fn public_config(cfg: &Config) -> Value {
    fn strip(v: Value) -> Value {
        match v {
            Value::Object(o) => Value::Object(o.into_iter().filter(|(k, _)| !RE_SECRET.is_match(k)).collect()),
            v => v,
        }
    }
    serde_json::json!({
        "assistant": { "name": cfg.assistant.name, "history_turns": cfg.assistant.history_turns },
        "router": cfg.router,
        "speech": {
            "stt": strip(serde_json::to_value(&cfg.speech.stt).unwrap_or_default()),
            "tts": strip(serde_json::to_value(&cfg.speech.tts).unwrap_or_default()),
            "play_rate": cfg.speech.play_rate,
        },
        "tools": cfg.tools.enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn substitute_env_vars_defaults_numbers() {
        let e = env(&[("KEY", "abc"), ("PORT", "9000")]);
        let out = substitute_env(
            &json!({"a": "${KEY}", "b": "x-${KEY}-y", "c": "${MISSING:-def}", "d": "${PORT}", "e": ["${KEY}"], "f": "${MISSING}"}),
            &e,
            "",
        );
        assert_eq!(out, json!({"a": "abc", "b": "x-abc-y", "c": "def", "d": 9000, "e": ["abc"], "f": ""}));
        let secret = substitute_env(
            &json!({"auth_token": "${T}", "api_key": "${T}", "port": "${T}"}),
            &env(&[("T", "0123")]),
            "",
        );
        assert_eq!(secret, json!({"auth_token": "0123", "api_key": "0123", "port": 123}));
    }

    #[test]
    fn defaults_are_filled() {
        let cfg = normalize_config(&json!({"agents": [{"id": "echo", "provider": "echo"}]})).unwrap();
        assert_eq!(cfg.router.default, "auto");
        assert_eq!(cfg.router.default_agent.as_deref(), Some("echo"));
        assert_eq!(cfg.agents[0].name, "echo");
        assert_eq!(cfg.agents[0].max_tokens, 700);
        assert_eq!(cfg.server.port(), 8080);
    }

    #[test]
    fn all_errors_at_once() {
        let err = normalize_config(&json!({
            "agents": [{"id": "a", "provider": "openai", "model": "m"}, {"id": "a", "provider": "nope"}],
            "router": {"default": "zzz", "fallback": ["q"]},
            "speech": {"stt": {"provider": "openai"}},
        }))
        .unwrap_err();
        for part in ["base_url", "повторяющийся id", "provider должен", "router.default", "router.fallback", "speech.stt.base_url"] {
            assert!(err.0.contains(part), "нет «{part}» в: {}", err.0);
        }
    }

    #[test]
    fn disabled_agents_are_dropped() {
        let cfg = normalize_config(&json!({"agents": [{"id": "echo", "provider": "echo"}, {"id": "off", "provider": "echo", "enabled": false}]}))
            .unwrap();
        assert_eq!(cfg.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["echo"]);
    }

    #[test]
    fn example_config_is_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cfg = load_config(Some("config/config.example.yaml"), dir, &env(&[("ANTHROPIC_API_KEY", "x")])).unwrap();
        assert_eq!(cfg.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["claude", "qwen", "kimi"]);
        assert_eq!(cfg.router.classifier.as_ref().unwrap().agent, "qwen");
        assert_eq!(cfg.agents[0].api_key.as_deref(), Some("x"));
    }
}
