//! Интерфейс колонки на экране 320×240: верхняя строка состояния, центральная область
//! (через буфер в памяти — без мерцания) и нижняя панель агентов и громкости.
//! Рисует в любой `DrawTarget<Color = Rgb565>` — на плате это ILI9341, в тестах — буфер.
use std::convert::Infallible;

use embedded_graphics::mono_font::iso_8859_5::{FONT_10X20, FONT_6X12, FONT_8X13};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Arc, Circle, PrimitiveStyle, Rectangle, RoundedRectangle, Triangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};

use crate::color::rgb565;
use crate::config::{FW_VERSION, VOLUME_DEFAULT};
use crate::dsp::level_unit;
use crate::text::{sanitize, trim_front, wrap};

pub const W: i32 = 320;
pub const H: i32 = 240;
pub const TOP_H: i32 = 26;
pub const BOT_H: i32 = 40;
pub const MID_Y: i32 = TOP_H;
pub const MID_H: i32 = H - TOP_H - BOT_H;

pub const COL_BG: u32 = 0x0E1116;
pub const COL_PANEL: u32 = 0x161B22;
pub const COL_TEXT: u32 = 0xECEFF4;
pub const COL_DIM: u32 = 0x8A93A3;
pub const COL_FAINT: u32 = 0x2C3440;
pub const COL_OK: u32 = 0x37C871;
pub const COL_BAD: u32 = 0xE5484D;
pub const COL_WARN: u32 = 0xF5A524;
pub const COL_USER: u32 = 0x9AA4B5;
pub const COL_AUTO: u32 = 0x5B8CFF;
pub const COL_BLACK: u32 = 0x000000;
pub const COL_WHITE: u32 = 0xFFFFFF;

#[derive(Clone, Copy)]
struct Font {
    font: &'static MonoFont<'static>,
    lh: i32,
}

const SMALL: Font = Font { font: &FONT_6X12, lh: 14 };
const TEXT: Font = Font { font: &FONT_8X13, lh: 18 };
const BIG: Font = Font { font: &FONT_10X20, lh: 26 };

impl Font {
    fn cw(&self) -> i32 {
        (self.font.character_size.width + self.font.character_spacing) as i32
    }
    fn chars(&self, width: i32) -> usize {
        (width / self.cw()).max(1) as usize
    }
    fn width(&self, s: &str) -> i32 {
        s.chars().count() as i32 * self.cw()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Boot,
    Connecting,
    Offline,
    Idle,
    Listening,
    Thinking,
    Speaking,
    Sleep,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TouchEvent {
    None,
    Talk,
    SelectAgent(String),
    VolumeUp,
    VolumeDown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentChip {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub available: bool,
}

// ─────────────────────────────────────────── буфер центральной области

/// Кадр в памяти (на плате — в PSRAM): рисуем сюда, потом одним блоком на экран.
pub struct Canvas {
    w: u32,
    h: u32,
    buf: Vec<Rgb565>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, buf: vec![Rgb565::BLACK; (w * h) as usize] }
    }
    pub fn pixels(&self) -> &[Rgb565] {
        &self.buf
    }
    pub fn pixel(&self, x: u32, y: u32) -> Rgb565 {
        self.buf[(y * self.w + x) as usize]
    }
}

impl OriginDimensions for Canvas {
    fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }
}

impl DrawTarget for Canvas {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Infallible> {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && (p.x as u32) < self.w && (p.y as u32) < self.h {
                self.buf[(p.y as u32 * self.w + p.x as u32) as usize] = c;
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), Infallible> {
        let a = area.intersection(&self.bounding_box());
        if let Some(br) = a.bottom_right() {
            for y in a.top_left.y..=br.y {
                let row = (y as u32 * self.w) as usize;
                self.buf[row + a.top_left.x as usize..=row + br.x as usize].fill(color);
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Rgb565) -> Result<(), Infallible> {
        self.buf.fill(color);
        Ok(())
    }
}

// ─────────────────────────────────────────── примитивы

fn fill<D: DrawTarget<Color = Rgb565>>(d: &mut D, x: i32, y: i32, w: i32, h: i32, c: u32) -> Result<(), D::Error> {
    d.fill_solid(&Rectangle::new(Point::new(x, y), Size::new(w.max(0) as u32, h.max(0) as u32)), rgb565(c))
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0) as u32, h.max(0) as u32))
}

