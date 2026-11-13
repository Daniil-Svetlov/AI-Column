//! Поведение колонки (порт main.cpp): состояния, кнопки, тач, детектор речи, события сервера.
//! Железо скрыто за трейтом [`Hw`] — на плате это ESP-IDF, в тестах — заглушка.
use serde_json::{Value, json};

use crate::button::{Button, ButtonEvent};
use crate::color::parse_color;
use crate::config::*;
use crate::dsp::{Vad, VadEvent};
use crate::leds::{Leds, Mode};
use crate::ui::{AgentChip, COL_AUTO, Status, TouchEvent, Ui};

pub const COLOR_AUTO: u32 = COL_AUTO;
pub const COLOR_WARN: u32 = 0xF5A524;
pub const COLOR_ERROR: u32 = 0xE5484D;
pub const COLOR_OK: u32 = 0x37C871;
/// Сэмплов в одном аудиокадре на сервер.
pub const TX_SAMPLES: usize = (MIC_SAMPLE_RATE * MIC_FRAME_MS / 1000) as usize;

/// Всё, что поведению нужно от железа.
pub trait Hw {
    fn now_ms(&self) -> u32;
    // ── микрофон ──
    /// Начать складывать PCM16 моно 16 кГц в буфер для отправки.
    fn start_capture(&mut self);
    fn stop_capture(&mut self);
    fn read_captured(&mut self, dst: &mut [i16]) -> usize;
    fn captured_available(&self) -> usize;
    /// Текущий уровень 0..1 (RMS, сглаженный).
    fn mic_level(&self) -> f32;
    /// Уровень фона в тишине (для детектора речи).
    fn ambient_level(&self) -> f32;
    /// Растёт с каждым блоком микрофона (~16 мс).
    fn mic_frame_counter(&self) -> u32;
    /// RMS последнего блока.
    fn last_block_rms(&self) -> f32;
    // ── динамик ──
    fn playback_begin(&mut self, rate: u32);
    fn playback_write(&mut self, pcm16: &[u8]);
    /// Доиграть буфер и остановиться.
    fn playback_end(&mut self);
    /// Немедленно (перебили).
    fn playback_stop(&mut self);
    /// Играет или ждёт данных.
    fn playing(&self) -> bool;
    fn out_level(&self) -> f32;
    fn set_volume(&mut self, v: u8);
    fn volume(&self) -> u8;
    // ── сеть ──
    fn wifi_connected(&self) -> bool;
    fn rssi(&self) -> i32;
    fn server_connected(&self) -> bool;
    /// "ip:port" или пусто, пока сервер не найден.
    fn server_address(&self) -> String;
    fn send_json(&mut self, v: Value);
    fn send_audio(&mut self, pcm: &[i16]);
    // ── прочее ──
    /// -1 — АКБ нет (питание от USB).
    fn battery_percent(&self) -> i32;
    fn battery_low(&self) -> bool;
    /// Держится ниже критического порога — пора выключаться.
    fn battery_critical(&self) -> bool;
    fn save_agent(&mut self, id: &str);
    fn save_volume(&mut self, v: u8);
    fn set_backlight(&mut self, level: u8);
}

/// Событие от сетевого потока.
#[derive(Debug, Clone)]
pub enum NetEvent {
    Connected,
    Disconnected,
    Json(Value),
    Audio(Vec<u8>),
}

/// Состояние входов за один проход цикла.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    pub talk_down: bool,
    pub menu_down: bool,
    /// Точка касания на экране.
    pub touch: Option<(i32, i32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Connecting,
    Idle,
    Listening,
    Thinking,
    Speaking,
    Sleep,
}

#[derive(Clone, Debug)]
struct AgentInfo {
    id: String,
    name: String,
    color: u32,
    available: bool,
}

