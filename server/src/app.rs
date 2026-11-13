//! Сборка сервера: конфиг → агенты → речь → WebSocket + REST.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::agents::{Registry, Router, Toolbox};
use crate::agents::tools::human_duration;
use crate::assistant::Assistant;
use crate::config::Config;
use crate::hub::DeviceHub;
use crate::logger::Logger;
use crate::speech::{Stt, Tts};
use crate::store::Store;
use crate::timers::TimerManager;

pub struct App {
    pub config: Arc<Config>,
    pub registry: Arc<Registry>,
    pub router: Arc<Router>,
    pub toolbox: Arc<Toolbox>,
    pub stt: Arc<Stt>,
    pub tts: Arc<Tts>,
    pub store: Arc<Store>,
    pub timers: Arc<TimerManager>,
    pub assistant: Arc<Assistant>,
    pub hub: Arc<DeviceHub>,
    pub log: Logger,
    pub started: Instant,
    stop: CancellationToken,
    mdns: Mutex<Option<mdns_sd::ServiceDaemon>>,
}

/// Имена агентов для подсказки Whisper: id/имя и первый русский псевдоним.
fn stt_hints(registry: &Registry) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for a in registry.list() {
        let ru = a.aliases().iter().find(|w| w.chars().any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')));
        for w in std::iter::once(a.name()).chain(ru.map(String::as_str)) {
            if !out.iter().any(|x| x == w) {
                out.push(w.to_string());
            }
        }
    }
    out
}

impl App {
    /// Собирает все части. Вызывать внутри tokio-рантайма.
    pub fn build(config: Config, log: Logger) -> Arc<App> {
        let config = Arc::new(config);
        let store = Arc::new(Store::open(&config.server.db_path, &log));
        let registry = Arc::new(Registry::new(&config.agents, log.child("agent")));
        let router = Arc::new(Router::new(registry.clone(), config.router.clone(), log.child("router")));
        let toolbox = Arc::new(Toolbox::new(config.clone(), log.child("tools")));
        let stt = Arc::new(Stt::new(&config.speech.stt, &stt_hints(&registry)));
        let tts = Arc::new(Tts::new(&config.speech.tts));
        let (timers, mut fired) = TimerManager::new(log.child("timer"));
        let assistant = Arc::new(Assistant {
            config: config.clone(),
            registry: registry.clone(),
            router: router.clone(),
            toolbox: toolbox.clone(),
            stt: stt.clone(),
            tts: tts.clone(),
            store: store.clone(),
            timers: timers.clone(),
            log: log.child("turn"),
        });
        let hub = DeviceHub::new(assistant.clone(), registry.clone(), store.clone(), config.clone(), log.child("ws"));

        // Сработавший таймер — колонка говорит об этом вслух.
        let (h, a, l) = (hub.clone(), assistant.clone(), log.clone());
        tokio::spawn(async move {
            while let Some((dev, t)) = fired.recv().await {
                let text = format!(
                    "Время вышло{}! Прошло {}.",
                    if t.label.is_empty() { String::new() } else { format!(": {}", t.label) },
                    human_duration(t.seconds)
                );
                match h.get(&dev) {
                    Some(s) => {
                        let a = a.clone();
                        tokio::spawn(async move { a.announce(&s, &text, Some("timer")).await });
                    }
                    None => l.warn(format!("таймер для {dev} сработал, но колонка не в сети")),
                }
            }
        });

        Arc::new(App {
            config,
            registry,
            router,
            toolbox,
            stt,
            tts,
            store,
            timers,
            assistant,
            hub,
            log,
            started: Instant::now(),
            stop: CancellationToken::new(),
            mdns: Mutex::new(None),
        })
    }

    /// Запускает HTTP/WebSocket-сервер. Возвращает фактический адрес (порт 0 → любой свободный).
    pub async fn listen(self: &Arc<Self>, host: &str, port: u16, health_checks: bool) -> std::io::Result<SocketAddr> {
        let listener = tokio::net::TcpListener::bind((host, port)).await?;
        let addr = listener.local_addr()?;
        let svc = crate::http::router(self.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, svc).with_graceful_shutdown(async move { stop.cancelled().await }).await;
        });

        let log = &self.log;
        let shown = if host == "0.0.0.0" { "localhost" } else { host };
        log.info(format!(
            "AI-Column {}: http://{shown}:{}  (колонки: ws://<ip>:{}/ws)",
            env!("CARGO_PKG_VERSION"),
            addr.port(),
            addr.port()
        ));
        log.info(format!("STT: {} | TTS: {} | история: {}", self.stt.name(), self.tts.name(), self.store.kind()));
        log.info(format!(
            "агенты: {}",
            self.registry
                .list()
                .iter()
                .map(|a| format!("{} ({}:{})", a.id(), a.cfg.provider, a.cfg.model.as_deref().unwrap_or("-")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if self.config.server.auth_token.is_empty() {
            log.warn("AUTH_TOKEN не задан — сервер открыт для всех в сети. Задай его в .env!");
        }
        if health_checks {
            self.registry.start_health_checks(Duration::from_secs(60), self.stop.clone());
        }
        if self.config.server.mdns {
            match self.publish_mdns(addr.port()) {
                Ok(d) => {
                    *self.mdns.lock().unwrap() = Some(d);
                    log.info("mDNS: объявлен сервис _aicolumn._tcp — колонка найдёт сервер сама");
                }
                Err(e) => log.warn(format!("mDNS не запустился ({e}) — укажи адрес сервера в прошивке вручную")),
            }
        }
        Ok(addr)
    }

    fn publish_mdns(&self, port: u16) -> Result<mdns_sd::ServiceDaemon, String> {
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
        let host = hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "ai-column".into());
        let instance: String = format!("AI-Column {host}").chars().take(60).collect();
        let props = [("path", "/ws"), ("v", env!("CARGO_PKG_VERSION"))];
        let info = mdns_sd::ServiceInfo::new("_aicolumn._tcp.local.", &instance, &format!("{host}.local."), "", port, &props[..])
            .map_err(|e| e.to_string())?
            .enable_addr_auto();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(daemon)
    }

    pub async fn close(&self) {
        self.stop.cancel();
        self.timers.stop();
        self.hub.close_all();
        if let Some(d) = self.mdns.lock().unwrap().take() {
            let _ = d.shutdown();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
