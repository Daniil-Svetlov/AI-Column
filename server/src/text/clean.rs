//! Очистка текста перед озвучкой и показом на экране колонки.
use std::sync::LazyLock;

use regex::Regex;

macro_rules! re {
    ($s:expr) => {
        LazyLock::new(|| Regex::new($s).unwrap())
    };
}

static CODE_BLOCK: LazyLock<Regex> = re!(r"```[\s\S]*?(?:```|$)");
static INLINE_CODE: LazyLock<Regex> = re!(r"`([^`]*)`");
static IMAGE: LazyLock<Regex> = re!(r"!\[([^\]]*)\]\([^)]*\)");
static LINK: LazyLock<Regex> = re!(r"\[([^\]]+)\]\([^)]*\)");
static URL: LazyLock<Regex> = re!(r"https?://\S+");
static HEADING: LazyLock<Regex> = re!(r"(?m)^\s{0,3}#{1,6}\s+");
static BULLET: LazyLock<Regex> = re!(r"(?m)^\s*(?:[-*+•]|\d+[.)])\s+");
static EMPH: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"(\*\*|__|\*|~~)(?=\S)([\s\S]*?\S)\1").unwrap());
static MARKUP: LazyLock<Regex> = re!(r"[*#_~|<>]");
static EMOJI: LazyLock<Regex> = re!(r"[\p{Extended_Pictographic}\x{1F1E6}-\x{1F1FF}\x{FE0F}\x{200D}]");
static RUBLE: LazyLock<Regex> = re!(r"\s*₽");
static QUOTES: LazyLock<Regex> = re!(r"[„“”]");
static SPACES: LazyLock<Regex> = re!(r"\s+");

/// Убирает markdown, ссылки, эмодзи — то, что синтезатор прочитает криво.
pub fn clean_for_speech(text: &str) -> String {
    let t = CODE_BLOCK.replace_all(text, " ");
    let t = INLINE_CODE.replace_all(&t, "$1");
    let t = IMAGE.replace_all(&t, "$1");
    let t = LINK.replace_all(&t, "$1");
    let t = URL.replace_all(&t, "ссылка");
    let t = HEADING.replace_all(&t, "");
    let t = BULLET.replace_all(&t, "");
    let t = EMPH.replace_all(&t, "$2");
    let t = MARKUP.replace_all(&t, " ");
    let t = EMOJI.replace_all(&t, "");
    let t = RUBLE.replace_all(&t, " рублей");
    let t = QUOTES.replace_all(&t, "\"");
    SPACES.replace_all(&t, " ").trim().to_string()
}

/// Нормализация для сравнения: нижний регистр, ё→е. Число символов не меняется.
pub fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            let l = c.to_lowercase().next().unwrap_or(c);
            if l == 'ё' { 'е' } else { l }
        })
        .collect()
}

/// Потоковый фильтр, вырезающий <think>…</think> (Qwen3, DeepSeek-R1 и т.п.).
pub struct ThinkFilter {
    open: &'static str,
    close: &'static str,
    in_think: bool,
    carry: String,
    started: bool,
}

impl Default for ThinkFilter {
    fn default() -> Self {
        ThinkFilter { open: "<think>", close: "</think>", in_think: false, carry: String::new(), started: false }
    }
}

/// Длина (в байтах) самого длинного суффикса s, который является началом tag.
fn partial_suffix(s: &str, tag: &str) -> usize {
    let max = (tag.len() - 1).min(s.len());
    (1..=max).rev().find(|&n| s.is_char_boundary(s.len() - n) && tag.starts_with(&s[s.len() - n..])).unwrap_or(0)
}

impl ThinkFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, text: &str) -> String {
        let mut s = std::mem::take(&mut self.carry) + text;
        let mut out = String::new();
        loop {
            if !self.in_think {
                if let Some(i) = s.find(self.open) {
                    out.push_str(&s[..i]);
                    s = s[i + self.open.len()..].to_string();
                    self.in_think = true;
                    continue;
                }
                let keep = partial_suffix(&s, self.open);
                out.push_str(&s[..s.len() - keep]);
                self.carry = s[s.len() - keep..].to_string();
                break;
            } else {
                if let Some(i) = s.find(self.close) {
                    s = s[i + self.close.len()..].to_string();
                    self.in_think = false;
                    continue;
                }
                let keep = partial_suffix(&s, self.close);
                self.carry = s[s.len() - keep..].to_string();
                break;
            }
        }
        if !self.started {
            out = out.trim_start().to_string();
            if !out.is_empty() {
                self.started = true;
            }
        }
        out
    }

    pub fn flush(&mut self) -> String {
        let rest = if self.in_think { String::new() } else { std::mem::take(&mut self.carry) };
        self.carry.clear();
        if self.started { rest } else { rest.trim_start().to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_markdown_links_emoji_code() {
        let t = clean_for_speech(
            "## Итог\n- **Жирный** и *курсив* 😀 см. [доку](https://x.y) или https://a.b/c\n```js\ncode()\n```\n`npm i`",
        );
        assert_eq!(t, "Итог Жирный и курсив см. доку или ссылка npm i");
    }

    #[test]
    fn clean_symbols() {
        assert_eq!(clean_for_speech("Стоит 500 ₽, „дёшево“"), "Стоит 500 рублей, \"дёшево\"");
    }

    #[test]
    fn think_filter_split_tags() {
        let mut f = ThinkFilter::new();
        let parts = ["<thi", "nk>Надо подумать", " ещё</th", "ink>\n\nОтвет: ", "42<", "b>"];
        let mut out: String = parts.iter().map(|p| f.push(p)).collect();
        out += &f.flush();
        assert_eq!(out, "Ответ: 42<b>");
    }

    #[test]
    fn think_filter_passthrough() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("Привет, мир") + &f.flush(), "Привет, мир");
    }

    #[test]
    fn normalize_keeps_length() {
        let s = "Ёжик ЁЛКА";
        assert_eq!(normalize(s), "ежик елка");
        assert_eq!(normalize(s).chars().count(), s.chars().count());
    }
}
