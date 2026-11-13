//! Сервер умной колонки AI-Column: речь → агенты (Claude, Qwen, Kimi и др.) → речь.
pub mod agents;
pub mod app;
pub mod assistant;
pub mod audio;
pub mod config;
pub mod http;
pub mod hub;
pub mod logger;
pub mod session;
pub mod speech;
pub mod store;
pub mod text;
pub mod timers;
