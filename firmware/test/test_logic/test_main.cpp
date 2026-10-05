// Тесты чистой логики прошивки на компьютере: pio test -e native
#include <unity.h>

#include <cmath>
#include <cstring>
#include <vector>

#include "dsp.h"
#include "ring_buffer.h"

void setUp() {}
void tearDown() {}

static void test_ring_wraps_and_keeps_order() {
  std::vector<uint8_t> storage(16);
  RingBuffer rb(storage.data(), storage.size());
  TEST_ASSERT_EQUAL(15, rb.capacity());
  uint8_t in[10], out[10];
  for (int i = 0; i < 10; i++) in[i] = i;
  TEST_ASSERT_EQUAL(10, rb.write(in, 10));
  TEST_ASSERT_EQUAL(6, rb.read(out, 6));
  TEST_ASSERT_EQUAL(10, rb.write(in, 10));  // перешли через конец буфера
  TEST_ASSERT_EQUAL(14, rb.available());
  uint8_t all[14];
  TEST_ASSERT_EQUAL(14, rb.read(all, 14));
  const uint8_t expect[14] = {6, 7, 8, 9, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9};
  TEST_ASSERT_EQUAL_UINT8_ARRAY(expect, all, 14);
}

static void test_ring_never_overflows() {
  std::vector<uint8_t> storage(8);
  RingBuffer rb(storage.data(), storage.size());
  uint8_t in[20] = {0};
  TEST_ASSERT_EQUAL(7, rb.write(in, 20));
  TEST_ASSERT_EQUAL(0, rb.freeSpace());
  rb.clear();
  TEST_ASSERT_EQUAL(0, rb.available());
}

static void test_mic_conversion_mix_and_gain() {
  // 24-битный сэмпл 0x100000 в старших битах 32-битного слота
  int32_t frames[4] = {0x100000 << 8, 0x100000 << 8, -(0x100000 << 8), -(0x100000 << 8)};
  int16_t out[2];
  dsp::DcBlocker dc;
  dc.r = 0.0f;  // без ФВЧ — проверяем чистое преобразование
  float rms = dsp::micFramesToPcm16(frames, 2, out, 16, 0, dc);
  // >>16: 0x10000000 >> 16 = 4096; второй кадр после DC-фильтра с r=0: x - x1 = -4096 - 4096
  TEST_ASSERT_EQUAL_INT16(4096, out[0]);
  TEST_ASSERT_EQUAL_INT16(-8192, out[1]);
  TEST_ASSERT_TRUE(rms > 0.1f);
}

static void test_mic_single_channel_select() {
  int32_t frames[2] = {1000 << 16, 3000 << 16};
  int16_t out[1];
  dsp::DcBlocker dc;
  dc.r = 0.0f;
  dsp::micFramesToPcm16(frames, 1, out, 16, 2, dc);
  TEST_ASSERT_EQUAL_INT16(3000, out[0]);
}

static void test_volume_curve() {
  TEST_ASSERT_EQUAL_FLOAT(0.0f, dsp::volumeToGain(0, -12.f));
  float g100 = dsp::volumeToGain(100, -12.f);
  TEST_ASSERT_FLOAT_WITHIN(0.002f, 0.2512f, g100);  // -12 дБ
  TEST_ASSERT_TRUE(dsp::volumeToGain(50, -12.f) < g100);
  TEST_ASSERT_TRUE(dsp::volumeToGain(50, -12.f) > dsp::volumeToGain(10, -12.f));
  TEST_ASSERT_EQUAL_FLOAT(g100, dsp::volumeToGain(250, -12.f));
}

static void test_mono_to_stereo_clamps() {
  int16_t in[2] = {20000, -20000};
  int16_t out[4];
  float peak = dsp::monoToStereo(in, 2, out, 2.0f);
  TEST_ASSERT_EQUAL_INT16(32767, out[0]);
  TEST_ASSERT_EQUAL_INT16(32767, out[1]);
  TEST_ASSERT_EQUAL_INT16(-32768, out[2]);
  TEST_ASSERT_TRUE(peak > 0.99f);
}

static void test_vad_detects_phrase_end() {
  dsp::Vad vad;
  vad.endSilenceMs = 600;
  vad.minSpeechMs = 200;
  vad.reset(0.002f);
  int ev = dsp::Vad::kNone;
  for (int i = 0; i < 10; i++) ev = vad.process(0.002f, 16);  // тишина
  TEST_ASSERT_EQUAL(dsp::Vad::kNone, ev);
  bool started = false;
  for (int i = 0; i < 40; i++) started |= vad.process(0.05f, 16) == dsp::Vad::kSpeechStart;  // ~0,6 с речи
  TEST_ASSERT_TRUE(started);
  bool ended = false;
  for (int i = 0; i < 60 && !ended; i++) ended = vad.process(0.002f, 16) == dsp::Vad::kSpeechEnd;
  TEST_ASSERT_TRUE(ended);
}

static void test_vad_no_speech_timeout() {
  dsp::Vad vad;
  vad.noSpeechMs = 1000;
  vad.reset(0.002f);
  int ev = dsp::Vad::kNone;
  for (int i = 0; i < 80 && ev == dsp::Vad::kNone; i++) ev = vad.process(0.0025f, 16);
  TEST_ASSERT_EQUAL(dsp::Vad::kNoSpeech, ev);
}

static void test_vad_ignores_short_click() {
  dsp::Vad vad;
  vad.reset(0.002f);
  TEST_ASSERT_EQUAL(dsp::Vad::kNone, vad.process(0.2f, 16));  // одиночный щелчок
  TEST_ASSERT_FALSE(vad.heardSpeech());
}

static void test_battery_percent() {
  TEST_ASSERT_EQUAL(100, dsp::batteryPercent(12.6f, 3));
  TEST_ASSERT_EQUAL(0, dsp::batteryPercent(9.0f, 3));
  int mid = dsp::batteryPercent(11.1f, 3);
  TEST_ASSERT_TRUE(mid > 30 && mid < 45);
}

static void test_led_index() {
  TEST_ASSERT_EQUAL(9, dsp::ledIndex(1, 1, 8, false));
  TEST_ASSERT_EQUAL(14, dsp::ledIndex(1, 1, 8, true));
  TEST_ASSERT_EQUAL(16, dsp::ledIndex(0, 2, 8, true));
}

int main() {
  UNITY_BEGIN();
  RUN_TEST(test_ring_wraps_and_keeps_order);
  RUN_TEST(test_ring_never_overflows);
  RUN_TEST(test_mic_conversion_mix_and_gain);
  RUN_TEST(test_mic_single_channel_select);
  RUN_TEST(test_volume_curve);
  RUN_TEST(test_mono_to_stereo_clamps);
  RUN_TEST(test_vad_detects_phrase_end);
  RUN_TEST(test_vad_no_speech_timeout);
  RUN_TEST(test_vad_ignores_short_click);
  RUN_TEST(test_battery_percent);
  RUN_TEST(test_led_index);
  return UNITY_END();
}
