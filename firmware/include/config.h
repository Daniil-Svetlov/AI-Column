// ─────────────────────────────────────────────────────────────────────────────
//  AI-Column — аппаратная конфигурация прошивки.
//  Пины совпадают со схемой hardware/schematic.svg. Wi-Fi/сервер — в secrets.h.
// ─────────────────────────────────────────────────────────────────────────────
#pragma once

#define FW_VERSION "0.1.0"

// ── Микрофоны INMP441 (I2S0, оба на одной шине: Mic A L/R→GND, Mic B L/R→3V3) ──
#define PIN_MIC_SCK 4
#define PIN_MIC_WS 5
#define PIN_MIC_SD 6

// ── ЦАП PCM5102A (I2S1) ──
#define PIN_DAC_BCK 7
#define PIN_DAC_LCK 15
#define PIN_DAC_DIN 16

// ── Дисплей ILI9341 + тач XPT2046 (общая шина SPI) ──
#define PIN_TFT_SCK 10
#define PIN_TFT_MOSI 8
#define PIN_TFT_MISO 18  // идёт только к T_DO тача; SDO дисплея не подключён
#define PIN_TFT_CS 14
#define PIN_TFT_DC 12
#define PIN_TFT_RST 13
#define PIN_TFT_LED 11
#define PIN_TOUCH_CS 9
#define PIN_TOUCH_IRQ 17

// ── Прочее ──
#define PIN_LED_DATA 2     // WS2812B через 74AHCT125
#define PIN_BTN_TALK 42    // SW1: говорить / перебить
#define PIN_BTN_MENU 41    // SW2: следующий агент; удержание — новый разговор
#define PIN_BAT_ADC 1      // делитель 100k/22k от VBAT

// ── Звук ──
#define MIC_SAMPLE_RATE 16000
// Сдвиг 32-битного сэмпла INMP441 → 16 бит. 16 = без усиления, 14 = +12 дБ, 12 = +24 дБ.
#define MIC_SHIFT 13
// 0 — оба микрофона (среднее), 1 — только левый (Mic A), 2 — только правый (Mic B)
#define MIC_CHANNELS 0
#define PLAY_SAMPLE_RATE 24000
// Потолок громкости, дБ относительно полной шкалы ЦАП. PCM5102A даёт до 2,1 В,
// TPA3116D2 усиливает ×20 (26 дБ) — без потолка 10-ваттный динамик легко перегрузить.
// Если на 100% тихо — подними до -12 (но не выше -6) или прибавь регулятором на плате усилителя.
#define VOLUME_MAX_GAIN_DB -16.0f
#define VOLUME_DEFAULT 60
#define PLAY_RING_BYTES (1024 * 1024)  // ~21 с при 24 кГц (в PSRAM)
#define MIC_RING_BYTES (64 * 1024)      // ~2 с при 16 кГц
#define PLAY_PREBUFFER_MS 180           // сколько накопить перед стартом воспроизведения
#define MIC_FRAME_MS 40                 // размер аудиокадра в WebSocket

// ── Определение конца фразы (режим «нажал и отпустил») ──
#define VAD_END_SILENCE_MS 900
#define VAD_NO_SPEECH_MS 5000
#define VAD_MIN_SPEECH_MS 250
#define HOLD_THRESHOLD_MS 450  // дольше — режим «держу кнопку, пока говорю»
#define MAX_UTTERANCE_MS 20000

// ── Светодиодная матрица ──
#define LED_COUNT 64
#define LED_MATRIX_W 8
#define LED_MATRIX_H 8
#define LED_SERPENTINE 0        // 1, если строки матрицы идут «змейкой»
#define LED_MAX_BRIGHTNESS 48   // из 255: 64 светодиода × 60 мА — без ограничения не хватит MP1584

// ── Аккумулятор 3S Li-ion ──
#define BAT_DIVIDER_RATIO ((100.0f + 22.0f) / 22.0f)
#define BAT_CELLS 3
#define BAT_LOW_V 9.9f       // предупреждение
#define BAT_CRITICAL_V 9.5f  // уход в глубокий сон
#define BAT_PRESENT_MIN_V 6.0f  // ниже — считаем, что делителя/АКБ нет (питание от USB)

// ── Сеть ──
#define WS_RECONNECT_MS 3000
#define STATUS_INTERVAL_MS 30000
