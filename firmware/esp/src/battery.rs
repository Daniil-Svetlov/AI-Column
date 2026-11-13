//! Напряжение аккумулятора 3S через делитель 100k/22k на GPIO1 (ADC1).
use column_core::config::*;
use column_core::dsp::battery_percent;
use esp_idf_svc::hal::adc::{ADC1, ADCCH0, ADCU1};
use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::config::{AdcChannelConfig, Calibration};
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::gpio::Gpio1;

pub struct Battery {
    ch: AdcChannelDriver<'static, ADCCH0<ADCU1>, &'static AdcDriver<'static, ADCU1>>,
    volts: f32,
    last_read: u32,
    critical_since: Option<u32>,
}

impl Battery {
    pub fn new(adc1: ADC1<'static>, pin: Gpio1<'static>) -> anyhow::Result<Battery> {
        let adc: &'static AdcDriver<'static, ADCU1> = Box::leak(Box::new(AdcDriver::new(adc1)?));
        let cfg = AdcChannelConfig { attenuation: DB_12, calibration: Calibration::Curve, ..Default::default() };
        let ch = AdcChannelDriver::new(adc, pin, &cfg)?;
        let mut b = Battery { ch, volts: 0.0, last_read: 0, critical_since: None };
        b.volts = b.sample();
        Ok(b)
    }

    fn sample(&mut self) -> f32 {
        let mut acc = 0u32;
        let mut n = 0;
        for _ in 0..16 {
            if let Ok(mv) = self.ch.read() {
                acc += mv as u32;
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { acc as f32 / n as f32 / 1000.0 * BAT_DIVIDER_RATIO }
    }

    /// Раз в ~0,5 с обновляет показания.
    pub fn update(&mut self, now: u32) {
        if now.wrapping_sub(self.last_read) < 500 {
            return;
        }
        self.last_read = now;
        let v = self.sample();
        self.volts = self.volts * 0.8 + v * 0.2; // сглаживаем просадки от усилителя
        if self.present() && self.volts < BAT_CRITICAL_V {
            self.critical_since.get_or_insert(now);
        } else {
            self.critical_since = None;
        }
    }

    /// Есть ли АКБ/делитель (иначе питание от USB).
    pub fn present(&self) -> bool {
        self.volts >= BAT_PRESENT_MIN_V
    }
    pub fn volts(&self) -> f32 {
        self.volts
    }
    pub fn percent(&self) -> i32 {
        if self.present() { battery_percent(self.volts, BAT_CELLS) } else { -1 }
    }
    pub fn low(&self) -> bool {
        self.present() && self.volts < BAT_LOW_V
    }
    /// Держится ниже критического порога дольше 30 с.
    pub fn critical(&self, now: u32) -> bool {
        self.critical_since.is_some_and(|t| now.wrapping_sub(t) > 30000)
    }
}
