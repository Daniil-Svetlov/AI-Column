//! Экран ILI9341 320×240 (mipidsi) и тач XPT2046 на общей шине SPI2, подсветка — ШИМ (LEDC).
use column_core::config::*;
use esp_idf_svc::hal::delay::Ets;
use esp_idf_svc::hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::ledc::{CHANNEL0, LedcDriver, LedcTimerDriver, TIMER0, config::TimerConfig};
use esp_idf_svc::hal::spi::config::{Config as SpiConfig, DriverConfig, MODE_0};
use esp_idf_svc::hal::spi::{Dma, SPI2, SpiDeviceDriver, SpiDriver};
use esp_idf_svc::hal::units::FromValueType;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation, Rotation};

use crate::board::pin;
use crate::touch::Xpt2046;

pub type SpiDev = SpiDeviceDriver<'static, &'static SpiDriver<'static>>;
pub type Lcd = mipidsi::Display<SpiInterface<'static, SpiDev, PinDriver<'static, Output>>, ILI9341Rgb565, PinDriver<'static, Output>>;

pub struct Screen {
    pub lcd: Lcd,
    pub touch: Xpt2046,
    pub backlight: Backlight,
}

/// Подсветка экрана (ШИМ).
pub struct Backlight(LedcDriver<'static>);

impl Backlight {
    /// Яркость 0..255.
    pub fn set(&mut self, level: u8) {
        let max = self.0.get_max_duty();
        let _ = self.0.set_duty(max * level as u32 / 255);
    }
}

impl Screen {
    pub fn new(spi2: SPI2<'static>, timer: TIMER0<'static>, channel: CHANNEL0<'static>) -> anyhow::Result<Screen> {
        let bus: &'static SpiDriver<'static> = Box::leak(Box::new(SpiDriver::new(
            spi2,
            pin(PIN_TFT_SCK),
            pin(PIN_TFT_MOSI),
            Some(pin(PIN_TFT_MISO)),
            &DriverConfig::new().dma(Dma::Auto(4096)),
        )?));
        let lcd_dev = SpiDeviceDriver::new(bus, Some(pin(PIN_TFT_CS)), &SpiConfig::new().baudrate(40.MHz().into()).data_mode(MODE_0))?;
        let touch_dev = SpiDeviceDriver::new(bus, Some(pin(PIN_TOUCH_CS)), &SpiConfig::new().baudrate(2.MHz().into()).data_mode(MODE_0))?;

        let dc = PinDriver::output(pin(PIN_TFT_DC))?;
        let rst = PinDriver::output(pin(PIN_TFT_RST))?;
        let buffer: &'static mut [u8] = Box::leak(vec![0u8; 4096].into_boxed_slice());
        let di = SpiInterface::new(lcd_dev, dc, buffer);
        let lcd = mipidsi::Builder::new(ILI9341Rgb565, di)
            .reset_pin(rst)
            .color_order(ColorOrder::Bgr)
            .orientation(Orientation::new().rotate(Rotation::Deg90).flip_horizontal())
            .init(&mut Ets)
            .map_err(|e| anyhow::anyhow!("экран не запустился: {e:?}"))?;

        let timer = LedcTimerDriver::new(timer, &TimerConfig::new().frequency(5.kHz().into()))?;
        let mut backlight = Backlight(LedcDriver::new(channel, timer, pin(PIN_TFT_LED))?);
        backlight.set(200);

        let touch = Xpt2046::new(touch_dev, pin(PIN_TOUCH_IRQ))?;
        Ok(Screen { lcd, touch, backlight })
    }
}
