//! Обработка звука и прочая «чистая» математика.

pub fn clamp16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn round_i32(x: f32) -> i32 {
    x.round_ties_even() as i32
}

/// Фильтр постоянной составляющей (ФВЧ ~ 30–80 Гц): y = x - x1 + R*y1
#[derive(Clone, Debug)]
pub struct DcBlocker {
    pub r: f32,
    x1: f32,
    y1: f32,
}

impl Default for DcBlocker {
    fn default() -> Self {
        DcBlocker { r: 0.995, x1: 0.0, y1: 0.0 }
    }
}

impl DcBlocker {
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Кадры I2S от INMP441: 32-битные слоты L,R; полезные 24 бита в старших разрядах.
/// channels: 0 — среднее двух микрофонов, 1 — левый, 2 — правый. Возвращает RMS блока (0..1).
pub fn mic_frames_to_pcm16(stereo: &[i32], out: &mut [i16], shift: u32, channels: u8, dc: &mut DcBlocker) -> f32 {
    let frames = (stereo.len() / 2).min(out.len());
    let mut acc = 0f64;
    for i in 0..frames {
        let l = stereo[2 * i] >> shift;
        let r = stereo[2 * i + 1] >> shift;
        let m = match channels {
            1 => l,
            2 => r,
            _ => (l + r) / 2,
        };
        let s = clamp16(round_i32(dc.process(m as f32)));
        out[i] = s;
        acc += s as f64 * s as f64;
    }
    if frames == 0 { 0.0 } else { ((acc / frames as f64).sqrt() / 32768.0) as f32 }
}

/// Громкость 0..100 → линейный коэффициент: перцептивная кривая + потолок max_gain_db.
pub fn volume_to_gain(volume: i32, max_gain_db: f32) -> f32 {
    if volume <= 0 {
        return 0.0;
    }
    let v = volume.min(100) as f32;
    let db = max_gain_db - 40.0 * (1.0 - v / 100.0); // 100% → max_gain_db, 1% → ≈ max_gain_db-40
    10f32.powf(db / 20.0)
}

/// Моно PCM16 → стерео PCM16 с усилением. Возвращает пиковый уровень (0..1).
pub fn mono_to_stereo(input: &[i16], out_stereo: &mut [i16], gain: f32) -> f32 {
    let mut peak = 0i32;
    for (i, &x) in input.iter().enumerate().take(out_stereo.len() / 2) {
        let s = clamp16(round_i32(x as f32 * gain));
        out_stereo[2 * i] = s;
        out_stereo[2 * i + 1] = s;
        peak = peak.max((s as i32).abs());
    }
    peak as f32 / 32768.0
}

/// Линейная передискретизация моно-потока блоками (сервер обычно шлёт ровно PLAY_SAMPLE_RATE,
/// но если нет — подгоняем, а не перенастраиваем I2S на лету).
#[derive(Clone, Debug)]
pub struct Resampler {
    step: f64,
    pos: f64,
    prev: i16,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Resampler {
        Resampler { step: from as f64 / to as f64, pos: 0.0, prev: 0 }
    }
    pub fn is_identity(&self) -> bool {
        (self.step - 1.0).abs() < 1e-9
    }
    /// Добавляет в `out` пересчитанные сэмплы блока `input`.
    pub fn process(&mut self, input: &[i16], out: &mut Vec<i16>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        // pos — позиция относительно начала блока; -1 соответствует последнему сэмплу прошлого блока
        let n = input.len() as f64;
        while self.pos < n - 1.0 + 1e-9 && !input.is_empty() {
            let i = self.pos.floor();
            let t = (self.pos - i) as f32;
            let a = if i < 0.0 { self.prev } else { input[i as usize] };
            let b = input[(i + 1.0).max(0.0) as usize];
            out.push(clamp16(round_i32(a as f32 + (b as f32 - a as f32) * t)));
            self.pos += self.step;
        }
        if let Some(&last) = input.last() {
            self.prev = last;
            self.pos -= n;
        }
    }
}

/// Событие детектора речи.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadEvent {
    None,
    SpeechStart,
    SpeechEnd,
    NoSpeech,
}

