//! Связь с сервером: Wi-Fi, поиск сервера (mDNS), WebSocket-протокол (docs/API.md).
//! Всё в отдельном потоке; с главным циклом общается каналами.
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use column_core::app::NetEvent;
use column_core::config::WS_RECONNECT_MS;
use column_core::mdns::{MDNS_ADDR, MDNS_PORT, SERVICE, build_query, parse_response};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};
use log::{info, warn};
use tungstenite::{Message, WebSocket};

use crate::secrets::SECRETS;

/// Что главный цикл отправляет на сервер.
pub enum NetOut {
    Json(String),
    Audio(Vec<u8>),
}

#[derive(Default)]
pub struct NetState {
    pub wifi: AtomicBool,
    pub rssi: AtomicI32,
    pub server: AtomicBool,
    pub address: Mutex<String>,
}

pub struct Net {
    pub state: Arc<NetState>,
    pub tx: Sender<NetOut>,
    pub rx: Receiver<NetEvent>,
    pub device_id: String,
}

fn rssi() -> i32 {
    let mut info: esp_idf_svc::sys::wifi_ap_record_t = Default::default();
    if unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut info) } == 0 { info.rssi as i32 } else { 0 }
}

/// Поиск сервера по mDNS (_aicolumn._tcp). ~2 с.
fn discover() -> Option<(String, u16)> {
    let sock = UdpSocket::bind(("0.0.0.0", MDNS_PORT)).or_else(|_| UdpSocket::bind("0.0.0.0:0")).ok()?;
    let _ = sock.join_multicast_v4(&MDNS_ADDR, &Ipv4Addr::UNSPECIFIED);
    sock.set_read_timeout(Some(Duration::from_millis(250))).ok()?;
    let q = build_query(SERVICE);
    let mut buf = [0u8; 1500];
    let t0 = Instant::now();
    let mut sent = 0;
    while t0.elapsed() < Duration::from_secs(2) {
        if sent < 3 && t0.elapsed() >= Duration::from_millis(sent * 600) {
            let _ = sock.send_to(&q, (MDNS_ADDR, MDNS_PORT));
            sent += 1;
        }
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if let Some(f) = parse_response(&buf[..n], SERVICE) {
                let ip = f.ip.map(|ip| ip.to_string()).unwrap_or_else(|| from.ip().to_string());
                info!("нашёл сервер {ip}:{} ({})", f.port, f.instance);
                return Some((ip, f.port));
            }
        }
    }
    info!("сервер в сети не найден (mDNS), повторю позже");
    None
}

