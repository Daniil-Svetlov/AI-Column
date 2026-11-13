//! Выбор агента для реплики: голосовое обращение → выбор на колонке → правила → классификатор → по умолчанию.
//! Плюс порядок запасных агентов, если выбранный не ответил.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::Registry;
use crate::config::RouterCfg;
use crate::logger::Logger;
use crate::text::normalize;

const END: &str = r"(?:$|[\s,.:;!?…—-])";
const TAIL: &str = r"[\s,.:;!?…—-]*";
const AUTO_WORDS: &str = r"(?:авто(?:матически(?:й|е))?(?:\s+(?:режим|выбор))?|автовыбор|auto)";

static RE_RESET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:новый разговор|начн(?:и|ем) (?:заново|сначала|новый разговор)|забудь (?:все|разговор|контекст)|сбрось (?:контекст|разговор|историю)|очисти (?:историю|контекст))").unwrap()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Reset,
    Switch(String),
    Address { agent_id: String, text: String },
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub agent_id: String,
    pub text: String,
    pub reason: &'static str,
}

pub struct Router {
    registry: Arc<Registry>,
    cfg: RouterCfg,
    log: Logger,
    alias_to_id: HashMap<String, String>,
    re_address: Regex,
    re_switch: Regex,
}

impl Router {
    pub fn new(registry: Arc<Registry>, cfg: RouterCfg, log: Logger) -> Router {
        let mut alias_to_id = HashMap::new();
        for a in registry.list() {
            for w in [a.id(), a.name()].into_iter().chain(a.aliases().iter().map(String::as_str)) {
                let k = normalize(w).trim().to_string();
                if !k.is_empty() {
                    alias_to_id.insert(k, a.id().to_string());
                }
            }
        }
        let mut words: Vec<&String> = alias_to_id.keys().collect();
        words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
        let alt = words.iter().map(|w| regex::escape(w)).collect::<Vec<_>>().join("|");
        let re_address = Regex::new(&format!(
            r"^\s*(?:(?:эй|ой|окей|ок|ok|hey|слушай|спроси|спросите|спросить|пусть|а|ну)[\s,]+)?({alt}){END}{TAIL}"
        ))
        .unwrap();
        let re_switch = Regex::new(&format!(
            r"^\s*(?:переключи(?:сь)?|перейди|смени(?:\s+агента)?|выбери|поставь|давай)\s+(?:на\s+|агента\s+)?({alt}|{AUTO_WORDS}){END}{TAIL}$"
        ))
        .unwrap();
        Router { registry, cfg, log, alias_to_id, re_address, re_switch }
    }

    pub fn parse(&self, text: &str) -> Option<Parsed> {
        let n = normalize(text);
        if RE_RESET.is_match(&n) {
            return Some(Parsed::Reset);
        }
        if !self.cfg.voice_prefix {
            return None;
        }
        if let Some(c) = self.re_switch.captures(&n) {
            let word = c[1].trim();
            let id = self.alias_to_id.get(word).cloned().unwrap_or_else(|| "auto".into());
            return Some(Parsed::Switch(id));
        }
        if let Some(c) = self.re_address.captures(&n) {
            let id = self.alias_to_id.get(c[1].trim()).cloned()?;
            // normalize сохраняет число символов — отрезаем столько же от исходного текста
            let skip = n[..c.get(0).unwrap().end()].chars().count();
            let rest: String = text.chars().skip(skip).collect::<String>().trim().to_string();
            if rest.is_empty() {
                return Some(Parsed::Switch(id));
            }
            return Some(Parsed::Address { agent_id: id, text: rest });
        }
        None
    }

    fn available(&self, id: &str) -> bool {
        self.registry.get(id).is_some_and(|a| a.available())
    }

    fn default_agent(&self) -> String {
        let order: Vec<String> =
            self.cfg.default_agent.iter().cloned().chain(self.cfg.fallback.iter().cloned()).chain(self.registry.ids()).collect();
        order
            .iter()
            .find(|id| self.available(id))
            .cloned()
            .or_else(|| self.cfg.default_agent.clone())
            .or_else(|| self.registry.ids().into_iter().next())
            .unwrap_or_default()
    }

    async fn classify(&self, text: &str, cancel: &CancellationToken) -> Option<String> {
        let c = self.cfg.classifier.as_ref()?;
        let judge = self.registry.get(&c.agent).filter(|a| a.available())?;
        let options: Vec<_> = self.registry.list().iter().filter(|a| a.available()).cloned().collect();
        if options.len() < 2 {
            return None;
        }
        let list = options
            .iter()
            .map(|a| format!("- {}: {}", a.id(), if a.description().is_empty() { a.name() } else { a.description() }))
            .collect::<Vec<_>>()
            .join("\n");
        let system = format!(
            "Ты диспетчер голосового помощника. Выбери, какой агент лучше ответит на запрос.\nАгенты:\n{list}\nОтветь одним словом — id агента, без пояснений."
        );
        let t0 = Instant::now();
        let child = cancel.child_token();
        let res = tokio::time::timeout(Duration::from_millis(c.timeout_ms), judge.complete(&system, text, 24, &child)).await;
        child.cancel();
        match res {
            Ok(Ok(out)) => {
                let n = normalize(&out);
                let hit = options.iter().find(|a| n.contains(&a.id().to_lowercase())).or_else(|| {
                    options.iter().find(|a| {
                        std::iter::once(a.name()).chain(a.aliases().iter().map(String::as_str)).any(|w| n.contains(&normalize(w)))
                    })
                });
                self.log.debug(format!(
                    "классификатор ({}) за {} мс: «{out}» → {}",
                    judge.id(),
                    t0.elapsed().as_millis(),
                    hit.map(|a| a.id()).unwrap_or("—")
                ));
                hit.map(|a| a.id().to_string())
            }
            Ok(Err(e)) => {
                self.log.warn(format!("классификатор не ответил ({e}) — беру агента по умолчанию"));
                None
            }
            Err(_) => {
                self.log.warn("классификатор не ответил (таймаут) — беру агента по умолчанию");
                None
            }
        }
    }