pub struct App {
    pub ui: Ui,
    pub leds: Leds,
    state: State,
    btn_talk: Button,
    btn_menu: Button,
    agents: Vec<AgentInfo>,
    selected: String,
    max_utterance_ms: u32,
    hold_decided: bool,
    hold_mode: bool,
    listen_started_at: u32,
    vad: Vad,
    vad_seen_frames: u32,
    tx_buf: Vec<i16>,
    accept_audio: bool,
    audio_ended: bool,
    thinking_since: u32,
    last_status_sent: u32,
    last_activity: u32,
    volume_changed_at: u32,
    volume_dirty: bool,
    backlight: u8,
    last_conn_ui: u32,
    last_indicators: Option<u32>,
    sleep: bool,
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn color_of_json(v: &Value, fallback: u32) -> u32 {
    v.as_str().and_then(parse_color).unwrap_or(fallback)
}

impl App {
    /// selected_agent и volume — сохранённые в NVS; *_down — кнопки при включении.
    pub fn new(hw: &mut dyn Hw, selected_agent: &str, volume: u8, talk_down: bool, menu_down: bool) -> App {
        let now = hw.now_ms();
        let mut app = App {
            ui: Ui::default(),
            leds: Leds::default(),
            state: State::Connecting,
            btn_talk: Button::new(60000, talk_down), // «долгое» событие не нужно: удержание ловим через held_ms()
            btn_menu: Button::new(900, menu_down),
            agents: vec![],
            selected: if selected_agent.is_empty() { "auto".into() } else { selected_agent.into() },
            max_utterance_ms: MAX_UTTERANCE_MS,
            hold_decided: false,
            hold_mode: false,
            listen_started_at: 0,
            vad: Vad::default(),
            vad_seen_frames: 0,
            tx_buf: vec![0; TX_SAMPLES],
            accept_audio: false,
            audio_ended: false,
            thinking_since: 0,
            last_status_sent: now,
            last_activity: now,
            volume_changed_at: 0,
            volume_dirty: false,
            backlight: 0,
            last_conn_ui: now,
            last_indicators: None,
            sleep: false,
        };
        app.ui.set_time(now);
        app.leds.set_mode(Mode::Boot, now);
        hw.set_volume(volume.min(100));
        app.ui.set_volume(volume.min(100));
        app.ui.set_selected_agent(&app.selected.clone());
        app.set_state(hw, State::Connecting, "Подключаюсь к Wi-Fi…");
        app
    }

    pub fn state(&self) -> State {
        self.state
    }
    pub fn selected_agent(&self) -> &str {
        &self.selected
    }
    /// АКБ разряжен: прошивка показывает экран сна и уходит в глубокий сон.
    pub fn sleep_requested(&self) -> bool {
        self.sleep
    }

    // ───────────────────────────────────────── вспомогательное

    fn color_of(&self, id: &str) -> u32 {
        if id == "auto" {
            return COLOR_AUTO;
        }
        self.agents.iter().find(|a| a.id == id).map(|a| a.color).unwrap_or(COLOR_AUTO)
    }

    fn send_type(hw: &mut dyn Hw, t: &str) {
        hw.send_json(json!({"type": t}));
    }

    fn set_state(&mut self, hw: &mut dyn Hw, st: State, hint: &str) {
        let now = hw.now_ms();
        self.state = st;
        self.last_activity = now;
        let accent = self.color_of(&self.selected.clone());
        match st {
            State::Connecting => {
                self.ui.set_status(if hw.wifi_connected() { Status::Offline } else { Status::Connecting }, hint);
                self.leds.set_mode(Mode::Offline, now);
            }
            State::Idle => {
                self.ui.set_status(Status::Idle, hint);
                self.leds.set_mode(Mode::Idle, now);
                self.leds.set_accent(accent);
            }
            State::Listening => {
                self.ui.set_status(Status::Listening, hint);
                self.leds.set_mode(Mode::Listening, now);
                self.leds.set_accent(accent);
            }
            State::Thinking => {
                self.ui.set_status(Status::Thinking, hint);
                self.leds.set_mode(Mode::Thinking, now);
            }
            State::Speaking => {
                self.ui.set_status(Status::Speaking, hint);
                self.leds.set_mode(Mode::Speaking, now);
            }
            State::Sleep => {
                self.ui.set_status(Status::Sleep, hint);
                self.leds.set_mode(Mode::Sleep, now);
            }
        }
    }

    fn push_agents_to_ui(&mut self) {
        let chips = self
            .agents
            .iter()
            .map(|a| AgentChip { id: a.id.clone(), name: a.name.clone(), color: a.color, available: a.available })
            .collect();
        self.ui.set_agents(chips);
    }

    fn select_agent(&mut self, hw: &mut dyn Hw, id: &str, notify_server: bool) {
        self.selected = id.to_string();
        self.ui.set_selected_agent(id);
        self.leds.set_accent(self.color_of(id));
        hw.save_agent(id);
        if notify_server {
            hw.send_json(json!({"type": "set_agent", "agent": id}));
        }
    }

    fn next_agent(&mut self, hw: &mut dyn Hw) {
        let mut order = vec!["auto".to_string()];
        order.extend(self.agents.iter().map(|a| a.id.clone()));
        let i = order.iter().position(|x| *x == self.selected).unwrap_or(order.len() - 1);
        let next = order[(i + 1) % order.len()].clone();
        self.select_agent(hw, &next, true);
        let name = self
            .agents
            .iter()
            .find(|a| a.id == next)
            .map(|a| format!("{}{}", a.name, if a.available { "" } else { " (недоступен)" }))
            .unwrap_or_else(|| "автовыбор".into());
        let c = self.color_of(&next);
        self.ui.toast(&format!("Отвечает: {name}"), if c == COLOR_AUTO { COLOR_WARN } else { c }, 1500);
    }

