//! Types: what kind of thing a page is ("persona", "lugar", "empresa").
//!
//! Types are plain names from a small vocabulary, not pages: a page carries its own list
//! ([`Page::types`], synced with the page, last writer wins). Topics are a different
//! mechanism: see `tags`.

use crate::error::{Error, Result};
use crate::model::{Page, PageId, title_key};
use crate::store::Graph;
use crate::tags::{MAX_TAG_CHARS, TagInfo, normalize_tag};
use std::collections::HashMap;

/// Types offered in pickers before they are used anywhere (the seed of the vocabulary).
pub const SUGGESTED_TYPES: [&str; 10] = [
    "persona",
    "lugar",
    "empresa",
    "software",
    "sueño",
    "concepto",
    "idea",
    "evento",
    "recurso",
    "receta",
];

impl Graph {
    /// Replaces the types of `page` (names cleaned, duplicates dropped case-insensitively,
    /// order kept). Journals and templates cannot be typed.
    pub fn set_page_types(&self, page: PageId, types: Vec<String>) -> Result<Page> {
        let tx = self.tx()?;
        let mut p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let mut next: Vec<String> = Vec::with_capacity(types.len());
        for raw in types {
            let Some(t) = normalize_tag(&raw) else {
                continue;
            };
            if t.chars().count() > MAX_TAG_CHARS {
                return Err(Error::Invalid(format!(
                    "type {t:?} is longer than {MAX_TAG_CHARS} characters"
                )));
            }
            if !next.iter().any(|x| title_key(x) == title_key(&t)) {
                next.push(t);
            }
        }
        if next == p.types {
            tx.commit()?;
            return Ok(p);
        }
        if (p.is_journal() || p.is_template()) && !next.is_empty() {
            return Err(Error::Invalid(
                "journals and templates cannot have types".into(),
            ));
        }
        p.types = next;
        p.updated_at = self.now_ms();
        self.write_page(&p, &self.tick())?;
        tx.commit()?;
        Ok(p)
    }

    /// Adds a type (no-op when the page already has it).
    pub fn add_page_type(&self, page: PageId, ty: &str) -> Result<Page> {
        let p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let mut types = p.types;
        types.push(ty.to_string());
        self.set_page_types(page, types)
    }

    /// Removes a type (no-op when the page does not have it).
    pub fn remove_page_type(&self, page: PageId, ty: &str) -> Result<Page> {
        let p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let key = title_key(ty);
        let types = p
            .types
            .into_iter()
            .filter(|t| title_key(t) != key)
            .collect();
        self.set_page_types(page, types)
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
            for t in types {
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
