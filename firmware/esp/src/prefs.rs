//! Настройки в NVS: выбранный агент, громкость, калибровка тача.
use column_core::touch::TouchCal;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use log::warn;

pub struct Prefs {
    nvs: Option<EspNvs<NvsDefault>>,
}

impl Prefs {
    pub fn open(part: EspDefaultNvsPartition) -> Prefs {
        match EspNvs::new(part, "column", true) {
            Ok(nvs) => Prefs { nvs: Some(nvs) },
            Err(e) => {
                warn!("NVS недоступна ({e}) — настройки не сохранятся");
                Prefs { nvs: None }
            }
        }
    }

    pub fn agent(&self) -> String {
        let mut buf = [0u8; 64];
        self.nvs.as_ref().and_then(|n| n.get_str("agent", &mut buf).ok().flatten().map(str::to_string)).unwrap_or_else(|| "auto".into())
    }
    pub fn set_agent(&self, id: &str) {
        if let Some(n) = &self.nvs {
            if self.agent() != id {
                let _ = n.set_str("agent", id);
            }
        }
    }
    pub fn volume(&self, default: u8) -> u8 {
        self.nvs.as_ref().and_then(|n| n.get_u8("vol").ok().flatten()).unwrap_or(default)
    }
    pub fn set_volume(&self, v: u8) {
        if let Some(n) = &self.nvs {
            let _ = n.set_u8("vol", v);
        }
    }
    pub fn touch_cal(&self) -> Option<TouchCal> {
        let mut buf = [0u8; 24];
        let n = self.nvs.as_ref()?;
        TouchCal::from_bytes(n.get_blob("tcal", &mut buf).ok().flatten()?)
    }
    pub fn set_touch_cal(&self, cal: &TouchCal) {
        if let Some(n) = &self.nvs {
            let _ = n.set_blob("tcal", &cal.to_bytes());
        }
    }
}
