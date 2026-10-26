//! Types: what kind of thing a page is, one per page ("persona", "lugar", "empresa").
//!
//! Types are plain names from a small vocabulary, not pages: a page carries its own
//! ([`Page::types`], at most one name, kept as a list for compatibility; synced with the page, last writer wins). Topics are a different
//! mechanism: see `tags`.

use crate::error::{Error, Result};
use crate::model::{Page, PageId, title_key};
use crate::store::Graph;
use crate::tags::{MAX_TAG_CHARS, TagInfo, normalize_tag};
use std::collections::HashMap;

/// Types offered in pickers before they are used anywhere (the seed of the vocabulary).
pub const SUGGESTED_TYPES: [&str; 10] = [
    "persona", "lugar", "empresa", "software", "sueño", "concepto", "idea", "evento", "recurso",
    "receta",
];

impl Graph {
    /// Sets the single type of `page` (name cleaned), or clears it with `None`. Journals and
    /// templates cannot be typed. The type is stored as a list of zero or one name.
    pub fn set_page_type(&self, page: PageId, ty: Option<&str>) -> Result<Page> {
        let tx = self.tx()?;
        let mut p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let next: Vec<String> = match ty.and_then(normalize_tag) {
            Some(t) => {
                if t.chars().count() > MAX_TAG_CHARS {
                    return Err(Error::Invalid(format!(
                        "type {t:?} is longer than {MAX_TAG_CHARS} characters"
                    )));
                }
                vec![t]
            }
            None => Vec::new(),
        };
        if next == p.types {
            tx.commit()?;
            return Ok(p);
        }
        if (p.is_journal() || p.is_template()) && !next.is_empty() {
            return Err(Error::Invalid(
                "journals and templates cannot have a type".into(),
            ));
        }
        p.types = next;
        p.updated_at = self.now_ms();
        self.write_page(&p, &self.tick())?;
        tx.commit()?;
        Ok(p)
    }

    /// The vocabulary: every type in use (spelling of its first page) plus the suggestions not
    /// used yet. Used types first by count then name, then unused suggestions in seed order.
    pub fn type_catalog(&self) -> Result<Vec<TagInfo>> {
        let mut st = self.conn().prepare_cached(
            "SELECT types FROM pages WHERE types != '[]' ORDER BY created_at, id",
        )?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut used: HashMap<String, (String, u32)> = HashMap::new();
        for row in rows {
            let types: Vec<String> = serde_json::from_str(&row?).unwrap_or_default();
            for t in types.into_iter().take(1) {
                used.entry(title_key(&t)).or_insert((t, 0)).1 += 1;
            }
        }
        let suggested = |k: &str| SUGGESTED_TYPES.iter().any(|s| title_key(s) == k);
        let mut out: Vec<TagInfo> = used
            .iter()
            .map(|(k, (name, count))| TagInfo {
                name: name.clone(),
                count: *count,
                suggested: suggested(k),
            })
            .collect();
        out.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| title_key(&a.name).cmp(&title_key(&b.name)))
        });
        for s in SUGGESTED_TYPES {
            if !used.contains_key(&title_key(s)) {
                out.push(TagInfo {
                    name: s.to_string(),
                    count: 0,
                    suggested: true,
                });
            }
        }
        Ok(out)
    }
}
