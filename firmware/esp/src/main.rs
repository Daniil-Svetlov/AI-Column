//! AI-Column — прошивка умной колонки на ESP32-S3.
//! Колонка слушает (кнопка/тач), стримит звук на сервер, показывает ответ на экране
//! и проигрывает речь, которую сервер присылает потоком. Вся «умность» — на сервере.
//! Поведение — в column_core::app (тестируется на ПК), здесь — железо.
use std::time::Duration;

use column_core::app::{App, Hw, Inputs};
use column_core::config::*;
use column_core::touch::{CAL_POINTS, TouchCal};
use column_core::ui::{self, Canvas, MID_H, W};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{info, warn};
use serde_json::Value;

use ai_column_fw::audio::Audio;
use ai_column_fw::battery::Battery;
use ai_column_fw::board::{RawButton, millis};
use ai_column_fw::display::{Backlight, Screen};
use ai_column_fw::leds::LedStrip;
use ai_column_fw::net::{Net, NetOut};
use ai_column_fw::prefs::Prefs;
use ai_column_fw::touch::Xpt2046;

/// Железо для поведения колонки.
struct Board {
    audio: Audio,
    net: Net,
    battery: Option<Battery>,
    prefs: Prefs,
    backlight: Backlight,
}

impl Hw for Board {
    fn now_ms(&self) -> u32 {
        millis()
    }
    fn start_capture(&mut self) {
        self.audio.start_capture()
    }
    fn stop_capture(&mut self) {
        self.audio.stop_capture()
    }
    fn read_captured(&mut self, dst: &mut [i16]) -> usize {
        self.audio.read_captured(dst)
    }
    fn captured_available(&self) -> usize {
        self.audio.captured_available()
    }
    fn mic_level(&self) -> f32 {
        self.audio.mic_level()
    }
    fn ambient_level(&self) -> f32 {
        self.audio.ambient_level()
    }
    fn mic_frame_counter(&self) -> u32 {
        self.audio.mic_frame_counter()
    }
    fn last_block_rms(&self) -> f32 {
        self.audio.last_block_rms()
    }
    fn playback_begin(&mut self, rate: u32) {
        self.audio.playback_begin(rate)
    }
    fn playback_write(&mut self, pcm16: &[u8]) {
        self.audio.playback_write(pcm16)
    }
    fn playback_end(&mut self) {
        self.audio.playback_end()
    }
    fn playback_stop(&mut self) {
        self.audio.playback_stop()
    }
    fn playing(&self) -> bool {
        self.audio.playing()
    }
    fn out_level(&self) -> f32 {
        self.audio.out_level()
    }
    fn set_volume(&mut self, v: u8) {
        self.audio.set_volume(v)
    }
    fn volume(&self) -> u8 {
        self.audio.volume()
    }
    fn wifi_connected(&self) -> bool {
        self.net.state.wifi.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn rssi(&self) -> i32 {
        self.net.state.rssi.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn server_connected(&self) -> bool {
        self.net.state.server.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn server_address(&self) -> String {
        self.net.state.address.lock().unwrap().clone()
    }
    fn send_json(&mut self, v: Value) {
        if self.server_connected() {
            let _ = self.net.tx.send(NetOut::Json(v.to_string()));
        }
    }
    fn send_audio(&mut self, pcm: &[i16]) {
        if self.server_connected() {
            let _ = self.net.tx.send(NetOut::Audio(pcm.iter().flat_map(|s| s.to_le_bytes()).collect()));
        }
    }
    fn battery_percent(&self) -> i32 {
        self.battery.as_ref().map(|b| b.percent()).unwrap_or(-1)
    }
    fn battery_low(&self) -> bool {
        self.battery.as_ref().is_some_and(|b| b.low())
    }
    fn battery_critical(&self) -> bool {
        self.battery.as_ref().is_some_and(|b| b.critical(millis()))
    }
    fn save_agent(&mut self, id: &str) {
        self.prefs.set_agent(id)
    }
    fn save_volume(&mut self, v: u8) {
        self.prefs.set_volume(v)
    }
    fn set_backlight(&mut self, level: u8) {
        self.backlight.set(level)
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Ждёт касания и отпускания; возвращает медиану сырых координат. None — таймаут или нажали SW1.
fn wait_tap(touch: &mut Xpt2046, skip: &RawButton, timeout_ms: u32) -> Option<(u16, u16)> {
    let t0 = millis();
    let mut xs = vec![];
    let mut ys = vec![];
    while millis().wrapping_sub(t0) < timeout_ms {
        if skip.is_down() {
            return None;
        }
        match touch.raw() {
            Some((x, y, _)) => {
                xs.push(x);
                ys.push(y);
            }
            None if xs.len() >= 3 => {
                return Some((column_core::touch::median(&mut xs), column_core::touch::median(&mut ys)));
            }
            None => {
                xs.clear();
                ys.clear();
            }
        }
        sleep_ms(15);
    }
    None
}

/// Калибровка тача по трём точкам.
fn calibrate(screen: &mut Screen, prefs: &Prefs, skip: &RawButton) {
    let mut raw = [(0u16, 0u16); 3];
    for (i, &(x, y)) in CAL_POINTS.iter().enumerate() {
        let _ = ui::calibration_target(&mut screen.lcd, x, y, i + 1, CAL_POINTS.len());
        match wait_tap(&mut screen.touch, skip, 30000) {
            Some(r) => raw[i] = r,
            None => return,
        }
        sleep_ms(300);
    }
    match TouchCal::from_points(CAL_POINTS, raw) {
        Some(cal) => {
            info!("тач откалиброван: {:?}", cal.a);
            screen.touch.cal = cal;
            prefs.set_touch_cal(&cal);
        }
        None => warn!("калибровка не удалась (точки на одной прямой)"),
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let p = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let prefs = Prefs::open(nvs.clone());
    let volume = prefs.volume(VOLUME_DEFAULT);
    let agent = prefs.agent();

    let sw1 = RawButton::new(PIN_BTN_TALK)?;
    let sw2 = RawButton::new(PIN_BTN_MENU)?;
    let recalibrate = sw2.is_down(); // SW2 зажата при включении — калибровка тача

    let mut leds = LedStrip::new().map_err(|e| warn!("светодиоды: {e}")).ok();
    let Screen { lcd, touch, backlight } = Screen::new(p.spi2, p.ledc.timer0, p.ledc.channel0)?;
    let mut screen = Screen { lcd, touch, backlight };
    let _ = ui::boot_screen(&mut screen.lcd);

    // ── тачскрин ──
    match prefs.touch_cal() {
        Some(cal) if !recalibrate => screen.touch.cal = cal,
        _ => {
            let offer = recalibrate || {
                // первый запуск: предлагаем, но не блокируемся, если тач не подключён
                let _ = ui::calibration_offer(&mut screen.lcd);
                wait_tap(&mut screen.touch, &sw1, 10000).is_some()
            };
            if offer {
                calibrate(&mut screen, &prefs, &sw1);
            }
            let _ = ui::boot_screen(&mut screen.lcd);
        }
    }

    let _ = ui::boot_message(&mut screen.lcd, "Запускаю звук…");
    let audio = Audio::start(p.i2s0, p.i2s1, volume);
    if !audio.ok {
        let _ = ui::boot_message(&mut screen.lcd, "Ошибка I2S — проверь пайку");
        sleep_ms(1500);
    }
    let battery = Battery::new(p.adc1, p.pins.gpio1).map_err(|e| warn!("АЦП батареи: {e}")).ok();

    let _ = ui::boot_message(&mut screen.lcd, "Подключаюсь к Wi-Fi…");
    let net = ai_column_fw::net::start(p.modem, sysloop, nvs)?;
    info!("AI-Column {FW_VERSION}, id {}", net.device_id);

    let Screen { mut lcd, mut touch, backlight } = screen;
    let mut hw = Board { audio, net, battery, prefs, backlight };
    let mut app = App::new(&mut hw, &agent, volume, sw1.is_down(), sw2.is_down());
    app.ui.invalidate();
    let mut canvas = Canvas::new(W as u32, MID_H as u32);
    let mut touch_point = None;
    let mut last_touch_poll = 0;

    loop {
        let now = millis();
        while let Ok(ev) = hw.net.rx.try_recv() {
            app.on_net(&mut hw, ev);
        }
        if let Some(b) = hw.battery.as_mut() {
            b.update(now);
        }
        if now.wrapping_sub(last_touch_poll) >= 16 {
            last_touch_poll = now;
            touch_point = touch.point();
        }
        app.tick(&mut hw, Inputs { talk_down: sw1.is_down(), menu_down: sw2.is_down(), touch: touch_point });

        if app.sleep_requested() {
            warn!("аккумулятор разряжен — засыпаю");
            let _ = ui::sleep_screen(&mut lcd);
            if let Some(l) = leds.as_mut() {
                l.off();
            }
            sleep_ms(2000);
            hw.backlight.set(0);
            // проснётся только по сбросу/включению — выключи питание и заряди АКБ
            unsafe { esp_idf_svc::sys::esp_deep_sleep_start() };
        }

        let _ = app.ui.render(&mut lcd, &mut canvas, now);
        if let Some(frame) = app.leds.render(now, hw.audio.mic_level(), hw.audio.out_level()) {
            if let Some(l) = leds.as_mut() {
                let _ = l.show(&frame);
            }
        }
        sleep_ms(2);
    }
}
