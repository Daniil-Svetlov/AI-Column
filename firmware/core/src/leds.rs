//! Подсветка: эффекты матрицы WS2812B 8×8 по состоянию колонки + ручной режим от агентов.
//! Выдаёт готовый кадр (с гаммой и ограничением яркости) — драйвер только отправляет его.
use crate::color::{self, gamma8, hue, scale};
use crate::config::{LED_COUNT, LED_MATRIX_H, LED_MATRIX_W, LED_MAX_BRIGHTNESS, LED_SERPENTINE};
use crate::dsp::{led_index, level_unit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Boot,
    Offline,
    Idle,
    Listening,
    Thinking,
    Speaking,
    Error,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Override {
    None,
    Off,
    Solid,
    Breathe,
    Rainbow,
}

pub type Frame = [(u8, u8, u8); LED_COUNT];

pub struct Leds {
    mode: Mode,
    accent: u32,
    ov: Override,
    ov_color: u32,
    ov_bright: u8,
    alert_until: u32,
    alert_color: u32,
    last_frame: Option<u32>,
    mode_since: u32,
    level: f32,
    px: [u32; LED_COUNT],
}

const DEFAULT_ACCENT: u32 = 0x3C64FF;

fn before(now: u32, t: u32) -> bool {
    (now.wrapping_sub(t) as i32) < 0
}

impl Default for Leds {
    fn default() -> Self {
        Leds {
            mode: Mode::Boot,
            accent: DEFAULT_ACCENT,
            ov: Override::None,
            ov_color: 0xFFB060,
            ov_bright: 100,
            alert_until: 0,
            alert_color: 0xFF7000,
            last_frame: None,
            mode_since: 0,
            level: 0.0,
            px: [0; LED_COUNT],
        }
    }
}

fn dist(x: i32, y: i32) -> f32 {
    let dx = x as f32 - (LED_MATRIX_W - 1) as f32 / 2.0;
    let dy = y as f32 - (LED_MATRIX_H - 1) as f32 / 2.0;
    (dx * dx + dy * dy).sqrt()
}

/// Пиксели периметра по кругу.
fn perimeter(i: i32) -> (i32, i32) {
    let (w, h) = (LED_MATRIX_W, LED_MATRIX_H);
    let n = 2 * (w + h) - 4;
    let mut i = i.rem_euclid(n);
    if i < w {
        return (i, 0);
    }
    i -= w;
    if i < h - 1 {
        return (w - 1, i + 1);
    }
    i -= h - 1;
    if i < w - 1 {
        return (w - 2 - i, h - 1);
    }
    i -= w - 1;
    (0, h - 2 - i)
}

impl Leds {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, m: Mode, now: u32) {
        if m != self.mode {
            self.mode_since = now;
        }
        self.mode = m;
    }

    pub fn set_accent(&mut self, rgb: u32) {
        self.accent = if rgb != 0 { rgb } else { DEFAULT_ACCENT };
    }

    /// Ручной режим от сервера (инструмент set_lights): off | solid | breathe | rainbow | auto
    pub fn set_override(&mut self, mode: &str, rgb: Option<u32>, brightness_pct: Option<u8>) {
        match mode {
            "auto" | "" => self.ov = Override::None,
            "off" => self.ov = Override::Off,
            "solid" => self.ov = Override::Solid,
            "breathe" => self.ov = Override::Breathe,
            "rainbow" => self.ov = Override::Rainbow,
            _ => {}
        }
        if let Some(c) = rgb.filter(|c| *c != 0) {
            self.ov_color = c;
        }
        if let Some(b) = brightness_pct.filter(|b| *b != 0) {
            self.ov_bright = b.min(100);
        }
    }

    pub fn flash_alert(&mut self, rgb: u32, ms: u32, now: u32) {
        self.alert_color = rgb;
        self.alert_until = now.wrapping_add(ms);
    }

    fn set(&mut self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= LED_MATRIX_W || y >= LED_MATRIX_H {
            return;
        }
        self.px[led_index(x, y, LED_MATRIX_W, LED_SERPENTINE)] = c;
    }

    fn fill(&mut self, c: u32) {
        self.px = [c; LED_COUNT];
    }

    fn rainbow(&mut self, t: u32, bright: f32) {
        for y in 0..LED_MATRIX_H {
            for x in 0..LED_MATRIX_W {
                let h = (t.wrapping_mul(12).wrapping_add(((x + y) * 4096) as u32) & 0xFFFF) as u16;
                self.set(x, y, scale(hue(h), bright));
            }
        }
    }

    fn breathe(&mut self, c: u32, t: u32, lo: f32, hi: f32, period: u32) {
        let ph = (t % period) as f32 / period as f32;
        let k = lo + (hi - lo) * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * ph).cos());
        self.fill(scale(c, k));
    }

    /// Новый кадр (~30 в секунду). None — ещё рано.
    pub fn render(&mut self, now: u32, mic_level: f32, out_level: f32) -> Option<Frame> {
        if let Some(last) = self.last_frame {
            if now.wrapping_sub(last) < 33 {
                return None;
            }
        }
        self.last_frame = Some(now);
        let t = now.wrapping_sub(self.mode_since);
        self.px = [0; LED_COUNT];
        let accent = self.accent;

        if before(now, self.alert_until) {
            if (now / 250) % 2 == 1 {
                self.fill(self.alert_color);
            }
        } else {
            match self.mode {
                Mode::Sleep => {}
                Mode::Error => {
                    if (t / 160) % 2 == 0 && t < 1500 {
                        self.fill(0xFF0000);
                    }
                }
                Mode::Boot => self.rainbow(now, 0.6),
                Mode::Offline => {
                    // без if между двумя f32-константами: LLVM для Xtensa на этом падает
                    let k = 0.05 + 0.30 * ((now / 600) % 2) as f32;
                    for y in 3..=4 {
                        for x in 3..=4 {
                            self.set(x, y, scale(0xFF2000, k));
                        }
                    }
                }
                Mode::Listening => {
                    self.level = self.level * 0.6 + level_unit(mic_level, -50.0) * 0.4;
                    let r = 0.8 + self.level * 4.2;
                    for y in 0..LED_MATRIX_H {
                        for x in 0..LED_MATRIX_W {
                            let d = dist(x, y);
                            if d <= r {
                                self.set(x, y, scale(accent, 0.25 + 0.75 * (1.0 - d / (r + 0.01))));
                            }
                        }
                    }
                }
                Mode::Thinking => {
                    let head = (t / 40) as i32;
                    for k in 0..9 {
                        let (x, y) = perimeter(head - k);
                        self.set(x, y, scale(accent, 1.0 - k as f32 / 9.0));
                    }
                }
                Mode::Speaking => {
                    self.level = self.level * 0.55 + level_unit(out_level * 0.5, -50.0) * 0.45;
                    for y in 0..LED_MATRIX_H {
                        for x in 0..LED_MATRIX_W {
                            let k = self.level * (1.0 - dist(x, y) / 5.0) + 0.06;
                            self.set(x, y, scale(accent, k));
                        }
                    }
                }
                Mode::Idle => match self.ov {
                    Override::Off => {}
                    Override::Solid => self.fill(self.ov_color),
                    Override::Breathe => self.breathe(self.ov_color, now, 0.15, 1.0, 4000),
                    Override::Rainbow => self.rainbow(now, 1.0),
                    Override::None => self.breathe(accent, now, 0.02, 0.18, 5000),
                },
            }
        }
        let bright = if self.ov == Override::None {
            LED_MAX_BRIGHTNESS as u32
        } else {
            LED_MAX_BRIGHTNESS as u32 * self.ov_bright as u32 / 100
        };
        let mut frame: Frame = [(0, 0, 0); LED_COUNT];
        for (f, &c) in frame.iter_mut().zip(self.px.iter()) {
            let (r, g, b) = color::split(c);
            let s = |v: u8| ((gamma8(v) as u32 * (bright + 1)) >> 8) as u8;
            *f = (s(r), s(g), s(b));
        }
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(f: &Frame) -> usize {
        f.iter().filter(|p| **p != (0, 0, 0)).count()
    }

    #[test]
    fn perimeter_walks_the_edge() {
        let pts: Vec<_> = (0..28).map(perimeter).collect();
        assert_eq!(pts[0], (0, 0));
        assert_eq!(pts[7], (7, 0));
        assert_eq!(pts[14], (7, 7));
        assert_eq!(pts[27], (0, 1));
        assert_eq!(perimeter(28), (0, 0));
        assert_eq!(perimeter(-1), (0, 1));
    }

    #[test]
    fn modes_and_brightness_cap() {
        let mut l = Leds::default();
        l.set_mode(Mode::Offline, 0);
        let f = l.render(600, 0.0, 0.0).unwrap();
        assert_eq!(lit(&f), 4, "красная точка 2×2 в центре");
        assert!(l.render(610, 0.0, 0.0).is_none(), "не чаще 30 кадров/с");

        l.set_mode(Mode::Listening, 700);
        let quiet = lit(&l.render(700, 0.0, 0.0).unwrap());
        let mut loud = 0;
        for i in 1..10 {
            loud = lit(&l.render(700 + i * 40, 0.3, 0.0).unwrap());
        }
        assert!(loud > quiet, "круг растёт от голоса: {quiet} → {loud}");

        l.set_mode(Mode::Idle, 2000);
        l.set_override("solid", Some(0xFFFFFF), Some(100));
        let f = l.render(3000, 0.0, 0.0).unwrap();
        assert!(f.iter().all(|p| p.0 == p.1 && p.0 <= LED_MAX_BRIGHTNESS && p.0 > 0));
        l.set_override("off", None, None);
        assert_eq!(lit(&l.render(3100, 0.0, 0.0).unwrap()), 0);

        l.flash_alert(0xFF0000, 1000, 3200);
        let f = l.render(3250, 0.0, 0.0).unwrap();
        assert_eq!(lit(&f), 64);
        assert!(f[0].1 == 0 && f[0].0 > 0);
    }
}
