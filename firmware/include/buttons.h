// Кнопки с антидребезгом: нажатие, отпускание (с длительностью) и долгое удержание.
#pragma once
#include <Arduino.h>

class Button {
 public:
  enum Event : uint8_t { kNone, kPressed, kReleased, kLongPress };

  explicit Button(uint8_t pin, uint16_t longMs = 800) : pin_(pin), longMs_(longMs) {}

  void begin() {
    pinMode(pin_, INPUT_PULLUP);
    stable_ = raw_ = digitalRead(pin_) == LOW;
  }

  // Вызывать часто (в loop). Возвращает одно событие за вызов.
  Event poll() {
    bool now = digitalRead(pin_) == LOW;
    uint32_t t = millis();
    if (now != raw_) {
      raw_ = now;
      changedAt_ = t;
    }
    if (stable_ != raw_ && t - changedAt_ >= 25) {
      stable_ = raw_;
      if (stable_) {
        pressedAt_ = t;
        longFired_ = false;
        return kPressed;
      }
      lastDuration_ = t - pressedAt_;
      return longFired_ ? kNone : kReleased;
    }
    if (stable_ && !longFired_ && t - pressedAt_ >= longMs_) {
      longFired_ = true;
      return kLongPress;
    }
    return kNone;
  }

  bool isDown() const { return stable_; }
  uint32_t heldMs() const { return stable_ ? millis() - pressedAt_ : 0; }
  uint32_t lastDuration() const { return lastDuration_; }
  void setLongMs(uint16_t ms) { longMs_ = ms; }

 private:
  uint8_t pin_;
  uint16_t longMs_;
  bool raw_ = false, stable_ = false, longFired_ = false;
  uint32_t changedAt_ = 0, pressedAt_ = 0, lastDuration_ = 0;
};
