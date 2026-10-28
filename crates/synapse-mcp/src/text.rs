//! Small text helpers: accent/case folding for matching, truncation and snippets.

/// Lowercases and strips common Latin diacritics so "Canción" matches "cancion". Keeps a
/// one-to-one char mapping (needed to map match offsets back onto the original text).
pub fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' | 'ő' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ñ' | 'ń' | 'ň' => 'n',
        'ç' | 'ć' | 'č' => 'c',
        'ś' | 'š' | 'ş' => 's',
        'ź' | 'ż' | 'ž' => 'z',
        'ł' => 'l',
        'ř' => 'r',
        'ď' => 'd',
        'ť' => 't',
        'ğ' => 'g',
        _ => c,
    }
}

pub fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// First `max` chars of `s` (with "…" when cut), on one line.
pub fn truncate(s: &str, max: usize) -> String {
    let flat = one_line(s);
    match flat.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}

/// Collapses newlines and runs of whitespace into single spaces.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A window of about `width` chars of `text` around the first occurrence of any folded
/// `term`, on one line.
pub fn snippet(text: &str, terms: &[String], width: usize) -> String {
    let flat = one_line(text);
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= width {
        return flat;
    }
    let folded: Vec<char> = chars.iter().map(|c| fold_char(*c)).collect();
    let hay: String = folded.iter().collect();
    let first = terms
        .iter()
        .filter_map(|t| hay.find(t.as_str()))
        .min()
        .map(|byte| hay[..byte].chars().count())
        .unwrap_or(0);
    let start = first.saturating_sub(width / 3);
    let end = (start + width).min(chars.len());
    let start = end.saturating_sub(width);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Number of non-overlapping occurrences of `needle` in `hay`.
pub fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    hay.matches(needle).count()
}

/// Whether `needle` occurs in `hay` delimited by non-alphanumeric chars on both sides.
pub fn contains_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, m)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + m.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Splits a query into folded terms; `"quoted phrases"` stay together.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, part) in query.split('"').enumerate() {
        if i % 2 == 1 {
            let p = fold(&one_line(part));
            if !p.is_empty() {
                out.push(p);
            }
        } else {
            out.extend(part.split_whitespace().map(fold));
        }
    }
    out.retain(|t| !t.is_empty());
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_and_terms() {
        assert_eq!(fold("Canción ÑANDÚ"), "cancion nandu");
        assert_eq!(
            query_terms(r#"foo "Bar  Baz" qux"#),
            vec!["foo", "bar baz", "qux"]
        );
        assert!(contains_word("hola rust!", "rust"));
        assert!(!contains_word("trusty", "rust"));
        assert_eq!(truncate("a\nb c", 3), "a b…");
    }

    #[test]
    fn snippet_centres_on_match() {
        let text = format!("{} needle {}", "x".repeat(200), "y".repeat(200));
        let s = snippet(&text, &["needle".into()], 40);
        assert!(s.contains("needle"), "{s}");
        assert!(s.starts_with('…') && s.ends_with('…'));
    }
}
