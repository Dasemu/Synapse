//! Deterministic Logseq-compatible Markdown export of pages.
//!
//! Layout: `pages/<encoded title>.md`, `journals/YYYY_MM_DD.md`, `assets.json`.
//! A page file starts with page properties (`key:: value`, aliases as `alias:: a, b`),
//! then one `- ` bullet per block, nested with tabs. Block properties follow the content;
//! blocks referenced by `((uuid))` carry an `id::` property.

use crate::Result;
use crate::error::Error;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashSet;
use synapse_core::{Block, BlockId, Graph, Page, Properties};

pub const PAGES_DIR: &str = "pages";
pub const JOURNALS_DIR: &str = "journals";
pub const MANIFEST: &str = "assets.json";

/// Encodes a page title as a file name (Logseq "triple-lowbar" format: `/` becomes `___`,
/// characters illegal on Windows and `%` are percent-encoded).
pub fn encode_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut run = 0usize;
    let mut prev_slash = false;
    // Underscore runs next to a `/` or of 3+ would read back as `/`, so they are encoded.
    let flush = |out: &mut String, run: &mut usize, prev_slash: bool, next_slash: bool| {
        let enc = *run >= 3 || prev_slash || next_slash;
        for _ in 0..*run {
            out.push_str(if enc { "%5F" } else { "_" });
        }
        *run = 0;
    };
    for c in title.chars() {
        if c == '_' {
            run += 1;
            continue;
        }
        flush(&mut out, &mut run, prev_slash, c == '/');
        prev_slash = c == '/';
        match c {
            '/' => out.push_str("___"),
            '%' | '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' => {
                out.push_str(&format!("%{:02X}", c as u32))
            }
            c if (c as u32) < 0x20 => out.push_str(&format!("%{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    flush(&mut out, &mut run, prev_slash, false);
    // Windows forbids trailing dots/spaces.
    while out.ends_with('.') || out.ends_with(' ') {
        let c = out.pop().unwrap();
        out.push_str(&format!("%{:02X}", c as u32));
    }
    out
}

/// Inverse of [`encode_title`].
pub fn decode_title(name: &str) -> String {
    let s = name.replace("___", "/");
    let bytes = s.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let Some(v) = hex {
            raw.push(v);
            i += 3;
        } else {
            raw.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

/// Repository-relative path of a page's Markdown file.
pub fn page_path(page: &Page) -> String {
    match page.journal_date {
        Some(d) => format!("{JOURNALS_DIR}/{}.md", d.format("%Y_%m_%d")),
        None => format!("{PAGES_DIR}/{}.md", encode_title(&page.title)),
    }
}

/// Title (or journal date) from a repository path, if it is a page file.
pub fn title_from_path(path: &str) -> Option<String> {
    if let Some(n) = path.strip_prefix("pages/") {
        return n.strip_suffix(".md").map(decode_title);
    }
    let n = path.strip_prefix("journals/")?.strip_suffix(".md")?;
    Some(n.replace('_', "-"))
}

pub(crate) fn render_value(v: &Value) -> String {
    match v {
        Value::String(s) => {
            if !s.contains('\n') && parse_value(s) == Value::String(s.clone()) {
                s.clone()
            } else {
                Value::String(s.clone()).to_string()
            }
        }
        other => other.to_string(),
    }
}

/// Parses a property value: JSON for non-strings and quoted strings, raw text otherwise.
pub(crate) fn parse_value(raw: &str) -> Value {
    let t = raw.trim();
    match serde_json::from_str::<Value>(t) {
        Ok(v) => v,
        Err(_) => Value::String(t.to_string()),
    }
}

fn push_props(out: &mut String, indent: &str, props: &[(String, String)]) {
    for (k, v) in props {
        out.push_str(indent);
        out.push_str(k);
        out.push_str(":: ");
        out.push_str(v);
        out.push('\n');
    }
}

fn sorted_props(p: &Properties) -> Vec<(String, String)> {
    let mut v: Vec<_> = p
        .iter()
        .map(|(k, v)| (k.clone(), render_value(v)))
        .collect();
    v.sort();
    v
}

/// Renders one block (without its children) at `depth`.
pub fn render_block(block: &Block, depth: usize, referenced: &HashSet<BlockId>) -> String {
    let indent = "\t".repeat(depth);
    let mut out = String::new();
    let mut lines = block.content.split('\n');
    let first = lines.next().unwrap_or("");
    out.push_str(&indent);
    out.push('-');
    if !first.is_empty() {
        out.push(' ');
        out.push_str(first);
    }
    out.push('\n');
    let cont = format!("{indent}  ");
    for l in lines {
        out.push_str(&cont);
        out.push_str(l);
        out.push('\n');
    }
    let mut props = block.properties.clone();
    props.remove("id");
    props.remove("collapsed");
    let mut list = sorted_props(&props);
    if block.collapsed {
        list.push(("collapsed".into(), "true".into()));
    }
    if referenced.contains(&block.id) {
        list.push(("id".into(), block.id.to_string()));
    }
    list.sort();
    push_props(&mut out, &cont, &list);
    out
}

/// Renders the full Markdown file of a page.
pub fn render_page(graph: &Graph, page: &Page, referenced: &HashSet<BlockId>) -> Result<String> {
    let mut props = page.properties.clone();
    props.remove("alias");
    let mut list = sorted_props(&props);
    let aliases = graph.aliases_of(page.id)?;
    if !aliases.is_empty() {
        list.push(("alias".into(), aliases.join(", ")));
    }
    list.sort();
    let mut out = String::new();
    push_props(&mut out, "", &list);
    if !list.is_empty() {
        out.push('\n');
    }
    for (depth, b) in graph.page_tree(page.id)? {
        out.push_str(&render_block(&b, depth, referenced));
    }
    Ok(out)
}

/// Every block id that is the target of a `((uuid))` reference or embed.
pub fn referenced_blocks(conn: &Connection) -> Result<HashSet<BlockId>> {
    let mut st = conn.prepare("SELECT DISTINCT target_block FROM block_refs")?;
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    let mut set = HashSet::new();
    for r in rows {
        if let Ok(id) = r?.parse() {
            set.insert(id);
        }
    }
    Ok(set)
}

/// Asset manifest (`assets.json`): sorted by hash, so it is deterministic.
pub fn render_manifest(conn: &Connection) -> Result<String> {
    let mut st =
        conn.prepare("SELECT hash, size, mime, original_name FROM assets ORDER BY hash")?;
    let rows = st.query_map([], |r| {
        Ok(serde_json::json!({
            "hash": r.get::<_, String>(0)?,
            "size": r.get::<_, i64>(1)?,
            "mime": r.get::<_, Option<String>>(2)?,
            "name": r.get::<_, Option<String>>(3)?,
        }))
    })?;
    let list: Vec<Value> = rows.collect::<std::result::Result<_, _>>()?;
    let mut s = serde_json::to_string_pretty(&list).map_err(Error::from)?;
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_encoding_roundtrips() {
        for t in [
            "Simple",
            "a/b/c",
            "100% sure?",
            "snake_case",
            "a___b",
            "x____y/z",
            "x_/_y",
            "end.",
            "q: \"w\" <e> |r| *t*",
            "Ünï/cödé 日本",
            "trail ",
        ] {
            let e = encode_title(t);
            assert!(!e.contains('/') && !e.contains(':'), "{e}");
            assert_eq!(decode_title(&e), t, "{t} -> {e}");
        }
    }
}
