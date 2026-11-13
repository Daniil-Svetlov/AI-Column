//! Звук: микрофоны INMP441 (I2S0) и ЦАП PCM5102A (I2S1).
//! Два фоновых потока: чтение микрофонов и вывод на ЦАП; обмен — через кольцевые буферы в PSRAM.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering::Relaxed};
use std::time::Duration;

use column_core::config::*;
use column_core::dsp::{DcBlocker, Resampler, mic_frames_to_pcm16, mono_to_stereo, volume_to_gain};
use column_core::ring::{Consumer, Producer, ring};
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::delay::BLOCK;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::i2s::config::{Config, DataBitWidth, SlotMode, StdClkConfig, StdConfig, StdGpioConfig, StdSlotConfig};
use esp_idf_svc::hal::i2s::{I2S0, I2S1, I2sDriver, I2sTx};
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use log::{error, warn};

use crate::board::pin;
use crate::mic::Mic;

/// f32 в атомике.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        AtomicF32(AtomicU32::new(v.to_bits()))
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
}

const IDLE: u8 = 0;
const BUFFERING: u8 = 1;
const PLAYING: u8 = 2;
const OUT_BLOCK: usize = 256; // кадров: ~10,7 мс при 24 кГц

pub struct Shared {
    capturing: AtomicBool,
    mic_level: AtomicF32,
    ambient: AtomicF32,
    last_rms: AtomicF32,
    mic_frames: AtomicU32,
    play_state: AtomicU8,
    end_requested: AtomicBool,
    stop_requested: AtomicBool,
    src_rate: AtomicU32,
    out_level: AtomicF32,
    volume: AtomicU8,
    gain: AtomicF32,
}

/// Управление звуком из главного цикла.
pub struct Audio {
    sh: Arc<Shared>,
    mic_rx: Consumer,
    play_tx: Producer,
    pub ok: bool,
}

fn spawn(name: &'static std::ffi::CStr, core: Core, f: impl FnOnce() + Send + 'static) {
    let _ = ThreadSpawnConfiguration { name: Some(name), stack_size: 8192, priority: 10, pin_to_core: Some(core), ..Default::default() }.set();
    if let Err(e) = std::thread::Builder::new().stack_size(8192).spawn(f) {
        error!("не удалось запустить поток {name:?}: {e}");
    }
    let _ = ThreadSpawnConfiguration::default().set();
}

