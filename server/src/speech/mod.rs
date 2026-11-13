//! Речь: распознавание (STT), синтез (TTS) и потоковая отправка звука на колонку.
pub mod streamer;
pub mod stt;
pub mod tts;

pub use streamer::SpeechStreamer;
pub use stt::{Stt, is_likely_hallucination};
pub use tts::Tts;
