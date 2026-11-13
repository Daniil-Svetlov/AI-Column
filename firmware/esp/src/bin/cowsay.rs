//! Корова на экране, как команда `cowsay`.
//!
//!   cargo run --release --bin cowsay
use std::time::Duration;

use column_core::color::rgb565;
use column_core::text::wrap;
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::iso_8859_5::FONT_8X13;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use esp_idf_svc::hal::peripherals::Peripherals;

use ai_column_fw::display::Screen;

const MESSAGE: &str = "Ну вот он меня прошил тварь ебаная.";
/// Ширина текста в облачке (символов): 320 px / 8 px − рамка.
const WIDTH: usize = 34;

const COW: &str = r"        \   ^__^
         \  (oo)\_______
            (__)\       )\/\
                ||----w |
                ||     ||";

/// Строки как у настоящего cowsay: облачко с рамкой, под ним корова.
fn cowsay(msg: &str) -> Vec<String> {
    let lines = wrap(msg, WIDTH);
    let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out = vec![format!(" {}", "_".repeat(w + 2))];
    let n = lines.len();
    for (i, l) in lines.iter().enumerate() {
        let (a, b) = match (n, i) {
            (1, _) => ('<', '>'),
            (_, 0) => ('/', '\\'),
            (_, i) if i == n - 1 => ('\\', '/'),
            _ => ('|', '|'),
        };
        let pad = w - l.chars().count();
        out.push(format!("{a} {l}{} {b}", " ".repeat(pad)));
    }
    out.push(format!(" {}", "-".repeat(w + 2)));
    out.extend(COW.lines().map(str::to_string));
    out
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let lines = cowsay(MESSAGE);
    for l in &lines {
        println!("{l}");
    }

    let p = Peripherals::take()?;
    let mut screen = Screen::new(p.spi2, p.ledc.timer0, p.ledc.channel0)?;
    let _ = screen.lcd.clear(rgb565(0x000000));

    let style = MonoTextStyleBuilder::new().font(&FONT_8X13).text_color(rgb565(0xECEFF4)).build();
    const STEP: i32 = 16;
    let top = (240 - lines.len() as i32 * STEP) / 2;
    let left = (320 - lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as i32 * 8) / 2;
    for (i, l) in lines.iter().enumerate() {
        let _ = Text::with_baseline(l, Point::new(left.max(0), top + i as i32 * STEP), style, Baseline::Top).draw(&mut screen.lcd);
    }

    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
