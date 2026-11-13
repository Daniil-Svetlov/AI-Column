//! Пины по схеме (номера — в column_core::config) и мелочи вроде времени и кнопок.
use esp_idf_svc::hal::gpio::{AnyIOPin, Input, PinDriver, Pull};

/// Пин по номеру из конфигурации.
///
/// # Safety
/// Каждый номер берём один раз при старте — пины не используются дважды.
pub fn pin(n: i32) -> AnyIOPin<'static> {
    unsafe { AnyIOPin::steal(n as u8) }
}

/// Миллисекунды с момента включения.
pub fn millis() -> u32 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u32
}

/// Кнопка на землю с внутренней подтяжкой: нажата = низкий уровень.
pub struct RawButton {
    pin: PinDriver<'static, Input>,
}

impl RawButton {
    pub fn new(n: i32) -> anyhow::Result<RawButton> {
        Ok(RawButton { pin: PinDriver::input(pin(n), Pull::Up)? })
    }
    pub fn is_down(&self) -> bool {
        self.pin.is_low()
    }
}