fn round_rect<D: DrawTarget<Color = Rgb565>>(d: &mut D, r: Rectangle, c: u32, filled: bool) -> Result<(), D::Error> {
    let style = if filled { PrimitiveStyle::with_fill(rgb565(c)) } else { PrimitiveStyle::with_stroke(rgb565(c), 1) };
    RoundedRectangle::with_equal_corners(r, Size::new(8, 8)).into_styled(style).draw(d)
}

fn circle<D: DrawTarget<Color = Rgb565>>(d: &mut D, cx: i32, cy: i32, r: i32, c: u32) -> Result<(), D::Error> {
    Circle::with_center(Point::new(cx, cy), (2 * r + 1) as u32).into_styled(PrimitiveStyle::with_fill(rgb565(c))).draw(d)
}

/// Кольцевая дуга от радиуса r_in до r_out; углы в градусах, 0° — «на 3 часа», по часовой.
#[allow(clippy::too_many_arguments)]
fn arc<D: DrawTarget<Color = Rgb565>>(d: &mut D, cx: i32, cy: i32, r_out: i32, r_in: i32, start: f32, sweep: f32, c: u32) -> Result<(), D::Error> {
    let width = (r_out - r_in).max(1) as u32;
    let diameter = (r_out + r_in + 1) as u32;
    Arc::with_center(Point::new(cx, cy), diameter, Angle::from_degrees(start), Angle::from_degrees(sweep))
        .into_styled(PrimitiveStyle::with_stroke(rgb565(c), width))
        .draw(d)
}

fn text<D: DrawTarget<Color = Rgb565>>(d: &mut D, s: &str, x: i32, y: i32, f: Font, c: u32, align: Alignment) -> Result<(), D::Error> {
    let s = sanitize(s);
    let style = MonoTextStyle::new(f.font, rgb565(c));
    let ts = TextStyleBuilder::new().alignment(align).baseline(Baseline::Top).build();
    Text::with_text_style(&s, Point::new(x, y), style, ts).draw(d)?;
    Ok(())
}

fn text_mid<D: DrawTarget<Color = Rgb565>>(d: &mut D, s: &str, cx: i32, cy: i32, f: Font, c: u32) -> Result<(), D::Error> {
    let s = sanitize(s);
    let style = MonoTextStyle::new(f.font, rgb565(c));
    let ts = TextStyleBuilder::new().alignment(Alignment::Center).baseline(Baseline::Middle).build();
    Text::with_text_style(&s, Point::new(cx, cy), style, ts).draw(d)?;
    Ok(())
}

fn before(now: u32, t: u32) -> bool {
    (now.wrapping_sub(t) as i32) < 0
}

fn hit(r: &Rectangle, x: i32, y: i32) -> bool {
    r.contains(Point::new(x, y))
}

// ─────────────────────────────────────────── интерфейс

pub struct Ui {
    now: u32,
    status: Status,
    hint: String,
    wifi: bool,
    server: bool,
    bat_low: bool,
    rssi: i32,
    bat: i32,
    vol: u8,
    agents: Vec<AgentChip>,
    selected: String,
    answer_name: String,
    answer_note: String,
    answer_color: u32,
    user_text: String,
    reply_text: String,
    user_lines: Vec<String>,
    reply_lines: Vec<String>,
    wrap_dirty: bool,
    toast: String,
    toast_color: u32,
    toast_until: Option<u32>,
    mic: f32,
    out: f32,
    top_dirty: bool,
    bot_dirty: bool,
    mid_dirty: bool,
    last_mid_draw: u32,
    chip_rects: Vec<(Rectangle, String)>,
    touching: bool,
    touch_at: (i32, i32),
    last_touch_poll: Option<u32>,
}

fn vol_down_rect() -> Rectangle {
    rect(W - 78, H - BOT_H + 5, 36, BOT_H - 10)
}
fn vol_up_rect() -> Rectangle {
    rect(W - 39, H - BOT_H + 5, 36, BOT_H - 10)
}

fn wifi_bars(connected: bool, rssi: i32) -> i32 {
    if !connected {
        0
    } else if rssi > -55 {
        4
    } else if rssi > -65 {
        3
    } else if rssi > -75 {
        2
    } else {
        1
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            now: 0,
            status: Status::Boot,
            hint: String::new(),
            wifi: false,
            server: false,
            bat_low: false,
            rssi: 0,
            bat: -1,
            vol: VOLUME_DEFAULT,
            agents: vec![],
            selected: "auto".into(),
            answer_name: String::new(),
            answer_note: String::new(),
            answer_color: COL_AUTO,
            user_text: String::new(),
            reply_text: String::new(),
            user_lines: vec![],
            reply_lines: vec![],
            wrap_dirty: true,
            toast: String::new(),
            toast_color: COL_WARN,
            toast_until: None,
            mic: 0.0,
            out: 0.0,
            top_dirty: true,
            bot_dirty: true,
            mid_dirty: true,
            last_mid_draw: 0,
            chip_rects: vec![],
            touching: false,
            touch_at: (0, 0),
            last_touch_poll: None,
        }
    }
}