/// Простой энергетический детектор речи с адаптивным уровнем шума.
#[derive(Clone, Debug)]
pub struct Vad {
    pub end_silence_ms: u32,
    pub no_speech_ms: u32,
    pub min_speech_ms: u32,
    /// во сколько раз громче шума — начало речи
    pub start_factor: f32,
    /// ниже — тишина
    pub stop_factor: f32,
    /// абсолютный порог (≈ -50 dBFS)
    pub min_level: f32,
    noise: f32,
    speaking: bool,
    heard_speech: bool,
    speech_ms: u32,
    silence_ms: u32,
    total_ms: u32,
    total_speech_ms: u32,
}

impl Default for Vad {
    fn default() -> Self {
        Vad {
            end_silence_ms: 900,
            no_speech_ms: 5000,
            min_speech_ms: 250,
            start_factor: 3.0,
            stop_factor: 1.8,
            min_level: 0.003,
            noise: 0.002,
            speaking: false,
            heard_speech: false,
            speech_ms: 0,
            silence_ms: 0,
            total_ms: 0,
            total_speech_ms: 0,
        }
    }
}

impl Vad {
    pub fn reset(&mut self, noise_floor: f32) {
        self.noise = noise_floor.max(0.0005);
        self.speaking = false;
        self.heard_speech = false;
        self.speech_ms = 0;
        self.silence_ms = 0;
        self.total_ms = 0;
        self.total_speech_ms = 0;
    }

    pub fn process(&mut self, rms: f32, frame_ms: u32) -> VadEvent {
        self.total_ms += frame_ms;
        let on = (self.noise * self.start_factor).max(self.min_level);
        let off = (self.noise * self.stop_factor).max(self.min_level * 0.7);
        if !self.speaking {
            if rms > on {
                self.speech_ms += frame_ms;
                if self.speech_ms >= 60 {
                    // защита от щелчков
                    self.speaking = true;
                    self.silence_ms = 0;
                    if !self.heard_speech {
                        self.heard_speech = true;
                        return VadEvent::SpeechStart;
                    }
                }
            } else {
                self.speech_ms = 0;
                self.noise = self.noise * 0.97 + rms * 0.03; // подстраиваемся под фон только в тишине
                if self.heard_speech {
                    self.silence_ms += frame_ms;
                    if self.silence_ms >= self.end_silence_ms && self.total_speech_ms >= self.min_speech_ms {
                        return VadEvent::SpeechEnd;
                    }
                } else if self.total_ms >= self.no_speech_ms {
                    return VadEvent::NoSpeech;
                }
            }
        } else {
            self.total_speech_ms += frame_ms;
            if rms < off {
                self.silence_ms += frame_ms;
                if self.silence_ms >= 150 {
                    self.speaking = false; // короткие паузы между словами — ещё речь
                }
            } else {
                self.silence_ms = 0;
            }
        }
        VadEvent::None
    }

    pub fn heard_speech(&self) -> bool {
        self.heard_speech
    }
    pub fn speaking(&self) -> bool {
        self.speaking
    }
    pub fn noise(&self) -> f32 {
        self.noise
    }
}

/// Напряжение Li-ion сборки → проценты (по типичной разрядной кривой ячейки).
pub fn battery_percent(pack_volts: f32, cells: u32) -> i32 {
    const V: [f32; 11] = [3.00, 3.30, 3.40, 3.50, 3.60, 3.70, 3.80, 3.90, 4.00, 4.10, 4.20];
    const P: [f32; 11] = [0.0, 3.0, 6.0, 12.0, 22.0, 38.0, 52.0, 65.0, 78.0, 90.0, 100.0];
    let c = pack_volts / cells as f32;
    if c <= V[0] {
        return 0;
    }
    if c >= V[10] {
        return 100;
    }
    for i in 1..=10 {
        if c <= V[i] {
            let t = (c - V[i - 1]) / (V[i] - V[i - 1]);
            return round_i32(P[i - 1] + t * (P[i] - P[i - 1]));
        }
    }
    100
}

/// Перевод координат матрицы (x,y) в номер светодиода.
pub fn led_index(x: i32, y: i32, w: i32, serpentine: bool) -> usize {
    (if serpentine && (y & 1) == 1 { y * w + (w - 1 - x) } else { y * w + x }) as usize
}

