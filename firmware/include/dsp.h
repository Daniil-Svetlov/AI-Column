// Обработка звука и прочая «чистая» математика — без Arduino, чтобы тестировать на ПК (env:native).
#pragma once
#include <cmath>
#include <cstddef>
#include <cstdint>

namespace dsp {

inline int16_t clamp16(int32_t v) {
  return v > 32767 ? 32767 : (v < -32768 ? -32768 : static_cast<int16_t>(v));
}

// Фильтр постоянной составляющей (ФВЧ ~ 30–80 Гц): y = x - x1 + R*y1
struct DcBlocker {
  float r = 0.995f;
  float x1 = 0, y1 = 0;
  float process(float x) {
    float y = x - x1 + r * y1;
    x1 = x;
    y1 = y;
    return y;
  }
};

// Кадры I2S от INMP441: 32-битные слоты L,R; полезные 24 бита в старших разрядах.
// channels: 0 — среднее двух микрофонов, 1 — левый, 2 — правый.
// Возвращает RMS блока (0..1).
inline float micFramesToPcm16(const int32_t* stereo, size_t frames, int16_t* out, int shift, int channels,
                              DcBlocker& dc) {
  double acc = 0;
  for (size_t i = 0; i < frames; i++) {
    int32_t l = stereo[2 * i] >> shift;
    int32_t r = stereo[2 * i + 1] >> shift;
    int32_t m = channels == 1 ? l : (channels == 2 ? r : (l + r) / 2);
    float y = dc.process(static_cast<float>(m));
    int16_t s = clamp16(static_cast<int32_t>(lrintf(y)));
    out[i] = s;
    acc += static_cast<double>(s) * s;
  }
  return frames ? static_cast<float>(std::sqrt(acc / frames) / 32768.0) : 0.f;
}

// Громкость 0..100 → линейный коэффициент: перцептивная кривая + потолок maxGainDb.
inline float volumeToGain(int volume, float maxGainDb) {
  if (volume <= 0) return 0.f;
  if (volume > 100) volume = 100;
  float db = maxGainDb - 40.f * (1.f - volume / 100.f);  // 100% → maxGainDb, 1% → ≈ maxGainDb-40
  return std::pow(10.f, db / 20.f);
}

// Моно PCM16 → стерео PCM16 с усилением. Возвращает пиковый уровень (0..1).
inline float monoToStereo(const int16_t* in, size_t n, int16_t* outStereo, float gain) {
  int32_t peak = 0;
  for (size_t i = 0; i < n; i++) {
    int16_t s = clamp16(static_cast<int32_t>(lrintf(in[i] * gain)));
    outStereo[2 * i] = s;
    outStereo[2 * i + 1] = s;
    int32_t a = s < 0 ? -static_cast<int32_t>(s) : s;
    if (a > peak) peak = a;
  }
  return peak / 32768.f;
}

// Простой энергетический детектор речи с адаптивным уровнем шума.
class Vad {
 public:
  enum Event { kNone, kSpeechStart, kSpeechEnd, kNoSpeech };

  uint32_t endSilenceMs = 900;
  uint32_t noSpeechMs = 5000;
  uint32_t minSpeechMs = 250;
  float startFactor = 3.0f;  // во сколько раз громче шума — начало речи
  float stopFactor = 1.8f;   // ниже — тишина
  float minLevel = 0.003f;   // абсолютный порог (≈ -50 dBFS)

  void reset(float noiseFloor) {
    noise_ = noiseFloor > 0.0005f ? noiseFloor : 0.0005f;
    speaking_ = false;
    heardSpeech_ = false;
    speechMs_ = silenceMs_ = totalMs_ = totalSpeechMs_ = 0;
  }

  Event process(float rms, uint32_t frameMs) {
    totalMs_ += frameMs;
    float on = std::fmax(noise_ * startFactor, minLevel);
    float off = std::fmax(noise_ * stopFactor, minLevel * 0.7f);
    if (!speaking_) {
      if (rms > on) {
        speechMs_ += frameMs;
        if (speechMs_ >= 60) {  // защита от щелчков
          speaking_ = true;
          silenceMs_ = 0;
          if (!heardSpeech_) {
            heardSpeech_ = true;
            return kSpeechStart;
          }
        }
      } else {
        speechMs_ = 0;
        noise_ = noise_ * 0.97f + rms * 0.03f;  // подстраиваемся под фон только в тишине
        if (heardSpeech_) {
          silenceMs_ += frameMs;
          if (silenceMs_ >= endSilenceMs && totalSpeechMs_ >= minSpeechMs) return kSpeechEnd;
        } else if (totalMs_ >= noSpeechMs) {
          return kNoSpeech;
        }
      }
    } else {
      totalSpeechMs_ += frameMs;
      if (rms < off) {
        silenceMs_ += frameMs;
        if (silenceMs_ >= 150) speaking_ = false;  // короткие паузы между словами — ещё речь
      } else {
        silenceMs_ = 0;
      }
    }
    return kNone;
  }

  bool heardSpeech() const { return heardSpeech_; }
  bool speaking() const { return speaking_; }
  float noise() const { return noise_; }

 private:
  float noise_ = 0.002f;
  bool speaking_ = false;
  bool heardSpeech_ = false;
  uint32_t speechMs_ = 0, silenceMs_ = 0, totalMs_ = 0, totalSpeechMs_ = 0;
};

// Напряжение 3S Li-ion → проценты (по типичной разрядной кривой ячейки).
inline int batteryPercent(float packVolts, int cells) {
  static const float v[] = {3.00f, 3.30f, 3.40f, 3.50f, 3.60f, 3.70f, 3.80f, 3.90f, 4.00f, 4.10f, 4.20f};
  static const float p[] = {0, 3, 6, 12, 22, 38, 52, 65, 78, 90, 100};
  float c = packVolts / cells;
  if (c <= v[0]) return 0;
  if (c >= v[10]) return 100;
  for (int i = 1; i <= 10; i++) {
    if (c <= v[i]) {
      float t = (c - v[i - 1]) / (v[i] - v[i - 1]);
      return static_cast<int>(lrintf(p[i - 1] + t * (p[i] - p[i - 1])));
    }
  }
  return 100;
}

// Перевод координат матрицы (x,y) в номер светодиода.
inline int ledIndex(int x, int y, int w, bool serpentine) {
  if (serpentine && (y & 1)) return y * w + (w - 1 - x);
  return y * w + x;
}

}  // namespace dsp
