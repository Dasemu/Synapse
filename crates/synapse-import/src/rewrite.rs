//! Content rewrites applied to every block: asset links and Logseq date page links.

use crate::config::DateFormat;
use crate::names::percent_decode;
use std::collections::HashMap;

const NEEDLE: &str = "../assets/";

/// Replaces `../assets/<path>` with `asset://<blake3>` keeping the surrounding Markdown
/// syntax. Paths not found in `assets` are left untouched and pushed to `missing`.
pub fn rewrite_assets(
    content: &str,
    assets: &HashMap<String, String>,
    missing: &mut Vec<String>,
) -> String {
    if !content.contains(NEEDLE) {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for (idx, _) in content.match_indices(NEEDLE) {
        if idx < last {
            continue;
        }
        let tail = &content[idx..];
        let end = match content[..idx].chars().next_back() {
            Some('(') => tail.find([')', '\n']),
            Some(q @ ('"' | '\'')) => tail.find([q, '\n']),
            _ => tail.find(|c: char| {
                c.is_whitespace() || matches!(c, ')' | ']' | '}' | '"' | '\'' | '<' | '>')
            }),
        }
        .unwrap_or(tail.len());
        let raw = &tail[..end];
        let (path, suffix) = match raw.find(['?', '#']) {
            Some(i) => raw.split_at(i),
            None => (raw, ""),
        };
        let enc = &path[NEEDLE.len()..];
        let dec = percent_decode(enc);
        match assets.get(&dec).or_else(|| assets.get(enc)) {
            Some(hash) => {
                out.push_str(&content[last..idx]);
                out.push_str("asset://");
                out.push_str(hash);
                out.push_str(suffix);
                last = idx + end;
            }
            None => missing.push(dec),
        }
    }
    out.push_str(&content[last..]);
    out
}

/// Rewrites `[[Mar 15th, 2024]]` (the configured journal title format) to `[[2024-03-15]]`,
/// the title Synapse gives journal pages. Code fences are left alone.
pub fn rewrite_dates(content: &str, fmt: &DateFormat) -> String {
    if !content.contains("[[") {
        return content.to_string();
    }
    let mut fence = false;
    let mut lines = Vec::new();
    for line in content.split('\n') {
        if line.trim_start().starts_with("```") {
            fence = !fence;
        }
        lines.push(if fence || !line.contains("[[") {
            line.to_string()
        } else {
            rewrite_line(line, fmt)
        });
    }
    lines.join("\n")
}

fn rewrite_line(line: &str, fmt: &DateFormat) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else { break };
        let inner = &after[..close];
        out.push_str(&rest[..open + 2]);
        match fmt.parse(inner).filter(|_| !inner.contains("[[")) {
            Some(d) => out.push_str(&d.format("%Y-%m-%d").to_string()),
            None => out.push_str(inner),
        }
        out.push_str("]]");
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

pub fn guess_mime(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "txt" | "md" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "html" => "text/html",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_asset_links() {
        let mut map = HashMap::new();
        map.insert("my pic.png".to_string(), "abc".to_string());
        map.insert("doc.pdf".to_string(), "def".to_string());
        let mut missing = Vec::new();
        let s = "![alt](../assets/my%20pic.png) [d](../assets/doc.pdf#p=2) {{video ../assets/nope.mp4}}";
        let out = rewrite_assets(s, &map, &mut missing);
        assert_eq!(
            out,
            "![alt](asset://abc) [d](asset://def#p=2) {{video ../assets/nope.mp4}}"
        );
        assert_eq!(missing, vec!["nope.mp4"]);
    }

    #[test]
    fn rewrites_dates() {
        let f = DateFormat::new("MMM do, yyyy");
        assert_eq!(
            rewrite_dates(
                "see [[Mar 15th, 2024]] and [[Other]] #[[Jan 1st, 2020]]",
                &f
            ),
            "see [[2024-03-15]] and [[Other]] #[[2020-01-01]]"
        );
    }
}
