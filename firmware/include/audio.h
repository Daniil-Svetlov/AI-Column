// Звук: микрофоны INMP441 (I2S0) и ЦАП PCM5102A (I2S1).
// Две фоновые задачи FreeRTOS: чтение микрофонов и вывод на ЦАП.
#pragma once
#include <Arduino.h>

namespace audio {

bool begin();

// ── Микрофон ──
void startCapture();                           // складывать PCM16 моно 16 кГц в буфер для отправки
void stopCapture();
bool capturing();
size_t readCaptured(int16_t* dst, size_t maxSamples);
size_t capturedAvailable();                    // сколько сэмплов ждут отправки
float micLevel();                              // текущий уровень 0..1 (RMS, сглаженный)
float ambientLevel();                          // уровень фона в тишине (для детектора речи)
uint32_t micFrameCounter();                    // растёт с каждым блоком (для синхронизации VAD)
float lastBlockRms();                          // RMS последнего блока (~16 мс)

// ── Воспроизведение (поток с сервера) ──
void playbackBegin(uint32_t sampleRate);       // audio_start
size_t playbackWrite(const uint8_t* pcm16, size_t bytes);
void playbackEnd();                            // audio_end: доиграть буфер и остановиться
void playbackStop();                           // немедленно (перебили)
bool playing();                                // играет или ждёт данных
float outLevel();                              // уровень выхода 0..1 (для подсветки)

void setVolume(uint8_t v);                     // 0..100
uint8_t volume();

}  // namespace audio