fn open_dac(i2s1: I2S1<'static>) -> anyhow::Result<I2sDriver<'static, I2sTx>> {
    let cfg = StdConfig::new(
        // при нехватке данных — тишина, а не повтор хвоста
        Config::new().dma_buffer_count(8).frames_per_buffer(OUT_BLOCK as u32).auto_clear(true),
        StdClkConfig::from_sample_rate_hz(PLAY_SAMPLE_RATE),
        StdSlotConfig::philips_slot_default(DataBitWidth::Bits16, SlotMode::Stereo),
        StdGpioConfig::default(),
    );
    // PCM5102A работает от внутреннего PLL (SCK на GND) — MCLK не нужен
    let mut i2s = I2sDriver::new_std_tx(i2s1, &cfg, pin(PIN_DAC_BCK), pin(PIN_DAC_DIN), None::<AnyIOPin>, pin(PIN_DAC_LCK))?;
    i2s.tx_enable()?;
    Ok(i2s)
}

fn mic_task(mut mic: Mic, sh: Arc<Shared>, mut tx: Producer) {
    let mut raw = vec![];
    let mut pcm = vec![0i16; MIC_BLOCK_FRAMES];
    let mut dc = DcBlocker::default();
    loop {
        if mic.read_block(&mut raw).is_err() || raw.is_empty() {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let frames = raw.len() / 2;
        let rms = mic_frames_to_pcm16(&raw, &mut pcm[..frames], MIC_SHIFT, MIC_CHANNELS, &mut dc);
        sh.last_rms.set(rms);
        sh.mic_frames.fetch_add(1, Relaxed);
        sh.mic_level.set(sh.mic_level.get() * 0.8 + rms * 0.2);
        let capturing = sh.capturing.load(Relaxed);
        if !capturing && sh.play_state.load(Relaxed) == IDLE {
            sh.ambient.set(sh.ambient.get() * 0.98 + rms * 0.02);
        }
        if capturing {
            let bytes: Vec<u8> = pcm[..frames].iter().flat_map(|s| s.to_le_bytes()).collect();
            tx.write(&bytes);
        }
    }
}

fn speaker_task(mut dac: I2sDriver<'static, I2sTx>, sh: Arc<Shared>, mut rx: Consumer) {
    let mut bytes = vec![0u8; OUT_BLOCK * 2];
    let mut mono_in: Vec<i16> = Vec::with_capacity(OUT_BLOCK);
    let mut mono: Vec<i16> = Vec::with_capacity(OUT_BLOCK * 3);
    let mut stereo = vec![0i16; OUT_BLOCK * 6];
    let mut out = vec![0u8; OUT_BLOCK * 12];
    let mut resampler = Resampler::new(PLAY_SAMPLE_RATE, PLAY_SAMPLE_RATE);
    let mut cur_rate = PLAY_SAMPLE_RATE;
    loop {
        if sh.stop_requested.load(Relaxed) {
            rx.clear();
            sh.play_state.store(IDLE, Relaxed);
            sh.end_requested.store(false, Relaxed);
            sh.out_level.set(0.0);
            sh.stop_requested.store(false, Relaxed); // подтверждение для playback_begin/stop
        }
        let rate = sh.src_rate.load(Relaxed);
        if rate != cur_rate {
            cur_rate = rate;
            resampler = Resampler::new(rate, PLAY_SAMPLE_RATE);
        }
        let st = sh.play_state.load(Relaxed);
        if st == IDLE {
            sh.out_level.set(sh.out_level.get() * 0.8);
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let avail = rx.available();
        if st == BUFFERING {
            let need = cur_rate as usize * 2 * PLAY_PREBUFFER_MS as usize / 1000;
            if avail >= need || sh.end_requested.load(Relaxed) {
                sh.play_state.store(PLAYING, Relaxed);
            } else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
        }
        if avail < 2 {
            if sh.end_requested.load(Relaxed) {
                // всё доиграли
                sh.play_state.store(IDLE, Relaxed);
                sh.end_requested.store(false, Relaxed);
                sh.out_level.set(0.0);
            } else {
                std::thread::sleep(Duration::from_millis(5)); // данные не успевают — ЦАП сам выдаёт тишину
            }
            continue;
        }
        let want = (avail & !1).min(bytes.len());
        let n = rx.read(&mut bytes[..want]);
        mono_in.clear();
        mono_in.extend(bytes[..n].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])));
        mono.clear();
        resampler.process(&mono_in, &mut mono);
        let frames = mono.len().min(stereo.len() / 2);
        let peak = mono_to_stereo(&mono[..frames], &mut stereo[..frames * 2], sh.gain.get());
        sh.out_level.set(peak.max(sh.out_level.get() * 0.85));
        for (i, s) in stereo[..frames * 2].iter().enumerate() {
            out[i * 2..i * 2 + 2].copy_from_slice(&s.to_le_bytes());
        }
        if let Err(e) = dac.write_all(&out[..frames * 4], BLOCK) {
            warn!("I2S ЦАП: {e}");
        }
    }
}