impl Ui {
    // ── состояние ──

    pub fn status(&self) -> Status {
        self.status
    }
    pub fn hint(&self) -> &str {
        &self.hint
    }
    pub fn toast_text(&self) -> Option<&str> {
        self.toast_until.filter(|t| before(self.now, *t)).map(|_| self.toast.as_str())
    }
    pub fn reply_text(&self) -> &str {
        &self.reply_text
    }
    pub fn user_text(&self) -> &str {
        &self.user_text
    }
    pub fn agents(&self) -> &[AgentChip] {
        &self.agents
    }
    pub fn selected(&self) -> &str {
        &self.selected
    }
    pub fn answering(&self) -> (&str, u32, &str) {
        (&self.answer_name, self.answer_color, &self.answer_note)
    }

    pub fn set_time(&mut self, now: u32) {
        self.now = now;
    }

    pub fn set_status(&mut self, s: Status, hint: &str) {
        if s != self.status || hint != self.hint {
            self.mid_dirty = true;
        }
        if self.status == Status::Boot && s != Status::Boot {
            self.top_dirty = true;
            self.bot_dirty = true;
        }
        self.status = s;
        self.hint = hint.to_string();
    }

    pub fn set_wifi(&mut self, connected: bool, rssi: i32) {
        if connected != self.wifi || wifi_bars(connected, rssi) != wifi_bars(self.wifi, self.rssi) {
            self.top_dirty = true;
        }
        self.wifi = connected;
        self.rssi = rssi;
    }

    pub fn set_server(&mut self, connected: bool) {
        if connected != self.server {
            self.top_dirty = true;
        }
        self.server = connected;
    }

    /// percent = -1 — АКБ нет (питание от USB).
    pub fn set_battery(&mut self, percent: i32, low: bool) {
        if percent != self.bat || low != self.bat_low {
            self.top_dirty = true;
        }
        self.bat = percent;
        self.bat_low = low;
    }

    pub fn set_volume(&mut self, v: u8) {
        if v != self.vol {
            self.top_dirty = true;
        }
        self.vol = v;
    }

    pub fn set_agents(&mut self, list: Vec<AgentChip>) {
        self.agents = list;
        self.top_dirty = true;
        self.bot_dirty = true;
        self.mid_dirty = true;
    }

    pub fn set_selected_agent(&mut self, id: &str) {
        if id != self.selected {
            self.top_dirty = true;
            self.bot_dirty = true;
            self.mid_dirty = true;
        }
        self.selected = id.to_string();
    }

    pub fn set_answering(&mut self, name: &str, color: u32, note: &str) {
        self.answer_name = name.to_string();
        self.answer_color = color;
        self.answer_note = note.to_string();
        self.mid_dirty = true;
        self.wrap_dirty = true;
    }

    pub fn set_user_text(&mut self, t: &str) {
        self.user_text = t.to_string();
        self.mid_dirty = true;
        self.wrap_dirty = true;
    }

    pub fn append_reply(&mut self, t: &str) {
        self.reply_text.push_str(t);
        trim_front(&mut self.reply_text, 1500, 1200);
        self.mid_dirty = true;
        self.wrap_dirty = true;
    }

    pub fn clear_conversation(&mut self) {
        self.user_text.clear();
        self.reply_text.clear();
        self.answer_name.clear();
        self.answer_note.clear();
        self.mid_dirty = true;
        self.wrap_dirty = true;
    }

    pub fn toast(&mut self, t: &str, color: u32, ms: u32) {
        self.toast = t.to_string();
        self.toast_color = color;
        self.toast_until = Some(self.now.wrapping_add(ms));
        self.mid_dirty = true;
    }

    pub fn set_levels(&mut self, mic: f32, out: f32) {
        self.mic = mic;
        self.out = out;
    }

    /// Всё перерисовать (например, после калибровки тача).
    pub fn invalidate(&mut self) {
        self.top_dirty = true;
        self.bot_dirty = true;
        self.mid_dirty = true;
    }

