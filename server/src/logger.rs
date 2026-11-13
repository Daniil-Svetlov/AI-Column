//! Простой логгер: уровни, время, «область» (scope).
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 10,
    Info = 20,
    Warn = 30,
    Error = 40,
    Silent = 100,
}

impl Level {
    pub fn parse(s: &str) -> Level {
        match s {
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            "silent" => Level::Silent,
            _ => Level::Info,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Logger {
    min: Level,
    scope: Arc<str>,
}

impl Logger {
    pub fn new(level: &str, scope: &str) -> Logger {
        Logger { min: Level::parse(level), scope: scope.into() }
    }

    pub fn silent() -> Logger {
        Logger { min: Level::Silent, scope: "test".into() }
    }

    pub fn child(&self, sub: &str) -> Logger {
        Logger { min: self.min, scope: format!("{}:{}", self.scope, sub).into() }
    }

    fn emit(&self, lvl: Level, name: &str, msg: &str) {
        if lvl < self.min {
            return;
        }
        let ts = chrono::Local::now().format("%H:%M:%S%.3f");
        let line = format!("{ts} {name:<5} [{}] {msg}", self.scope);
        if lvl >= Level::Warn {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }

    pub fn debug(&self, msg: impl AsRef<str>) {
        self.emit(Level::Debug, "DEBUG", msg.as_ref());
    }
    pub fn info(&self, msg: impl AsRef<str>) {
        self.emit(Level::Info, "INFO", msg.as_ref());
    }
    pub fn warn(&self, msg: impl AsRef<str>) {
        self.emit(Level::Warn, "WARN", msg.as_ref());
    }
    pub fn error(&self, msg: impl AsRef<str>) {
        self.emit(Level::Error, "ERROR", msg.as_ref());
    }
}
