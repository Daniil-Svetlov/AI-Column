//! Симулятор колонки: проверить сервер без железа.
//!
//!   ai-column-server simulate --text "Квен, который час?"
//!   ai-column-server simulate --wav вопрос.wav --out ответ.wav
//!
//! Опции:
//!   --url ws://localhost:8080/ws   адрес сервера
//!   --token XXX                    AUTH_TOKEN (по умолчанию из .env)
//!   --id sim-1                     id «колонки»
//!   --agent qwen                   принудительно выбрать агента (только для --text)
//!   --fast                         отправлять WAV быстрее реального времени
//!   --out reply.wav                куда сохранить звук ответа
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use ai_column_server::audio::{bytes_to_pcm, encode_wav, parse_wav, pcm_to_bytes, resample};

const MIC_RATE: u32 = 16000;
const PLAY_RATE: u32 = 24000;

fn parse_args(args: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            match args.get(i + 1).filter(|n| !n.starts_with("--")) {
                Some(v) => {
                    out.insert(k.to_string(), v.clone());
                    i += 1;
                }
                None => {
                    out.insert(k.to_string(), "true".into());
                }
            }
        }
        i += 1;
    }
    out
}

pub async fn run(base: &Path, args: &[String]) -> ExitCode {
    let _ = dotenvy::from_path(base.join(".env"));
    let opt = parse_args(args);
    if !opt.contains_key("text") && !opt.contains_key("wav") {
        println!("Нужно --text \"вопрос\" или --wav файл.wav (см. ai-column-server simulate --help)");
        return ExitCode::from(2);
    }
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let base_url = opt.get("url").cloned().unwrap_or_else(|| format!("ws://localhost:{port}/ws"));
    let mut url = format!("{base_url}?id={}", urlencoding::encode(opt.get("id").map(String::as_str).unwrap_or("simulator")));
    if let Some(t) = opt.get("token").cloned().or_else(|| std::env::var("AUTH_TOKEN").ok()).filter(|t| !t.is_empty()) {
        url += &format!("&token={}", urlencoding::encode(&t));
    }
    let t0 = Instant::now();
    let stamp = || format!("{:>5} мс", t0.elapsed().as_millis());

    let (ws, _) = match tokio_tungstenite::connect_async(&url).await {
        Ok(c) => c,
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
            eprintln!("✖ сервер ответил {} — проверь --token / AUTH_TOKEN", r.status().as_u16());
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("✖ {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}  подключился к {base_url}", stamp());
    let (mut tx, mut rx) = ws.split();
    let hello = json!({"type": "hello", "fw": "simulator", "mic_rate": MIC_RATE, "play_rate": PLAY_RATE, "battery": 100});
    let _ = tx.send(Message::Text(hello.to_string().into())).await;

    let mut audio: Vec<u8> = vec![];
    let (mut audio_start, mut audio_end, mut reply_end) = (false, false, false);
    let mut reply = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(deadline);

    loop {
        let msg = tokio::select! {
            _ = &mut deadline => {
                eprintln!("✖ сервер не ответил за 120 с");
                return ExitCode::FAILURE;
            }
            m = rx.next() => m,
        };
        let Some(Ok(msg)) = msg else { break };
        let m: Value = match msg {
            Message::Binary(b) => {
                if audio_start && !audio_end {
                    audio.extend_from_slice(&b);
                }
                continue;
            }
            Message::Text(t) => serde_json::from_str(&t).unwrap_or(Value::Null),
            Message::Close(_) => break,
            _ => continue,
        };
        match m["type"].as_str().unwrap_or("") {
            "welcome" => {
                let agents = m["agents"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| format!("{}{}", x["name"].as_str().unwrap_or(""), if x["available"] == false { " (недоступен)" } else { "" }))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!("{}  сервер: ассистент «{}», агенты: {agents}; выбрано: {}", stamp(), m["assistant"].as_str().unwrap_or(""), m["agent"]);
                if let Some(file) = opt.get("wav") {
                    let wav = match std::fs::read(file).map_err(|e| e.to_string()).and_then(|b| parse_wav(&b).map_err(|e| e.0)) {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("✖ {file}: {e}");
                            return ExitCode::FAILURE;
                        }
                    };
                    let pcm = pcm_to_bytes(&resample(&wav.samples, wav.sample_rate, MIC_RATE));
                    let _ = tx.send(Message::Text(json!({"type": "listen_start", "mode": "tap"}).to_string().into())).await;
                    let frame = (MIC_RATE / 50 * 2) as usize; // 20 мс
                    for chunk in pcm.chunks(frame) {
                        let _ = tx.send(Message::Binary(chunk.to_vec().into())).await;
                        if !opt.contains_key("fast") {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    }
                    let _ = tx.send(Message::Text(json!({"type": "listen_stop"}).to_string().into())).await;
                    println!("{}  → отправил {:.1} с звука", stamp(), pcm.len() as f64 / 2.0 / MIC_RATE as f64);
                } else {
                    let msg = json!({"type": "text", "text": opt["text"], "agent": opt.get("agent")});
                    let _ = tx.send(Message::Text(msg.to_string().into())).await;
                }
            }
            "transcript" => println!("{}  распознано: «{}»", stamp(), m["text"].as_str().unwrap_or("")),
            "agent" => println!("{}  отвечает: {} ({})", stamp(), m["name"].as_str().unwrap_or(""), m["reason"].as_str().unwrap_or("")),
            "delegate" => println!("{}  {} спрашивает {}…", stamp(), m["from"].as_str().unwrap_or(""), m["name"].as_str().unwrap_or("")),
            "reply_delta" => {
                let t = m["text"].as_str().unwrap_or("");
                reply.push_str(t);
                println!("{}  текст: {t}", stamp());
            }
            "audio_start" => {
                audio_start = true;
                println!("{}  ♪ начало звука ({} Гц)", stamp(), m["rate"]);
            }
            "audio_end" => {
                audio_end = true;
                println!("{}  ♪ конец звука ({} с)", stamp(), m["seconds"]);
            }
            "reply_end" => {
                reply_end = true;
                if reply.is_empty() {
                    reply = m["text"].as_str().unwrap_or("").to_string();
                }
            }
            "error" => println!("{}  ⚠ ошибка: {}", stamp(), m["message"].as_str().unwrap_or("")),
            "state" => {}
            _ => println!("{}  {m}", stamp()),
        }
        let _ = std::io::stdout().flush();
        if reply_end && (!audio_start || audio_end) {
            break;
        }
    }

    println!("\nОтвет: {}", reply.trim());
    let pcm = bytes_to_pcm(&audio);
    if pcm.is_empty() {
        println!("Звука в ответе нет (проверь TTS).");
    } else {
        let out = opt.get("out").cloned().unwrap_or_else(|| "reply.wav".into());
        if let Err(e) = std::fs::write(&out, encode_wav(&pcm, PLAY_RATE)) {
            eprintln!("✖ {out}: {e}");
            return ExitCode::FAILURE;
        }
        println!("Звук ответа: {:.1} с → {out}", pcm.len() as f64 / PLAY_RATE as f64);
    }
    let _ = tx.close().await;
    ExitCode::SUCCESS
}
