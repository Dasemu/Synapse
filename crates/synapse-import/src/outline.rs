//! Parser for Logseq Markdown outlines (and raw `.org` files) into a block tree.

use serde_json::Value;
use synapse_core::Properties;
use synapse_core::parse::property_split;
use synapse_core::position::between;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PBlock {
    /// Logseq `id::` when present and valid.
    pub explicit_id: Option<Uuid>,
    /// Index of the parent in the block list.
    pub parent: Option<usize>,
    pub position: String,
    pub content: String,
    pub properties: Properties,
    pub collapsed: bool,
}

#[derive(Debug, Default)]
pub struct Outline {
    /// Page-level properties (first property-only block or lines before the first bullet).
    pub page_props: Properties,
    pub blocks: Vec<PBlock>,
}

struct Raw {
    indent: usize,
    parent: Option<usize>,
    lines: Vec<String>,
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Removes up to `width` columns of leading whitespace.
fn strip_indent(line: &str, width: usize) -> &str {
    let mut w = 0;
    for (i, c) in line.char_indices() {
        if w >= width || (c != ' ' && c != '\t') {
            return &line[i..];
        }
        w += if c == '\t' { 4 } else { 1 };
    }
    ""
}

fn bullet_text(line: &str) -> Option<&str> {
    let t = line.trim_start_matches([' ', '\t']);
    if t == "-" {
        Some("")
    } else {
        t.strip_prefix("- ")
    }
}

fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

pub fn parse_markdown(text: &str) -> Outline {
    let mut pre: Vec<&str> = Vec::new();
    let mut raws: Vec<Raw> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let bullet = if in_fence { None } else { bullet_text(line) };
        if let Some(first) = bullet {
            let indent = indent_width(line);
            while stack.last().is_some_and(|&i| raws[i].indent >= indent) {
                stack.pop();
            }
            raws.push(Raw {
                indent,
                parent: stack.last().copied(),
                lines: vec![first.to_string()],
            });
            stack.push(raws.len() - 1);
            in_fence = is_fence(first);
        } else if let Some(cur) = raws.last_mut() {
            if is_fence(line) {
                in_fence = !in_fence;
            }
            cur.lines
                .push(strip_indent(line, cur.indent + 2).to_string());
        } else {
            pre.push(line);
        }
    }

    let mut out = Outline::default();
    let mut pre_text_lines = parse_pre(&pre, &mut out.page_props);
    // Plain Markdown without bullets: one root block per paragraph.
    let mut lead: Vec<Raw> = Vec::new();
    if raws.is_empty() {
        for para in split_paragraphs(&mut pre_text_lines) {
            lead.push(Raw {
                indent: 0,
                parent: None,
                lines: para,
            });
        }
    } else if pre_text_lines.iter().any(|l| !l.trim().is_empty()) {
        lead.push(Raw {
            indent: 0,
            parent: None,
            lines: pre_text_lines,
        });
    }
    let shift = lead.len();
    let all: Vec<Raw> = lead
        .into_iter()
        .chain(raws.into_iter().map(|mut r| {
            r.parent = r.parent.map(|p| p + shift);
            r
        }))
        .collect();

    // Build blocks; positions are assigned sequentially per parent.
    let mut last_pos: std::collections::HashMap<Option<usize>, String> = Default::default();
    let mut first_block_props = false;
    let first_has_kids = all.iter().any(|r| r.parent == Some(0));
    for (i, raw) in all.into_iter().enumerate() {
        let (content, props) = split_props(raw.lines);
        let (explicit_id, collapsed, props) = take_structural(props);
        if i == 0
            && raw.parent.is_none()
            && content.trim().is_empty()
            && !props.is_empty()
            && out.page_props.is_empty()
            && !first_has_kids
        {
            // First block holds only properties: they belong to the page.
            first_block_props = true;
            out.page_props = props;
            continue;
        }
        let position = between(last_pos.get(&raw.parent).map(String::as_str), None);
        last_pos.insert(raw.parent, position.clone());
        out.blocks.push(PBlock {
            explicit_id,
            parent: raw.parent,
            position,
            content,
            properties: props,
            collapsed,
        });
    }
    fix_parents(&mut out.blocks, first_block_props);
    out
}

/// When the first block was consumed as page properties, every stored parent index (which
/// counted that block) is shifted down by one.
fn fix_parents(blocks: &mut [PBlock], skipped_first: bool) {
    if skipped_first {
        for b in blocks {
            b.parent = b.parent.map(|p| p - 1);
        }
    }
}

/// Splits the lines before the first bullet into page properties and leftover text.
fn parse_pre(pre: &[&str], props: &mut Properties) -> Vec<String> {
    let mut lines: Vec<&str> = pre.to_vec();
    // YAML front matter.
    if lines.first().is_some_and(|l| l.trim() == "---")
        && let Some(end) = lines[1..].iter().position(|l| l.trim() == "---")
    {
        for l in &lines[1..=end] {
            if let Some((k, v)) = l.split_once(':') {
                props.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
            }
        }
        lines.drain(..end + 2);
    }
    let mut i = 0;
    while i < lines.len() {
        if let Some((k, v)) = property_split(lines[i]) {
            props.insert(k.to_string(), Value::String(v.to_string()));
        } else if !lines[i].trim().is_empty() {
            break;
        }
        i += 1;
    }
    lines[i..].iter().map(|s| s.to_string()).collect()
}

