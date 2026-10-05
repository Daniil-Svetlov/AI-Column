// Подсветка: матрица WS2812B 8×8. Эффекты по состоянию колонки + ручной режим от агентов.
#pragma once
#include <Arduino.h>

namespace leds {

enum class Mode : uint8_t { Boot, Offline, Idle, Listening, Thinking, Speaking, Error, Alert, Sleep };

void begin();
void setMode(Mode m);
void setAccent(uint32_t rgb);     // цвет текущего агента
// Ручной режим от сервера (инструмент set_lights): off | solid | breathe | rainbow | auto
void setOverride(const char* mode, uint32_t rgb, uint8_t brightnessPct);
void flashAlert(uint32_t rgb, uint32_t ms);
void loop(float micLevel, float outLevel);  // ~30 кадров/с

}  // namespace leds
