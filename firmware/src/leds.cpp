#include "leds.h"

#include <Adafruit_NeoPixel.h>
#include <math.h>

#include "config.h"
#include "dsp.h"

namespace leds {
namespace {

enum class Override : uint8_t { None, Off, Solid, Breathe, Rainbow };

Adafruit_NeoPixel strip(LED_COUNT, PIN_LED_DATA, NEO_GRB + NEO_KHZ800);
Mode mode_ = Mode::Boot;
uint32_t accent_ = 0x3C64FF;
Override ov_ = Override::None;
uint32_t ovColor_ = 0xFFB060;
uint8_t ovBright_ = 100;
uint32_t alertUntil_ = 0;
uint32_t alertColor_ = 0xFF7000;
uint32_t lastFrame_ = 0;
uint32_t modeSince_ = 0;
float level_ = 0;

uint32_t scale(uint32_t rgb, float k) {
  if (k <= 0) return 0;
  if (k > 1) k = 1;
  uint8_t r = ((rgb >> 16) & 0xFF) * k, g = ((rgb >> 8) & 0xFF) * k, b = (rgb & 0xFF) * k;
  return Adafruit_NeoPixel::Color(r, g, b);
}

void px(int x, int y, uint32_t c) {
  if (x < 0 || y < 0 || x >= LED_MATRIX_W || y >= LED_MATRIX_H) return;
  strip.setPixelColor(dsp::ledIndex(x, y, LED_MATRIX_W, LED_SERPENTINE), strip.gamma32(c));
}

void fill(uint32_t c) {
  for (int y = 0; y < LED_MATRIX_H; y++)
    for (int x = 0; x < LED_MATRIX_W; x++) px(x, y, c);
}

float dist(int x, int y) {
  float dx = x - (LED_MATRIX_W - 1) / 2.0f, dy = y - (LED_MATRIX_H - 1) / 2.0f;
  return sqrtf(dx * dx + dy * dy);
}

// Уровень 0..1 в «логарифмическом» виде: −50 дБ → 0, −10 дБ → 1.
float toUnit(float rms) {
  if (rms < 1e-5f) return 0;
  float u = (20.f * log10f(rms) + 50.f) / 40.f;
  return u < 0 ? 0 : (u > 1 ? 1 : u);
}

// Пиксели периметра по кругу.
void perimeter(int i, int& x, int& y) {
  const int w = LED_MATRIX_W, h = LED_MATRIX_H;
  const int n = 2 * (w + h) - 4;
  i = ((i % n) + n) % n;
  if (i < w) { x = i; y = 0; return; }
  i -= w;
  if (i < h - 1) { x = w - 1; y = i + 1; return; }
  i -= h - 1;
  if (i < w - 1) { x = w - 2 - i; y = h - 1; return; }
  i -= w - 1;
  x = 0;
  y = h - 2 - i;
}

void rainbow(uint32_t t, float bright) {
  for (int y = 0; y < LED_MATRIX_H; y++)
    for (int x = 0; x < LED_MATRIX_W; x++) {
      uint16_t hue = (t * 12 + (x + y) * 4096) & 0xFFFF;
      px(x, y, scale(Adafruit_NeoPixel::ColorHSV(hue), bright));
    }
}

void breathe(uint32_t c, uint32_t t, float lo, float hi, float periodMs) {
  float k = lo + (hi - lo) * (0.5f - 0.5f * cosf(2 * PI * (t % (uint32_t)periodMs) / periodMs));
  fill(scale(c, k));
}

}  // namespace

void begin() {
  strip.begin();
  strip.setBrightness(LED_MAX_BRIGHTNESS);
  strip.clear();
  strip.show();
  modeSince_ = millis();
}

void setMode(Mode m) {
  if (m != mode_) modeSince_ = millis();
  mode_ = m;
}

void setAccent(uint32_t rgb) { accent_ = rgb ? rgb : 0x3C64FF; }

void setOverride(const char* mode, uint32_t rgb, uint8_t brightnessPct) {
  if (!mode || !strcmp(mode, "auto")) ov_ = Override::None;
  else if (!strcmp(mode, "off")) ov_ = Override::Off;
  else if (!strcmp(mode, "solid")) ov_ = Override::Solid;
  else if (!strcmp(mode, "breathe")) ov_ = Override::Breathe;
  else if (!strcmp(mode, "rainbow")) ov_ = Override::Rainbow;
  if (rgb) ovColor_ = rgb;
  if (brightnessPct) ovBright_ = brightnessPct > 100 ? 100 : brightnessPct;
}

void flashAlert(uint32_t rgb, uint32_t ms) {
  alertColor_ = rgb;
  alertUntil_ = millis() + ms;
}

void loop(float micLevel, float outLevel) {
  uint32_t now = millis();
  if (now - lastFrame_ < 33) return;
  lastFrame_ = now;
  uint32_t t = now - modeSince_;
  strip.setBrightness(ov_ == Override::None ? LED_MAX_BRIGHTNESS : (uint8_t)(LED_MAX_BRIGHTNESS * ovBright_ / 100));
  strip.clear();

  if (now < alertUntil_) {
    if ((now / 250) % 2) fill(alertColor_);
  } else if (mode_ == Mode::Sleep) {
    // всё выключено
  } else if (mode_ == Mode::Error) {
    if ((t / 160) % 2 == 0 && t < 1500) fill(0xFF0000);
  } else if (mode_ == Mode::Boot) {
    rainbow(now, 0.6f);
  } else if (mode_ == Mode::Offline) {
    float k = (now / 600) % 2 ? 0.35f : 0.05f;
    for (int y = 3; y <= 4; y++)
      for (int x = 3; x <= 4; x++) px(x, y, scale(0xFF2000, k));
  } else if (mode_ == Mode::Listening) {
    level_ = level_ * 0.6f + toUnit(micLevel) * 0.4f;
    float r = 0.8f + level_ * 4.2f;
    for (int y = 0; y < LED_MATRIX_H; y++)
      for (int x = 0; x < LED_MATRIX_W; x++) {
        float d = dist(x, y);
        if (d <= r) px(x, y, scale(accent_, 0.25f + 0.75f * (1 - d / (r + 0.01f))));
      }
  } else if (mode_ == Mode::Thinking) {
    const int n = 2 * (LED_MATRIX_W + LED_MATRIX_H) - 4;
    int head = (t / 40) % n;
    for (int k = 0; k < 9; k++) {
      int x, y;
      perimeter(head - k, x, y);
      px(x, y, scale(accent_, 1.0f - k / 9.0f));
    }
  } else if (mode_ == Mode::Speaking) {
    level_ = level_ * 0.55f + toUnit(outLevel * 0.5f) * 0.45f;
    for (int y = 0; y < LED_MATRIX_H; y++)
      for (int x = 0; x < LED_MATRIX_W; x++) {
        float d = dist(x, y) / 5.0f;
        float k = level_ * (1.0f - d) + 0.06f;
        px(x, y, scale(accent_, k));
      }
  } else if (ov_ == Override::Off) {
    // подсветку попросили выключить
  } else if (ov_ == Override::Solid) {
    fill(ovColor_);
  } else if (ov_ == Override::Breathe) {
    breathe(ovColor_, now, 0.15f, 1.0f, 4000);
  } else if (ov_ == Override::Rainbow) {
    rainbow(now, 1.0f);
  } else {  // Idle
    breathe(accent_, now, 0.02f, 0.18f, 5000);
  }
  strip.show();
}

}  // namespace leds
