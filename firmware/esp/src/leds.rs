//! Матрица WS2812B 8×8 через RMT. Кадры считает column_core::leds — здесь только отправка.
//!
//! RMT идёт напрямую через ESP-IDF: обёртка энкодера в esp-idf-hal 0.47 паникует в прерывании,
//! когда драйвер возвращает составное состояние (COMPLETE | MEM_FULL).
use column_core::config::{LED_COUNT, PIN_LED_DATA};
use column_core::leds::Frame;
use esp_idf_svc::sys::*;

pub struct LedStrip {
    ch: rmt_channel_handle_t,
    enc: rmt_encoder_handle_t,
    buf: Vec<u8>,
}

// Хэндлы ESP-IDF; пользуемся ими только из одного потока.
unsafe impl Send for LedStrip {}

/// Символ RMT: `high` тиков высокого уровня, затем `low` тиков низкого.
fn sym(high: u32, low: u32) -> rmt_symbol_word_t {
    rmt_symbol_word_t { val: high | (1 << 15) | (low << 16) }
}

impl LedStrip {
    pub fn new() -> anyhow::Result<LedStrip> {
        unsafe {
            let cfg = rmt_tx_channel_config_t {
                gpio_num: PIN_LED_DATA as i32,
                clk_src: soc_periph_rmt_clk_src_t_RMT_CLK_SRC_DEFAULT,
                resolution_hz: 10_000_000, // тик 0,1 мкс
                mem_block_symbols: 64,
                trans_queue_depth: 4,
                ..Default::default()
            };
            let mut ch: rmt_channel_handle_t = core::ptr::null_mut();
            esp!(rmt_new_tx_channel(&cfg, &mut ch))?;
            // «0» — 0,4 мкс высокий + 0,85 мкс низкий, «1» — 0,8 + 0,45
            let mut ecfg = rmt_bytes_encoder_config_t { bit0: sym(4, 9), bit1: sym(8, 5), ..Default::default() };
            ecfg.flags.set_msb_first(1);
            let mut enc: rmt_encoder_handle_t = core::ptr::null_mut();
            esp!(rmt_new_bytes_encoder(&ecfg, &mut enc))?;
            esp!(rmt_enable(ch))?;
            Ok(LedStrip { ch, enc, buf: vec![0; LED_COUNT * 3] })
        }
    }

    pub fn show(&mut self, frame: &Frame) -> anyhow::Result<()> {
        for (i, (r, g, b)) in frame.iter().enumerate() {
            self.buf[i * 3..i * 3 + 3].copy_from_slice(&[*g, *r, *b]); // порядок WS2812 — GRB
        }
        unsafe {
            let tcfg = rmt_transmit_config_t::default();
            esp!(rmt_transmit(self.ch, self.enc, self.buf.as_ptr().cast(), self.buf.len(), &tcfg))?;
            esp!(rmt_tx_wait_all_done(self.ch, 100))?;
        }
        Ok(())
    }

    pub fn off(&mut self) {
        let _ = self.show(&[(0, 0, 0); LED_COUNT]);
    }
}