fn connect_ws(host: &str, port: u16, device_id: &str) -> anyhow::Result<WebSocket<TcpStream>> {
    let addr: SocketAddr = (host, port).to_socket_addrs()?.next().ok_or_else(|| anyhow::anyhow!("адрес {host} не найден"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut url = format!("ws://{host}:{port}/ws?id={device_id}");
    if !SECRETS.device_token.is_empty() {
        url += &format!("&token={}", SECRETS.device_token);
    }
    let (ws, _) = tungstenite::client(url.as_str(), stream).map_err(|e| anyhow::anyhow!("рукопожатие WebSocket: {e}"))?;
    // дальше читаем короткими порциями, чтобы успевать отправлять звук
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(5)))?;
    Ok(ws)
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Обслуживает соединение, пока оно живо.
fn serve(ws: &mut WebSocket<TcpStream>, wifi: &EspWifi<'static>, out: &Receiver<NetOut>, events: &Sender<NetEvent>) {
    let mut last_rx = Instant::now();
    let mut last_ping = Instant::now();
    loop {
        // исходящие
        let mut sent = false;
        loop {
            match out.try_recv() {
                Ok(NetOut::Json(s)) => {
                    sent = true;
                    if ws.write(Message::text(s)).is_err() {
                        return;
                    }
                }
                Ok(NetOut::Audio(b)) => {
                    sent = true;
                    if ws.write(Message::binary(b)).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if sent || last_ping.elapsed() > Duration::from_secs(15) {
            if last_ping.elapsed() > Duration::from_secs(15) {
                last_ping = Instant::now();
                let _ = ws.write(Message::Ping(Default::default()));
            }
            match ws.flush() {
                Ok(()) => {}
                Err(e) if would_block(&e) => {}
                Err(e) => {
                    warn!("WebSocket: {e}");
                    return;
                }
            }
        }
        // входящие
        match ws.read() {
            Ok(m) => {
                last_rx = Instant::now();
                match m {
                    Message::Text(t) => match serde_json::from_str(t.as_str()) {
                        Ok(v) => {
                            let _ = events.send(NetEvent::Json(v));
                        }
                        Err(e) => warn!("плохой JSON от сервера: {e}"),
                    },
                    Message::Binary(b) => {
                        let _ = events.send(NetEvent::Audio(b.to_vec()));
                    }
                    Message::Close(_) => return,
                    _ => {}
                }
            }
            Err(e) if would_block(&e) => {}
            Err(e) => {
                warn!("WebSocket: {e}");
                return;
            }
        }
        if last_rx.elapsed() > Duration::from_secs(25) {
            warn!("сервер молчит 25 с — переподключаюсь");
            return;
        }
        if !wifi.is_connected().unwrap_or(false) {
            return;
        }
    }
}

pub fn start(modem: Modem<'static>, sysloop: EspSystemEventLoop, nvs: EspDefaultNvsPartition) -> anyhow::Result<Net> {
    let mut wifi = EspWifi::new(modem, sysloop, Some(nvs))?;
    let mac = wifi.sta_netif().get_mac()?;
    let device_id = format!("column-{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5]);
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: SECRETS.wifi_ssid.try_into().map_err(|_| anyhow::anyhow!("слишком длинный wifi_ssid"))?,
        password: SECRETS.wifi_password.try_into().map_err(|_| anyhow::anyhow!("слишком длинный wifi_password"))?,
        auth_method: if SECRETS.wifi_password.is_empty() { AuthMethod::None } else { AuthMethod::WPA2Personal },
        ..Default::default()
    }))?;
    wifi.start()?;
    unsafe {
        esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE); // звук идёт ровнее
    }
    info!("устройство {device_id}, подключаюсь к Wi-Fi «{}»", SECRETS.wifi_ssid);

    let state = Arc::new(NetState::default());
    let (tx, out_rx) = channel::<NetOut>();
    let (ev_tx, rx) = channel::<NetEvent>();
    let (st, id) = (state.clone(), device_id.clone());
    std::thread::Builder::new().name("net".into()).stack_size(16384).spawn(move || net_thread(wifi, st, id, out_rx, ev_tx))?;
    Ok(Net { state, tx, rx, device_id })
}

fn net_thread(mut wifi: EspWifi<'static>, st: Arc<NetState>, device_id: String, out: Receiver<NetOut>, events: Sender<NetEvent>) {
    let fixed_host = !SECRETS.server_host.is_empty();
    let mut host = SECRETS.server_host.to_string();
    let mut port = SECRETS.server_port;
    let mut last_discovery: Option<Instant> = None;
    let mut lost_since: Option<Instant> = None;
    let mut last_wifi_try: Option<Instant> = None;
    loop {
        let connected = wifi.is_connected().unwrap_or(false) && wifi.sta_netif().is_up().unwrap_or(false);
        st.wifi.store(connected, Relaxed);
        if !connected {
            if last_wifi_try.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                last_wifi_try = Some(Instant::now());
                info!("жду Wi-Fi…");
                let _ = wifi.connect();
            }
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        st.rssi.store(rssi(), Relaxed);
        if host.is_empty() {
            if last_discovery.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            last_discovery = Some(Instant::now());
            match discover() {
                Some((h, p)) => {
                    host = h;
                    port = p;
                }
                None => continue,
            }
        }
        *st.address.lock().unwrap() = format!("{host}:{port}");
        match connect_ws(&host, port, &device_id) {
            Ok(mut ws) => {
                info!("WebSocket подключён к {host}:{port}");
                while out.try_recv().is_ok() {} // старое не отправляем
                lost_since = None;
                st.server.store(true, Relaxed);
                let _ = events.send(NetEvent::Connected);
                serve(&mut ws, &wifi, &out, &events);
                st.server.store(false, Relaxed);
                let _ = events.send(NetEvent::Disconnected);
                warn!("WebSocket отключён");
                let _ = ws.close(None);
            }
            Err(e) => warn!("сервер {host}:{port} недоступен: {e}"),
        }
        let since = *lost_since.get_or_insert_with(Instant::now);
        // Сервер найден через mDNS, но долго недоступен — возможно, сменил адрес. Ищем заново.
        if !fixed_host && since.elapsed() > Duration::from_secs(30) {
            info!("сервер недоступен 30 с — ищу его в сети заново");
            host.clear();
            last_discovery = None;
            lost_since = None;
        }
        st.rssi.store(rssi(), Relaxed);
        std::thread::sleep(Duration::from_millis(WS_RECONNECT_MS as u64));
    }
}
