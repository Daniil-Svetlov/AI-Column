//! Тачскрин XPT2046 (сырые значения + калибровка из column_core::touch).
use column_core::touch::{CMD_X, CMD_Y, CMD_Z1, CMD_Z2, PRESSURE_MIN, TouchCal, median, parse12};
use esp_idf_svc::hal::gpio::{AnyIOPin, Input, PinDriver, Pull};

use crate::display::SpiDev;

pub struct Xpt2046 {
    dev: SpiDev,
    irq: PinDriver<'static, Input>,
    pub cal: TouchCal,
}

impl Xpt2046 {
    pub fn new(dev: SpiDev, irq: AnyIOPin<'static>) -> anyhow::Result<Xpt2046> {
        let mut t = Xpt2046 { dev, irq: PinDriver::input(irq, Pull::Up)?, cal: TouchCal::default() };
        t.cmd(0x80); // режим с включённым PENIRQ
        Ok(t)
    }

    fn cmd(&mut self, c: u8) -> u16 {
        let mut buf = [c, 0, 0];
        match self.dev.transfer_in_place(&mut buf) {
            Ok(()) => parse12(buf[1], buf[2]),
            Err(_) => 0,
        }
    }

    /// Сырые координаты и «давление», если экрана касаются.
    pub fn raw(&mut self) -> Option<(u16, u16, i32)> {
        if self.irq.is_high() {
            return None; // PENIRQ активен низким уровнем
        }
        let z1 = self.cmd(CMD_Z1) as i32;
        let z2 = self.cmd(CMD_Z2) as i32;
        let z = z1 + 4095 - z2;
        if z < PRESSURE_MIN {
            self.cmd(0x80);
            return None;
        }
        let mut xs = [0u16; 5];
        let mut ys = [0u16; 5];
        for i in 0..5 {
            xs[i] = self.cmd(CMD_X);
            ys[i] = self.cmd(CMD_Y);
        }
        self.cmd(0x80); // снова включить PENIRQ
        Some((median(&mut xs), median(&mut ys), z))
    }

    /// Точка касания на экране 320×240.
    pub fn point(&mut self) -> Option<(i32, i32)> {
        let (x, y, _) = self.raw()?;
        let (sx, sy) = self.cal.map(x, y);
        Some((sx.clamp(0, 319), sy.clamp(0, 239)))
    }
}
