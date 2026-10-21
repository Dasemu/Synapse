//! The few `logseq/config.edn` settings the importer needs, plus Logseq date formats.

use chrono::NaiveDate;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    /// `:journal/file-name-format` (default `yyyy_MM_dd`).
    pub journal_file_format: DateFormat,
    /// `:journal/page-title-format` (default `MMM do, yyyy`).
    pub journal_title_format: DateFormat,
    /// `:file/name-format :triple-lowbar` (otherwise legacy: `.` separates namespaces).
    pub triple_lowbar: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            journal_file_format: DateFormat::new("yyyy_MM_dd"),
            journal_title_format: DateFormat::new("MMM do, yyyy"),
            triple_lowbar: false,
        }
    }
}

impl Config {
    pub fn load(graph_dir: &Path) -> Self {
        match std::fs::read_to_string(graph_dir.join("logseq/config.edn")) {
            Ok(text) => Self::from_edn(&text),
            Err(_) => Self::default(),
        }
    }

    pub fn from_edn(text: &str) -> Self {
        let text: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with(';'))
            .collect::<Vec<_>>()
            .join("\n");
        let mut c = Self::default();
        if let Some(f) = edn_string(&text, ":journal/file-name-format") {
            c.journal_file_format = DateFormat::new(&f);
        }
        if let Some(f) = edn_string(&text, ":journal/page-title-format") {
            c.journal_title_format = DateFormat::new(&f);
        }
        c.triple_lowbar = text
            .split_once(":file/name-format")
            .is_some_and(|(_, rest)| rest.trim_start().starts_with(":triple-lowbar"));
        c
    }
}

fn edn_string(text: &str, key: &str) -> Option<String> {
    let rest = text.split_once(key)?.1.trim_start().strip_prefix('"')?;
    Some(rest.split_once('"')?.0.to_string())
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Year4,
    Year2,
    Month(u8), // 1, 2 digits, 3 = short name, 4 = full name
    Day,       // d / dd
    DayOrd,    // do (1st, 2nd...)
    Weekday,
    Lit(char),
}

/// Parser for the Java-style date patterns Logseq uses.
#[derive(Debug, Clone)]
pub struct DateFormat {
    toks: Vec<Tok>,
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

impl DateFormat {
    pub fn new(fmt: &str) -> Self {
        let cs: Vec<char> = fmt.chars().collect();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < cs.len() {
            let c = cs[i];
            let run = cs[i..].iter().take_while(|&&x| x == c).count();
            match c {
                'y' => toks.push(if run >= 3 { Tok::Year4 } else { Tok::Year2 }),
                'M' => toks.push(Tok::Month(run.min(4) as u8)),
                'd' if cs.get(i + run) == Some(&'o') && run == 1 => {
                    toks.push(Tok::DayOrd);
                    i += 1;
                }
                'd' => toks.push(Tok::Day),
                'E' => toks.push(Tok::Weekday),
                _ => {
                    toks.extend(std::iter::repeat_n(Tok::Lit(c), run));
                    i += run;
                    continue;
                }
            }
            i += run;
        }
        Self { toks }
    }

    /// Parses `s` completely; `None` if it does not match the format.
    pub fn parse(&self, s: &str) -> Option<NaiveDate> {
        let mut rest = s.trim();
        let (mut y, mut m, mut d) = (None, None, None);
        for t in &self.toks {
            match t {
                Tok::Year4 => {
                    let (v, r) = num(rest, 4, 4)?;
                    (y, rest) = (Some(v), r);
                }
                Tok::Year2 => {
                    let (v, r) = num(rest, 2, 2)?;
                    (y, rest) = (Some(2000 + v), r);
                }
                Tok::Month(n @ (1 | 2)) => {
                    let (v, r) = num(rest, if *n == 1 { 1 } else { 2 }, 2)?;
                    (m, rest) = (Some(v), r);
                }
                Tok::Month(n) => {
                    let (v, r) = month_name(rest, *n == 3)?;
                    (m, rest) = (Some(v), r);
                }
                Tok::Day => {
                    let (v, r) = num(rest, 1, 2)?;
                    (d, rest) = (Some(v), r);
                }
                Tok::DayOrd => {
                    let (v, r) = num(rest, 1, 2)?;
                    let r = r.get(2..).filter(|_| {
                        matches!(
                            r[..2].to_ascii_lowercase().as_str(),
                            "st" | "nd" | "rd" | "th"
                        )
                    })?;
                    (d, rest) = (Some(v), r);
                }
                Tok::Weekday => {
                    let n = rest.chars().take_while(|c| c.is_alphabetic()).count();
                    if n < 3 {
                        return None;
                    }
                    rest = &rest[n..];
                }
                Tok::Lit(c) => rest = rest.strip_prefix(*c)?,
            }
        }
        if !rest.is_empty() {
            return None;
        }
        NaiveDate::from_ymd_opt(y? as i32, m?, d?)
    }
}

fn num(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    if n < min {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn month_name(s: &str, short: bool) -> Option<(u32, &str)> {
    let lower = s.to_ascii_lowercase();
    for (i, name) in MONTHS.iter().enumerate() {
        let n = if short { 3 } else { name.len() };
        if lower.starts_with(&name[..n]) {
            return Some((i as u32 + 1, &s[n..]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, d)
    }

    #[test]
    fn parses_formats() {
        assert_eq!(
            DateFormat::new("yyyy_MM_dd").parse("2024_03_15"),
            d(2024, 3, 15)
        );
        assert_eq!(
            DateFormat::new("MMM do, yyyy").parse("Mar 1st, 2024"),
            d(2024, 3, 1)
        );
        assert_eq!(
            DateFormat::new("EEEE, dd-MM-yyyy").parse("Friday, 15-03-2024"),
            d(2024, 3, 15)
        );
        assert_eq!(
            DateFormat::new("MMMM d, yyyy").parse("March 5, 2024"),
            d(2024, 3, 5)
        );
        assert_eq!(DateFormat::new("yyyy_MM_dd").parse("2024_13_01"), None);
        assert_eq!(DateFormat::new("yyyy_MM_dd").parse("2024_03_15x"), None);
    }

    #[test]
    fn reads_edn() {
        let c = Config::from_edn(
            ";; :journal/file-name-format \"bad\"\n{:journal/file-name-format \"yyyy-MM-dd\"\n :file/name-format :triple-lowbar}",
        );
        assert!(c.triple_lowbar);
        assert_eq!(c.journal_file_format.parse("2024-03-15"), d(2024, 3, 15));
    }
}
