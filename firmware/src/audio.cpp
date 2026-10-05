#include "audio.h"

#include <driver/i2s.h>
#include <esp_heap_caps.h>

#include <algorithm>

#include "config.h"
#include "dsp.h"
#include "ring_buffer.h"

namespace audio {
namespace {

constexpr i2s_port_t kMicPort = I2S_NUM_0;
constexpr i2s_port_t kDacPort = I2S_NUM_1;
constexpr size_t kMicBlock = 256;  // кадров: 16 мс при 16 кГц
constexpr size_t kOutBlock = 256;  // кадров: ~10,7 мс при 24 кГц

enum PlayState : uint8_t { kIdle, kBuffering, kPlaying };

RingBuffer micRing;
RingBuffer playRing;

volatile bool capturing_ = false;
volatile float micLevel_ = 0.f;
volatile float ambient_ = 0.002f;
volatile float lastRms_ = 0.f;
volatile uint32_t micFrames_ = 0;

volatile PlayState playState_ = kIdle;
volatile bool endRequested_ = false;
volatile bool stopRequested_ = false;
volatile uint32_t playRate_ = PLAY_SAMPLE_RATE;
volatile uint32_t requestedRate_ = PLAY_SAMPLE_RATE;
volatile float outLevel_ = 0.f;
volatile uint8_t volume_ = VOLUME_DEFAULT;
volatile float gain_ = 0.f;

uint8_t* allocBuffer(size_t bytes, size_t fallback) {
  uint8_t* p = static_cast<uint8_t*>(heap_caps_malloc(bytes, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT));
  if (p) return p;
  log_w("PSRAM недоступна, буфер %u байт в обычной памяти", (unsigned)fallback);
  return static_cast<uint8_t*>(malloc(fallback));
}

void micTask(void*) {
  static int32_t raw[kMicBlock * 2];
  static int16_t pcm[kMicBlock];
  dsp::DcBlocker dc;
  for (;;) {
    size_t bytes = 0;
    if (i2s_read(kMicPort, raw, sizeof(raw), &bytes, portMAX_DELAY) != ESP_OK || bytes == 0) continue;
    size_t frames = bytes / 8;
    float rms = dsp::micFramesToPcm16(raw, frames, pcm, MIC_SHIFT, MIC_CHANNELS, dc);
    lastRms_ = rms;
    micFrames_ = micFrames_ + 1;
    micLevel_ = micLevel_ * 0.8f + rms * 0.2f;
    if (!capturing_ && playState_ == kIdle) ambient_ = ambient_ * 0.98f + rms * 0.02f;
    if (capturing_) micRing.write(reinterpret_cast<uint8_t*>(pcm), frames * 2);
  }
}

void speakerTask(void*) {
  static int16_t mono[kOutBlock];
  static int16_t stereo[kOutBlock * 2];
  for (;;) {
    if (stopRequested_) {
      playRing.clear();
      playState_ = kIdle;
      endRequested_ = false;
      i2s_zero_dma_buffer(kDacPort);
      outLevel_ = 0;
      stopRequested_ = false;  // подтверждение для playbackBegin/Stop
    }
    if (requestedRate_ != playRate_) {
      i2s_set_sample_rates(kDacPort, requestedRate_);
      playRate_ = requestedRate_;
    }
    PlayState st = playState_;
    if (st == kIdle) {
      outLevel_ = outLevel_ * 0.8f;
      vTaskDelay(pdMS_TO_TICKS(10));
      continue;
    }
    size_t avail = playRing.available();
    if (st == kBuffering) {
      size_t need = static_cast<size_t>(playRate_) * 2 * PLAY_PREBUFFER_MS / 1000;
      if (avail >= need || endRequested_) {
        playState_ = kPlaying;
      } else {
        vTaskDelay(pdMS_TO_TICKS(5));
        continue;
      }
    }
    if (avail < 2) {
      if (endRequested_) {  // всё доиграли
        playState_ = kIdle;
        endRequested_ = false;
        outLevel_ = 0;
      } else {
        vTaskDelay(pdMS_TO_TICKS(5));  // данные не успевают — ЦАП сам выдаёт тишину (tx_desc_auto_clear)
      }
      continue;
    }
    size_t want = std::min(avail & ~static_cast<size_t>(1), sizeof(mono));
    size_t n = playRing.read(reinterpret_cast<uint8_t*>(mono), want) / 2;
    float peak = dsp::monoToStereo(mono, n, stereo, gain_);
    outLevel_ = std::max(peak, outLevel_ * 0.85f);
    size_t written = 0;
    i2s_write(kDacPort, stereo, n * 4, &written, portMAX_DELAY);
  }
}

void waitAck(uint32_t timeoutMs) {
  uint32_t t0 = millis();
  while (stopRequested_ && millis() - t0 < timeoutMs) vTaskDelay(1);
}

bool initMic() {
  i2s_config_t cfg = {};
  cfg.mode = static_cast<i2s_mode_t>(I2S_MODE_MASTER | I2S_MODE_RX);
  cfg.sample_rate = MIC_SAMPLE_RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_32BIT;
  cfg.channel_format = I2S_CHANNEL_FMT_RIGHT_LEFT;
  cfg.communication_format = I2S_COMM_FORMAT_STAND_I2S;
  cfg.intr_alloc_flags = ESP_INTR_FLAG_LEVEL1;
  cfg.dma_buf_count = 8;
  cfg.dma_buf_len = kMicBlock;
  cfg.use_apll = false;
  if (i2s_driver_install(kMicPort, &cfg, 0, nullptr) != ESP_OK) return false;
  i2s_pin_config_t pins = {};
  pins.mck_io_num = I2S_PIN_NO_CHANGE;  // MCLK не нужен (иначе по умолчанию уйдёт на GPIO0!)
  pins.bck_io_num = PIN_MIC_SCK;
  pins.ws_io_num = PIN_MIC_WS;
  pins.data_out_num = I2S_PIN_NO_CHANGE;
  pins.data_in_num = PIN_MIC_SD;
  return i2s_set_pin(kMicPort, &pins) == ESP_OK;
}

bool initDac() {
  i2s_config_t cfg = {};
  cfg.mode = static_cast<i2s_mode_t>(I2S_MODE_MASTER | I2S_MODE_TX);
  cfg.sample_rate = PLAY_SAMPLE_RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_16BIT;
  cfg.channel_format = I2S_CHANNEL_FMT_RIGHT_LEFT;
  cfg.communication_format = I2S_COMM_FORMAT_STAND_I2S;
  cfg.intr_alloc_flags = ESP_INTR_FLAG_LEVEL1;
  cfg.dma_buf_count = 8;
  cfg.dma_buf_len = kOutBlock;
  cfg.use_apll = false;
  cfg.tx_desc_auto_clear = true;  // при нехватке данных — тишина, а не повтор хвоста
  if (i2s_driver_install(kDacPort, &cfg, 0, nullptr) != ESP_OK) return false;
  i2s_pin_config_t pins = {};
  pins.mck_io_num = I2S_PIN_NO_CHANGE;  // PCM5102A работает от внутреннего PLL (SCK на GND)
  pins.bck_io_num = PIN_DAC_BCK;
  pins.ws_io_num = PIN_DAC_LCK;
  pins.data_out_num = PIN_DAC_DIN;
  pins.data_in_num = I2S_PIN_NO_CHANGE;
  if (i2s_set_pin(kDacPort, &pins) != ESP_OK) return false;
  i2s_zero_dma_buffer(kDacPort);
  return true;
}

}  // namespace

bool begin() {
  micRing.attach(allocBuffer(MIC_RING_BYTES, 16 * 1024), psramFound() ? MIC_RING_BYTES : 16 * 1024);
  playRing.attach(allocBuffer(PLAY_RING_BYTES, 64 * 1024), psramFound() ? PLAY_RING_BYTES : 64 * 1024);
  setVolume(volume_);
  bool ok = initMic();
  if (!ok) log_e("не удалось запустить I2S микрофонов");
  bool okDac = initDac();
  if (!okDac) log_e("не удалось запустить I2S ЦАП");
  xTaskCreatePinnedToCore(micTask, "mic", 4096, nullptr, 10, nullptr, 0);
  xTaskCreatePinnedToCore(speakerTask, "speaker", 4096, nullptr, 10, nullptr, 1);
  return ok && okDac;
}

void startCapture() {
  micRing.clear();
  capturing_ = true;
}

void stopCapture() { capturing_ = false; }
bool capturing() { return capturing_; }
size_t readCaptured(int16_t* dst, size_t maxSamples) { return micRing.read(reinterpret_cast<uint8_t*>(dst), maxSamples * 2) / 2; }
size_t capturedAvailable() { return micRing.available() / 2; }
float micLevel() { return micLevel_; }
float ambientLevel() { return ambient_; }
uint32_t micFrameCounter() { return micFrames_; }
float lastBlockRms() { return lastRms_; }

void playbackBegin(uint32_t sampleRate) {
  stopRequested_ = true;
  waitAck(50);
  if (sampleRate >= 8000 && sampleRate <= 48000) requestedRate_ = sampleRate;
  endRequested_ = false;
  playState_ = kBuffering;
}

size_t playbackWrite(const uint8_t* pcm16, size_t bytes) {
  if (playState_ == kIdle) return 0;
  size_t n = playRing.write(pcm16, bytes);
  if (n < bytes) log_w("буфер воспроизведения переполнен, потеряно %u байт", (unsigned)(bytes - n));
  return n;
}

void playbackEnd() {
  if (playState_ != kIdle) endRequested_ = true;
}

void playbackStop() {
  stopRequested_ = true;
  waitAck(50);
}

bool playing() { return playState_ != kIdle; }
float outLevel() { return outLevel_; }

void setVolume(uint8_t v) {
  volume_ = v > 100 ? 100 : v;
  gain_ = dsp::volumeToGain(volume_, VOLUME_MAX_GAIN_DB);
}

uint8_t volume() { return volume_; }

}  // namespace audio
