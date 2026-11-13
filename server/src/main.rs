//! Точка входа сервера AI-Column.
//!   ai-column-server                    — запустить сервер
//!   ai-column-server check-config [--say] — проверить конфиг, агентов, STT и TTS
//!   ai-column-server simulate --text "…" — симулятор колонки (см. cli/simulate.rs)
mod cli;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use ai_column_server::app::App;
use ai_column_server::config::{Config, load_config};
use ai_column_server::logger::Logger;

/// Папка сервера: $AI_COLUMN_DIR или текущая. Там лежат config/, public/, .env, data/.
fn base_dir() -> PathBuf {
    std::env::var("AI_COLUMN_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
}

fn env_map() -> HashMap<String, String> {
    std::env::vars().collect()
}

pub fn load(base: &std::path::Path, file: Option<&str>) -> Result<Config, String> {
    // .env необязателен — переменные могут прийти из окружения (docker, systemd)
    let _ = dotenvy::from_path(base.join(".env"));
    load_config(file, base, &env_map()).map_err(|e| e.0)
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = base_dir();
    match args.first().map(String::as_str) {
        Some("check-config") => return cli::check_config::run(&base, &args).await,
        Some("simulate") => return cli::simulate::run(&base, &args[1..]).await,
        Some("-h" | "--help") => {
            println!("ai-column-server [--config путь]\nai-column-server check-config [--say]\nai-column-server simulate --text \"вопрос\" | --wav файл.wav");
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let config = match load(&base, flag_value(&args, "--config").as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let log = Logger::new(&config.server.log_level, "server");
    if let Some(f) = &config.file {
        log.info(format!("конфиг: {}", f.strip_prefix(&base).unwrap_or(f).display()));
    }
    let (host, port) = (config.server.host.clone(), config.server.port());
    let app = App::build(config, log.clone());
    if let Err(e) = app.listen(&host, port, true).await {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            eprintln!("Порт занят: {e}. Поменяй PORT в .env.");
        } else {
            eprintln!("{e}");
        }
        return ExitCode::FAILURE;
    }

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    log.info("останавливаюсь…");
    tokio::select! {
        _ = app.close() => {}
        _ = tokio::signal::ctrl_c() => return ExitCode::FAILURE,
    }
    ExitCode::SUCCESS
}
