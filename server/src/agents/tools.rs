//! Инструменты, которые агенты могут вызывать (function calling).
//! Описания нейтральные (JSON Schema) — провайдеры сами переводят их в свой формат.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Timelike};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{Agent, Registry};
use crate::config::Config;
use crate::logger::Logger;
use crate::session::DeviceSession;
use crate::timers::TimerManager;

pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

fn def(name: &str, description: impl Into<String>, parameters: Value) -> ToolDef {
    ToolDef { name: name.into(), description: description.into(), parameters }
}

/// Контекст вызова: какая колонка, какой агент, глубина делегирования и т.п.
#[derive(Clone)]
pub struct ToolContext {
    pub session: Option<Arc<DeviceSession>>,
    pub agent_id: String,
    pub agent_name: String,
    pub depth: u32,
    pub registry: Option<Arc<Registry>>,
    pub timers: Option<Arc<TimerManager>>,
    pub cancel: CancellationToken,
    pub base_system: String,
    pub on_delegate: Option<Arc<dyn Fn(&Agent) + Send + Sync>>,
}

impl ToolContext {
    pub fn new(agent_id: &str) -> ToolContext {
        ToolContext {
            session: None,
            agent_id: agent_id.into(),
            agent_name: agent_id.into(),
            depth: 0,
            registry: None,
            timers: None,
            cancel: CancellationToken::new(),
            base_system: String::new(),
            on_delegate: None,
        }
    }
}

const COLOR_NAMES: &[(&str, &str)] = &[
    ("красный", "#ff0000"), ("red", "#ff0000"),
    ("оранжевый", "#ff6600"), ("orange", "#ff6600"),
    ("желтый", "#ffcc00"), ("жёлтый", "#ffcc00"), ("yellow", "#ffcc00"),
    ("зеленый", "#00ff00"), ("зелёный", "#00ff00"), ("green", "#00ff00"),
    ("голубой", "#00ccff"), ("cyan", "#00ccff"),
    ("синий", "#0033ff"), ("blue", "#0033ff"),
    ("фиолетовый", "#8800ff"), ("purple", "#8800ff"),
    ("розовый", "#ff3399"), ("pink", "#ff3399"),
    ("белый", "#ffffff"), ("white", "#ffffff"),
    ("теплый", "#ffb060"), ("тёплый", "#ffb060"), ("warm", "#ffb060"),
];

fn weather_code(code: i64) -> &'static str {
    match code {
        0 => "ясно",
        1 => "преимущественно ясно",
        2 => "переменная облачность",
        3 => "пасмурно",
        45 => "туман",
        48 => "изморозь и туман",
        51 => "слабая морось",
        53 => "морось",
        55 => "сильная морось",
        56 => "ледяная морось",
        57 => "сильная ледяная морось",
        61 => "небольшой дождь",
        63 => "дождь",
        65 => "сильный дождь",
        66 => "ледяной дождь",
        67 => "сильный ледяной дождь",
        71 => "небольшой снег",
        73 => "снег",
        75 => "сильный снег",
        77 => "снежная крупа",
        80 => "ливень",
        81 => "сильный ливень",
        82 => "очень сильный ливень",
        85 => "снегопад",
        86 => "сильный снегопад",
        95 => "гроза",
        96 => "гроза с градом",
        99 => "сильная гроза с градом",
        _ => "без осадков",
    }
}

pub fn parse_color(input: &str) -> Option<String> {
    let s = input.trim().to_lowercase();
    let hex = s.strip_prefix('#').unwrap_or(&s);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("#{hex}"));
    }
    COLOR_NAMES.iter().find(|(n, _)| *n == s).map(|(_, c)| c.to_string())
}

fn plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (n10, n100) = (n % 10, n % 100);
    if n10 == 1 && n100 != 11 {
        one
    } else if (2..=4).contains(&n10) && !(10..20).contains(&n100) {
        few
    } else {
        many
    }
}

pub fn human_duration(sec: u64) -> String {
    let (h, m, s) = (sec / 3600, (sec % 3600) / 60, sec % 60);
    let mut parts = vec![];
    if h > 0 {
        parts.push(format!("{h} {}", plural(h, "час", "часа", "часов")));
    }
    if m > 0 {
        parts.push(format!("{m} {}", plural(m, "минуту", "минуты", "минут")));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{s} {}", plural(s, "секунду", "секунды", "секунд")));
    }
    parts.join(" ")
}

const WEEKDAYS: [&str; 7] = ["понедельник", "вторник", "среда", "четверг", "пятница", "суббота", "воскресенье"];
const MONTHS: [&str; 12] =
    ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];