    fn agent_name(&self, id: &str) -> String {
        if id == "auto" {
            return "Авто".into();
        }
        self.agents.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| id.to_string())
    }

    fn agent_color(&self, id: &str) -> u32 {
        self.agents.iter().find(|a| a.id == id).map(|a| a.color).unwrap_or(COL_AUTO)
    }

    fn animating(&self) -> bool {
        matches!(self.status, Status::Listening | Status::Thinking | Status::Speaking)
            || self.toast_until.is_some_and(|t| before(self.now, t.wrapping_add(100)))
    }

    // ── отрисовка ──

    /// Перерисовать изменившиеся части. Вызывать часто (в главном цикле).
    pub fn render<D: DrawTarget<Color = Rgb565>>(&mut self, lcd: &mut D, canvas: &mut Canvas, now: u32) -> Result<(), D::Error> {
        self.now = now;
        if self.status == Status::Boot {
            return Ok(());
        }
        if self.top_dirty {
            self.draw_top(lcd)?;
        }
        if self.bot_dirty {
            self.draw_bottom(lcd)?;
        }
        let period = if self.animating() { 50 } else { 250 };
        let due = now.wrapping_sub(self.last_mid_draw) >= period;
        if self.mid_dirty || (self.animating() && due) {
            let _ = self.draw_middle(canvas);
            lcd.fill_contiguous(&rect(0, MID_Y, W, MID_H), canvas.pixels().iter().copied())?;
            self.last_mid_draw = now;
            self.mid_dirty = false;
        }
        if let Some(t) = self.toast_until {
            if !before(now, t.wrapping_add(100)) {
                self.toast_until = None;
                self.mid_dirty = true;
            }
        }
        Ok(())
    }

    fn draw_battery<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, x: i32, y: i32) -> Result<(), D::Error> {
        if self.bat < 0 {
            return text(d, "USB", x, y + 2, SMALL, COL_DIM, Alignment::Left);
        }
        let c = if self.bat_low { COL_BAD } else if self.bat < 30 { COL_WARN } else { COL_OK };
        rect(x, y + 2, 20, 11).into_styled(PrimitiveStyle::with_stroke(rgb565(COL_DIM), 1)).draw(d)?;
        fill(d, x + 20, y + 5, 2, 5, COL_DIM)?;
        let w = self.bat * 16 / 100;
        if w > 0 {
            fill(d, x + 2, y + 4, w, 7, c)?;
        }
        text(d, &format!("{}%", self.bat), x + 25, y + 2, SMALL, COL_TEXT, Alignment::Left)
    }

    fn draw_top<D: DrawTarget<Color = Rgb565>>(&mut self, d: &mut D) -> Result<(), D::Error> {
        fill(d, 0, 0, W, TOP_H, COL_PANEL)?;
        // слева — выбранный агент
        let c = if self.selected == "auto" { COL_AUTO } else { self.agent_color(&self.selected) };
        circle(d, 10, 13, 5, c)?;
        let name: String = self.agent_name(&self.selected).chars().take(18).collect();
        text(d, &name, 21, 7, TEXT, COL_TEXT, Alignment::Left)?;
        // справа налево: батарея, громкость, сервер, Wi-Fi
        let mut x = W - 56;
        self.draw_battery(d, x, 5)?;
        x -= 48;
        let dim = PrimitiveStyle::with_fill(rgb565(COL_DIM));
        fill(d, x, 9, 4, 7, COL_DIM)?;
        Triangle::new(Point::new(x + 4, 9), Point::new(x + 9, 4), Point::new(x + 9, 20)).into_styled(dim).draw(d)?;
        Triangle::new(Point::new(x + 4, 16), Point::new(x + 9, 20), Point::new(x + 4, 9)).into_styled(dim).draw(d)?;
        text(d, &self.vol.to_string(), x + 13, 7, SMALL, COL_TEXT, Alignment::Left)?;
        x -= 16;
        circle(d, x, 13, 4, if self.server { COL_OK } else { COL_BAD })?;
        x -= 26;
        let bars = wifi_bars(self.wifi, self.rssi);
        for i in 0..4 {
            let h = 3 + i * 3;
            fill(d, x + i * 5, 19 - h, 3, h, if i < bars { COL_TEXT } else { COL_FAINT })?;
        }
        self.top_dirty = false;
        Ok(())
    }

    fn draw_bottom<D: DrawTarget<Color = Rgb565>>(&mut self, d: &mut D) -> Result<(), D::Error> {
        let y = H - BOT_H;
        fill(d, 0, y, W, BOT_H, COL_PANEL)?;
        self.chip_rects.clear();
        let mut items: Vec<(String, String)> = vec![("auto".into(), "Авто".into())];
        items.extend(self.agents.iter().take(4).map(|a| (a.id.clone(), a.name.clone())));
        let area_w = W - 84;
        let cw = area_w / items.len() as i32;
        for (i, (id, name)) in items.iter().enumerate() {
            let r = rect(4 + i as i32 * cw, y + 5, cw - 4, BOT_H - 10);
            self.chip_rects.push((r, id.clone()));
            let sel = *id == self.selected;
            let c = if id == "auto" { COL_AUTO } else { self.agent_color(id) };
            let avail = self.agents.iter().find(|a| a.id == *id).map(|a| a.available).unwrap_or(true);
            round_rect(d, r, if sel || avail { c } else { COL_FAINT }, sel)?;
            let inner = r.size.width as i32 - 6;
            let f = if TEXT.width(name) <= inner { TEXT } else { SMALL };
            let label: String = name.chars().take(f.chars(inner)).collect();
            let col = if sel { COL_BLACK } else if avail { COL_TEXT } else { COL_DIM };
            text_mid(d, &label, r.top_left.x + r.size.width as i32 / 2, r.top_left.y + r.size.height as i32 / 2, f, col)?;
        }
        for (r, plus) in [(vol_down_rect(), false), (vol_up_rect(), true)] {
            round_rect(d, r, COL_DIM, false)?;
            let (cx, cy) = (r.top_left.x + r.size.width as i32 / 2, r.top_left.y + r.size.height as i32 / 2);
            fill(d, cx - 7, cy - 1, 14, 3, COL_TEXT)?;
            if plus {
                fill(d, cx - 1, cy - 7, 3, 14, COL_TEXT)?;
            }
        }
        self.bot_dirty = false;
        Ok(())
    }

    fn rewrap(&mut self) {
        let width = W - 16;
        self.user_lines = if self.user_text.is_empty() {
            vec![]
        } else {
            wrap(&sanitize(&format!("Вы: {}", self.user_text)), SMALL.chars(width))
        };
        self.reply_lines = if self.reply_text.is_empty() { vec![] } else { wrap(&sanitize(&self.reply_text), TEXT.chars(width)) };
        self.wrap_dirty = false;
    }

    fn centered_block(&self, c: &mut Canvas, title: &str, sub: &str, title_color: u32) -> Result<(), Infallible> {
        let lines = wrap(&sanitize(title), BIG.chars(W - 24));
        let subs = if sub.is_empty() { vec![] } else { wrap(&sanitize(sub), TEXT.chars(W - 24)) };
        let total = lines.len() as i32 * BIG.lh + subs.len() as i32 * TEXT.lh + if subs.is_empty() { 0 } else { 6 };
        let mut y = (MID_H - total) / 2;
        for l in &lines {
            text(c, l, W / 2, y, BIG, title_color, Alignment::Center)?;
            y += BIG.lh;
        }
        y += 6;
        for l in &subs {
            text(c, l, W / 2, y, TEXT, COL_DIM, Alignment::Center)?;
            y += TEXT.lh;
        }
        Ok(())
    }

    fn conversation(&mut self, c: &mut Canvas, bottom_reserve: i32) -> Result<(), Infallible> {
        if self.wrap_dirty {
            self.rewrap();
        }
        let (lh_user, lh_label, lh_reply) = (SMALL.lh, 18, TEXT.lh);
        let avail = MID_H - 8 - bottom_reserve;
        let need = self.user_lines.len() as i32 * lh_user
            + if self.answer_name.is_empty() { 0 } else { lh_label }
            + self.reply_lines.len() as i32 * lh_reply;
        let skip = (need - avail).max(0); // прокрутка к последним строкам
        let mut y = 4 - skip;
        for l in &self.user_lines {
            if y > -lh_user {
                text(c, l, 8, y, SMALL, COL_USER, Alignment::Left)?;
            }
            y += lh_user;
        }
        if !self.answer_name.is_empty() {
            if y > -lh_label {
                circle(c, 12, y + 7, 4, self.answer_color)?;
                let mut label = self.answer_name.clone();
                if !self.answer_note.is_empty() {
                    label += &format!(" · {}", self.answer_note);
                }
                text(c, &label, 22, y + 1, SMALL, self.answer_color, Alignment::Left)?;
            }
            y += lh_label;
        }
        for l in &self.reply_lines {
            if y > -lh_reply {
                text(c, l, 8, y, TEXT, COL_TEXT, Alignment::Left)?;
            }
            y += lh_reply;
        }
        Ok(())
    }

    fn draw_middle(&mut self, c: &mut Canvas) -> Result<(), Infallible> {
        let now = self.now;
        c.clear(rgb565(COL_BG))?;
        match self.status {
            Status::Boot | Status::Connecting => {
                let t = if self.hint.is_empty() { "Подключаюсь…".to_string() } else { self.hint.clone() };
                self.centered_block(c, &t, "", COL_TEXT)?;
            }
            Status::Offline => self.centered_block(c, "Нет связи", &self.hint.clone(), COL_WARN)?,
            Status::Sleep => self.centered_block(c, "Заряди меня", "Аккумулятор разряжен — колонка засыпает", COL_BAD)?,
            Status::Idle => {
                if self.user_text.is_empty() && self.reply_text.is_empty() {
                    let sub = if self.hint.is_empty() { "SW1 — говорить · SW2 — другой агент".to_string() } else { self.hint.clone() };
                    self.centered_block(c, "Нажми и говори", &sub, COL_TEXT)?;
                } else {
                    self.conversation(c, 0)?;
                }
            }
            Status::Listening => {
                let (cx, cy) = (W / 2, MID_H / 2 - 14);
                let lvl = level_unit(self.mic, -50.0);
                let col = if self.selected == "auto" { COL_AUTO } else { self.agent_color(&self.selected) };
                circle(c, cx, cy, 30 + (lvl * 26.0) as i32, COL_FAINT)?;
                circle(c, cx, cy, 30, col)?;
                round_rect(c, rect(cx - 7, cy - 16, 14, 22), COL_WHITE, true)?; // микрофон
                arc(c, cx, cy - 2, 13, 11, 0.0, 180.0, COL_WHITE)?;
                fill(c, cx - 1, cy + 11, 3, 6, COL_WHITE)?;
                text(c, "Слушаю…", W / 2, cy + 40, BIG, COL_TEXT, Alignment::Center)?;
                if !self.hint.is_empty() {
                    let h: String = self.hint.chars().take(SMALL.chars(W - 8)).collect();
                    text(c, &h, W / 2, cy + 68, SMALL, COL_DIM, Alignment::Center)?;
                }
            }
            Status::Thinking => {
                if self.wrap_dirty {
                    self.rewrap();
                }
                let mut y = 6;
                for l in self.user_lines.iter().take(3) {
                    text(c, l, 8, y, SMALL, COL_USER, Alignment::Left)?;
                    y += SMALL.lh;
                }
                let (cx, cy) = (W / 2, MID_H / 2 + 6);
                let a = ((now / 4) % 360) as f32;
                arc(c, cx, cy, 26, 21, a, 100.0, self.answer_color)?;
                arc(c, cx, cy, 26, 21, a + 180.0, 100.0, self.answer_color)?;
                let who = if self.answer_name.is_empty() { "Думаю…".to_string() } else { format!("{} думает…", self.answer_name) };
                text(c, &who, W / 2, cy + 34, TEXT, COL_TEXT, Alignment::Center)?;
                if !self.answer_note.is_empty() {
                    text(c, &self.answer_note.clone(), W / 2, cy + 54, SMALL, COL_DIM, Alignment::Center)?;
                }
            }
            Status::Speaking => {
                self.conversation(c, 16)?;
                let (n, bw, gap) = (24, 8, 4);
                let x0 = (W - n * (bw + gap)) / 2;
                let base = MID_H - 3;
                let lvl = level_unit(self.out, -45.0);
                for i in 0..n {
                    let wob = 0.5 + 0.5 * (now as f32 / 90.0 + i as f32 * 0.9).sin();
                    let h = 2 + (lvl * 11.0 * wob) as i32;
                    fill(c, x0 + i * (bw + gap), base - h, bw, h, self.answer_color)?;
                }
            }
        }
        if let Some(t) = self.toast_until {
            if before(now, t) && !self.toast.is_empty() {
                let lines = wrap(&sanitize(&self.toast), TEXT.chars(W - 40));
                let h = lines.len() as i32 * TEXT.lh + 10;
                let y = MID_H - h - 4;
                round_rect(c, rect(10, y, W - 20, h), self.toast_color, true)?;
                let mut ly = y + 6;
                for l in &lines {
                    text(c, l, W / 2, ly, TEXT, COL_BLACK, Alignment::Center)?;
                    ly += TEXT.lh;
                }
            }
        }
        Ok(())
    }

    // ── касания ──

    /// point — координаты касания на экране (None — не касаются). Событие — по отпусканию.
    pub fn touch(&mut self, point: Option<(i32, i32)>, now: u32) -> TouchEvent {
        if self.last_touch_poll.is_some_and(|t| now.wrapping_sub(t) < 16) {
            return TouchEvent::None; // ~60 раз в секунду достаточно
        }
        self.last_touch_poll = Some(now);
        match point {
            Some(p) if !self.touching => {
                self.touching = true;
                self.touch_at = p;
                TouchEvent::None
            }
            Some(_) => TouchEvent::None,
            None if self.touching => {
                self.touching = false;
                let (x, y) = self.touch_at;
                if let Some((_, id)) = self.chip_rects.iter().find(|(r, _)| hit(r, x, y)) {
                    return TouchEvent::SelectAgent(id.clone());
                }
                if hit(&vol_down_rect(), x, y) {
                    TouchEvent::VolumeDown
                } else if hit(&vol_up_rect(), x, y) {
                    TouchEvent::VolumeUp
                } else if (MID_Y..MID_Y + MID_H).contains(&y) {
                    TouchEvent::Talk
                } else {
                    TouchEvent::None
                }
            }
            None => TouchEvent::None,
        }
    }
}

