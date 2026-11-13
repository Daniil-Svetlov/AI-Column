//! Звук: WAV (разбор и запись), пересэмплирование, нормализация громкости,
//! декодирование ответа TTS в моно PCM16 (WAV — сами, остальное — через ffmpeg).
use std::fmt;

#[derive(Debug, Clone)]
pub struct AudioError(pub String);

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AudioError {}

pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub samples: Vec<i16>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn is_wav(buf: &[u8]) -> bool {
    buf.len() >= 12 && &buf[0..4] == b"RIFF" && &buf[8..12] == b"WAVE"
}

fn clamp16(v: f64) -> i16 {
    v.round().clamp(-32768.0, 32767.0) as i16
}

/// Разбирает WAV (PCM 8/16/24/32 бит, float32/64, WAVE_FORMAT_EXTENSIBLE) и сводит в моно PCM16.
/// Терпим к «потоковым» WAV, где размер data = 0 или 0xFFFFFFFF (так отдают некоторые TTS).
pub fn parse_wav(buf: &[u8]) -> Result<Wav, AudioError> {
    if !is_wav(buf) {
        return Err(AudioError("не WAV (нет заголовка RIFF/WAVE)".into()));
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let mut size = u32le(buf, pos + 4) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            if body + 16 > buf.len() {
                break;
            }
            let mut format = u16le(buf, body);
            let channels = u16le(buf, body + 2);
            let rate = u32le(buf, body + 4);
            let bits = u16le(buf, body + 14);
            if format == 0xfffe && size >= 26 && body + 26 <= buf.len() {
                format = u16le(buf, body + 24); // SubFormat GUID: первые 2 байта
            }
            fmt = Some((format, channels, rate, bits));
        } else if id == b"data" {
            if size == 0 || size == 0xffff_ffff || body + size > buf.len() {
                size = buf.len() - body;
            }
            data = Some((body, size));
            break;
        }
        pos = body + size + (size % 2);
    }
    let (format, channels, sample_rate, bits) = fmt.ok_or_else(|| AudioError("в WAV нет блока fmt".into()))?;
    let (start, len) = data.ok_or_else(|| AudioError("в WAV нет блока data".into()))?;
    if channels == 0 || sample_rate == 0 || bits < 8 {
        return Err(AudioError("битый заголовок WAV".into()));
    }
    let bytes_per = (bits / 8) as usize;
    let read: fn(&[u8]) -> f64 = match (format, bits) {
        (1, 16) => |b| i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0,
        (1, 8) => |b| (b[0] as f64 - 128.0) / 128.0,
        (1, 24) => |b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f64 / 8388608.0,
        (1, 32) => |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / 2147483648.0,
        (3, 32) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        (3, 64) => |b| f64::from_le_bytes(b[..8].try_into().unwrap()),
        _ => return Err(AudioError(format!("неподдерживаемый формат WAV: format={format}, bits={bits}"))),
    };
    let frame = bytes_per * channels as usize;
    let frames = len / frame;
    let mut samples = Vec::with_capacity(frames);
    for i in 0..frames {
        let base = start + i * frame;
        let mut acc = 0.0;
        for c in 0..channels as usize {
            acc += read(&buf[base + c * bytes_per..]);
        }
        samples.push(clamp16(acc / channels as f64 * 32767.0));
    }
    Ok(Wav { sample_rate, channels, bits_per_sample: bits, samples })
}

/// Кодирует моно PCM16 в WAV.
pub fn encode_wav(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&pcm_to_bytes(samples));
    out
}

/// PCM16LE → сэмплы (нечётный последний байт отбрасывается).
pub fn bytes_to_pcm(buf: &[u8]) -> Vec<i16> {
    buf.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

pub fn pcm_to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn lowpass_box(samples: &[i16], ratio: f64) -> Vec<i16> {
    let win = (ratio.round() as usize).max(1);
    if win <= 1 {
        return samples.to_vec();
    }
    let mut out = Vec::with_capacity(samples.len());
    let mut acc: i64 = 0;
    for i in 0..samples.len() {
        acc += samples[i] as i64;
        if i >= win {
            acc -= samples[i - win] as i64;
        }
        out.push((acc as f64 / (i + 1).min(win) as f64).round() as i16);
    }
    out
}

/// Пересэмплирование моно PCM16 линейной интерполяцией; при понижении частоты — со сглаживанием.
pub fn resample(samples: &[i16], from: u32, to: u32) -> Vec<i16> {
    if samples.is_empty() || from == to {
        return samples.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let src = if ratio > 1.15 { lowpass_box(samples, ratio) } else { samples.to_vec() };
    let out_len = ((samples.len() as f64 / ratio).floor() as usize).max(1);
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = (pos.floor() as usize).min(src.len() - 1);
            let i1 = (i0 + 1).min(src.len() - 1);
            let frac = pos - i0 as f64;
            (src[i0] as f64 * (1.0 - frac) + src[i1] as f64 * frac).round() as i16
        })
        .collect()
}

/// RMS (0…1).
pub fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let s: f64 = samples.iter().map(|&v| v as f64 * v as f64).sum();
    (s / samples.len() as f64).sqrt() / 32768.0
}

