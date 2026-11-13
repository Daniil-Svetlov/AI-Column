//! Распознавание речи (STT). Провайдеры:
//!   openai     — OpenAI-совместимый /v1/audio/transcriptions (speaches, faster-whisper-server, vLLM, LocalAI, OpenAI, Groq…)
//!   whispercpp — родной сервер whisper.cpp (POST /inference)
//!   mock       — для тестов: возвращает заранее заданный текст
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use reqwest::multipart::{Form, Part};
use serde_json::Value;

use crate::audio::encode_wav;
use crate::config::SttCfg;

pub struct Stt {
    cfg: SttCfg,
    base: String,
    prompt: Option<String>,
    http: reqwest::Client,
    mock_queue: Mutex<VecDeque<String>>,
}

fn norm(s: &str) -> String {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

impl Stt {
    /// hints — имена агентов, чтобы Whisper писал их правильно.
    pub fn new(cfg: &SttCfg, hints: &[String]) -> Stt {
        let prompt = cfg.prompt.clone().or_else(|| (!hints.is_empty()).then(|| format!("{}.", hints.join(", "))));
        Stt {
            base: cfg.base_url.clone().unwrap_or_default().trim_end_matches('/').to_string(),
            prompt,
            http: reqwest::Client::builder().timeout(Duration::from_millis(cfg.timeout_ms)).build().expect("HTTP-клиент"),
            mock_queue: Mutex::new(VecDeque::new()),
            cfg: cfg.clone(),
        }
    }

    pub fn name(&self) -> String {
        match self.cfg.provider.as_str() {
            "mock" => "mock".into(),
            "whispercpp" => format!("whispercpp({})", self.base),
            _ => format!("openai({}, {})", self.base, self.cfg.model.as_deref().unwrap_or("whisper-1")),
        }
    }

    /// Для тестов (provider: mock): что «распознать» в следующий раз.
    pub fn mock_push(&self, text: &str) {
        self.mock_queue.lock().unwrap().push_back(text.into());
    }

    /// На тишине Whisper иногда просто повторяет подсказку — это не речь.
    fn not_echo(&self, text: String) -> String {
        match &self.prompt {
            Some(p) if norm(&text) == norm(p) => String::new(),
            _ => text,
        }
    }

    pub async fn transcribe(&self, samples: &[i16], rate: u32) -> Result<String, String> {
        if self.cfg.provider == "mock" {
            let next = self.mock_queue.lock().unwrap().pop_front();
            return Ok(next.or_else(|| self.cfg.mock_text.clone()).unwrap_or_default());
        }
        let wav = Part::bytes(encode_wav(samples, rate)).file_name("speech.wav").mime_str("audio/wav").unwrap();
        let mut form = Form::new().part("file", wav);
        let url = if self.cfg.provider == "whispercpp" {
            form = form.text("response_format", "json").text("temperature", "0.0");
            format!("{}{}", self.base, self.cfg.path.as_deref().unwrap_or("/inference"))
        } else {
            form = form.text("model", self.cfg.model.clone().unwrap_or_else(|| "whisper-1".into())).text("response_format", "json");
            if let Some(t) = self.cfg.temperature {
                form = form.text("temperature", t.to_string());
            }
            format!("{}/audio/transcriptions", self.base)
        };
        if let Some(l) = &self.cfg.language {
            form = form.text("language", l.clone());
        }
        if let Some(p) = &self.prompt {
            form = form.text("prompt", p.clone());
        }
        let mut rb = self.http.post(url).multipart(form);
        if let Some(k) = self.cfg.api_key.as_deref().filter(|k| *k != "none") {
            rb = rb.bearer_auth(k);
        }
        let res = rb.send().await.map_err(|e| format!("STT: {e}"))?;
        let status = res.status();
        let body = res.text().await.map_err(|e| format!("STT: {e}"))?;
        if !status.is_success() {
            let short: String = body.chars().take(300).collect();
            return Err(format!("STT HTTP {}: {short}", status.as_u16()));
        }
        let text = match serde_json::from_str::<Value>(&body) {
            Ok(v) => v["text"].as_str().unwrap_or("").to_string(),
            Err(_) => body,
        };
        Ok(self.not_echo(text.trim().to_string()))
    }
}

/// Whisper на тишине любит «галлюцинировать» такие фразы — считаем их пустым распознаванием.
static HALLUCINATIONS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)^продолжение следует\.?\.?\.?$",
        r"(?i)^субтитры (?:сделал|делал|создавал)",
        r"(?i)^редактор субтитров",
        r"(?i)^спасибо за просмотр",
        r"(?i)^подписывайтесь на канал",
        r"(?i)^thank you\.?$",
        r"^\.+$",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});

pub fn is_likely_hallucination(text: &str) -> bool {
    let t = text.trim();
    t.is_empty() || HALLUCINATIONS.iter().any(|r| r.is_match(t))
}

#[cfg(test)]
mod tests {
    #[test]
    fn hallucinations() {
        assert!(super::is_likely_hallucination("Продолжение следует..."));
        assert!(super::is_likely_hallucination("  "));
        assert!(!super::is_likely_hallucination("Который час?"));
    }
}