/// «пятница, 13 ноября 2026 г.» и «14:05» в нужном часовом поясе.
pub fn ru_now(tz: &str) -> (String, String) {
    let tz: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::Europe::Moscow);
    let now = chrono::Utc::now().with_timezone(&tz);
    let date = format!(
        "{}, {} {} {} г.",
        WEEKDAYS[now.weekday().num_days_from_monday() as usize],
        now.day(),
        MONTHS[now.month0() as usize],
        now.year()
    );
    (date, format!("{:02}:{:02}", now.hour(), now.minute()))
}

pub struct Toolbox {
    config: Arc<Config>,
    http: reqwest::Client,
    log: Logger,
    /// Базовые адреса open-meteo (подменяются в тестах).
    pub geocoding_url: String,
    pub forecast_url: String,
}

type BoxFut<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;

fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|f| f.is_finite())
}

impl Toolbox {
    pub fn new(config: Arc<Config>, log: Logger) -> Toolbox {
        Toolbox {
            config,
            http: reqwest::Client::builder().timeout(Duration::from_secs(8)).build().expect("HTTP-клиент"),
            log,
            geocoding_url: "https://geocoding-api.open-meteo.com/v1/search".into(),
            forecast_url: "https://api.open-meteo.com/v1/forecast".into(),
        }
    }

    fn has(&self, t: &str) -> bool {
        self.config.tools.enabled.iter().any(|x| x == t)
    }

    /// Список инструментов для конкретного вызова (контекст влияет: глубина, текущий агент, наличие колонки).
    pub fn definitions(&self, ctx: &ToolContext) -> Vec<ToolDef> {
        let mut defs = vec![];
        let empty = || json!({"type": "object", "properties": {}, "additionalProperties": false});
        if self.has("datetime") {
            defs.push(def("get_datetime", "Текущие дата, время и день недели. Вызывай, когда спрашивают про время или дату.", empty()));
        }
        if self.has("weather") {
            defs.push(def(
                "get_weather",
                "Текущая погода и прогноз на сегодня-завтра. Если город не назван — город по умолчанию.",
                json!({"type": "object", "properties": {"city": {"type": "string", "description": "Город, например «Казань»"}}, "additionalProperties": false}),
            ));
        }
        if ctx.session.is_some() {
            if self.has("volume") {
                defs.push(def(
                    "set_volume",
                    "Изменить громкость колонки. Либо level (0–100), либо change (относительно, например -10 или +20).",
                    json!({"type": "object", "properties": {
                        "level": {"type": "integer", "minimum": 0, "maximum": 100},
                        "change": {"type": "integer", "minimum": -100, "maximum": 100},
                    }, "additionalProperties": false}),
                ));
            }
            if self.has("lights") {
                defs.push(def(
                    "set_lights",
                    "Управлять подсветкой колонки (светодиодная матрица).",
                    json!({"type": "object", "properties": {
                        "mode": {"type": "string", "enum": ["off", "solid", "breathe", "rainbow", "auto"], "description": "auto — подсветка по состоянию колонки"},
                        "color": {"type": "string", "description": "Цвет: #RRGGBB или название (красный, синий, тёплый…)"},
                        "brightness": {"type": "integer", "minimum": 1, "maximum": 100},
                    }, "required": ["mode"], "additionalProperties": false}),
                ));
            }
            if self.has("timer") && ctx.timers.is_some() {
                defs.push(def(
                    "set_timer",
                    "Поставить таймер. Когда время выйдет, колонка скажет об этом вслух.",
                    json!({"type": "object", "properties": {
                        "seconds": {"type": "integer", "minimum": 1, "maximum": 86400},
                        "label": {"type": "string", "description": "Для чего таймер, например «пицца»"},
                    }, "required": ["seconds"], "additionalProperties": false}),
                ));
                defs.push(def("cancel_timers", "Отменить все таймеры на колонке.", empty()));
            }
            if let (true, Some(reg)) = (self.has("switch_agent"), &ctx.registry) {
                let mut ids = vec!["auto".to_string()];
                ids.extend(reg.ids());
                defs.push(def(
                    "switch_agent",
                    "Переключить, какой агент отвечает на этой колонке дальше (auto — выбирать автоматически).",
                    json!({"type": "object", "properties": {"agent": {"type": "string", "enum": ids}}, "required": ["agent"], "additionalProperties": false}),
                ));
            }
        }
        if let (true, 0, Some(reg)) = (self.has("ask_agent"), ctx.depth, &ctx.registry) {
            let others: Vec<_> = reg.list().iter().filter(|a| a.id() != ctx.agent_id).collect();
            if !others.is_empty() {
                let list = others
                    .iter()
                    .map(|a| format!("{} — {}", a.id(), if a.description().is_empty() { a.name() } else { a.description() }))
                    .collect::<Vec<_>>()
                    .join("; ");
                defs.push(def(
                    "ask_agent",
                    format!("Задать вопрос другому агенту и получить его ответ. Агенты: {list}."),
                    json!({"type": "object", "properties": {
                        "agent": {"type": "string", "enum": others.iter().map(|a| a.id()).collect::<Vec<_>>()},
                        "question": {"type": "string"},
                    }, "required": ["agent", "question"], "additionalProperties": false}),
                ));
            }
        }
        defs
    }