    pub async fn choose(&self, text: &str, device_agent: &str, cancel: &CancellationToken) -> Choice {
        if let Some(Parsed::Address { agent_id, text }) = self.parse(text) {
            return Choice { agent_id, text, reason: "voice" };
        }
        if !device_agent.is_empty() && device_agent != "auto" && self.registry.has(device_agent) {
            return Choice { agent_id: device_agent.into(), text: text.into(), reason: "device" };
        }
        let n = normalize(text);
        for rule in &self.cfg.rules {
            if rule.keywords.iter().any(|k| n.contains(k.as_str())) && self.available(&rule.agent) {
                return Choice { agent_id: rule.agent.clone(), text: text.into(), reason: "rule" };
            }
        }
        if let Some(id) = self.classify(text, cancel).await {
            return Choice { agent_id: id, text: text.into(), reason: "classifier" };
        }
        Choice { agent_id: self.default_agent(), text: text.into(), reason: "default" }
    }

    /// Порядок попыток: выбранный агент, затем запасные (недоступные по health-check пропускаем).
    pub fn candidates(&self, primary: &str) -> Vec<String> {
        let mut order: Vec<String> = vec![];
        for id in std::iter::once(primary.to_string()).chain(self.cfg.fallback.iter().cloned()) {
            if !id.is_empty() && !order.contains(&id) && self.registry.has(&id) {
                order.push(id);
            }
        }
        let avail: Vec<String> = order.iter().filter(|id| self.available(id)).cloned().collect();
        if !avail.is_empty() {
            avail
        } else if !order.is_empty() {
            order
        } else {
            vec![primary.to_string()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::normalize_config;
    use serde_json::json;

    fn setup(router: serde_json::Value) -> (Arc<Registry>, Router) {
        let mut r = json!({"default_agent": "qwen", "fallback": ["qwen", "claude"]});
        r.as_object_mut().unwrap().extend(router.as_object().unwrap().clone());
        let cfg = normalize_config(&json!({
            "agents": [
                {"id": "claude", "name": "Claude", "provider": "echo", "aliases": ["клод", "клауд"], "description": "сложное"},
                {"id": "qwen", "name": "Qwen", "provider": "echo", "aliases": ["квен", "куэн"], "description": "быстрое"},
                {"id": "kimi", "name": "Kimi", "provider": "echo", "aliases": ["кими"], "description": "тексты"},
            ],
            "router": r,
        }))
        .unwrap();
        let reg = Arc::new(Registry::new(&cfg.agents, Logger::silent()));
        let router = Router::new(reg.clone(), cfg.router, Logger::silent());
        (reg, router)
    }

    fn addr(id: &str, text: &str) -> Option<Parsed> {
        Some(Parsed::Address { agent_id: id.into(), text: text.into() })
    }

    #[test]
    fn address_by_name() {
        let (_, r) = setup(json!({}));
        assert_eq!(r.parse("Клод, сколько будет два плюс два?"), addr("claude", "сколько будет два плюс два?"));
        assert_eq!(r.parse("Спроси квен: какая столица Франции"), addr("qwen", "какая столица Франции"));
        assert_eq!(r.parse("Эй, Кими — напиши стих"), addr("kimi", "напиши стих"));
        assert_eq!(r.parse("Клодетта пришла"), None, "часть слова не считается обращением");
        assert_eq!(r.parse("Какая погода?"), None);
    }

    #[test]
    fn switch_and_reset() {
        let (_, r) = setup(json!({}));
        assert_eq!(r.parse("Переключись на Кими."), Some(Parsed::Switch("kimi".into())));
        assert_eq!(r.parse("переключись на авто"), Some(Parsed::Switch("auto".into())));
        assert_eq!(r.parse("Квен"), Some(Parsed::Switch("qwen".into())));
        assert_eq!(r.parse("Новый разговор"), Some(Parsed::Reset));
        assert_eq!(r.parse("Забудь всё"), Some(Parsed::Reset));
    }

    #[tokio::test]
    async fn priority_voice_device_rules_default() {
        let (_, r) = setup(json!({"rules": [{"agent": "claude", "keywords": ["код"]}]}));
        let c = CancellationToken::new();
        assert_eq!(r.choose("Кими, привет", "qwen", &c).await.agent_id, "kimi");
        assert_eq!(r.choose("напиши код", "kimi", &c).await.agent_id, "kimi");
        let rule = r.choose("Напиши код сортировки", "auto", &c).await;
        assert_eq!((rule.agent_id.as_str(), rule.reason), ("claude", "rule"));
        let def = r.choose("Как дела?", "auto", &c).await;
        assert_eq!((def.agent_id.as_str(), def.reason), ("qwen", "default"));
    }

    #[test]
    fn candidates_skip_unavailable() {
        let (reg, r) = setup(json!({}));
        assert_eq!(r.candidates("kimi"), ["kimi", "qwen", "claude"]);
        reg.get("kimi").unwrap().mark_down("x");
        assert_eq!(r.candidates("kimi"), ["qwen", "claude"]);
        for a in reg.list() {
            a.mark_down("x");
        }
        assert_eq!(r.candidates("kimi"), ["kimi", "qwen", "claude"], "если все «лежат» — всё равно пробуем");
    }
}
