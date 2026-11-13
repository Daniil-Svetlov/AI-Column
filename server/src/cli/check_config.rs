//! Проверяет конфиг и доступность агентов, STT и TTS — без запуска сервера.
//!   ai-column-server check-config         — проверить конфиг и опросить агентов
//!   ai-column-server check-config --say   — ещё и синтезировать/распознать тестовую фразу
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use ai_column_server::agents::Registry;
use ai_column_server::logger::Logger;
use ai_column_server::speech::{Stt, Tts};

pub async fn run(base: &Path, args: &[String]) -> ExitCode {
    let cfg = match crate::load(base, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let file = cfg.file.as_ref().map(|f| f.strip_prefix(base).unwrap_or(f).display().to_string()).unwrap_or_default();
    println!("✔ конфиг {file} корректен");
    println!(
        "  агентов: {}, по умолчанию: {}/{}, запасные: {}",
        cfg.agents.len(),
        cfg.router.default,
        cfg.router.default_agent.as_deref().unwrap_or("-"),
        if cfg.router.fallback.is_empty() { "—".into() } else { cfg.router.fallback.join(" → ") }
    );
    if cfg.server.auth_token.is_empty() {
        println!("⚠ AUTH_TOKEN не задан — сервер будет открыт для всей сети");
    }
    let reg = Registry::new(&cfg.agents, Logger::silent());
    reg.check_all().await;
    for a in reg.list() {
        let st = a.status();
        let model = a.cfg.model.as_deref().unwrap_or("-");
        if st.ok == Some(true) {
            println!("✔ агент {} ({}:{model}) доступен, {} мс", a.id(), a.cfg.provider, st.latency_ms.unwrap_or(0));
        } else {
            println!("✖ агент {} ({}:{model}) — {}", a.id(), a.cfg.provider, st.error.unwrap_or_default());
        }
    }
    let tts = Tts::new(&cfg.speech.tts);
    let stt = Stt::new(&cfg.speech.stt, &[]);
    if args.iter().any(|a| a == "--say") {
        let t0 = Instant::now();
        match tts.synthesize("Привет! Это проверка синтеза речи.", 16000).await {
            Ok(pcm) => {
                println!("✔ TTS {}: {:.1} с звука за {} мс", tts.name(), pcm.len() as f64 / 16000.0, t0.elapsed().as_millis());
                let t1 = Instant::now();
                match stt.transcribe(&pcm, 16000).await {
                    Ok(text) => println!("✔ STT {}: «{text}» за {} мс", stt.name(), t1.elapsed().as_millis()),
                    Err(e) => println!("✖ речь: {e}"),
                }
            }
            Err(e) => println!("✖ речь: {e}"),
        }
    } else {
        println!("  STT: {}\n  TTS: {}\n  (добавь --say, чтобы проверить синтез и распознавание по-настоящему)", stt.name(), tts.name());
    }
    ExitCode::SUCCESS
}
