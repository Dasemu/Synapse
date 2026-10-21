//! Auto-link suggestions: unlinked references (Aho-Corasick over page titles/aliases) and
//! semantic suggestions (page embeddings close to a block).

use crate::error::{Error, Result};
use aho_corasick::{AhoCorasick, MatchKind};
use std::collections::HashMap;
use std::ops::Range;
use synapse_core::{Block, BlockId, Graph, PageId, parse};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reason {
    /// The block text contains the title of an existing page.
    ExactTitle,
    /// The block text contains an alias of an existing page.
    Alias,
    /// The block is semantically close to the page (cosine similarity).
    Semantic { similarity: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub block: BlockId,
    /// Byte range in the block content to replace by `[[Title]]`. `None` for semantic
    /// suggestions, which are appended to the block instead.
    pub span: Option<Range<usize>>,
    /// The text found at `span` (used to detect stale suggestions).
    pub matched: String,
    pub target_page: PageId,
    pub score: f32,
    pub reason: Reason,
}

struct Pattern {
    page: PageId,
    alias: bool,
    len: usize,
}

/// Finds occurrences of page titles and aliases in text.
pub struct LinkMatcher {
    ac: Option<AhoCorasick>,
    patterns: Vec<Pattern>,
    min_chars: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub span: Range<usize>,
    pub page: PageId,
    pub alias: bool,
}

impl LinkMatcher {
    /// Builds a matcher from every page title and alias of the graph.
    pub fn new(graph: &Graph) -> Result<Self> {
        let aliases: std::collections::HashSet<String> = {
            let mut st = graph.conn().prepare("SELECT alias FROM aliases")?;
            st.query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        let titles = graph
            .all_titles()?
            .into_iter()
            .map(|(t, id)| {
                let a = aliases.contains(&t);
                (t, id, a)
            })
            .collect();
        Ok(Self::from_titles(titles))
    }

    /// Builds a matcher from `(text, page, is_alias)` entries. Entries shorter than 3
    /// characters are ignored (too noisy).
    pub fn from_titles(titles: Vec<(String, PageId, bool)>) -> Self {
        let min_chars = 3;
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut keys = Vec::new();
        let mut patterns = Vec::new();
        // Titles before aliases so a title wins when both spell the same text.
        let mut titles = titles;
        titles.sort_by_key(|t| t.2);
        for (text, page, alias) in titles {
            let key = text.trim().to_lowercase();
            if key.chars().count() < min_chars || seen.insert(key.clone(), ()).is_some() {
                continue;
            }
            patterns.push(Pattern {
                page,
                alias,
                len: key.len(),
            });
            keys.push(key);
        }
        let ac = if keys.is_empty() {
            None
        } else {
            AhoCorasick::builder()
                .match_kind(MatchKind::Standard)
                .build(&keys)
                .ok()
        };
        Self {
            ac,
            patterns,
            min_chars,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn min_chars(&self) -> usize {
        self.min_chars
    }

    /// Non-overlapping, longest-first, word-bounded matches outside code spans, existing
    /// links, tags, URLs and property lines.
    pub fn find(&self, text: &str) -> Vec<Match> {
        let Some(ac) = &self.ac else {
            return Vec::new();
        };
        let (lower, map) = lower_with_map(text);
        let orig = |i: usize| map.as_ref().map_or(i, |m| m[i]);
        let excluded = excluded_ranges(text);
        let mut cands: Vec<Match> = Vec::new();
        for m in ac.find_overlapping_iter(&lower) {
            let (s, e) = (orig(m.start()), orig(m.end()));
            if s >= e || !word_bounded(text, s, e) {
                continue;
            }
            if excluded.iter().any(|r| s < r.end && r.start < e) {
                continue;
            }
            let p = &self.patterns[m.pattern().as_usize()];
            debug_assert!(p.len > 0);
            cands.push(Match {
                span: s..e,
                page: p.page,
                alias: p.alias,
            });
        }
        // Greedy: leftmost first, longest first.
        cands.sort_by(|a, b| {
            a.span
                .start
                .cmp(&b.span.start)
                .then(b.span.end.cmp(&a.span.end))
        });
        let mut out: Vec<Match> = Vec::new();
        for c in cands {
            if out.last().is_none_or(|l| c.span.start >= l.span.end) {
                out.push(c);
            }
        }
        out
    }

    /// Unlinked-reference suggestions for one block.
    pub fn suggest_block(&self, block: &Block, skip_own_page: bool) -> Vec<Suggestion> {
        self.find(&block.content)
            .into_iter()
            .filter(|m| !(skip_own_page && m.page == block.page_id))
            .map(|m| {
                let matched = block.content[m.span.clone()].to_string();
                let chars = matched.chars().count() as f32;
                Suggestion {
                    block: block.id,
                    score: 0.8 + 0.2 * (chars / 20.0).min(1.0),
                    reason: if m.alias {
                        Reason::Alias
                    } else {
                        Reason::ExactTitle
                    },
                    target_page: m.page,
                    span: Some(m.span),
                    matched,
                }
            })
            .collect()
    }

    /// Unlinked-reference suggestions for every block of a page.
    pub fn suggest_page(&self, graph: &Graph, page: PageId) -> Result<Vec<Suggestion>> {
        let mut out = Vec::new();
        for (_, b) in graph.page_tree(page)? {
            out.extend(self.suggest_block(&b, true));
        }
        Ok(out)
    }
}

/// Rewrites the block so the suggested text becomes `[[Title]]` (semantic suggestions are
/// appended as ` [[Title]]`). Fails with [`Error::Stale`] if the block changed meanwhile.
pub fn apply_suggestion(graph: &Graph, s: &Suggestion) -> Result<Block> {
    let block = graph
        .get_block(s.block)?
        .ok_or_else(|| Error::Stale(format!("block {} is gone", s.block)))?;
    let page = graph
        .get_page(s.target_page)?
        .ok_or_else(|| Error::Stale(format!("page {} is gone", s.target_page)))?;
    let link = format!("[[{}]]", page.title);
    let content = match &s.span {
        Some(r) => {
            let ok = block.content.get(r.clone()) == Some(s.matched.as_str());
            if !ok {
                return Err(Error::Stale("block text changed".into()));
            }
            format!(
                "{}{}{}",
                &block.content[..r.start],
                link,
                &block.content[r.end..]
            )
        }
        None => format!("{} {}", block.content.trim_end(), link),
    };
    Ok(graph.update_block(s.block, &content)?)
}

fn lower_with_map(text: &str) -> (String, Option<Vec<usize>>) {
    let lower = text.to_lowercase();
    if lower.len() == text.len() {
        return (lower, None);
    }
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        for lc in c.to_lowercase() {
            let before = out.len();
            out.push(lc);
            map.extend(std::iter::repeat_n(i, out.len() - before));
        }
    }
    map.push(text.len());
    (out, Some(map))
}

fn word_bounded(text: &str, s: usize, e: usize) -> bool {
    let first = text[s..].chars().next();
    let last = text[..e].chars().next_back();
    let before_ok = !first.is_some_and(|c| c.is_alphanumeric())
        || !text[..s]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric());
    let after_ok = !last.is_some_and(|c| c.is_alphanumeric())
        || !text[e..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric());
    before_ok && after_ok
}

/// Byte ranges where a match must not be turned into a link.
pub(crate) fn excluded_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push(start..offset);
            continue;
        }
        if in_fence || parse::property_split(line.trim_end_matches(['\n', '\r'])).is_some() {
            out.push(start..offset);
            continue;
        }
        scan_inline(line, start, &mut out);
    }
    out
}

