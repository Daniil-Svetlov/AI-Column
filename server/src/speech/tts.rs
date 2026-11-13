//! Синтез речи (TTS). Провайдеры:
//!   piper  — HTTP-сервер Piper (python3 -m piper.http_server), POST /synthesize
//!   openai — OpenAI-совместимый /v1/audio/speech (OpenAI, speaches, LocalAI, Kokoro-FastAPI, openedai-speech…)
//!   mock   — для тестов: короткий тональный сигнал вместо речи
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use crate::audio::decode_to_pcm;
use crate::config::TtsCfg;

pub struct Tts {
    cfg: TtsCfg,
    base: String,
    http: reqwest::Client,
    /// Piper: путь запроса (старые версии принимали POST на «/»).
    piper_path: Mutex<String>,
}

fn tone(text: &str, rate: u32) -> Vec<i16> {
    let dur = (0.25 + text.chars().count() as f64 * 0.03).min(3.0);
    let n = (dur * rate as f64).round() as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let env = 1f64.min(t / 0.02).min((dur - t) / 0.05).max(0.0);
            ((2.0 * std::f64::consts::PI * 440.0 * t).sin() * 6000.0 * env).round() as i16
        })
        .collect()
}

impl Tts {
    pub fn new(cfg: &TtsCfg) -> Tts {
        Tts {
            base: cfg.base_url.clone().unwrap_or_default().trim_end_matches('/').to_string(),
            http: reqwest::Client::builder().timeout(Duration::from_millis(cfg.timeout_ms)).build().expect("HTTP-клиент"),
            piper_path: Mutex::new(cfg.path.clone().unwrap_or_else(|| "/synthesize".into())),
            cfg: cfg.clone(),
        }
    }

    pub fn name(&self) -> String {
        match self.cfg.provider.as_str() {
            "mock" => "mock".into(),
            "piper" => format!("piper({}, {})", self.base, self.cfg.voice.as_deref().unwrap_or("голос по умолчанию")),
            _ => format!(
                "openai({}, {}/{})",
                self.base,
                self.cfg.model.as_deref().unwrap_or("tts-1"),
                self.cfg.voice.as_deref().unwrap_or("alloy")
            ),
        }
    }

    async fn post(&self, url: &str, body: &Value) -> Result<Vec<u8>, (u16, String)> {
        let mut rb = self.http.post(url).json(body);
        if let Some(k) = self.cfg.api_key.as_deref().filter(|k| *k != "none") {
            rb = rb.bearer_auth(k);
        }
        let res = rb.send().await.map_err(|e| (0, format!("TTS: {e}")))?;
        let status = res.status().as_u16();
        if status >= 400 {
            let body: String = res.text().await.unwrap_or_default().chars().take(300).collect();
            return Err((status, format!("TTS HTTP {status}: {body}")));
        }
        res.bytes().await.map(|b| b.to_vec()).map_err(|e| (0, format!("TTS: {e}")))
    }

    /// Текст → моно PCM16 с частотой rate.
    pub async fn synthesize(&self, text: &str, rate: u32) -> Result<Vec<i16>, String> {
        match self.cfg.provider.as_str() {
            "mock" => Ok(tone(text, rate)),
            "piper" => {
                let mut body = json!({"text": text});
                if let Some(v) = &self.cfg.voice {
                    body["voice"] = json!(v);
                }
                if let Some(s) = &self.cfg.speaker {
                    body["speaker"] = s.clone();
                }
                if self.cfg.speed > 0.0 && self.cfg.speed != 1.0 {
                    body["length_scale"] = json!(1.0 / self.cfg.speed);
                }
                let path = self.piper_path.lock().unwrap().clone();
                let wav = match self.post(&format!("{}{path}", self.base), &body).await {
                    Ok(w) => w,
                    Err((404, _)) if path != "/" => {
                        *self.piper_path.lock().unwrap() = "/".into();
                        self.post(&format!("{}/", self.base), &body).await.map_err(|e| e.1)?
                    }
                    Err((_, e)) => return Err(e),
                };
                decode_to_pcm(&wav, rate, None).await.map_err(|e| e.0)
            }
            _ => {
                let format = self.cfg.response_format.clone().unwrap_or_else(|| "wav".into());
                let mut body = json!({
                    "model": self.cfg.model.as_deref().unwrap_or("tts-1"),
                    "voice": self.cfg.voice.as_deref().unwrap_or("alloy"),
                    "input": text,
                    "response_format": format,
                });
                if self.cfg.speed > 0.0 && self.cfg.speed != 1.0 {
                    body["speed"] = json!(self.cfg.speed);
                }
                if let Some(i) = &self.cfg.instructions {
                    body["instructions"] = json!(i);
                }
                let audio = self.post(&format!("{}/audio/speech", self.base), &body).await.map_err(|e| e.1)?;
                // response_format=pcm у OpenAI — сырой PCM16 24 кГц без заголовка.
                let raw = (format == "pcm").then(|| self.cfg.pcm_rate.unwrap_or(24000));
                decode_to_pcm(&audio, rate, raw).await.map_err(|e| e.0)
            }
        }
    }
}
