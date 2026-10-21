//! Extracts references from Logseq-flavoured Markdown block content.

use crate::model::RefKind;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Refs {
    /// Referenced page titles with their kind (deduplicated, original casing).
    pub pages: Vec<(String, RefKind)>,
    /// `((uuid))` block references.
    pub blocks: Vec<Uuid>,
    /// `{{embed ((uuid))}}` block embeds.
    pub embeds: Vec<Uuid>,
}

impl Refs {
    fn push_page(&mut self, title: &str, kind: RefKind) {
        let t = title.trim();
        if t.is_empty() {
            return;
        }
        let key = crate::model::title_key(t);
        if !self
            .pages
            .iter()
            .any(|(p, k)| *k == kind && crate::model::title_key(p) == key)
        {
            self.pages.push((t.to_string(), kind));
        }
    }
}

/// Parses refs, skipping inline code and fenced code blocks.
pub fn extract_refs(content: &str) -> Refs {
    let mut refs = Refs::default();
    let mut in_fence = false;
    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let prop_line = property_split(line);
        scan_line(
            prop_line.map(|(_, v)| v).unwrap_or(line),
            prop_line.is_some(),
            &mut refs,
        );
    }
    refs
}

/// `key:: value` → `(key, value)`.
pub fn property_split(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once("::")?;
    let k = k.trim();
    (!k.is_empty()
        && k.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_'))
    .then_some((k, v.trim()))
}

fn scan_line(line: &str, is_prop: bool, refs: &mut Refs) {
    let link_kind = if is_prop {
        RefKind::Property
    } else {
        RefKind::Link
    };
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'`' => {
                i = line[i + 1..]
                    .find('`')
                    .map(|j| i + 1 + j + 1)
                    .unwrap_or(b.len());
            }
            b'[' if line[i..].starts_with("[[") => match find_close(line, i + 2) {
                Some(end) => {
                    refs.push_page(&line[i + 2..end], link_kind);
                    i = end + 2;
                }
                None => i += 2,
            },
            b'(' if line[i..].starts_with("((") => match line[i + 2..].find("))") {
                Some(j) => {
                    if let Ok(id) = Uuid::parse_str(line[i + 2..i + 2 + j].trim()) {
                        let embed = line[..i].trim_end().ends_with("{{embed");
                        let list = if embed {
                            &mut refs.embeds
                        } else {
                            &mut refs.blocks
                        };
                        if !list.contains(&id) {
                            list.push(id);
                        }
                    }
                    i += 2 + j + 2;
                }
                None => i += 2,
            },
            b'#' if i == 0 || b[i - 1].is_ascii_whitespace() => {
                if line[i + 1..].starts_with("[[")
                    && let Some(end) = find_close(line, i + 3)
                {
                    refs.push_page(&line[i + 3..end], RefKind::Tag);
                    i = end + 2;
                    continue;
                }
                let rest = &line[i + 1..];
                let len = rest
                    .find(|c: char| c.is_whitespace() || ",.;:!?()[]{}\"'".contains(c))
                    .unwrap_or(rest.len());
                // `#` alone or a Markdown heading (`# `, `## `) is not a tag.
                if len > 0 && !rest.starts_with('#') {
                    refs.push_page(&rest[..len], RefKind::Tag);
                }
                i += 1 + len;
            }
            _ => i += 1,
        }
    }
}

/// Finds the `]]` closing a `[[` that opened before `from`, allowing nested `[[..]]`.
fn find_close(s: &str, from: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = from;
    while i + 1 < s.len() {
        if s[i..].starts_with("[[") {
            depth += 1;
            i += 2;
        } else if s[i..].starts_with("]]") {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
            i += 2;
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_tags_props_and_block_refs() {
        let id = Uuid::now_v7();
        let c = format!(
            "Met [[Ana López]] about #rust and #[[Second Brain]] see (({id}))\n\
             type:: [[Meeting]]\n`[[not a link]]` # heading-ish\n{{{{embed (({id}))}}}}"
        );
        let r = extract_refs(&c);
        assert_eq!(
            r.pages,
            vec![
                ("Ana López".into(), RefKind::Link),
                ("rust".into(), RefKind::Tag),
                ("Second Brain".into(), RefKind::Tag),
                ("Meeting".into(), RefKind::Property),
            ]
        );
        assert_eq!(r.blocks, vec![id]);
        assert_eq!(r.embeds, vec![id]);
    }

    #[test]
    fn skips_code_fences_and_headings() {
        let r = extract_refs("## Title\n```\n[[inside]]\n```\nok [[outside]]");
        assert_eq!(r.pages, vec![("outside".into(), RefKind::Link)]);
    }

    #[test]
    fn nested_links() {
        let r = extract_refs("[[a [[b]] c]]");
        assert_eq!(r.pages[0].0, "a [[b]] c");
    }
}

/// Rewrites references to the page whose normalised title is `old_key` so they point at
/// `new_title`: `[[old]]`, `#[[old]]` and `#old`.
pub fn rewrite_refs(content: &str, old_key: &str, new_title: &str) -> String {
    let tag = if new_title.contains(char::is_whitespace) {
        format!("#[[{new_title}]]")
    } else {
        format!("#{new_title}")
    };
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < content.len() {
        let rest = &content[i..];
        if rest.starts_with("[[")
            && let Some(end) = find_close(content, i + 2)
        {
            if crate::model::title_key(&content[i + 2..end]) == old_key {
                out.push_str(&format!("[[{new_title}]]"));
            } else {
                out.push_str(&content[i..end + 2]);
            }
            i = end + 2;
            continue;
        }
        if rest.starts_with('#') && !rest.starts_with("#[[") {
            let prev_ok = i == 0 || content[..i].ends_with(char::is_whitespace);
            let body = &rest[1..];
            let len = body
                .find(|c: char| c.is_whitespace() || ",.;:!?()[]{}\"'".contains(c))
                .unwrap_or(body.len());
            if prev_ok && len > 0 && crate::model::title_key(&body[..len]) == old_key {
                out.push_str(&tag);
                i += 1 + len;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod rewrite_tests {
    use super::rewrite_refs;

    #[test]
    fn rewrites_links_and_tags() {
        assert_eq!(
            rewrite_refs(
                "see [[Foo]], #foo and #[[foo]] not #foobar",
                "foo",
                "Big Bar"
            ),
            "see [[Big Bar]], #[[Big Bar]] and #[[Big Bar]] not #foobar"
        );
    }
}
