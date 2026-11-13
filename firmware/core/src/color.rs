//! Цвета: 0xRRGGBB ↔ RGB565, яркость, радуга и гамма для светодиодов.
use embedded_graphics::pixelcolor::Rgb565;

/// "#RRGGBB" или "RRGGBB" → 0xRRGGBB.
pub fn parse_color(hex: &str) -> Option<u32> {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    if h.len() != 6 {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

pub fn rgb565(rgb: u32) -> Rgb565 {
    Rgb565::new(((rgb >> 19) & 0x1F) as u8, ((rgb >> 10) & 0x3F) as u8, ((rgb >> 3) & 0x1F) as u8)
}

pub fn split(rgb: u32) -> (u8, u8, u8) {
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub fn join(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

pub fn scale(rgb: u32, k: f32) -> u32 {
    if k <= 0.0 {
        return 0;
    }
    let k = k.min(1.0);
    let (r, g, b) = split(rgb);
    join((r as f32 * k) as u8, (g as f32 * k) as u8, (b as f32 * k) as u8)
}

/// Цвет по оттенку 0..65535 (как Adafruit_NeoPixel::ColorHSV при полной насыщенности и яркости).
pub fn hue(h: u16) -> u32 {
    let h = ((h as u32 * 1530 + 32768) / 65536) as i32;
    let (r, g, b) = if h < 510 {
        if h < 255 { (255, h, 0) } else { (510 - h, 255, 0) }
    } else if h < 1020 {
        if h < 765 { (0, 255, h - 510) } else { (0, 1020 - h, 255) }
    } else if h < 1530 {
        if h < 1275 { (h - 1020, 0, 255) } else { (255, 0, 1530 - h) }
    } else {
        (255, 0, 0)
    };
    join(r as u8, g as u8, b as u8)
}

/// Гамма-коррекция 2.6 (как таблица Adafruit gamma8).
pub fn gamma8(x: u8) -> u8 {
    ((x as f32 / 255.0).powf(2.6) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#FF7A1A"), Some(0xFF7A1A));
        assert_eq!(parse_color("5b8cff"), Some(0x5B8CFF));
        assert_eq!(parse_color("#fff"), None);
        assert_eq!(parse_color("zzzzzz"), None);
        assert_eq!(rgb565(0xFFFFFF), Rgb565::new(31, 63, 31));
        assert_eq!(scale(0x804020, 0.5), 0x402010);
        assert_eq!(hue(0), 0xFF0000);
        assert_eq!(hue(21845), 0x00FF00);
        assert_eq!((gamma8(0), gamma8(255), gamma8(128)), (0, 255, 42));
    }
}
