//! Wi-Fi, адрес сервера и токен — из firmware/esp/cfg.toml (шаблон: cfg.example.toml).
#[toml_cfg::toml_config]
pub struct Secrets {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_password: &'static str,
    /// Пусто — найти сервер по mDNS.
    #[default("")]
    server_host: &'static str,
    #[default(8080)]
    server_port: u16,
    #[default("")]
    device_token: &'static str,
}
