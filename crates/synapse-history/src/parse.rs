//! Parser for the Markdown files produced by [`crate::export`] (and plain outline pages).

use crate::export::{
    DESCRIPTION_PROP, HUBS_PROP, KIND_PROP, TAGS_PROP, TYPE_PROP, TYPES_PROP, parse_value,
};
use serde_json::Value;
use synapse_core::{BlockId, Properties, parse::property_split};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBlock {
    pub depth: usize,
    /// From an `id::` property.
    pub id: Option<BlockId>,
    pub content: String,
    pub properties: Properties,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedPage {
    /// Page properties except `alias` and the `synapse-*` hub properties.
    pub properties: Properties,
    pub aliases: Vec<String>,
    /// `synapse-kind:: hub`.
    pub is_hub: bool,
    /// `synapse-description::`.
    pub description: String,
    /// Titles from `synapse-hubs::`.
    pub hubs: Vec<String>,
    /// Tag names from `synapse-tags::`.
    pub tags: Vec<String>,
    /// The type from `synapse-type::` (or the first of the old `synapse-types::`): 0 or 1 name.
    pub types: Vec<String>,
    pub blocks: Vec<ParsedBlock>,
}

/// Visual width of leading whitespace (a tab counts as 2 columns) and its byte length.
fn indent_of(line: &str) -> (usize, usize) {
    let mut cols = 0;
    let mut bytes = 0;
    for c in line.chars() {
        match c {
            '\t' => cols += 2,
            ' ' => cols += 1,
            _ => break,
        }
        bytes += 1;
    }
    (cols, bytes)
}

fn strip_cols(line: &str, cols: usize) -> &str {
    let mut left = cols;
    let mut bytes = 0;
    for c in line.chars() {
        let w = match c {
            '\t' => 2,
            ' ' => 1,
            _ => break,
        };
        if w > left {
            break;
        }
        left -= w;
        bytes += 1;
        if left == 0 {
            break;
        }
    }
    &line[bytes..]
}

fn bullet_text(trimmed: &str) -> Option<&str> {
    if trimmed == "-" {
        Some("")
    } else {
        trimmed.strip_prefix("- ")
    }
}

struct Raw {
    depth: usize,
    lines: Vec<String>,
}

pub fn parse_page(text: &str) -> ParsedPage {
    let mut page = ParsedPage::default();
    let mut raws: Vec<Raw> = Vec::new();
    let mut stack: Vec<usize> = Vec::new(); // bullet columns of open ancestors
    let mut col_of_current = 0usize;

    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (cols, bytes) = indent_of(line);
        if let Some(rest) = bullet_text(&line[bytes..]) {
            while stack.last().is_some_and(|&c| c >= cols) {
                stack.pop();
            }
            raws.push(Raw {
                depth: stack.len(),
                lines: vec![rest.to_string()],
            });
            stack.push(cols);
            col_of_current = cols;
        } else if let Some(cur) = raws.last_mut() {
            cur.lines
                .push(strip_cols(line, col_of_current + 2).to_string());
        } else if let Some((k, v)) = property_split(line.trim()) {
            if k == "alias" {
                page.aliases.extend(
                    v.split(',')
                        .map(|a| a.trim().trim_matches(['[', ']']).to_string())
                        .filter(|a| !a.is_empty()),
                );
            } else {
                page.properties.insert(k.to_string(), parse_value(v));
            }
        }
    }

    let kind = page.properties.remove(KIND_PROP);
    page.is_hub = matches!(&kind, Some(Value::String(k)) if k == "hub");
    page.description = match page.properties.remove(DESCRIPTION_PROP) {
        Some(Value::String(d)) => d,
        Some(other) => other.to_string(),
        None => String::new(),
    };
    page.hubs = match page.properties.remove(HUBS_PROP) {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::new(),
    };
    page.tags = match page.properties.remove(TAGS_PROP) {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::new(),
    };
    let old_types = page.properties.remove(TYPES_PROP);
    let ty = match (page.properties.remove(TYPE_PROP), old_types) {
        (Some(Value::String(s)), _) => Some(s),
        (_, Some(Value::Array(items))) => items.into_iter().find_map(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        }),
        (_, Some(Value::String(s))) => Some(s),
        _ => None,
    };
    page.types = ty.into_iter().filter(|s| !s.trim().is_empty()).collect();

    for raw in raws {
        let mut lines = raw.lines;
        // Blank separator lines between blocks are not content.
        while lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let mut props = Properties::new();
        let mut split = lines.len();
        while split > 1 {
            match property_split(&lines[split - 1]) {
                Some(_) => split -= 1,
                None => break,
            }
        }
        for l in &lines[split..] {
            if let Some((k, v)) = property_split(l) {
                props.insert(k.to_string(), parse_value(v));
            }
        }
        lines.truncate(split);
        let id = match props.remove("id") {
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        };
        let collapsed = matches!(props.remove("collapsed"), Some(Value::Bool(true)));
        page.blocks.push(ParsedBlock {
            depth: raw.depth,
            id,
            content: lines.join("\n"),
            properties: props,
            collapsed,
        });
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nesting_props_and_multiline() {
        let text = "alias:: A, B\ntype:: note\n\n- root\n\tsecond line\n\tcolor:: red\n\t- child\n\t\t- deep\n- next\n";
        let p = parse_page(text);
        assert_eq!(p.aliases, ["A", "B"]);
        assert_eq!(p.properties["type"], "note");
        let d: Vec<_> = p
            .blocks
            .iter()
            .map(|b| (b.depth, b.content.as_str()))
            .collect();
        assert_eq!(
            d,
            [
                (0, "root\nsecond line"),
                (1, "child"),
                (2, "deep"),
                (0, "next")
            ]
        );
        assert_eq!(p.blocks[0].properties["color"], "red");
    }

    #[test]
    fn space_indent() {
        let p = parse_page("- a\n  - b\n    - c\n  - d\n");
        let d: Vec<_> = p.blocks.iter().map(|b| b.depth).collect();
        assert_eq!(d, [0, 1, 2, 1]);
    }
}
