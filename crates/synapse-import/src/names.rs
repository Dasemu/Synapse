//! Logseq file-name encodings.

use crate::config::Config;
use chrono::NaiveDate;

/// Percent-decodes `%XX` sequences (invalid ones are kept verbatim).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Page title encoded in a file stem (`a___b` / `a.b` → `a/b`, `%2F` → `/`).
pub fn title_from_stem(stem: &str, cfg: &Config) -> String {
    let sep = if cfg.triple_lowbar { "___" } else { "." };
    stem.split(sep)
        .map(percent_decode)
        .collect::<Vec<_>>()
        .join("/")
}

/// Journal date of a file stem, using the configured format with common fallbacks.
pub fn journal_date(stem: &str, cfg: &Config) -> Option<NaiveDate> {
    cfg.journal_file_format
        .parse(stem)
        .or_else(|| NaiveDate::parse_from_str(stem, "%Y_%m_%d").ok())
        .or_else(|| NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_titles() {
        let tl = Config {
            triple_lowbar: true,
            ..Config::default()
        };
        assert_eq!(title_from_stem("a___b%3F", &tl), "a/b?");
        assert_eq!(title_from_stem("a%2Fb", &tl), "a/b");
        assert_eq!(title_from_stem("a.b", &Config::default()), "a/b");
        assert_eq!(title_from_stem("100%", &tl), "100%");
        assert_eq!(title_from_stem("caf%C3%A9", &tl), "café");
    }

    #[test]
    fn journal_names() {
        let c = Config::default();
        assert_eq!(
            journal_date("2024_03_15", &c),
            NaiveDate::from_ymd_opt(2024, 3, 15)
        );
        assert_eq!(journal_date("notes", &c), None);
    }
}