// ─────────────────────────────────────────── экраны загрузки и калибровки

pub fn boot_screen<D: DrawTarget<Color = Rgb565>>(d: &mut D) -> Result<(), D::Error> {
    d.clear(rgb565(COL_BG))?;
    text_mid(d, "AI-Column", W / 2, H / 2 - 20, BIG, COL_TEXT)?;
    text_mid(d, &format!("прошивка {FW_VERSION} (Rust)"), W / 2, H / 2 + 8, SMALL, COL_DIM)
}

pub fn boot_message<D: DrawTarget<Color = Rgb565>>(d: &mut D, line: &str) -> Result<(), D::Error> {
    fill(d, 0, H / 2 + 24, W, 24, COL_BG)?;
    text(d, line, W / 2, H / 2 + 28, TEXT, COL_TEXT, Alignment::Center)
}

/// Первый запуск: предложение откалибровать тач.
pub fn calibration_offer<D: DrawTarget<Color = Rgb565>>(d: &mut D) -> Result<(), D::Error> {
    d.clear(rgb565(COL_BG))?;
    text_mid(d, "Коснись экрана —", W / 2, H / 2 - 22, TEXT, COL_TEXT)?;
    text_mid(d, "откалибруем тачскрин", W / 2, H / 2, TEXT, COL_TEXT)?;
    text_mid(d, "SW1 — пропустить (потом: держи SW2 при включении)", W / 2, H / 2 + 30, SMALL, COL_DIM)
}