/// Уровень RMS 0..1 → «логарифмическая» шкала: floor_db → 0, floor_db+40 → 1.
pub fn level_unit(rms: f32, floor_db: f32) -> f32 {
    if rms < 1e-5 {
        return 0.0;
    }
    ((20.0 * rms.log10() - floor_db) / 40.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_conversion_mix_and_gain() {
        // 24-битный сэмпл 0x100000 в старших битах 32-битного слота
        let frames = [0x100000 << 8, 0x100000 << 8, -(0x100000 << 8), -(0x100000 << 8)];
        let mut out = [0i16; 2];
        let mut dc = DcBlocker { r: 0.0, ..Default::default() }; // без ФВЧ — проверяем чистое преобразование
        let rms = mic_frames_to_pcm16(&frames, &mut out, 16, 0, &mut dc);
        // >>16: 0x10000000 >> 16 = 4096; второй кадр после DC-фильтра с r=0: x - x1 = -4096 - 4096
        assert_eq!(out, [4096, -8192]);
        assert!(rms > 0.1);
    }

    #[test]
    fn mic_single_channel_select() {
        let frames = [1000 << 16, 3000 << 16];
        let mut out = [0i16; 1];
        let mut dc = DcBlocker { r: 0.0, ..Default::default() };
        mic_frames_to_pcm16(&frames, &mut out, 16, 2, &mut dc);
        assert_eq!(out[0], 3000);
    }

    #[test]
    fn volume_curve() {
        assert_eq!(volume_to_gain(0, -12.0), 0.0);
        let g100 = volume_to_gain(100, -12.0);
        assert!((g100 - 0.2512).abs() < 0.002); // -12 дБ
        assert!(volume_to_gain(50, -12.0) < g100);
        assert!(volume_to_gain(50, -12.0) > volume_to_gain(10, -12.0));
        assert_eq!(volume_to_gain(250, -12.0), g100);
    }

    #[test]
    fn mono_to_stereo_clamps() {
        let mut out = [0i16; 4];
        let peak = mono_to_stereo(&[20000, -20000], &mut out, 2.0);
        assert_eq!(out, [32767, 32767, -32768, -32768]);
        assert!(peak > 0.99);
    }

    #[test]
    fn resampler_rate_and_continuity() {
        let mut r = Resampler::new(16000, 24000);
        let input: Vec<i16> = (0..1600).map(|i| (i * 10) as i16).collect();
        let mut out = vec![];
        for block in input.chunks(100) {
            r.process(block, &mut out);
        }
        assert!((out.len() as i64 - 2400).abs() <= 2, "{}", out.len());
        // монотонная рампа остаётся монотонной и на стыках блоков
        assert!(out.windows(2).all(|w| w[1] >= w[0]));
        let mut same = Resampler::new(24000, 24000);
        let mut o2 = vec![];
        same.process(&[1, 2, 3], &mut o2);
        assert_eq!(o2, [1, 2, 3]);
    }

    #[test]
    fn vad_detects_phrase_end() {
        let mut vad = Vad { end_silence_ms: 600, min_speech_ms: 200, ..Default::default() };
        vad.reset(0.002);
        let mut ev = VadEvent::None;
        for _ in 0..10 {
            ev = vad.process(0.002, 16); // тишина
        }
        assert_eq!(ev, VadEvent::None);
        let mut started = false;
        for _ in 0..40 {
            started |= vad.process(0.05, 16) == VadEvent::SpeechStart; // ~0,6 с речи
        }
        assert!(started);
        let ended = (0..60).any(|_| vad.process(0.002, 16) == VadEvent::SpeechEnd);
        assert!(ended);
    }

    #[test]
    fn vad_no_speech_timeout() {
        let mut vad = Vad { no_speech_ms: 1000, ..Default::default() };
        vad.reset(0.002);
        let mut ev = VadEvent::None;
        for _ in 0..80 {
            ev = vad.process(0.0025, 16);
            if ev != VadEvent::None {
                break;
            }
        }
        assert_eq!(ev, VadEvent::NoSpeech);
    }

    #[test]
    fn vad_ignores_short_click() {
        let mut vad = Vad::default();
        vad.reset(0.002);
        assert_eq!(vad.process(0.2, 16), VadEvent::None); // одиночный щелчок
        assert!(!vad.heard_speech());
    }

    #[test]
    fn battery() {
        assert_eq!(battery_percent(12.6, 3), 100);
        assert_eq!(battery_percent(9.0, 3), 0);
        let mid = battery_percent(11.1, 3);
        assert!(mid > 30 && mid < 45, "{mid}");
    }

    #[test]
    fn led_indices() {
        assert_eq!(led_index(1, 1, 8, false), 9);
        assert_eq!(led_index(1, 1, 8, true), 14);
        assert_eq!(led_index(0, 2, 8, true), 16);
    }
}
