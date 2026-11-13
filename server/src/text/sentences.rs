//! Нарезка потокового ответа LLM на предложения для синтеза речи.
//! Первое предложение отдаём как можно раньше (быстрый старт речи),
//! дальше склеиваем короткие, чтобы не дёргать TTS на каждое «Да.».

const CLOSERS: &[char] = &['"', '\'', '»', ')', ']', '”', '’'];
const ENDERS: &[char] = &['.', '!', '?', '…'];

pub struct SentenceSplitter {
    min_chars: usize,
    max_chars: usize,
    buf: Vec<char>,
    emitted: usize,
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self::new(40, 240)
    }
}

impl SentenceSplitter {
    pub fn new(min_chars: usize, max_chars: usize) -> Self {
        SentenceSplitter { min_chars, max_chars, buf: vec![], emitted: 0 }
    }

    /// Конец предложения, начиная с from: индекс (не включая) или None, если решать рано.
    fn find_boundary(&self, from: usize) -> Option<usize> {
        let s = &self.buf;
        let mut i = from;
        while i < s.len() {
            let ch = s[i];
            i += 1;
            if ch == '\n' {
                return Some(i);
            }
            if !ENDERS.contains(&ch) {
                continue;
            }
            let mut j = i;
            while j < s.len() && (ENDERS.contains(&s[j]) || CLOSERS.contains(&s[j])) {
                j += 1;
            }
            if j >= s.len() {
                return None; // не знаем, что дальше
            }
            if !s[j].is_whitespace() {
                continue; // «3.5», «т.е.» без пробела
            }
            let mut k = j;
            while k < s.len() && s[k].is_whitespace() {
                k += 1;
            }
            if k >= s.len() {
                return None; // ждём следующий непробельный символ
            }
            // Строчная буква после точки — скорее сокращение («т. е.», «и т. д. и»), не конец.
            if ch == '.' && s[k].is_lowercase() {
                continue;
            }
            return Some(j);
        }
        None
    }

    fn split_long(&self) -> usize {
        let s: String = self.buf[..self.max_chars.min(self.buf.len())].iter().collect();
        let chars: Vec<char> = s.chars().collect();
        let rfind = |pat: &str| -> Option<usize> {
            let p: Vec<char> = pat.chars().collect();
            (0..=chars.len().saturating_sub(p.len())).rev().find(|&i| chars[i..].starts_with(&p))
        };
        for sep in ["; ", ": ", ", ", " — ", " - "] {
            if let Some(idx) = rfind(sep) {
                if idx as f64 > self.max_chars as f64 * 0.4 {
                    return idx + sep.chars().count();
                }
            }
        }
        match rfind(" ") {
            Some(sp) if sp > 0 => sp + 1,
            _ => self.max_chars,
        }
    }

    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.extend(text.chars());
        let mut out = vec![];
        let mut search_from = 0;
        loop {
            let Some(b) = self.find_boundary(search_from) else {
                if self.buf.len() > self.max_chars {
                    let cut = self.split_long();
                    let piece: String = self.buf.drain(..cut).collect();
                    self.emit(&piece, &mut out);
                    search_from = 0;
                    continue;
                }
                break;
            };
            let candidate: String = self.buf[..b].iter().collect();
            let need = if self.emitted == 0 { 1 } else { self.min_chars };
            if candidate.trim().chars().count() < need && b < self.buf.len() {
                search_from = b; // слишком коротко — приклеим следующее предложение
                continue;
            }
            let piece: String = self.buf.drain(..b).collect();
            self.emit(&piece, &mut out);
            search_from = 0;
        }
        out
    }

    pub fn flush(&mut self) -> Vec<String> {
        let mut out = vec![];
        let piece: String = self.buf.drain(..).collect();
        self.emit(&piece, &mut out);
        out
    }

    fn emit(&mut self, piece: &str, out: &mut Vec<String>) {
        let t = piece.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            out.push(t);
            self.emitted += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(s: &mut SentenceSplitter, text: &str, step: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = vec![];
        for c in chars.chunks(step) {
            out.extend(s.push(&c.iter().collect::<String>()));
        }
        out.extend(s.flush());
        out
    }

    #[test]
    fn first_sentence_fast_then_glue() {
        let mut s = SentenceSplitter::new(30, 240);
        let text = "Да. Конечно, сейчас расскажу. Ок. Это займёт пару минут, не больше.";
        let out = feed(&mut s, text, 3);
        assert_eq!(out[0], "Да.");
        assert!(out.len() >= 2);
        assert_eq!(out.join(" "), text);
    }

    #[test]
    fn numbers_and_abbreviations() {
        let mut s = SentenceSplitter::new(1, 240);
        let out = feed(&mut s, "Число пи равно 3.14 примерно. Это т. е. константа и т. д. всем известна. Конец!", 3);
        assert_eq!(out, ["Число пи равно 3.14 примерно.", "Это т. е. константа и т. д. всем известна.", "Конец!"]);
    }

    #[test]
    fn questions_quotes_ellipsis() {
        let mut s = SentenceSplitter::new(1, 240);
        let out = feed(&mut s, "Ты спросил «почему?» Отвечаю… Потому что! А ещё?", 3);
        assert_eq!(out, ["Ты спросил «почему?»", "Отвечаю…", "Потому что!", "А ещё?"]);
    }

    #[test]
    fn long_piece_split_by_comma() {
        let mut s = SentenceSplitter::new(1, 60);
        let long = "раз два три четыре пять, шесть семь восемь девять десять, одиннадцать двенадцать тринадцать четырнадцать пятнадцать";
        let out = feed(&mut s, long, 3);
        assert!(out.len() >= 2);
        for p in &out {
            assert!(p.chars().count() <= 60, "{p}");
        }
        assert_eq!(out.join(" "), long);
    }

    #[test]
    fn newline_ends_sentence() {
        let mut s = SentenceSplitter::new(1, 240);
        assert_eq!(feed(&mut s, "Первое\nВторое", 3), ["Первое", "Второе"]);
    }
}
