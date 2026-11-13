//! Кнопка с антидребезгом: нажатие, отпускание (с длительностью) и долгое удержание.
//! Железо передаёт «нажата ли сейчас» и время в мс — логика тестируется на ПК.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEvent {
    None,
    Pressed,
    Released,
    LongPress,
}

#[derive(Clone, Debug)]
pub struct Button {
    long_ms: u32,
    raw: bool,
    stable: bool,
    long_fired: bool,
    changed_at: u32,
    pressed_at: u32,
    last_duration: u32,
}

impl Button {
    pub fn new(long_ms: u32, down_now: bool) -> Button {
        Button { long_ms, raw: down_now, stable: down_now, long_fired: down_now, changed_at: 0, pressed_at: 0, last_duration: 0 }
    }

    /// Вызывать часто. Возвращает одно событие за вызов.
    pub fn poll(&mut self, down: bool, now: u32) -> ButtonEvent {
        if down != self.raw {
            self.raw = down;
            self.changed_at = now;
        }
        if self.stable != self.raw && now.wrapping_sub(self.changed_at) >= 25 {
            self.stable = self.raw;
            if self.stable {
                self.pressed_at = now;
                self.long_fired = false;
                return ButtonEvent::Pressed;
            }
            self.last_duration = now.wrapping_sub(self.pressed_at);
            return if self.long_fired { ButtonEvent::None } else { ButtonEvent::Released };
        }
        if self.stable && !self.long_fired && now.wrapping_sub(self.pressed_at) >= self.long_ms {
            self.long_fired = true;
            return ButtonEvent::LongPress;
        }
        ButtonEvent::None
    }

    pub fn is_down(&self) -> bool {
        self.stable
    }
    pub fn held_ms(&self, now: u32) -> u32 {
        if self.stable { now.wrapping_sub(self.pressed_at) } else { 0 }
    }
    pub fn last_duration(&self) -> u32 {
        self.last_duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_press_release_long() {
        let mut b = Button::new(800, false);
        assert_eq!(b.poll(true, 100), ButtonEvent::None);
        assert_eq!(b.poll(false, 110), ButtonEvent::None); // дребезг
        assert_eq!(b.poll(true, 115), ButtonEvent::None);
        assert_eq!(b.poll(true, 141), ButtonEvent::Pressed);
        assert_eq!(b.held_ms(341), 200);
        assert_eq!(b.poll(false, 400), ButtonEvent::None);
        assert_eq!(b.poll(false, 426), ButtonEvent::Released);
        assert_eq!(b.last_duration(), 285);

        assert_eq!(b.poll(true, 1000), ButtonEvent::None);
        assert_eq!(b.poll(true, 1030), ButtonEvent::Pressed);
        assert_eq!(b.poll(true, 1830), ButtonEvent::LongPress);
        assert_eq!(b.poll(true, 1900), ButtonEvent::None);
        assert_eq!(b.poll(false, 2000), ButtonEvent::None);
        assert_eq!(b.poll(false, 2030), ButtonEvent::None, "после долгого удержания отпускание не событие");
    }

    #[test]
    fn held_at_boot_is_not_a_press() {
        let mut b = Button::new(800, true);
        assert!(b.is_down());
        assert_eq!(b.poll(true, 5000), ButtonEvent::None);
        assert_eq!(b.poll(false, 5100), ButtonEvent::None);
        assert_eq!(b.poll(false, 5130), ButtonEvent::None);
    }
}