fn scan_inline(line: &str, base: usize, out: &mut Vec<Range<usize>>) {
    let b = line.as_bytes();
    let mut i = 0;
    let find = |from: usize, pat: &str| line[from..].find(pat).map(|p| p + from);
    while i < b.len() {
        match b[i] {
            b'`' => {
                let run = b[i..].iter().take_while(|&&c| c == b'`').count();
                let fence = "`".repeat(run);
                match find(i + run, &fence) {
                    Some(end) => {
                        out.push(base + i..base + end + run);
                        i = end + run;
                    }
                    None => i += run,
                }
            }
            b'[' if b.get(i + 1) == Some(&b'[') => match find(i + 2, "]]") {
                Some(end) => {
                    out.push(base + i..base + end + 2);
                    i = end + 2;
                }
                None => i += 2,
            },
            b'[' => {
                // [text](target) markdown link.
                let close = find(i + 1, "](");
                match close.and_then(|c| find(c + 2, ")").map(|e| (c, e))) {
                    Some((_, end)) => {
                        out.push(base + i..base + end + 1);
                        i = end + 1;
                    }
                    None => i += 1,
                }
            }
            b'(' if b.get(i + 1) == Some(&b'(') => match find(i + 2, "))") {
                Some(end) => {
                    out.push(base + i..base + end + 2);
                    i = end + 2;
                }
                None => i += 2,
            },
            b'{' if b.get(i + 1) == Some(&b'{') => match find(i + 2, "}}") {
                Some(end) => {
                    out.push(base + i..base + end + 2);
                    i = end + 2;
                }
                None => i += 2,
            },
            b'#' if i == 0 || b[i - 1].is_ascii_whitespace() || b[i - 1] == b'(' => {
                let end = line[i..]
                    .find(char::is_whitespace)
                    .map_or(line.len(), |p| p + i);
                out.push(base + i..base + end);
                i = end.max(i + 1);
            }
            b'h' if line[i..].starts_with("http://") || line[i..].starts_with("https://") => {
                let end = line[i..]
                    .find(char::is_whitespace)
                    .map_or(line.len(), |p| p + i);
                out.push(base + i..base + end);
                i = end;
            }
            _ => i += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn matcher(titles: &[(&str, bool)]) -> (LinkMatcher, Vec<Uuid>) {
        let ids: Vec<Uuid> = titles.iter().map(|_| Uuid::now_v7()).collect();
        let m = LinkMatcher::from_titles(
            titles
                .iter()
                .zip(&ids)
                .map(|((t, a), id)| (t.to_string(), *id, *a))
                .collect(),
        );
        (m, ids)
    }

    fn found(m: &LinkMatcher, text: &str) -> Vec<String> {
        m.find(text)
            .into_iter()
            .map(|x| text[x.span].to_string())
            .collect()
    }

    #[test]
    fn word_boundaries_and_longest() {
        let (m, _) = matcher(&[("Rust", false), ("Rust Language", false), ("art", false)]);
        assert_eq!(
            found(&m, "I love rust language and Rusty things, smart"),
            vec!["rust language"]
        );
        assert_eq!(found(&m, "(Rust)"), vec!["Rust"]);
    }

    #[test]
    fn skips_code_links_tags_urls_properties() {
        let (m, _) = matcher(&[("Rust", false)]);
        assert!(
            found(
                &m,
                "`Rust` and [[Rust]] and #Rust and https://rust.org/Rust"
            )
            .is_empty()
        );
        assert!(found(&m, "[Rust](http://x) ((Rust))").is_empty());
        assert!(found(&m, "```\nRust\n```").is_empty());
        assert!(found(&m, "tags:: Rust").is_empty());
        assert_eq!(found(&m, "line one\nRust here"), vec!["Rust"]);
    }

    #[test]
    fn unicode_case_and_accents() {
        let (m, _) = matcher(&[("Canción", false)]);
        assert_eq!(found(&m, "Una CANCIÓN bonita"), vec!["CANCIÓN"]);
        assert!(found(&m, "una cancion bonita").is_empty());
    }

    #[test]
    fn short_titles_ignored_and_alias_flag() {
        let (m, ids) = matcher(&[
            ("ab", false),
            ("Machine Learning", false),
            ("ML tech", true),
        ]);
        assert_eq!(found(&m, "ab is short"), Vec::<String>::new());
        let r = m.find("ml tech rocks");
        assert_eq!(r.len(), 1);
        assert!(r[0].alias);
        assert_eq!(r[0].page, ids[2]);
    }

    #[test]
    fn offsets_survive_length_changing_lowercase() {
        // 'İ' lowercases to two chars ("i̇"), shifting byte offsets.
        let (m, _) = matcher(&[("Rust", false)]);
        let text = "İİ Rust";
        let r = m.find(text);
        assert_eq!(r.len(), 1);
        assert_eq!(&text[r[0].span.clone()], "Rust");
    }
}
