//! Подготовка текста для экрана: замена символов, которых нет в шрифте, и перенос строк.

/// Шрифты экрана — ISO-8859-5 (латиница + кириллица). Типографику заменяем на близкие символы.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '…' => out.push_str("..."),
            '—' | '–' | '−' => out.push('-'),
            '«' | '»' | '“' | '”' | '„' => out.push('"'),
            '‘' | '’' => out.push('\''),
            '·' | '•' => out.push('*'),
            '\u{a0}' | '\t' => out.push(' '),
            '\r' => {}
            c if (c as u32) < 0x80 || ('\u{400}'..='\u{45f}').contains(&c) => out.push(c),
            c if c.is_whitespace() => out.push(' '),
            _ => {} // эмодзи и прочее — не нарисуем
        }
    }
    out
}

/// Перенос по словам для моноширинного шрифта: не больше `max` символов в строке.
/// Слишком длинное слово режется. `\n` — принудительный перенос.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = vec![];
    for para in text.split('\n') {
        let mut line = String::new();
        let mut len = 0usize;
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let mut wl = word.chars().count();
            let need = if len == 0 { wl } else { len + 1 + wl };
            if need <= max {
                if len > 0 {
                    line.push(' ');
                }
                line.push_str(word);
                len = need;
                continue;
            }
            if len > 0 {
                lines.push(std::mem::take(&mut line));
            }
            let mut rest: Vec<char> = word.chars().collect();
            while wl > max {
                lines.push(rest[..max].iter().collect());
                rest.drain(..max);
                wl -= max;
            }
            line = rest.into_iter().collect();
            len = wl;
        }
        lines.push(line);
    }
    // последняя пустая строка (текст закончился переносом) не нужна
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// Ограничивает длину текста ответа: держим последние `keep` символов, режем по пробелу.
pub fn trim_front(s: &mut String, limit: usize, keep: usize) {
    let n = s.chars().count();
    if n <= limit {
        return;
    }
    let start_char = n - keep;
    let byte = s.char_indices().nth(start_char).map(|(i, _)| i).unwrap_or(0);
    let cut = s[byte..].find(' ').map(|i| byte + i + 1).unwrap_or(byte);
    *s = format!("...{}", &s[cut..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_and_cuts_long_ones() {
        assert_eq!(wrap("Привет как дела", 10), ["Привет как", "дела"]);
        assert_eq!(wrap("абвгдеёжзий", 4), ["абвг", "деёж", "зий"]);
        assert_eq!(wrap("a\nb c", 10), ["a", "b c"]);
        assert_eq!(wrap("", 10), Vec::<String>::new());
        assert_eq!(wrap("one  two", 3), ["one", "two"]);
    }

    #[test]
    fn sanitize_typography() {
        assert_eq!(sanitize("«Ёлка» — это…\t😀ok"), "\"Ёлка\" - это... ok");
    }

    #[test]
    fn trims_front_on_word() {
        let mut s = "раз два три четыре пять".to_string();
        trim_front(&mut s, 10, 8);
        assert_eq!(s, "...пять");
    }
}
