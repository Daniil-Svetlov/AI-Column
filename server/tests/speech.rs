//! STT/TTS-клиенты против поддельных серверов (формат запросов как у Piper, speaches/OpenAI, whisper.cpp).
mod common;

use ai_column_server::audio::{encode_wav, pcm_to_bytes};
use ai_column_server::config::{SttCfg, TtsCfg};
use ai_column_server::speech::{Stt, Tts};
use common::{FakeResp, fake_http, tone};
use serde_json::json;

fn tts_cfg(provider: &str, base: &str) -> TtsCfg {
    TtsCfg { provider: provider.into(), base_url: Some(base.into()), timeout_ms: 5000, ..Default::default() }
}

fn stt_cfg(provider: &str, base: &str) -> SttCfg {
    SttCfg { provider: provider.into(), base_url: Some(base.into()), timeout_ms: 5000, language: None, ..Default::default() }
}

#[tokio::test]
async fn piper_synthesize_json_wav_resampled() {
    let fake = fake_http(|r| {
        if r.path != "/synthesize" {
            return FakeResp::Text(404, String::new());
        }
        let b = r.json();
        assert_eq!(b["text"], "Привет");
        assert_eq!(b["voice"], "ru_RU-irina-medium");
        assert!((b["length_scale"].as_f64().unwrap() - 1.0 / 1.25).abs() < 1e-9);
        FakeResp::Bytes(200, encode_wav(&tone(22050), 22050))
    })
    .await;
    let mut cfg = tts_cfg("piper", &fake.url);
    cfg.voice = Some("ru_RU-irina-medium".into());
    cfg.speed = 1.25;
    let pcm = Tts::new(&cfg).synthesize("Привет", 24000).await.unwrap();
    assert!((pcm.len() as i64 - 24000).abs() <= 1);
}

#[tokio::test]
async fn old_piper_falls_back_to_root() {
    let fake = fake_http(|r| {
        if r.path == "/" {
            return FakeResp::Bytes(200, encode_wav(&tone(1600), 16000));
        }
        FakeResp::Text(404, "not found".into())
    })
    .await;
    let tts = Tts::new(&tts_cfg("piper", &fake.url));
    assert_eq!(tts.synthesize("Тест", 16000).await.unwrap().len(), 1600);
    assert_eq!(fake.requests().iter().map(|r| r.path.clone()).collect::<Vec<_>>(), ["/synthesize", "/"]);
}

#[tokio::test]
async fn openai_tts_wav_and_raw_pcm() {
    let fake = fake_http(|r| {
        assert_eq!(r.path, "/v1/audio/speech");
        assert_eq!(r.header("authorization"), "Bearer k");
        if r.json()["response_format"] == "pcm" {
            return FakeResp::Bytes(200, pcm_to_bytes(&tone(2400)));
        }
        FakeResp::Bytes(200, encode_wav(&tone(2400), 24000))
    })
    .await;
    let mut cfg = tts_cfg("openai", &format!("{}/v1", fake.url));
    cfg.api_key = Some("k".into());
    assert_eq!(Tts::new(&cfg).synthesize("a", 24000).await.unwrap().len(), 2400);
    cfg.response_format = Some("pcm".into());
    assert_eq!(Tts::new(&cfg).synthesize("a", 12000).await.unwrap().len(), 1200);
    assert_eq!(fake.requests()[0].json()["input"], "a");
}

#[tokio::test]
async fn openai_stt_multipart_with_language_and_hints() {
    let fake = fake_http(|r| {
        assert_eq!(r.path, "/v1/audio/transcriptions");
        assert!(r.header("content-type").contains("multipart/form-data"));
        let body = r.text();
        assert!(body.contains("name=\"model\"\r\n\r\nSystran/faster-whisper-medium"));
        assert!(body.contains("name=\"language\"\r\n\r\nru"));
        assert!(body.contains("name=\"prompt\"\r\n\r\nClaude, Qwen."));
        assert!(body.contains("RIFF"));
        FakeResp::Json(200, json!({"text": " Привет, колонка! "}))
    })
    .await;
    let mut cfg = stt_cfg("openai", &format!("{}/v1", fake.url));
    cfg.model = Some("Systran/faster-whisper-medium".into());
    cfg.language = Some("ru".into());
    let stt = Stt::new(&cfg, &["Claude".into(), "Qwen".into()]);
    assert_eq!(stt.transcribe(&tone(16000), 16000).await.unwrap(), "Привет, колонка!");
}

#[tokio::test]
async fn whisper_echoing_the_prompt_is_silence() {
    let fake = fake_http(|_| FakeResp::Json(200, json!({"text": "Claude, Qwen"}))).await;
    let mut cfg = stt_cfg("openai", &fake.url);
    cfg.model = Some("m".into());
    let stt = Stt::new(&cfg, &["Claude".into(), "Qwen".into()]);
    assert_eq!(stt.transcribe(&tone(1600), 16000).await.unwrap(), "");
}

#[tokio::test]
async fn whispercpp_inference() {
    let fake = fake_http(|r| {
        assert_eq!(r.path, "/inference");
        assert!(r.text().contains("name=\"response_format\"\r\n\r\njson"));
        FakeResp::Json(200, json!({"text": "Который час?"}))
    })
    .await;
    let mut cfg = stt_cfg("whispercpp", &fake.url);
    cfg.language = Some("ru".into());
    assert_eq!(Stt::new(&cfg, &[]).transcribe(&tone(8000), 16000).await.unwrap(), "Который час?");
}

#[tokio::test]
async fn stt_server_error_is_readable() {
    let fake = fake_http(|_| FakeResp::Text(500, "model not loaded".into())).await;
    let mut cfg = stt_cfg("openai", &fake.url);
    cfg.model = Some("m".into());
    let e = Stt::new(&cfg, &[]).transcribe(&tone(100), 16000).await.unwrap_err();
    assert!(e.contains("STT HTTP 500: model not loaded"), "{e}");
}