impl Audio {
    pub fn start(i2s0: I2S0<'static>, i2s1: I2S1<'static>, volume: u8) -> Audio {
        let sh = Arc::new(Shared {
            capturing: AtomicBool::new(false),
            mic_level: AtomicF32::new(0.0),
            ambient: AtomicF32::new(0.002),
            last_rms: AtomicF32::new(0.0),
            mic_frames: AtomicU32::new(0),
            play_state: AtomicU8::new(IDLE),
            end_requested: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            src_rate: AtomicU32::new(PLAY_SAMPLE_RATE),
            out_level: AtomicF32::new(0.0),
            volume: AtomicU8::new(0),
            gain: AtomicF32::new(0.0),
        });
        // буферы — в PSRAM (крупные выделения уходят туда сами, см. sdkconfig)
        let (mic_tx, mic_rx) = ring(MIC_RING_BYTES);
        let (play_tx, play_rx) = ring(PLAY_RING_BYTES);
        let mut ok = true;
        match Mic::new(i2s0) {
            Ok(mic) => {
                let s = sh.clone();
                spawn(c"mic", Core::Core0, move || mic_task(mic, s, mic_tx));
            }
            Err(e) => {
                error!("не удалось запустить I2S микрофонов: {e}");
                ok = false;
            }
        }
        match open_dac(i2s1) {
            Ok(dac) => {
                let s = sh.clone();
                spawn(c"speaker", Core::Core1, move || speaker_task(dac, s, play_rx));
            }
            Err(e) => {
                error!("не удалось запустить I2S ЦАП: {e}");
                ok = false;
            }
        }
        let mut a = Audio { sh, mic_rx, play_tx, ok };
        a.set_volume(volume);
        a
    }

    fn wait_ack(&self, ms: u32) {
        for _ in 0..ms {
            if !self.sh.stop_requested.load(Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // ── микрофон ──
    pub fn start_capture(&mut self) {
        self.mic_rx.clear();
        self.sh.capturing.store(true, Relaxed);
    }
    pub fn stop_capture(&mut self) {
        self.sh.capturing.store(false, Relaxed);
    }
    pub fn read_captured(&mut self, dst: &mut [i16]) -> usize {
        let mut bytes = vec![0u8; dst.len() * 2];
        let n = self.mic_rx.read(&mut bytes) / 2;
        for (i, d) in dst.iter_mut().take(n).enumerate() {
            *d = i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
        }
        n
    }
    pub fn captured_available(&self) -> usize {
        self.mic_rx.available() / 2
    }
    pub fn mic_level(&self) -> f32 {
        self.sh.mic_level.get()
    }
    pub fn ambient_level(&self) -> f32 {
        self.sh.ambient.get()
    }
    pub fn mic_frame_counter(&self) -> u32 {
        self.sh.mic_frames.load(Relaxed)
    }
    pub fn last_block_rms(&self) -> f32 {
        self.sh.last_rms.get()
    }

    // ── воспроизведение ──
    pub fn playback_begin(&mut self, rate: u32) {
        self.sh.stop_requested.store(true, Relaxed);
        self.wait_ack(50);
        if (8000..=48000).contains(&rate) {
            self.sh.src_rate.store(rate, Relaxed);
        }
        self.sh.end_requested.store(false, Relaxed);
        self.sh.play_state.store(BUFFERING, Relaxed);
    }
    pub fn playback_write(&mut self, pcm16: &[u8]) {
        if self.sh.play_state.load(Relaxed) == IDLE {
            return;
        }
        let n = self.play_tx.write(pcm16);
        if n < pcm16.len() {
            warn!("буфер воспроизведения переполнен, потеряно {} байт", pcm16.len() - n);
        }
    }
    pub fn playback_end(&mut self) {
        if self.sh.play_state.load(Relaxed) != IDLE {
            self.sh.end_requested.store(true, Relaxed);
        }
    }
    pub fn playback_stop(&mut self) {
        self.sh.stop_requested.store(true, Relaxed);
        self.wait_ack(50);
    }
    pub fn playing(&self) -> bool {
        self.sh.play_state.load(Relaxed) != IDLE
    }
    pub fn out_level(&self) -> f32 {
        self.sh.out_level.get()
    }
    pub fn set_volume(&mut self, v: u8) {
        let v = v.min(100);
        self.sh.volume.store(v, Relaxed);
        self.sh.gain.set(volume_to_gain(v as i32, VOLUME_MAX_GAIN_DB));
    }
    pub fn volume(&self) -> u8 {
        self.sh.volume.load(Relaxed)
    }
}
