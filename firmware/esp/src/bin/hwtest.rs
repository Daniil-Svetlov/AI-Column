//! Проверка железа без сервера: кнопки, микрофоны, экран (цвета, ориентация) и тачскрин.
//!
//!   cargo run --release --bin hwtest
//!
//! На экране и в логе (115200) — состояние кнопок, уровни обоих микрофонов, касания.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use column_core::color::rgb565;
use column_core::config::*;
use column_core::dsp::DcBlocker;
use embedded_graphics::mono_font::iso_8859_5::{FONT_6X12, FONT_8X13};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyleBuilder};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use esp_idf_svc::hal::peripherals::Peripherals;
use log::{info, warn};

use ai_column_fw::board::{RawButton, millis};
use ai_column_fw::display::{Lcd, Screen};
use ai_column_fw::mic::Mic;

const BG: u32 = 0x0E1116;
const PANEL: u32 = 0x161B22;
const TEXT: u32 = 0xECEFF4;
const DIM: u32 = 0x8A93A3;
const OK: u32 = 0x37C871;
const BAD: u32 = 0xE5484D;
const WARN: u32 = 0xF5A524;

/// Уровни одного микрофона.
#[derive(Clone, Copy, Default)]
struct Chan {
    /// RMS, дБ относительно полной шкалы 24 бит (≈ -90 — тишина/нет сигнала, 0 — максимум)
    db: f32,
    peak_db: f32,
    /// Все сэмплы одинаковые — микрофон молчит (не подключён SD, нет питания, L/R не туда).
    dead: bool,
}