    fn set_volume(&mut self, hw: &mut dyn Hw, v: i32, notify_server: bool) {
        let v = v.clamp(0, 100) as u8;
        hw.set_volume(v);
        self.ui.set_volume(v);
        self.volume_dirty = true;
        self.volume_changed_at = hw.now_ms();
        if notify_server {
            hw.send_json(json!({"type": "set_volume", "value": v}));
        }
    }

    // ───────────────────────────────────────── слушаем

    fn start_listening(&mut self, hw: &mut dyn Hw, hold_known: bool) {
        let now = hw.now_ms();
        if !hw.server_connected() {
            self.ui.toast("Нет связи с сервером", COLOR_WARN, 2500);
            self.leds.flash_alert(COLOR_WARN, 600, now);
            return;
        }
        if self.state == State::Speaking || self.accept_audio {
            hw.playback_stop(); // перебили ответ
            self.accept_audio = false;
        }
        hw.send_json(json!({"type": "listen_start", "agent": self.selected}));
        hw.start_capture();
        self.vad.end_silence_ms = VAD_END_SILENCE_MS;
        self.vad.no_speech_ms = VAD_NO_SPEECH_MS;
        self.vad.min_speech_ms = VAD_MIN_SPEECH_MS;
        self.vad.reset(hw.ambient_level());
        self.vad_seen_frames = hw.mic_frame_counter();
        self.hold_decided = hold_known;
        self.hold_mode = false;
        self.listen_started_at = now;
        self.ui.clear_conversation();
        self.set_state(hw, State::Listening, if hold_known { "замолчи — и я отвечу" } else { "" });
    }

    fn pump_mic(&mut self, hw: &mut dyn Hw) {
        while hw.captured_available() >= TX_SAMPLES {
            let n = hw.read_captured(&mut self.tx_buf);
            hw.send_audio(&self.tx_buf[..n]);
        }
    }

    fn finish_listening(&mut self, hw: &mut dyn Hw, send_stop: bool) {
        hw.stop_capture();
        loop {
            let n = hw.read_captured(&mut self.tx_buf);
            if n == 0 {
                break;
            }
            hw.send_audio(&self.tx_buf[..n]);
        }
        if send_stop {
            Self::send_type(hw, "listen_stop");
        }
        self.thinking_since = hw.now_ms();
        let c = self.color_of(&self.selected.clone());
        self.ui.set_answering("", c, "");
        self.set_state(hw, State::Thinking, "");
    }

    fn cancel_listening(&mut self, hw: &mut dyn Hw, why: &str) {
        hw.stop_capture();
        Self::send_type(hw, "cancel");
        self.set_state(hw, State::Idle, "");
        self.ui.toast(why, COLOR_WARN, 2500);
    }

    fn tick_listening(&mut self, hw: &mut dyn Hw) {
        self.pump_mic(hw);
        let now = hw.now_ms();
        if !self.hold_decided && self.btn_talk.is_down() && self.btn_talk.held_ms(now) >= HOLD_THRESHOLD_MS {
            self.hold_decided = true;
            self.hold_mode = true;
            self.ui.set_status(Status::Listening, "отпусти кнопку, когда закончишь");
        }
        // Детектор речи: по новым блокам микрофона
        let frames = hw.mic_frame_counter();
        if frames != self.vad_seen_frames {
            let ms = frames.wrapping_sub(self.vad_seen_frames) * MIC_BLOCK_MS;
            self.vad_seen_frames = frames;
            let ev = self.vad.process(hw.last_block_rms(), ms);
            if !self.hold_mode {
                match ev {
                    VadEvent::SpeechEnd => return self.finish_listening(hw, true),
                    VadEvent::NoSpeech => return self.cancel_listening(hw, "Не слышу тебя"),
                    _ => {}
                }
            }
        }
        if now.wrapping_sub(self.listen_started_at) > self.max_utterance_ms {
            self.finish_listening(hw, true);
        }
    }

    // ───────────────────────────────────────── сервер

    pub fn on_net(&mut self, hw: &mut dyn Hw, ev: NetEvent) {
        match ev {
            NetEvent::Connected => self.on_connected(hw),
            NetEvent::Disconnected => self.on_disconnected(hw),
            NetEvent::Json(v) => self.on_message(hw, &v),
            NetEvent::Audio(b) => {
                if self.accept_audio {
                    hw.playback_write(&b);
                }
            }
        }
    }

