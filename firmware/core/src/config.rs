//! Аппаратная конфигурация. Пины совпадают со схемой hardware/schematic.svg.
//! Wi-Fi, адрес сервера и токен — в firmware/esp/cfg.toml (не в git).

pub const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Микрофоны INMP441 (I2S0, оба на одной шине: Mic A L/R→GND, Mic B L/R→3V3) ──
pub const PIN_MIC_SCK: i32 = 4;
pub const PIN_MIC_WS: i32 = 5;
pub const PIN_MIC_SD: i32 = 6;

// ── ЦАП PCM5102A (I2S1) ──
pub const PIN_DAC_BCK: i32 = 7;
pub const PIN_DAC_LCK: i32 = 15;
pub const PIN_DAC_DIN: i32 = 16;

// ── Дисплей ILI9341 + тач XPT2046 (общая шина SPI) ──
pub const PIN_TFT_SCK: i32 = 10;
pub const PIN_TFT_MOSI: i32 = 8;
pub const PIN_TFT_MISO: i32 = 18; // идёт только к T_DO тача; SDO дисплея не подключён
pub const PIN_TFT_CS: i32 = 14;
pub const PIN_TFT_DC: i32 = 12;
pub const PIN_TFT_RST: i32 = 13;
pub const PIN_TFT_LED: i32 = 11;
pub const PIN_TOUCH_CS: i32 = 9;
pub const PIN_TOUCH_IRQ: i32 = 17;

// ── Прочее ──
pub const PIN_LED_DATA: i32 = 2; // WS2812B через 74AHCT125
pub const PIN_BTN_TALK: i32 = 42; // SW1: говорить / перебить
pub const PIN_BTN_MENU: i32 = 41; // SW2: следующий агент; удержание — новый разговор
pub const PIN_BAT_ADC: i32 = 1; // делитель 100k/22k от VBAT

// ── Звук ──
pub const MIC_SAMPLE_RATE: u32 = 16000;
/// Сдвиг 32-битного сэмпла INMP441 → 16 бит. 16 = без усиления, 14 = +12 дБ, 12 = +24 дБ.
pub const MIC_SHIFT: u32 = 13;
/// 0 — оба микрофона (среднее), 1 — только левый (Mic A), 2 — только правый (Mic B)
pub const MIC_CHANNELS: u8 = 0;
pub const PLAY_SAMPLE_RATE: u32 = 24000;
/// Потолок громкости, дБ относительно полной шкалы ЦАП. PCM5102A даёт до 2,1 В,
/// TPA3116D2 усиливает ×20 (26 дБ) — без потолка 10-ваттный динамик легко перегрузить.
/// Если на 100% тихо — подними до -12 (но не выше -6) или прибавь регулятором на плате усилителя.
pub const VOLUME_MAX_GAIN_DB: f32 = -16.0;
pub const VOLUME_DEFAULT: u8 = 60;
pub const PLAY_RING_BYTES: usize = 1024 * 1024; // ~21 с при 24 кГц (в PSRAM)
pub const MIC_RING_BYTES: usize = 64 * 1024; // ~2 с при 16 кГц
pub const PLAY_PREBUFFER_MS: u32 = 180; // сколько накопить перед стартом воспроизведения
pub const MIC_FRAME_MS: u32 = 40; // размер аудиокадра в WebSocket
pub const MIC_BLOCK_FRAMES: usize = 256; // блок I2S микрофона: 16 мс при 16 кГц
pub const MIC_BLOCK_MS: u32 = (MIC_BLOCK_FRAMES as u32 * 1000) / MIC_SAMPLE_RATE;

// ── Определение конца фразы (режим «нажал и отпустил») ──
pub const VAD_END_SILENCE_MS: u32 = 900;
pub const VAD_NO_SPEECH_MS: u32 = 5000;
pub const VAD_MIN_SPEECH_MS: u32 = 250;
pub const HOLD_THRESHOLD_MS: u32 = 450; // дольше — режим «держу кнопку, пока говорю»
pub const MAX_UTTERANCE_MS: u32 = 20000;

// ── Светодиодная матрица ──
pub const LED_COUNT: usize = 64;
pub const LED_MATRIX_W: i32 = 8;
pub const LED_MATRIX_H: i32 = 8;
pub const LED_SERPENTINE: bool = false; // true, если строки матрицы идут «змейкой»
pub const LED_MAX_BRIGHTNESS: u8 = 48; // из 255: 64 светодиода × 60 мА — без ограничения не хватит MP1584

// ── Аккумулятор 3S Li-ion ──
pub const BAT_DIVIDER_RATIO: f32 = (100.0 + 22.0) / 22.0;
pub const BAT_CELLS: u32 = 3;
pub const BAT_LOW_V: f32 = 9.9; // предупреждение
pub const BAT_CRITICAL_V: f32 = 9.5; // уход в глубокий сон
pub const BAT_PRESENT_MIN_V: f32 = 6.0; // ниже — считаем, что делителя/АКБ нет (питание от USB)

// ── Сеть ──
pub const WS_RECONNECT_MS: u32 = 3000;
pub const STATUS_INTERVAL_MS: u32 = 30000;
