//! Микрофоны INMP441 на I2S0: 32-битные слоты, стерео (Mic A — левый, Mic B — правый).
use column_core::config::*;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::i2s::config::{Config, DataBitWidth, SlotMode, StdClkConfig, StdConfig, StdGpioConfig, StdSlotConfig};
use esp_idf_svc::hal::i2s::{I2S0, I2sDriver, I2sRx};

use crate::board::pin;

pub struct Mic {
    i2s: I2sDriver<'static, I2sRx>,
    buf: Vec<u8>,
}

impl Mic {
    pub fn new(i2s0: I2S0<'static>) -> anyhow::Result<Mic> {
        let cfg = StdConfig::new(
            Config::new().dma_buffer_count(8).frames_per_buffer(MIC_BLOCK_FRAMES as u32),
            StdClkConfig::from_sample_rate_hz(MIC_SAMPLE_RATE),
            StdSlotConfig::philips_slot_default(DataBitWidth::Bits32, SlotMode::Stereo),
            StdGpioConfig::default(),
        );
        // MCLK не нужен — None, иначе драйвер займёт лишний пин
        let mut i2s = I2sDriver::new_std_rx(i2s0, &cfg, pin(PIN_MIC_SCK), pin(PIN_MIC_SD), None::<AnyIOPin>, pin(PIN_MIC_WS))?;
        i2s.rx_enable()?;
        Ok(Mic { i2s, buf: vec![0u8; MIC_BLOCK_FRAMES * 8] })
    }

    /// Читает блок (~16 мс) кадров L,R. Возвращает срез пар 32-битных сэмплов.
    pub fn read_block(&mut self, out: &mut Vec<i32>) -> anyhow::Result<()> {
        let n = self.i2s.read(&mut self.buf, esp_idf_svc::hal::delay::BLOCK)?;
        out.clear();
        out.extend(self.buf[..n - n % 4].chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])));
        Ok(())
    }
}