/// Подтягивает громкость фразы перед распознаванием: пик → 0.7, но усиление не больше 8×.
/// Тихие микрофоны (INMP441 без усиления) Whisper распознаёт хуже.
pub fn normalize_peak(samples: Vec<i16>) -> Vec<i16> {
    let peak = samples.iter().map(|&v| (v as i32).abs()).max().unwrap_or(0);
    if peak < 50 {
        return samples; // тишина — усиливать только шум
    }
    let gain = (8.0f64).min(0.7 * 32767.0 / peak as f64);
    if gain <= 1.05 {
        return samples;
    }
    samples.iter().map(|&v| clamp16(v as f64 * gain)).collect()
}

async fn ffmpeg_to_pcm(buf: &[u8], rate: u32) -> Result<Vec<i16>, AudioError> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i", "pipe:0", "-f", "s16le", "-ac", "1", "-ar"])
        .arg(rate.to_string())
        .arg("pipe:1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AudioError("аудио не в WAV, а ffmpeg не установлен — попроси TTS отдавать wav или поставь ffmpeg".into())
            } else {
                AudioError(format!("ffmpeg: {e}"))
            }
        })?;
    let mut stdin = child.stdin.take().unwrap();
    let input = buf.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
    });
    let out = child.wait_with_output().await.map_err(|e| AudioError(format!("ffmpeg: {e}")))?;
    let _ = writer.await;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AudioError(format!("ffmpeg: {}", if err.is_empty() { out.status.to_string() } else { err })));
    }
    Ok(bytes_to_pcm(&out.stdout))
}

/// WAV/MP3/… от TTS (или сырой PCM16 при raw_rate) → моно PCM16 нужной частоты.
pub async fn decode_to_pcm(buf: &[u8], target_rate: u32, raw_rate: Option<u32>) -> Result<Vec<i16>, AudioError> {
    if let Some(r) = raw_rate {
        return Ok(resample(&bytes_to_pcm(buf), r, target_rate));
    }
    if is_wav(buf) {
        let w = parse_wav(buf)?;
        return Ok(resample(&w.samples, w.sample_rate, target_rate));
    }
    ffmpeg_to_pcm(buf, target_rate).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, rate: u32, freq: f64, amp: f64) -> Vec<i16> {
        (0..n).map(|i| ((2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() * amp).round() as i16).collect()
    }

    #[test]
    fn wav_roundtrip() {
        let s = sine(1000, 16000, 440.0, 10000.0);
        let w = parse_wav(&encode_wav(&s, 16000)).unwrap();
        assert_eq!(w.sample_rate, 16000);
        assert_eq!(w.channels, 1);
        assert_eq!(w.samples, s);
    }

    #[test]
    fn wav_float_stereo_to_mono() {
        let frames = 100;
        let mut data = vec![];
        for _ in 0..frames {
            data.extend_from_slice(&0.5f32.to_le_bytes());
            data.extend_from_slice(&(-0.5f32).to_le_bytes());
        }
        let mut h = vec![];
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&3u16.to_le_bytes());
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&22050u32.to_le_bytes());
        h.extend_from_slice(&(22050u32 * 8).to_le_bytes());
        h.extend_from_slice(&8u16.to_le_bytes());
        h.extend_from_slice(&32u16.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend(data);
        let w = parse_wav(&h).unwrap();
        assert_eq!(w.sample_rate, 22050);
        assert_eq!(w.samples.len(), frames);
        assert!(w.samples.iter().all(|&v| v == 0));
    }

    #[test]
    fn streaming_wav_size_ffffffff() {
        let mut wav = encode_wav(&sine(500, 24000, 440.0, 10000.0), 24000);
        wav[40..44].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        assert_eq!(parse_wav(&wav).unwrap().samples.len(), 500);
    }

    #[test]
    fn resample_length_and_level() {
        let s = sine(24000, 24000, 300.0, 10000.0);
        let down = resample(&s, 24000, 16000);
        assert_eq!(down.len(), 16000);
        assert!((rms(&down) - rms(&s)).abs() < 0.02);
        let up = resample(&sine(22050, 22050, 300.0, 10000.0), 22050, 24000);
        assert!((up.len() as i64 - 24000).abs() <= 1);
    }

    #[tokio::test]
    async fn decode_wav_and_raw() {
        let s = sine(2205, 22050, 440.0, 10000.0);
        assert_eq!(decode_to_pcm(&encode_wav(&s, 22050), 22050, None).await.unwrap().len(), 2205);
        let raw = pcm_to_bytes(&s);
        let b = decode_to_pcm(&raw, 11025, Some(22050)).await.unwrap();
        assert!((b.len() as i64 - 1102).abs() <= 1);
        assert_eq!(&bytes_to_pcm(&raw)[..5], &s[..5]);
    }

    #[test]
    fn normalize_peak_boosts_only_quiet_speech() {
        let louder = normalize_peak(sine(1600, 16000, 300.0, 2000.0));
        let max = *louder.iter().max().unwrap();
        assert!(max > 15000);
        let silence = sine(1600, 16000, 300.0, 20.0);
        assert_eq!(normalize_peak(silence.clone()), silence);
        let loud = sine(1600, 16000, 300.0, 30000.0);
        assert_eq!(normalize_peak(loud.clone()), loud);
    }
}
