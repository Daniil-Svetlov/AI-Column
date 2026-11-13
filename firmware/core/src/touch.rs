//! Тачскрин XPT2046: разбор ответов контроллера и калибровка (аффинное преобразование по 3 точкам).

/// Команды XPT2046 (12 бит, дифференциальный режим, без отключения питания между замерами).
pub const CMD_X: u8 = 0xD1;
pub const CMD_Y: u8 = 0x91;
pub const CMD_Z1: u8 = 0xB1;
pub const CMD_Z2: u8 = 0xC1;
/// Порог «давления»: меньше — касания нет.
pub const PRESSURE_MIN: i32 = 400;

/// 12-битное значение из двух байт ответа.
pub fn parse12(hi: u8, lo: u8) -> u16 {
    ((((hi as u16) << 8) | lo as u16) >> 3) & 0x0FFF
}

/// Медиана — отсекает выбросы от наводок.
pub fn median(v: &mut [u16]) -> u16 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// Экран = A · (сырые x, y, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchCal {
    pub a: [f32; 6],
}

impl Default for TouchCal {
    /// Без калибровки: типичный диапазон 200..3900 по обеим осям, экран 320×240.
    fn default() -> Self {
        let kx = 320.0 / 3700.0;
        let ky = 240.0 / 3700.0;
        TouchCal { a: [kx, 0.0, -200.0 * kx, 0.0, ky, -200.0 * ky] }
    }
}

impl TouchCal {
    /// По трём точкам (экран ↔ сырые значения). None — точки на одной прямой (плохая калибровка).
    pub fn from_points(screen: [(i32, i32); 3], raw: [(u16, u16); 3]) -> Option<TouchCal> {
        let r: Vec<(f32, f32)> = raw.iter().map(|&(x, y)| (x as f32, y as f32)).collect();
        let det = r[0].0 * (r[1].1 - r[2].1) - r[0].1 * (r[1].0 - r[2].0) + (r[1].0 * r[2].1 - r[2].0 * r[1].1);
        if det.abs() < 1000.0 {
            return None;
        }
        let solve = |s: [f32; 3]| -> [f32; 3] {
            let a = (s[0] * (r[1].1 - r[2].1) - r[0].1 * (s[1] - s[2]) + (s[1] * r[2].1 - s[2] * r[1].1)) / det;
            let b = (r[0].0 * (s[1] - s[2]) - s[0] * (r[1].0 - r[2].0) + (r[1].0 * s[2] - r[2].0 * s[1])) / det;
            let c = (r[0].0 * (r[1].1 * s[2] - r[2].1 * s[1]) - r[0].1 * (r[1].0 * s[2] - r[2].0 * s[1])
                + s[0] * (r[1].0 * r[2].1 - r[2].0 * r[1].1))
                / det;
            [a, b, c]
        };
        let x = solve(screen.map(|p| p.0 as f32));
        let y = solve(screen.map(|p| p.1 as f32));
        Some(TouchCal { a: [x[0], x[1], x[2], y[0], y[1], y[2]] })
    }

    pub fn map(&self, raw_x: u16, raw_y: u16) -> (i32, i32) {
        let (rx, ry) = (raw_x as f32, raw_y as f32);
        let a = &self.a;
        ((a[0] * rx + a[1] * ry + a[2]).round() as i32, (a[3] * rx + a[4] * ry + a[5]).round() as i32)
    }

    pub fn to_bytes(&self) -> [u8; 24] {
        let mut out = [0u8; 24];
        for (i, v) in self.a.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<TouchCal> {
        if b.len() != 24 {
            return None;
        }
        let mut a = [0f32; 6];
        for (i, v) in a.iter_mut().enumerate() {
            *v = f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().ok()?);
        }
        a.iter().all(|v| v.is_finite()).then_some(TouchCal { a })
    }
}

/// Где показать перекрестья калибровки (экран 320×240).
pub const CAL_POINTS: [(i32, i32); 3] = [(24, 24), (296, 24), (24, 216)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values() {
        assert_eq!(parse12(0x7F, 0xF8), 0x0FFF);
        assert_eq!(parse12(0x40, 0x00), 0x0800);
        assert_eq!(median(&mut [5, 900, 7]), 7);
    }

    #[test]
    fn calibration_handles_swapped_and_flipped_axes() {
        // Тач повёрнут: сырой Y растёт вправо по экрану, сырой X — вверх.
        let raw_of = |x: i32, y: i32| -> (u16, u16) { ((3900 - y * 15) as u16, (250 + x * 11) as u16) };
        let raw = CAL_POINTS.map(|(x, y)| raw_of(x, y));
        let cal = TouchCal::from_points(CAL_POINTS, raw).unwrap();
        for (x, y) in [(160, 120), (0, 0), (319, 239), (50, 200)] {
            let (rx, ry) = raw_of(x, y);
            assert_eq!(cal.map(rx, ry), (x, y));
        }
        assert_eq!(TouchCal::from_bytes(&cal.to_bytes()), Some(cal));
        assert_eq!(TouchCal::from_bytes(&[0; 3]), None);
    }

    #[test]
    fn rejects_degenerate_points() {
        assert!(TouchCal::from_points(CAL_POINTS, [(100, 100), (100, 100), (100, 100)]).is_none());
        let def = TouchCal::default();
        assert_eq!(def.map(200, 200), (0, 0));
        assert_eq!(def.map(3900, 3900), (320, 240));
    }
}
