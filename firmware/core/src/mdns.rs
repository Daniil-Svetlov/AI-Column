//! Минимальный mDNS-клиент: поиск сервера AI-Column (_aicolumn._tcp) в локальной сети.
//! Здесь — сборка запроса и разбор ответа; отправку по UDP делает прошивка.
use std::net::Ipv4Addr;

pub const SERVICE: &str = "_aicolumn._tcp.local";
pub const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;

const T_A: u16 = 1;
const T_PTR: u16 = 12;
const T_SRV: u16 = 33;

/// Найденный сервер.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub instance: String,
    pub host: String,
    pub port: u16,
    /// Адрес из A-записи (если сервер её прислал).
    pub ip: Option<Ipv4Addr>,
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// PTR-запрос с битом «ответь мне напрямую» (QU).
pub fn build_query(service: &str) -> Vec<u8> {
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    push_name(&mut q, service);
    q.extend_from_slice(&T_PTR.to_be_bytes());
    q.extend_from_slice(&0x8001u16.to_be_bytes());
    q
}

struct Reader<'a> {
    p: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.p.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        (self.pos + n <= self.p.len()).then(|| self.pos += n)
    }
    /// Имя с поддержкой сжатия (указателей).
    fn name(&mut self) -> Option<String> {
        let mut labels: Vec<String> = vec![];
        let mut pos = self.pos;
        let mut jumped = false;
        for _ in 0..64 {
            let len = *self.p.get(pos)? as usize;
            if len & 0xC0 == 0xC0 {
                let ptr = ((len & 0x3F) << 8) | *self.p.get(pos + 1)? as usize;
                if !jumped {
                    self.pos = pos + 2;
                }
                jumped = true;
                pos = ptr;
                continue;
            }
            if len == 0 {
                if !jumped {
                    self.pos = pos + 1;
                }
                return Some(labels.join("."));
            }
            let s = self.p.get(pos + 1..pos + 1 + len)?;
            labels.push(String::from_utf8_lossy(s).into_owned());
            pos += 1 + len;
        }
        None
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches('.').eq_ignore_ascii_case(b.trim_end_matches('.'))
}

/// Разбирает mDNS-ответ. None — в пакете нет нашего сервиса.
pub fn parse_response(pkt: &[u8], service: &str) -> Option<Found> {
    let mut r = Reader { p: pkt, pos: 0 };
    r.skip(2)?;
    let flags = r.u16()?;
    if flags & 0x8000 == 0 {
        return None; // это запрос, а не ответ
    }
    let qd = r.u16()?;
    let total = r.u16()? as usize + r.u16()? as usize + r.u16()? as usize;
    for _ in 0..qd {
        r.name()?;
        r.skip(4)?;
    }
    let mut ptrs: Vec<String> = vec![];
    let mut srvs: Vec<(String, u16, String)> = vec![];
    let mut addrs: Vec<(String, Ipv4Addr)> = vec![];
    for _ in 0..total {
        let name = r.name()?;
        let typ = r.u16()?;
        r.skip(6)?; // класс + TTL
        let len = r.u16()? as usize;
        let start = r.pos;
        match typ {
            T_PTR if same(&name, service) => ptrs.push(r.name()?),
            T_SRV => {
                r.skip(4)?; // приоритет, вес
                let port = r.u16()?;
                srvs.push((name, port, r.name()?));
            }
            T_A if len == 4 => {
                let b = pkt.get(start..start + 4)?;
                addrs.push((name, Ipv4Addr::new(b[0], b[1], b[2], b[3])));
            }
            _ => {}
        }
        r.pos = start;
        r.skip(len)?;
    }
    let suffix = format!(".{}", service.trim_end_matches('.'));
    let (instance, port, host) = srvs
        .iter()
        .find(|(n, _, _)| ptrs.iter().any(|p| same(p, n)))
        .or_else(|| srvs.iter().find(|(n, _, _)| n.to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase())))
        .cloned()?;
    let ip = addrs.iter().find(|(n, _)| same(n, &host)).map(|(_, ip)| *ip);
    let instance = instance.strip_suffix(&suffix).unwrap_or(&instance).to_string();
    Some(Found { instance, host, port, ip })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rr(out: &mut Vec<u8>, name: &[u8], typ: u16, rdata: &[u8]) {
        out.extend_from_slice(name);
        out.extend_from_slice(&typ.to_be_bytes());
        out.extend_from_slice(&[0x80, 0x01, 0, 0, 0x11, 0x94]);
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(rdata);
    }

    #[test]
    fn query_layout() {
        let q = build_query(SERVICE);
        assert_eq!(&q[4..6], &[0, 1]);
        assert_eq!(&q[12..22], b"\x09_aicolumn");
        assert_eq!(&q[q.len() - 4..], &[0, 12, 0x80, 0x01]);
    }

    #[test]
    fn parses_response_with_compression() {
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 3, 0, 0, 0, 1];
        // PTR: _aicolumn._tcp.local → "AI-Column box._aicolumn._tcp.local" (суффикс — указатель на 12)
        let mut name = vec![];
        push_name(&mut name, SERVICE);
        let mut target = vec![13];
        target.extend_from_slice(b"AI-Column box");
        target.extend_from_slice(&[0xC0, 12]);
        rr(&mut p, &name, T_PTR, &target);
        // SRV: имя экземпляра — указатель на rdata PTR; цель — box.local
        let inst_ptr = (12 + name.len() + 10) as u8;
        let mut srv = vec![0, 0, 0, 0, 0x1F, 0x90];
        push_name(&mut srv, "box.local");
        rr(&mut p, &[0xC0, inst_ptr], T_SRV, &srv);
        // лишняя запись другого типа
        rr(&mut p, &[0xC0, inst_ptr], 16, b"\x07path=/ws");
        let mut host = vec![];
        push_name(&mut host, "box.local");
        rr(&mut p, &host, T_A, &[192, 168, 1, 50]);
        let f = parse_response(&p, SERVICE).unwrap();
        assert_eq!(f, Found { instance: "AI-Column box".into(), host: "box.local".into(), port: 8080, ip: Some(Ipv4Addr::new(192, 168, 1, 50)) });
    }

    #[test]
    fn ignores_queries_and_garbage() {
        assert_eq!(parse_response(&build_query(SERVICE), SERVICE), None);
        assert_eq!(parse_response(&[0x00, 0x00, 0x84], SERVICE), None);
        let mut loop_pkt = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        loop_pkt.extend_from_slice(&[0xC0, 12]); // имя указывает само на себя
        assert_eq!(parse_response(&loop_pkt, SERVICE), None);
    }
}