/// Шаг калибровки: перекрестье в точке (x, y).
pub fn calibration_target<D: DrawTarget<Color = Rgb565>>(d: &mut D, x: i32, y: i32, step: usize, total: usize) -> Result<(), D::Error> {
    d.clear(rgb565(COL_BG))?;
    text_mid(d, "Калибровка тачскрина", W / 2, H / 2 - 12, TEXT, COL_TEXT)?;
    text_mid(d, &format!("коснись крестика ({step} из {total})"), W / 2, H / 2 + 12, SMALL, COL_DIM)?;
    fill(d, x - 12, y, 25, 1, COL_WHITE)?;
    fill(d, x, y - 12, 1, 25, COL_WHITE)?;
    Circle::with_center(Point::new(x, y), 13).into_styled(PrimitiveStyle::with_stroke(rgb565(COL_WARN), 1)).draw(d)
}

/// Экран сна (разряжен АКБ) — рисуется целиком.
pub fn sleep_screen<D: DrawTarget<Color = Rgb565>>(d: &mut D) -> Result<(), D::Error> {
    d.clear(rgb565(COL_BG))?;
    text_mid(d, "Заряди меня", W / 2, H / 2 - 12, BIG, COL_BAD)?;
    text_mid(d, "Аккумулятор разряжен — колонка засыпает", W / 2, H / 2 + 16, SMALL, COL_DIM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Canvas {
        Canvas::new(W as u32, H as u32)
    }

    fn count(c: &Canvas, col: u32) -> usize {
        c.pixels().iter().filter(|p| **p == rgb565(col)).count()
    }

    fn chips() -> Vec<AgentChip> {
        vec![
            AgentChip { id: "claude".into(), name: "Claude".into(), color: 0xFF7A1A, available: true },
            AgentChip { id: "qwen".into(), name: "Qwen".into(), color: 0x8B5CF6, available: false },
        ]
    }

    #[test]
    fn renders_every_status_without_panics() {
        let mut ui = Ui::default();
        let mut lcd = screen();
        let mut mid = Canvas::new(W as u32, MID_H as u32);
        ui.set_agents(chips());
        ui.set_battery(76, false);
        ui.set_wifi(true, -60);
        ui.set_server(true);
        let mut t = 0;
        for s in [Status::Connecting, Status::Offline, Status::Idle, Status::Listening, Status::Thinking, Status::Speaking, Status::Sleep] {
            ui.set_status(s, "подсказка");
            ui.set_user_text("Который час в Токио?");
            ui.set_answering("Claude", 0xFF7A1A, "запасной агент");
            ui.append_reply(&"Очень длинный ответ про время в Японии. ".repeat(20));
            ui.toast("Громкость 70%", COL_OK, 1000);
            ui.set_levels(0.1, 0.2);
            t += 60;
            ui.render(&mut lcd, &mut mid, t).unwrap();
        }
        assert!(count(&lcd, COL_PANEL) > 1000, "верхняя/нижняя панели нарисованы");
        assert!(count(&lcd, 0xFF7A1A) > 0, "цвет агента виден");
        assert!(ui.reply_text().chars().count() <= 1500, "длинный ответ обрезается");
    }

    #[test]
    fn idle_screen_shows_text_in_middle() {
        let mut ui = Ui::default();
        let mut lcd = screen();
        let mut mid = Canvas::new(W as u32, MID_H as u32);
        ui.set_status(Status::Idle, "");
        ui.render(&mut lcd, &mut mid, 10).unwrap();
        let text_px = (0..MID_H as u32).flat_map(|y| (0..W as u32).map(move |x| (x, y))).filter(|&(x, y)| mid.pixel(x, y) == rgb565(COL_TEXT)).count();
        assert!(text_px > 200, "заголовок «Нажми и говори» нарисован ({text_px} px)");
    }

    #[test]
    fn touch_events_after_release() {
        let mut ui = Ui::default();
        let mut lcd = screen();
        let mut mid = Canvas::new(W as u32, MID_H as u32);
        ui.set_agents(chips());
        ui.set_status(Status::Idle, "");
        ui.render(&mut lcd, &mut mid, 0).unwrap();
        let tap = |ui: &mut Ui, x: i32, y: i32, t: u32| {
            assert_eq!(ui.touch(Some((x, y)), t), TouchEvent::None);
            ui.touch(None, t + 20)
        };
        // фишки: auto, claude, qwen — ширина (320-84)/3 = 78
        assert_eq!(tap(&mut ui, 4 + 78 + 10, H - 20, 100), TouchEvent::SelectAgent("claude".into()));
        assert_eq!(tap(&mut ui, 10, H - 20, 200), TouchEvent::SelectAgent("auto".into()));
        assert_eq!(tap(&mut ui, W - 60, H - 20, 300), TouchEvent::VolumeDown);
        assert_eq!(tap(&mut ui, W - 20, H - 20, 400), TouchEvent::VolumeUp);
        assert_eq!(tap(&mut ui, 160, 120, 500), TouchEvent::Talk);
        assert_eq!(tap(&mut ui, 160, 5, 600), TouchEvent::None);
        // слишком частый опрос игнорируется
        assert_eq!(ui.touch(Some((1, 1)), 700), TouchEvent::None);
        assert_eq!(ui.touch(None, 705), TouchEvent::None);
    }

    #[test]
    fn toast_expires() {
        let mut ui = Ui::default();
        ui.set_time(1000);
        ui.toast("Привет", COL_OK, 500);
        assert_eq!(ui.toast_text(), Some("Привет"));
        ui.set_time(1600);
        assert_eq!(ui.toast_text(), None);
    }

    #[test]
    fn helper_screens() {
        let mut lcd = screen();
        boot_screen(&mut lcd).unwrap();
        boot_message(&mut lcd, "Подключаюсь к Wi-Fi…").unwrap();
        calibration_offer(&mut lcd).unwrap();
        calibration_target(&mut lcd, 24, 24, 1, 3).unwrap();
        assert_eq!(lcd.pixel(24, 24), rgb565(COL_WHITE));
        sleep_screen(&mut lcd).unwrap();
    }
}