    /// Выполнить инструмент. Ошибки превращаются в текст — его увидит модель.
    pub fn execute<'a>(&'a self, name: &'a str, args: &'a Value, ctx: &'a ToolContext) -> BoxFut<'a> {
        Box::pin(async move {
            let t0 = Instant::now();
            if !self.definitions(ctx).iter().any(|d| d.name == name) {
                return format!("Ошибка: инструмент {name} недоступен");
            }
            let res = match name {
                "get_datetime" => Ok(self.get_datetime()),
                "get_weather" => self.get_weather(args, ctx).await,
                "set_volume" => Ok(self.set_volume(args, ctx)),
                "set_lights" => Ok(self.set_lights(args, ctx)),
                "set_timer" => Ok(self.set_timer(args, ctx)),
                "cancel_timers" => Ok(self.cancel_timers(ctx)),
                "switch_agent" => Ok(self.switch_agent(args, ctx)),
                "ask_agent" => self.ask_agent(args, ctx).await,
                _ => Err(format!("инструмент {name} недоступен")),
            };
            match res {
                Ok(r) => {
                    let short: String = r.chars().take(120).collect();
                    self.log.info(format!("инструмент {name}({args}) → {short} [{} мс]", t0.elapsed().as_millis()));
                    r
                }
                Err(e) => {
                    self.log.warn(format!("инструмент {name} упал: {e}"));
                    format!("Ошибка инструмента {name}: {e}")
                }
            }
        })
    }

    fn get_datetime(&self) -> String {
        let tz = &self.config.server.timezone;
        let (date, time) = ru_now(tz);
        format!("Сейчас {time}, {date} (часовой пояс {tz})")
    }

    async fn get_weather(&self, args: &Value, ctx: &ToolContext) -> Result<String, String> {
        let city = args["city"].as_str().map(str::trim).filter(|s| !s.is_empty());
        let name = city.unwrap_or(&self.config.tools.weather.default_city).to_string();
        let fetch = async {
            let geo: Value = self
                .http
                .get(&self.geocoding_url)
                .query(&[("count", "1"), ("language", "ru"), ("name", &name)])
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let place = &geo["results"][0];
            if place.is_null() {
                return Ok(format!("Не нашёл город «{name}»"));
            }
            let (lat, lon) = (place["latitude"].to_string(), place["longitude"].to_string());
            let w: Value = self
                .http
                .get(&self.forecast_url)
                .query(&[
                    ("latitude", lat.as_str()),
                    ("longitude", lon.as_str()),
                    ("current", "temperature_2m,apparent_temperature,weather_code,wind_speed_10m,relative_humidity_2m"),
                    ("daily", "temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code"),
                    ("timezone", "auto"),
                    ("forecast_days", "2"),
                    ("wind_speed_unit", "ms"),
                ])
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let r = |v: &Value| v.as_f64().map(|f| f.round() as i64).unwrap_or(0);
            let c = &w["current"];
            let d = &w["daily"];
            let desc = |v: &Value| weather_code(v.as_i64().unwrap_or(-1));
            let admin = place["admin1"].as_str().map(|a| format!(" ({a})")).unwrap_or_default();
            let day = |i: usize, label: &str| {
                format!(
                    "{label} от {} до {}°C, {}, вероятность осадков {}%.",
                    r(&d["temperature_2m_min"][i]),
                    r(&d["temperature_2m_max"][i]),
                    desc(&d["weather_code"][i]),
                    r(&d["precipitation_probability_max"][i])
                )
            };
            Ok(format!(
                "{}{admin}: сейчас {}°C, ощущается как {}°C, {}, ветер {} м/с, влажность {}%. {} {}",
                place["name"].as_str().unwrap_or(&name),
                r(&c["temperature_2m"]),
                r(&c["apparent_temperature"]),
                desc(&c["weather_code"]),
                r(&c["wind_speed_10m"]),
                r(&c["relative_humidity_2m"]),
                day(0, "Сегодня"),
                day(1, "Завтра"),
            ))
        };
        tokio::select! {
            _ = ctx.cancel.cancelled() => Err("отменено".into()),
            r = fetch => r,
        }
    }

