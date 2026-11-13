fn main() {
    if !std::path::Path::new("cfg.toml").exists() {
        println!("cargo:warning=Нет firmware/esp/cfg.toml — скопируй cfg.example.toml и впиши Wi-Fi и токен");
    }
    println!("cargo:rerun-if-changed=cfg.toml");
    embuild::espidf::sysenv::output();
}
