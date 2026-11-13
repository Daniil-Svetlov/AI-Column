//! Потоковая озвучка: предложения → TTS (с опережением) → PCM-кадры на колонку с контролем темпа.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::Tts;
use crate::audio::pcm_to_bytes;
use crate::logger::Logger;
use crate::session::DeviceSession;

/// Сколько предложений синтезировать заранее.
const LOOKAHEAD: usize = 2;
/// Насколько секунд можно обгонять воспроизведение.
const AHEAD_SEC: f64 = 3.0;
/// Размер бинарного кадра.
const CHUNK_BYTES: usize = 4096;
const MAX_BUFFERED: usize = 256 * 1024;

#[derive(Default)]
struct Shared {
    started: AtomicBool,
    first_audio: Mutex<Option<Instant>>,
    errors: AtomicUsize,
    sent_samples: Mutex<u64>,
}

pub struct SpeechStreamer {
    tx: Option<mpsc::UnboundedSender<String>>,
    task: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    session: Arc<DeviceSession>,
    cancel: CancellationToken,
    rate: u32,
    turn_id: u64,
}

struct Worker {
    rx: mpsc::UnboundedReceiver<String>,
    closed: bool,
    pending: VecDeque<String>,
    inflight: VecDeque<JoinHandle<Option<Vec<i16>>>>,
    tts: Arc<Tts>,
    session: Arc<DeviceSession>,
    rate: u32,
    cancel: CancellationToken,
    log: Logger,
    shared: Arc<Shared>,
    turn_id: u64,
    t0: Instant,
    sent: u64,
}

impl Worker {
    fn drain_rx(&mut self) {
        while let Ok(t) = self.rx.try_recv() {
            self.pending.push_back(t);
        }
    }

    fn start_synth(&mut self) {
        while self.inflight.len() < 1 + LOOKAHEAD {
            let Some(text) = self.pending.pop_front() else { break };
            let (tts, rate, cancel, log, shared) =
                (self.tts.clone(), self.rate, self.cancel.clone(), self.log.clone(), self.shared.clone());
            self.inflight.push_back(tokio::spawn(async move {
                let r = tokio::select! {
                    _ = cancel.cancelled() => return None,
                    r = tts.synthesize(&text, rate) => r,
                };
                match r {
                    Ok(pcm) => Some(pcm),
                    Err(e) => {
                        if !cancel.is_cancelled() {
                            shared.errors.fetch_add(1, Ordering::Relaxed);
                            let short: String = text.chars().take(40).collect();
                            log.warn(format!("TTS не смог озвучить «{short}»: {e}"));
                        }
                        None
                    }
                }
            }));
        }
    }

    async fn run(mut self) {
        loop {
            self.start_synth();
            if self.inflight.is_empty() {
                if self.closed {
                    break;
                }
                tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    m = self.rx.recv() => match m {
                        Some(t) => self.pending.push_back(t),
                        None => self.closed = true,
                    },
                }
                continue;
            }
            let front = self.inflight.front_mut().unwrap();
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break,
                r = front => {
                    self.inflight.pop_front();
                    if let Ok(Some(pcm)) = r {
                        if !pcm.is_empty() {
                            self.send(&pcm).await;
                        }
                    }
                }
                m = self.rx.recv(), if !self.closed => match m {
                    Some(t) => self.pending.push_back(t),
                    None => self.closed = true,
                },
            }
        }
        for h in &self.inflight {
            h.abort();
        }
    }

    async fn send(&mut self, pcm: &[i16]) {
        if !self.shared.started.swap(true, Ordering::Relaxed) {
            self.t0 = Instant::now();
            self.session.send(json!({"type": "audio_start", "rate": self.rate, "format": "pcm16", "turn": self.turn_id}));
            self.session.state().state = "speaking".into();
            *self.shared.first_audio.lock().unwrap() = Some(Instant::now());
        }
        let rate = self.rate as f64;
        for chunk in pcm_to_bytes(pcm).chunks(CHUNK_BYTES) {
            loop {
                if self.cancel.is_cancelled() || !self.session.is_open() {
                    return;
                }
                let ahead = self.sent as f64 / rate - self.t0.elapsed().as_secs_f64();
                if ahead < -0.25 {
                    // колонка «голодала» — переякоримся
                    self.t0 = Instant::now() - Duration::from_secs_f64(self.sent as f64 / rate);
                }
                if ahead < AHEAD_SEC && self.session.buffered() < MAX_BUFFERED {
                    break;
                }
                self.drain_rx();
                self.start_synth();
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            self.session.send_audio(chunk.to_vec());
            self.sent += chunk.len() as u64 / 2;
            *self.shared.sent_samples.lock().unwrap() = self.sent;
            let left = self.sent as f64 / rate - self.t0.elapsed().as_secs_f64();
            self.session.state().speaking_until = chrono::Utc::now().timestamp_millis() + (left * 1000.0) as i64 + 500;
        }
    }
}

impl SpeechStreamer {
    pub fn new(
        tts: Arc<Tts>,
        session: Arc<DeviceSession>,
        rate: u32,
        cancel: CancellationToken,
        log: Logger,
        turn_id: u64,
    ) -> SpeechStreamer {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared::default());
        let worker = Worker {
            rx,
            closed: false,
            pending: VecDeque::new(),
            inflight: VecDeque::new(),
            tts,
            session: session.clone(),
            rate,
            cancel: cancel.clone(),
            log,
            shared: shared.clone(),
            turn_id,
            t0: Instant::now(),
            sent: 0,
        };
        let task = tokio::spawn(worker.run());
        SpeechStreamer { tx: Some(tx), task: Some(task), shared, session, cancel, rate, turn_id }
    }

    pub fn say(&self, text: &str) {
        if text.is_empty() || self.cancel.is_cancelled() {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(text.to_string());
        }
    }

    pub fn has_audio(&self) -> bool {
        self.shared.started.load(Ordering::Relaxed)
    }

    pub fn errors(&self) -> usize {
        self.shared.errors.load(Ordering::Relaxed)
    }

    pub fn first_audio_at(&self) -> Option<Instant> {
        *self.shared.first_audio.lock().unwrap()
    }

    /// Дождаться, пока всё сказанное уйдёт на колонку; закрыть аудиопоток.
    pub async fn finish(&mut self) {
        self.tx.take();
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
        if self.has_audio() && !self.cancel.is_cancelled() {
            let sent = *self.shared.sent_samples.lock().unwrap();
            let seconds = (sent as f64 / self.rate as f64 * 100.0).round() / 100.0;
            self.session.send(json!({"type": "audio_end", "turn": self.turn_id, "seconds": seconds}));
        }
    }
}

impl Drop for SpeechStreamer {
    fn drop(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}