    fn set_volume(&self, args: &Value, ctx: &ToolContext) -> String {
        let s = ctx.session.as_ref().unwrap();
        let v = num(&args["level"]).unwrap_or_else(|| s.volume() as f64 + num(&args["change"]).unwrap_or(0.0));
        let v = v.round().clamp(0.0, 100.0) as u8;
        s.set_volume(v, true);
        format!("Громкость {v}%")
    }

    fn set_lights(&self, args: &Value, ctx: &ToolContext) -> String {
        let mode = args["mode"].as_str().unwrap_or("auto");
        let color = args["color"].as_str().filter(|s| !s.is_empty());
        let hex = color.and_then(parse_color);
        if let (Some(c), None) = (color, &hex) {
            return format!("Не знаю цвет «{c}». Используй #RRGGBB или простое название.");
        }
        let brightness = num(&args["brightness"]).map(|b| b.round().clamp(1.0, 100.0) as u8);
        ctx.session.as_ref().unwrap().set_lights(mode, hex.as_deref(), brightness);
        if mode == "off" {
            "Подсветка выключена".into()
        } else {
            format!("Подсветка: {mode}{}", hex.map(|h| format!(", цвет {h}")).unwrap_or_default())
        }
    }

    fn set_timer(&self, args: &Value, ctx: &ToolContext) -> String {
        let sec = num(&args["seconds"]).unwrap_or(60.0).round().clamp(1.0, 86400.0) as u64;
        let label = args["label"].as_str().unwrap_or("").trim().to_string();
        let s = ctx.session.as_ref().unwrap();
        ctx.timers.as_ref().unwrap().add(s.device_id(), sec, &label);
        let l = if label.is_empty() { String::new() } else { format!(" «{label}»") };
        format!("Таймер{l} поставлен на {}", human_duration(sec))
    }

    fn cancel_timers(&self, ctx: &ToolContext) -> String {
        let n = ctx.timers.as_ref().unwrap().cancel_all(ctx.session.as_ref().unwrap().device_id());
        if n > 0 { format!("Отменено таймеров: {n}") } else { "Активных таймеров нет".into() }
    }

    fn switch_agent(&self, args: &Value, ctx: &ToolContext) -> String {
        let agent = args["agent"].as_str().unwrap_or("");
        let reg = ctx.registry.as_ref().unwrap();
        if agent != "auto" && !reg.has(agent) {
            return format!("Нет агента {agent}");
        }
        ctx.session.as_ref().unwrap().set_agent(agent, true);
        let name = if agent == "auto" { "автовыбор".to_string() } else { reg.get(agent).unwrap().name().to_string() };
        format!("Дальше отвечает: {name}")
    }

    async fn ask_agent(&self, args: &Value, ctx: &ToolContext) -> Result<String, String> {
        let id = args["agent"].as_str().unwrap_or("");
        let question = args["question"].as_str().unwrap_or("");
        let Some(target) = ctx.registry.as_ref().and_then(|r| r.get(id)) else {
            return Ok(format!("Нет агента {id}"));
        };
        if !target.available() {
            return Ok(format!("{} сейчас недоступен", target.name()));
        }
        if let Some(f) = &ctx.on_delegate {
            f(&target);
        }
        let system = format!(
            "{}\nТебе задаёт вопрос другой агент ({}). Ответь по существу, кратко, обычным текстом.",
            ctx.base_system, ctx.agent_name
        );
        let answer = target.complete(&system, question, 600, &ctx.cancel).await.map_err(|e| e.message)?;
        Ok(format!("Ответ {}: {answer}", target.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_colors() {
        assert_eq!(human_duration(1), "1 секунду");
        assert_eq!(human_duration(125), "2 минуты 5 секунд");
        assert_eq!(human_duration(3600 * 5 + 60 * 21), "5 часов 21 минуту");
        assert_eq!(parse_color("Тёплый").as_deref(), Some("#ffb060"));
        assert_eq!(parse_color("FF0000").as_deref(), Some("#ff0000"));
        assert_eq!(parse_color("бордовый"), None);
    }
}