fn split_paragraphs(lines: &mut Vec<String>) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut fence = false;
    for l in lines.drain(..) {
        if is_fence(&l) {
            fence = !fence;
        }
        if l.trim().is_empty() && !fence {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Separates `key:: value` lines (right after the first line, or from the start when the first
/// line is itself a property) from the text content.
fn split_props(mut lines: Vec<String>) -> (String, Properties) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut props = Properties::new();
    let first_is_prop = lines.first().is_some_and(|l| property_split(l).is_some());
    let start = usize::from(!first_is_prop).min(lines.len());
    let mut end = start;
    while end < lines.len() {
        match property_split(&lines[end]) {
            Some((k, v)) => {
                props.insert(k.to_string(), Value::String(v.to_string()));
                end += 1;
            }
            None => break,
        }
    }
    lines.drain(start..end);
    // Properties may also trail a multi-line block.
    let mut tail = lines.len();
    while tail > 1 && property_split(&lines[tail - 1]).is_some() {
        tail -= 1;
    }
    if tail < lines.len()
        && !lines[..tail]
            .iter()
            .filter(|l| is_fence(l))
            .count()
            .is_multiple_of(2)
    {
        tail = lines.len();
    }
    for l in lines.drain(tail..) {
        if let Some((k, v)) = property_split(&l) {
            props.insert(k.to_string(), Value::String(v.to_string()));
        }
    }
    (lines.join("\n"), props)
}

fn take_structural(mut props: Properties) -> (Option<Uuid>, bool, Properties) {
    let id = props
        .remove("id")
        .and_then(|v| v.as_str().and_then(|s| Uuid::parse_str(s.trim()).ok()));
    let collapsed = props
        .remove("collapsed")
        .is_some_and(|v| v.as_str().is_some_and(|s| s.trim() == "true"));
    (id, collapsed, props)
}

/// `.org` files are imported as raw text: one root block per top-level heading.
pub fn parse_org_raw(text: &str) -> Vec<PBlock> {
    let mut chunks: Vec<Vec<&str>> = vec![Vec::new()];
    for l in text.lines() {
        if l.starts_with("* ") && chunks.last().is_some_and(|c| !c.is_empty()) {
            chunks.push(Vec::new());
        }
        chunks.last_mut().unwrap().push(l);
    }
    let mut prev: Option<String> = None;
    let mut out = Vec::new();
    for c in chunks {
        let content = c.join("\n").trim_end().to_string();
        if content.trim().is_empty() {
            continue;
        }
        let position = between(prev.as_deref(), None);
        prev = Some(position.clone());
        out.push(PBlock {
            explicit_id: None,
            parent: None,
            position,
            content,
            properties: Properties::new(),
            collapsed: false,
        });
    }
    out
}

/// Aliases from an `alias::` value: comma separated, optionally `[[wrapped]]`.
pub fn split_aliases(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in value.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = (depth - 1).max(0),
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter()
        .map(|a| {
            a.trim()
                .trim_start_matches("[[")
                .trim_end_matches("]]")
                .trim()
                .to_string()
        })
        .filter(|a| !a.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bullets_do_not_panic() {
        for text in ["-", "-\n- a", "- a\n\t-\n", "\n-\n\n"] {
            parse_markdown(text);
        }
    }

    #[test]
    fn nesting_and_multiline() {
        let o = parse_markdown(
            "- a\n\t- b\n\t  more\n\t  k:: v\n\t\t- c\n- d\n  ```\n  - not a bullet\n  ```\n",
        );
        let b = &o.blocks;
        assert_eq!(b.len(), 4);
        assert_eq!(b[1].parent, Some(0));
        assert_eq!(b[1].content, "b\nmore");
        assert_eq!(b[1].properties["k"], "v");
        assert_eq!(b[2].parent, Some(1));
        assert_eq!(b[3].parent, None);
        assert!(b[3].content.contains("- not a bullet"));
        assert!(b[0].position < b[3].position);
    }

    #[test]
    fn page_props_and_structural() {
        let o = parse_markdown(
            "alias:: x, [[y z]]\n\n- id:: 11111111-1111-4111-8111-111111111111\n  hello\n  collapsed:: true\n  - child\n",
        );
        assert_eq!(o.page_props["alias"], "x, [[y z]]");
        assert_eq!(o.blocks.len(), 2);
        assert!(o.blocks[0].explicit_id.is_some());
        assert!(o.blocks[0].collapsed);
        assert_eq!(o.blocks[1].parent, Some(0));
        let o = parse_markdown("- title:: T\n  alias:: a\n- real\n  - kid\n");
        assert_eq!(o.page_props["title"], "T");
        assert_eq!(o.blocks.len(), 2);
        assert_eq!(o.blocks[1].parent, Some(0));
    }

    #[test]
    fn aliases() {
        assert_eq!(split_aliases("a, [[b, c]], d"), vec!["a", "b, c", "d"]);
    }
}
