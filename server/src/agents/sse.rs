//! Минимальный разбор Server-Sent Events из потокового HTTP-ответа.
use futures_util::StreamExt;
use futures_util::stream::BoxStream;

#[allow(dead_code)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

pub struct SseReader {
    stream: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
    buf: Vec<u8>,
    done: bool,
}

impl SseReader {
    pub fn new(resp: reqwest::Response) -> Self {
        SseReader { stream: resp.bytes_stream().boxed(), buf: vec![], done: false }
    }

    fn take_event(&mut self) -> Option<SseEvent> {
        loop {
            // Граница события — пустая строка (\n\n или \r\n\r\n).
            let (end, skip) = find_boundary(&self.buf)?;
            let raw: Vec<u8> = self.buf.drain(..end + skip).take(end).collect();
            let text = String::from_utf8_lossy(&raw);
            let mut event = None;
            let mut data: Vec<&str> = vec![];
            for line in text.split('\n') {
                let line = line.trim_end_matches('\r');
                if let Some(v) = line.strip_prefix("event:") {
                    event = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            if event.is_some() || !data.is_empty() {
                return Some(SseEvent { event, data: data.join("\n") });
            }
        }
    }

    pub async fn next(&mut self) -> Option<reqwest::Result<SseEvent>> {
        loop {
            if let Some(ev) = self.take_event() {
                return Some(Ok(ev));
            }
            if self.done {
                if self.buf.iter().all(|b| b.is_ascii_whitespace()) {
                    return None;
                }
                self.buf.extend_from_slice(b"\n\n");
                continue;
            }
            match self.stream.next().await {
                Some(Ok(chunk)) => self.buf.extend_from_slice(&chunk),
                Some(Err(e)) => return Some(Err(e)),
                None => self.done = true,
            }
        }
    }
}

fn find_boundary(b: &[u8]) -> Option<(usize, usize)> {
    for i in 0..b.len() {
        if b[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
        if b[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
    }
    None
}