    fn on_connected(&mut self, hw: &mut dyn Hw) {
        let mut hello = json!({
            "type": "hello",
            "fw": FW_VERSION,
            "mic_rate": MIC_SAMPLE_RATE,
            "play_rate": PLAY_SAMPLE_RATE,
            "volume": hw.volume(),
        });
        let bat = hw.battery_percent();
        if bat >= 0 {
            hello["battery"] = json!(bat);
        }
        hw.send_json(hello);
        self.ui.set_server(true);
    }

    fn on_disconnected(&mut self, hw: &mut dyn Hw) {
        self.ui.set_server(false);
        if self.state == State::Listening {
            hw.stop_capture();
        }
        self.accept_audio = false;
        let hint = format!("Сервер {} недоступен", hw.server_address());
        self.set_state(hw, State::Connecting, &hint);
    }

    fn on_message(&mut self, hw: &mut dyn Hw, doc: &Value) {
        let now = hw.now_ms();
        match s(&doc["type"]) {
            t @ ("welcome" | "agents") => {
                self.agents = doc["agents"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|a| AgentInfo {
                                id: s(&a["id"]).into(),
                                name: s(&a["name"]).into(),
                                color: color_of_json(&a["color"], COLOR_AUTO),
                                available: a["available"].as_bool().unwrap_or(true),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.push_agents_to_ui();
                if t == "welcome" {
                    let server_agent = doc["agent"].as_str().unwrap_or("auto").to_string();
                    // Выбор с колонки главнее: если он есть в списке — сообщим серверу.
                    let known = self.selected == "auto" || self.agents.iter().any(|a| a.id == self.selected);
                    if known && self.selected != server_agent {
                        let sel = self.selected.clone();
                        self.select_agent(hw, &sel, true);
                    } else {
                        self.select_agent(hw, &server_agent, false);
                    }
                    if let Some(v) = doc["volume"].as_i64() {
                        self.set_volume(hw, v as i32, false);
                    }
                    self.max_utterance_ms =
                        doc["max_utterance_seconds"].as_u64().map(|v| v as u32 * 1000).unwrap_or(MAX_UTTERANCE_MS);
                    let name = doc["assistant"].as_str().unwrap_or("AI-Column");
                    self.set_state(hw, State::Idle, &format!("Я {name}. SW1 — говорить, SW2 — сменить агента"));
                }
            }
            "agent_selected" => {
                let a = doc["agent"].as_str().unwrap_or("auto").to_string();
                self.select_agent(hw, &a, false);
            }
            "state" => {
                if s(&doc["state"]) == "idle" && self.state == State::Thinking {
                    self.set_state(hw, State::Idle, "");
                }
            }
            "stop_listening" => {
                if self.state == State::Listening {
                    self.finish_listening(hw, false);
                }
            }
            "transcript" => {
                let t = s(&doc["text"]);
                self.ui.set_user_text(if t.is_empty() { "…" } else { t });
            }
            "agent" => {
                let c = color_of_json(&doc["color"], COLOR_AUTO);
                let note = if s(&doc["reason"]) == "fallback" { "запасной агент" } else { "" };
                self.ui.set_answering(s(&doc["name"]), c, note);
                self.leds.set_accent(c);
            }
            "delegate" => {
                let name = doc["name"].as_str().unwrap_or("другого агента");
                self.ui.toast(&format!("Спрашиваю {name}…"), color_of_json(&doc["color"], COLOR_AUTO), 2500);
            }
            "reply_delta" => self.ui.append_reply(s(&doc["text"])),
            "audio_start" => {
                hw.playback_begin(doc["rate"].as_u64().map(|r| r as u32).unwrap_or(PLAY_SAMPLE_RATE));
                self.accept_audio = true;
                self.audio_ended = false;
                if self.state != State::Listening {
                    self.set_state(hw, State::Speaking, "");
                }
            }
            "audio_end" => {
                self.accept_audio = false;
                self.audio_ended = true;
                hw.playback_end();
            }
            "set_volume" => {
                let v = doc["value"].as_f64().map(|v| v.round() as i32).unwrap_or(hw.volume() as i32);
                self.set_volume(hw, v, false);
                self.ui.toast(&format!("Громкость {}%", hw.volume()), COLOR_OK, 1200);
            }
            "led" => {
                let c = doc["color"].as_str().and_then(parse_color);
                let b = doc["brightness"].as_u64().map(|b| b.min(100) as u8);
                self.leds.set_override(doc["mode"].as_str().unwrap_or("auto"), c, b);
            }
            "alert" => {
                self.leds.flash_alert(0xFF7000, 3000, now);
                self.ui.toast(doc["text"].as_str().unwrap_or("Напоминание"), COLOR_WARN, 6000);
            }
            "error" => {
                self.ui.toast(doc["message"].as_str().unwrap_or("Ошибка"), COLOR_ERROR, 3500);
                self.leds.flash_alert(COLOR_ERROR, 500, now);
            }
            _ => {}
        }
    }

    fn send_status(&mut self, hw: &mut dyn Hw) {
        let mut d = json!({"type": "status", "rssi": hw.rssi(), "volume": hw.volume()});
        let bat = hw.battery_percent();
        if bat >= 0 {
            d["battery"] = json!(bat);
        }
        hw.send_json(d);
    }

    // ───────────────────────────────────────── главный цикл

    /// Один проход главного цикла (вызывать каждые ~1–5 мс).
    pub fn tick(&mut self, hw: &mut dyn Hw, input: Inputs) {
        let now = hw.now_ms();
        self.ui.set_time(now);
        if self.sleep {
            return;
        }

        // ── кнопки ──
        match self.btn_talk.poll(input.talk_down, now) {
            ButtonEvent::Pressed => match self.state {
                State::Listening => {
                    if self.hold_decided && !self.hold_mode {
                        self.finish_listening(hw, true); // второе нажатие в режиме «нажал-отпустил»
                    }
                }
                State::Idle | State::Speaking | State::Thinking => self.start_listening(hw, false),
                _ => self.ui.toast("Нет связи с сервером", COLOR_WARN, 2500),
            },
            ButtonEvent::Released if self.state == State::Listening => {
                if !self.hold_decided {
                    self.hold_decided = true; // короткое нажатие — дальше сам определю конец фразы
                    self.hold_mode = false;
                    self.ui.set_status(Status::Listening, "говори — я пойму, когда закончишь");
                } else if self.hold_mode {
                    self.finish_listening(hw, true);
                }
            }
            _ => {}
        }

        match self.btn_menu.poll(input.menu_down, now) {
            ButtonEvent::Released if self.state != State::Connecting => self.next_agent(hw),
            ButtonEvent::LongPress if hw.server_connected() => {
                Self::send_type(hw, "new_conversation");
                self.ui.clear_conversation();
                self.ui.toast("Новый разговор", COLOR_OK, 1500);
            }
            _ => {}
        }

        // ── тачскрин ──
        let t = self.ui.touch(input.touch, now);
        if t != TouchEvent::None {
            self.last_activity = now;
        }
        match t {
            TouchEvent::Talk => match self.state {
                State::Listening => self.finish_listening(hw, true),
                State::Idle | State::Speaking | State::Thinking => self.start_listening(hw, true),
                _ => {}
            },
            TouchEvent::SelectAgent(id) => self.select_agent(hw, &id, true),
            TouchEvent::VolumeUp => {
                let v = hw.volume() as i32 + 10;
                self.set_volume(hw, v, true);
            }
            TouchEvent::VolumeDown => {
                let v = hw.volume() as i32 - 10;
                self.set_volume(hw, v, true);
            }
            TouchEvent::None => {}
        }

        // ── состояния ──
        match self.state {
            State::Connecting => {
                if now.wrapping_sub(self.last_conn_ui) > 500 {
                    self.last_conn_ui = now;
                    let (st, hint) = if hw.wifi_connected() {
                        let addr = hw.server_address();
                        (
                            Status::Offline,
                            if addr.is_empty() { "Ищу сервер в сети…".to_string() } else { format!("Сервер {addr} не отвечает") },
                        )
                    } else {
                        (Status::Connecting, "Подключаюсь к Wi-Fi…".to_string())
                    };
                    self.ui.set_status(st, &hint);
                }
            }
            State::Listening => self.tick_listening(hw),
            State::Thinking => {
                if now.wrapping_sub(self.thinking_since) > 60000 {
                    self.ui.toast("Сервер не ответил", COLOR_ERROR, 2500);
                    Self::send_type(hw, "cancel");
                    self.set_state(hw, State::Idle, "");
                }
            }
            State::Speaking => {
                if self.audio_ended && !hw.playing() {
                    self.audio_ended = false;
                    self.set_state(hw, State::Idle, "");
                }
            }
            State::Idle | State::Sleep => {}
        }

        // ── батарея, индикаторы, подсветка ──
        if self.last_indicators.is_none_or(|t| now.wrapping_sub(t) > 1000) {
            self.last_indicators = Some(now);
            self.ui.set_battery(hw.battery_percent(), hw.battery_low());
            self.ui.set_wifi(hw.wifi_connected(), hw.rssi());
        }
        self.ui.set_server(hw.server_connected());
        self.ui.set_levels(hw.mic_level(), hw.out_level());
        if hw.battery_critical() {
            self.set_state(hw, State::Sleep, "");
            hw.playback_stop();
            self.sleep = true;
            return;
        }

        let want_light = if self.state == State::Idle && now.wrapping_sub(self.last_activity) > 60000 { 40 } else { 200 };
        if want_light != self.backlight {
            self.backlight = want_light;
            hw.set_backlight(want_light);
        }
        if hw.server_connected() && now.wrapping_sub(self.last_status_sent) > STATUS_INTERVAL_MS {
            self.last_status_sent = now;
            self.send_status(hw);
        }
        if self.volume_dirty && now.wrapping_sub(self.volume_changed_at) > 3000 {
            self.volume_dirty = false;
            let v = hw.volume();
            hw.save_volume(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Mock {
        now: u32,
        wifi: bool,
        server: bool,
        sent: Vec<Value>,
        audio_sent: usize,
        captured: usize,
        capturing: bool,
        frames: u32,
        rms: f32,
        playing: bool,
        play_rate: Option<u32>,
        played: usize,
        ended: bool,
        stopped: u32,
        volume: u8,
        saved_agent: Option<String>,
        saved_volume: Option<u8>,
        critical: bool,
        backlight: u8,
    }

    impl Mock {
        fn types(&self) -> Vec<&str> {
            self.sent.iter().map(|v| v["type"].as_str().unwrap()).collect()
        }
        fn last(&self, t: &str) -> Option<&Value> {
            self.sent.iter().rev().find(|v| v["type"] == t)
        }
    }

    impl Hw for Mock {
        fn now_ms(&self) -> u32 {
            self.now
        }
        fn start_capture(&mut self) {
            self.capturing = true;
            self.captured = 0;
        }
        fn stop_capture(&mut self) {
            self.capturing = false;
        }
        fn read_captured(&mut self, dst: &mut [i16]) -> usize {
            let n = self.captured.min(dst.len());
            self.captured -= n;
            n
        }
        fn captured_available(&self) -> usize {
            self.captured
        }
        fn mic_level(&self) -> f32 {
            self.rms
        }
        fn ambient_level(&self) -> f32 {
            0.002
        }
        fn mic_frame_counter(&self) -> u32 {
            self.frames
        }
        fn last_block_rms(&self) -> f32 {
            self.rms
        }
        fn playback_begin(&mut self, rate: u32) {
            self.play_rate = Some(rate);
            self.playing = true;
            self.ended = false;
        }
        fn playback_write(&mut self, pcm16: &[u8]) {
            self.played += pcm16.len();
        }
        fn playback_end(&mut self) {
            self.ended = true;
        }
        fn playback_stop(&mut self) {
            self.stopped += 1;
            self.playing = false;
        }
        fn playing(&self) -> bool {
            self.playing
        }
        fn out_level(&self) -> f32 {
            0.0
        }
        fn set_volume(&mut self, v: u8) {
            self.volume = v;
        }
        fn volume(&self) -> u8 {
            self.volume
        }
        fn wifi_connected(&self) -> bool {
            self.wifi
        }
        fn rssi(&self) -> i32 {
            -60
        }
        fn server_connected(&self) -> bool {
            self.server
        }
        fn server_address(&self) -> String {
            "192.168.1.5:8080".into()
        }
        fn send_json(&mut self, v: Value) {
            self.sent.push(v);
        }
        fn send_audio(&mut self, pcm: &[i16]) {
            self.audio_sent += pcm.len();
        }
        fn battery_percent(&self) -> i32 {
            80
        }
        fn battery_low(&self) -> bool {
            false
        }
        fn battery_critical(&self) -> bool {
            self.critical
        }
        fn save_agent(&mut self, id: &str) {
            self.saved_agent = Some(id.into());
        }
        fn save_volume(&mut self, v: u8) {
            self.saved_volume = Some(v);
        }
        fn set_backlight(&mut self, level: u8) {
            self.backlight = level;
        }
    }

    fn welcome() -> Value {
        json!({"type": "welcome", "protocol": 1, "assistant": "Колонка", "agent": "auto", "volume": 55,
            "max_utterance_seconds": 20,
            "agents": [
                {"id": "claude", "name": "Claude", "color": "#FF7A1A", "available": true},
                {"id": "qwen", "name": "Qwen", "color": "#8B5CF6", "available": false}
            ]})
    }

    /// Колонка подключена и получила welcome.
    fn connected(saved_agent: &str) -> (Mock, App) {
        let mut hw = Mock { wifi: true, server: true, volume: 60, now: 1000, ..Default::default() };
        let mut app = App::new(&mut hw, saved_agent, 60, false, false);
        app.on_net(&mut hw, NetEvent::Connected);
        app.on_net(&mut hw, NetEvent::Json(welcome()));
        (hw, app)
    }

    fn run(app: &mut App, hw: &mut Mock, ms: u32, input: Inputs) {
        for _ in 0..ms / 5 {
            hw.now += 5;
            app.tick(hw, input);
        }
    }

    fn speak(app: &mut App, hw: &mut Mock, rms: f32, ms: u32) {
        for _ in 0..ms / 16 {
            hw.now += 16;
            hw.frames += 1;
            hw.rms = rms;
            if hw.capturing {
                hw.captured += 256;
            }
            app.tick(hw, Inputs::default());
        }
    }

    #[test]
    fn hello_and_welcome() {
        let (hw, app) = connected("qwen");
        let hello = &hw.sent[0];
        assert_eq!(hello["type"], "hello");
        assert_eq!((hello["mic_rate"].as_u64(), hello["play_rate"].as_u64()), (Some(16000), Some(24000)));
        assert_eq!(hello["battery"], 80);
        // сохранённый на колонке агент есть у сервера — колонка сообщает свой выбор
        assert_eq!(hw.last("set_agent").unwrap()["agent"], "qwen");
        assert_eq!(app.state(), State::Idle);
        assert_eq!(hw.volume, 55);
        assert_eq!(app.ui.agents().len(), 2);
        assert!(app.ui.hint().contains("Колонка"));
    }

    #[test]
    fn unknown_saved_agent_takes_server_choice() {
        let (hw, app) = connected("gone");
        assert!(hw.last("set_agent").is_none());
        assert_eq!(app.selected_agent(), "auto");
        assert_eq!(hw.saved_agent.as_deref(), Some("auto"));
    }

    #[test]
    fn tap_then_vad_ends_phrase() {
        let (mut hw, mut app) = connected("auto");
        let down = Inputs { talk_down: true, ..Default::default() };
        run(&mut app, &mut hw, 100, down); // короткое нажатие
        assert_eq!(app.state(), State::Listening);
        assert_eq!(hw.last("listen_start").unwrap()["agent"], "auto");
        run(&mut app, &mut hw, 60, Inputs::default());
        assert!(app.ui.hint().contains("пойму"));
        speak(&mut app, &mut hw, 0.05, 800); // речь
        assert_eq!(app.state(), State::Listening);
        speak(&mut app, &mut hw, 0.002, 1200); // тишина
        assert_eq!(app.state(), State::Thinking);
        assert_eq!(hw.types().last(), Some(&"listen_stop"));
        assert!(hw.audio_sent > 16000, "звук ушёл на сервер: {}", hw.audio_sent);

        // ответ сервера
        for m in [
            json!({"type": "transcript", "text": "Который час?"}),
            json!({"type": "agent", "id": "claude", "name": "Claude", "color": "#FF7A1A", "reason": "fallback"}),
            json!({"type": "reply_delta", "text": "Полдень. "}),
            json!({"type": "audio_start", "rate": 24000, "format": "pcm16", "turn": 1}),
        ] {
            app.on_net(&mut hw, NetEvent::Json(m));
        }
        assert_eq!(app.state(), State::Speaking);
        assert_eq!(app.ui.answering(), ("Claude", 0xFF7A1A, "запасной агент"));
        app.on_net(&mut hw, NetEvent::Audio(vec![0; 4096]));
        app.on_net(&mut hw, NetEvent::Json(json!({"type": "audio_end", "seconds": 0.1})));
        app.on_net(&mut hw, NetEvent::Audio(vec![0; 4096])); // после audio_end — не наше
        assert_eq!(hw.played, 4096);
        assert!(hw.ended);
        run(&mut app, &mut hw, 20, Inputs::default());
        assert_eq!(app.state(), State::Speaking, "пока доигрывает буфер");
        hw.playing = false;
        run(&mut app, &mut hw, 20, Inputs::default());
        assert_eq!(app.state(), State::Idle);
        assert_eq!(app.ui.reply_text(), "Полдень. ");
    }

    #[test]
    fn hold_mode_ends_on_release_even_with_silence() {
        let (mut hw, mut app) = connected("auto");
        let down = Inputs { talk_down: true, ..Default::default() };
        run(&mut app, &mut hw, 600, down);
        assert!(app.ui.hint().contains("отпусти"));
        // тишина в режиме удержания не обрывает запись
        for _ in 0..400 {
            hw.now += 16;
            hw.frames += 1;
            hw.rms = 0.002;
            app.tick(&mut hw, down);
        }
        assert_eq!(app.state(), State::Listening);
        run(&mut app, &mut hw, 60, Inputs::default());
        assert_eq!(app.state(), State::Thinking);
        assert_eq!(hw.types().last(), Some(&"listen_stop"));
    }

    #[test]
    fn no_speech_cancels() {
        let (mut hw, mut app) = connected("auto");
        run(&mut app, &mut hw, 60, Inputs { talk_down: true, ..Default::default() });
        run(&mut app, &mut hw, 40, Inputs::default());
        speak(&mut app, &mut hw, 0.0021, 5200);
        assert_eq!(app.state(), State::Idle);
        assert_eq!(hw.types().last(), Some(&"cancel"));
        assert_eq!(app.ui.toast_text(), Some("Не слышу тебя"));
    }

    #[test]
    fn talking_interrupts_answer() {
        let (mut hw, mut app) = connected("auto");
        app.on_net(&mut hw, NetEvent::Json(json!({"type": "audio_start", "rate": 24000})));
        assert_eq!(app.state(), State::Speaking);
        run(&mut app, &mut hw, 60, Inputs { talk_down: true, ..Default::default() });
        assert_eq!(app.state(), State::Listening);
        assert_eq!(hw.stopped, 1);
        app.on_net(&mut hw, NetEvent::Audio(vec![0; 100]));
        assert_eq!(hw.played, 0, "хвост старого ответа не играем");
    }

    #[test]
    fn menu_button_cycles_agents_and_long_press_resets() {
        let (mut hw, mut app) = connected("auto");
        let press = |app: &mut App, hw: &mut Mock| {
            run(app, hw, 60, Inputs { menu_down: true, ..Default::default() });
            run(app, hw, 60, Inputs::default());
        };
        press(&mut app, &mut hw);
        assert_eq!(app.selected_agent(), "claude");
        press(&mut app, &mut hw);
        assert_eq!(app.selected_agent(), "qwen");
        assert_eq!(app.ui.toast_text(), Some("Отвечает: Qwen (недоступен)"));
        press(&mut app, &mut hw);
        assert_eq!(app.selected_agent(), "auto");
        assert_eq!(hw.sent.iter().filter(|v| v["type"] == "set_agent").count(), 3);

        run(&mut app, &mut hw, 1000, Inputs { menu_down: true, ..Default::default() });
        run(&mut app, &mut hw, 60, Inputs::default());
        assert_eq!(hw.types().last(), Some(&"new_conversation"));
        assert_eq!(app.selected_agent(), "auto", "долгое нажатие не листает агентов");
    }

    #[test]
    fn server_commands_volume_led_alert() {
        let (mut hw, mut app) = connected("auto");
        app.on_net(&mut hw, NetEvent::Json(json!({"type": "set_volume", "value": 35})));
        assert_eq!(hw.volume, 35);
        assert_eq!(app.ui.toast_text(), Some("Громкость 35%"));
        run(&mut app, &mut hw, 3100, Inputs::default());
        assert_eq!(hw.saved_volume, Some(35), "громкость сохраняется в NVS с задержкой");
        app.on_net(&mut hw, NetEvent::Json(json!({"type": "agent_selected", "agent": "claude"})));
        assert_eq!(app.selected_agent(), "claude");
        app.on_net(&mut hw, NetEvent::Json(json!({"type": "led", "mode": "solid", "color": "#00FF00", "brightness": 50})));
        app.on_net(&mut hw, NetEvent::Json(json!({"type": "alert", "kind": "timer", "text": "Время вышло"})));
        assert_eq!(app.ui.toast_text(), Some("Время вышло"));
    }

    #[test]
    fn disconnect_and_status_and_sleep() {
        let (mut hw, mut app) = connected("auto");
        run(&mut app, &mut hw, 61000, Inputs::default());
        assert!(hw.last("status").is_some_and(|s| s["rssi"] == -60 && s["battery"] == 80));
        assert_eq!(hw.backlight, 40, "в простое экран приглушается");

        hw.server = false;
        app.on_net(&mut hw, NetEvent::Disconnected);
        assert_eq!(app.state(), State::Connecting);
        assert!(app.ui.hint().contains("192.168.1.5:8080"));
        run(&mut app, &mut hw, 60, Inputs { talk_down: true, ..Default::default() });
        assert_eq!(app.ui.toast_text(), Some("Нет связи с сервером"));

        hw.critical = true;
        run(&mut app, &mut hw, 10, Inputs::default());
        assert!(app.sleep_requested());
        assert_eq!(app.state(), State::Sleep);
    }

    #[test]
    fn thinking_timeout() {
        let (mut hw, mut app) = connected("auto");
        run(&mut app, &mut hw, 60, Inputs { talk_down: true, ..Default::default() });
        run(&mut app, &mut hw, 60, Inputs::default());
        // тап по экрану во время прослушивания — закончить фразу
        hw.now += 20;
        app.tick(&mut hw, Inputs { touch: Some((160, 120)), ..Default::default() });
        hw.now += 20;
        app.tick(&mut hw, Inputs::default());
        assert_eq!(app.state(), State::Thinking);
        run(&mut app, &mut hw, 61000, Inputs::default());
        assert_eq!(app.state(), State::Idle);
        assert_eq!(hw.types().last(), Some(&"cancel"));
    }
}
