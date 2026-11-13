//! Логика прошивки AI-Column, не зависящая от железа ESP32-S3.
//! Всё здесь собирается и тестируется на компьютере: `cargo test`.
pub mod app;
pub mod button;
pub mod color;
pub mod config;
pub mod dsp;
pub mod leds;
pub mod mdns;
pub mod ring;
pub mod text;
pub mod touch;
pub mod ui;
