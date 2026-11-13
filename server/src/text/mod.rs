//! Работа с текстом: очистка для озвучки, нарезка на предложения, фильтр <think>.
mod clean;
mod sentences;

pub use clean::{ThinkFilter, clean_for_speech, normalize};
pub use sentences::SentenceSplitter;
