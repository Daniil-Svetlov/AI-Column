#include "battery.h"

#include "config.h"
#include "dsp.h"

namespace battery {
namespace {
float volts_ = 0;
uint32_t lastRead_ = 0;
uint32_t criticalSince_ = 0;

float sample() {
  uint32_t acc = 0;
  for (int i = 0; i < 16; i++) acc += analogReadMilliVolts(PIN_BAT_ADC);
  return (acc / 16.0f) / 1000.0f * BAT_DIVIDER_RATIO;
}
}  // namespace

void begin() {
  analogReadResolution(12);
  analogSetPinAttenuation(PIN_BAT_ADC, ADC_11db);
  volts_ = sample();
}

void loop() {
  uint32_t now = millis();
  if (now - lastRead_ < 500) return;
  lastRead_ = now;
  float v = sample();
  volts_ = volts_ * 0.8f + v * 0.2f;  // сглаживаем просадки от усилителя
  if (present() && volts_ < BAT_CRITICAL_V) {
    if (!criticalSince_) criticalSince_ = now;
  } else {
    criticalSince_ = 0;
  }
}

bool present() { return volts_ >= BAT_PRESENT_MIN_V; }
float volts() { return volts_; }
int percent() { return present() ? dsp::batteryPercent(volts_, BAT_CELLS) : -1; }
bool low() { return present() && volts_ < BAT_LOW_V; }
bool critical() { return criticalSince_ && millis() - criticalSince_ > 30000; }

}  // namespace battery