#[derive(Clone, Copy, Default)]
struct MicStats {
    l: Chan,
    r: Chan,
    blocks: u32,
    error: bool,
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

fn fill(d: &mut Lcd, x: i32, y: i32, w: u32, h: u32, c: u32) {
    let _ = d.fill_solid(&rect(x, y, w, h), rgb565(c));
}

fn text(d: &mut Lcd, s: &str, x: i32, y: i32, font: &MonoFont, fg: u32, bg: Option<u32>, align: Alignment) {
    let mut st = MonoTextStyleBuilder::new().font(font).text_color(rgb565(fg));
    if let Some(b) = bg {
        st = st.background_color(rgb565(b));
    }
    let ts = TextStyleBuilder::new().alignment(align).baseline(Baseline::Top).build();
    let _ = Text::with_text_style(&column_core::text::sanitize(s), Point::new(x, y), st.build(), ts).draw(d);
}

fn label(d: &mut Lcd, s: &str, x: i32, y: i32) {
    text(d, s, x, y, &FONT_8X13, DIM, None, Alignment::Left);
}

/// Поток микрофонов: читает I2S и считает уровни каналов.
fn mic_thread(mut mic: Mic, stats: Arc<Mutex<MicStats>>) {
    let mut buf = vec![];
    let mut dc = [DcBlocker::default(), DcBlocker::default()];
    loop {
        if let Err(e) = mic.read_block(&mut buf) {
            warn!("I2S микрофонов: {e}");
            stats.lock().unwrap().error = true;
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let frames = buf.len() / 2;
        if frames == 0 {
            continue;
        }
        let mut chans = [Chan::default(); 2];
        for (c, ch) in chans.iter_mut().enumerate() {
            let first = buf[c];
            let (mut acc, mut peak, mut same) = (0f64, 0f32, true);
            for i in 0..frames {
                let raw = buf[2 * i + c];
                same &= raw == first;
                let s = dc[c].process((raw >> 8) as f32) / 8_388_608.0; // 24 бита → -1..1
                acc += (s * s) as f64;
                peak = peak.max(s.abs());
            }
            let rms = (acc / frames as f64).sqrt() as f32;
            ch.db = 20.0 * rms.max(1e-6).log10();
            ch.peak_db = 20.0 * peak.max(1e-6).log10();
            ch.dead = same;
        }
        let mut guard = stats.lock().unwrap();
        let st = &mut *guard;
        // столбик падает плавно, растёт сразу
        for (old, new) in [(&mut st.l, chans[0]), (&mut st.r, chans[1])] {
            old.db = new.db.max(old.db - 1.5);
            old.peak_db = new.peak_db.max(old.peak_db - 0.5);
            old.dead = new.dead;
        }
        st.blocks += 1;
        st.error = false;
    }
}

fn draw_static(d: &mut Lcd) {
    let _ = d.clear(rgb565(BG));
    fill(d, 0, 0, 320, 22, PANEL);
    text(d, "AI-Column: проверка железа", 6, 4, &FONT_8X13, TEXT, None, Alignment::Left);
    label(d, "Кнопки", 6, 28);
    label(d, "Микрофоны: уровень 0..200", 6, 86);
    label(d, "Цвета: должно совпадать с подписью", 6, 150);
    let bars = [(0xFF0000, "красный", TEXT), (0x00FF00, "зелёный", 0x000000), (0x0000FF, "синий", TEXT), (0xFFFFFF, "белый", 0x000000)];
    for (i, (c, name, fg)) in bars.iter().enumerate() {
        let x = 6 + i as i32 * 78;
        fill(d, x, 166, 74, 22, *c);
        text(d, name, x + 37, 171, &FONT_6X12, *fg, None, Alignment::Center);
    }
    label(d, "Тач: коснись внизу экрана - там появится точка", 6, 194);
}

fn draw_button(d: &mut Lcd, x: i32, name: &str, down: bool, presses: u32) {
    let c = if down { OK } else { PANEL };
    fill(d, x, 44, 150, 34, c);
    let fg = if down { 0x000000 } else { TEXT };
    let state = if down { "нажата" } else { "отпущена" };
    text(d, &format!("{name}: {state}"), x + 75, 48, &FONT_8X13, fg, None, Alignment::Center);
    text(d, &format!("нажатий: {presses}"), x + 75, 63, &FONT_6X12, fg, None, Alignment::Center);
}

/// Уровень по шкале 0..200: 0 — тишина (−100 дБFS и ниже), 200 — максимум микрофона (0 дБFS).
fn level200(db: f32) -> i32 {
    ((db + 100.0) * 2.0).round().clamp(0.0, 200.0) as i32
}

/// Что сейчас нарисовано в строке микрофона — чтобы перерисовывать только изменения.
#[derive(Clone, Copy, PartialEq, Default)]
struct MicRow {
    level: i32,
    peak: i32,
    dead: bool,
    drawn: bool,
}

const BAR_X: i32 = 70;
const BAR_W: i32 = 170;

/// Рисует строку микрофона поверх старой, без очистки фоном — так экран не мигает.
fn draw_mic(d: &mut Lcd, y: i32, name: &str, ch: &Chan, row: &mut MicRow) {
    let new = MicRow { level: level200(ch.db), peak: level200(ch.peak_db), dead: ch.dead, drawn: true };
    if *row == new {
        return;
    }
    if !row.drawn {
        text(d, name, 6, y + 6, &FONT_8X13, TEXT, Some(BG), Alignment::Left);
    }
    if new.dead {
        if !row.dead || !row.drawn {
            fill(d, BAR_X, y + 2, BAR_W as u32, 20, PANEL);
            text(d, "нет сигнала", BAR_X + 6, y + 6, &FONT_8X13, BAD, Some(PANEL), Alignment::Left);
        }
    } else {
        let w = BAR_W * new.level / 200;
        let c = if new.peak >= 198 { BAD } else if new.level >= 140 { WARN } else { OK };
        if w > 0 {
            fill(d, BAR_X, y + 4, w as u32, 16, c);
        }
        if w < BAR_W {
            fill(d, BAR_X + w, y + 4, (BAR_W - w) as u32, 16, PANEL);
        }
        // полосы над и под столбиком — там живёт метка пика
        fill(d, BAR_X, y + 2, BAR_W as u32, 2, BG);
        fill(d, BAR_X, y + 20, BAR_W as u32, 2, BG);
        let px = (BAR_X + BAR_W * new.peak / 200).min(BAR_X + BAR_W - 2);
        fill(d, px, y + 2, 2, 20, TEXT);
    }
    text(d, &format!("{:>3}", if new.dead { 0 } else { new.level }), 250, y + 6, &FONT_8X13, TEXT, Some(BG), Alignment::Left);
    text(d, "/200", 274, y + 6, &FONT_8X13, DIM, Some(BG), Alignment::Left);
    *row = new;
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("AI-Column hwtest {FW_VERSION}");

    let p = Peripherals::take()?;
    let mut screen = Screen::new(p.spi2, p.ledc.timer0, p.ledc.channel0)?;
    let sw1 = RawButton::new(PIN_BTN_TALK)?;
    let sw2 = RawButton::new(PIN_BTN_MENU)?;

    let stats = Arc::new(Mutex::new(MicStats::default()));
    match Mic::new(p.i2s0) {
        Ok(mic) => {
            let st = stats.clone();
            std::thread::Builder::new().name("mic".into()).stack_size(8192).spawn(move || mic_thread(mic, st))?;
        }
        Err(e) => {
            warn!("микрофоны не запустились: {e}");
            stats.lock().unwrap().error = true;
        }
    }

    draw_static(&mut screen.lcd);
    let (mut d1, mut d2) = (!sw1.is_down(), !sw2.is_down());
    let (mut n1, mut n2) = (0u32, 0u32);
    let mut last_mic = 0;
    let (mut row_a, mut row_b) = (MicRow::default(), MicRow::default());
    let mut i2s_error_shown = false;
    let mut last_log = 0;
    let mut last_touch: Option<(i32, i32)> = None;
    let mut touch_text_dirty = false;
    let mut blocks_at_log = 0;

    loop {
        let now = millis();
        // ── кнопки ──
        let (b1, b2) = (sw1.is_down(), sw2.is_down());
        if b1 != d1 {
            if b1 {
                n1 += 1;
                info!("SW1 нажата ({n1})");
            }
            d1 = b1;
            draw_button(&mut screen.lcd, 6, "SW1", b1, n1);
        }
        if b2 != d2 {
            if b2 {
                n2 += 1;
                info!("SW2 нажата ({n2})");
            }
            d2 = b2;
            draw_button(&mut screen.lcd, 164, "SW2", b2, n2);
        }

        // ── микрофоны ──
        if now.wrapping_sub(last_mic) >= 100 {
            last_mic = now;
            let st = *stats.lock().unwrap();
            if st.error {
                if !i2s_error_shown {
                    i2s_error_shown = true;
                    fill(&mut screen.lcd, 0, 100, 320, 48, BG);
                    text(&mut screen.lcd, "Ошибка I2S микрофонов", 6, 112, &FONT_8X13, BAD, None, Alignment::Left);
                }
            } else {
                draw_mic(&mut screen.lcd, 100, "Mic A", &st.l, &mut row_a);
                draw_mic(&mut screen.lcd, 124, "Mic B", &st.r, &mut row_b);
            }
        }

        // ── тач ──
        let raw = screen.touch.raw();
        if let Some((rx, ry, z)) = raw {
            let (x, y) = screen.touch.cal.map(rx, ry);
            let p = (x.clamp(0, 319), y.clamp(0, 239));
            if last_touch != Some(p) {
                if let Some((ox, oy)) = last_touch.filter(|(_, oy)| *oy >= 210) {
                    fill(&mut screen.lcd, ox - 2, oy - 2, 5, 5, BG);
                }
                if p.1 >= 210 {
                    fill(&mut screen.lcd, p.0 - 2, p.1 - 2, 5, 5, WARN);
                }
                let line = format!("x={:3} y={:3}  сырые {rx:4} {ry:4}  z={z:<5}", p.0, p.1);
                text(&mut screen.lcd, &format!("{line:<50}"), 6, 212, &FONT_6X12, TEXT, Some(BG), Alignment::Left);
                if !touch_text_dirty {
                    text(&mut screen.lcd, "касаются          ", 6, 226, &FONT_6X12, OK, Some(BG), Alignment::Left);
                }
                last_touch = Some(p);
                touch_text_dirty = true;
            }
        } else if touch_text_dirty && last_touch.is_some() {
            touch_text_dirty = false;
            text(&mut screen.lcd, "отпустили         ", 6, 226, &FONT_6X12, DIM, Some(BG), Alignment::Left);
        }

        // ── лог раз в секунду ──
        if now.wrapping_sub(last_log) >= 1000 {
            last_log = now;
            let st = *stats.lock().unwrap();
            let fps = st.blocks - blocks_at_log;
            blocks_at_log = st.blocks;
            info!(
                "SW1={} SW2={} | Mic A {}/200 ({:.0} дБFS){} | Mic B {}/200 ({:.0} дБFS){} | блоков I2S/с: {fps} (ждём ~62) | тач: {:?}",
                if b1 { "нажата" } else { "-" },
                if b2 { "нажата" } else { "-" },
                level200(st.l.db),
                st.l.db,
                if st.l.dead { " (нет сигнала)" } else { "" },
                level200(st.r.db),
                st.r.db,
                if st.r.dead { " (нет сигнала)" } else { "" },
                raw
            );
            text(&mut screen.lcd, &format!("{:>4} с", now / 1000), 250, 4, &FONT_6X12, DIM, Some(PANEL), Alignment::Left);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
