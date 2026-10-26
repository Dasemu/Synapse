//! Tags: topic names ("neurociencia", "finanzas") that notes are filed under.
//!
//! A tag is only a name: it has no page. A page lists its tags in [`Page::tags`] (synced with
//! the page, last writer wins), any number of them. Its type ([`Page::types`], at most one) implies a tag of
//! the same name without being stored twice, and a derived `page_tags` index (tags and types,
//! one row per page and [`title_key`]) answers "which pages have this tag". Names compare
//! case-insensitively. What a page *is* ("persona", "lugar") is the name-only mechanism in
//! `types`.

use crate::error::{Error, Result};
use crate::model::{Page, PageId, title_key};
use crate::schema;
use crate::store::{Graph, parse_id};
use std::collections::{HashMap, HashSet};

/// Longest tag (or type) name, in characters.
pub const MAX_TAG_CHARS: usize = 48;

/// A tag with the number of pages filed under it (explicitly or through a type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagInfo {
    pub name: String,
    pub count: u32,
    /// Part of the seeded suggestions (types only).
    pub suggested: bool,
    /// Emoji of the type ([`crate::TypeDef::icon`]; types only).
    pub icon: Option<String>,
}

/// Cleans a tag name: leading `#` dropped, whitespace collapsed. `None` when nothing is left.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches('#');
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then_some(t)
}

/// Tags sorted by [`title_key`], duplicates (case-insensitive) dropped keeping the first.
pub(crate) fn sorted_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for t in tags {
        if !out.iter().any(|x| title_key(x) == title_key(t)) {
            out.push(t.clone());
        }
    }
    out.sort_by_cached_key(|t| title_key(t));
    out
}

impl Graph {
    /// Replaces the tags of `page` (names cleaned, duplicates dropped case-insensitively,
    /// kept sorted by [`title_key`]). Journals and templates cannot be tagged.
    pub fn set_page_tags(&self, page: PageId, tags: Vec<String>) -> Result<Page> {
        let tx = self.tx()?;
        let mut p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let mut next: Vec<String> = Vec::with_capacity(tags.len());
        for raw in tags {
            let Some(t) = normalize_tag(&raw) else {
                continue;
            };
            if t.chars().count() > MAX_TAG_CHARS {
                return Err(Error::Invalid(format!(
                    "tag {t:?} is longer than {MAX_TAG_CHARS} characters"
                )));
            }
            if !next.iter().any(|x| title_key(x) == title_key(&t)) {
                next.push(t);
            }
        }
        next.sort_by_cached_key(|t| title_key(t));
        if (p.is_journal() || p.is_template()) && !next.is_empty() {
            return Err(Error::Invalid(
                "journals and templates cannot have tags".into(),
            ));
        }
        if next == p.tags {
            tx.commit()?;
            return Ok(p);
        }
        p.tags = next;
        p.updated_at = self.now_ms();
        self.write_page(&p, &self.tick())?;
        tx.commit()?;
        Ok(p)
    }

    /// Adds a tag (no-op when the page already has it).
    pub fn add_page_tag(&self, page: PageId, tag: &str) -> Result<Page> {
        let p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let mut tags = p.tags;
        tags.push(tag.to_string());
        self.set_page_tags(page, tags)
    }

    /// Removes an explicit tag (no-op when the page does not list it; a type of the same name
    /// keeps implying it).
    pub fn remove_page_tag(&self, page: PageId, tag: &str) -> Result<Page> {
        let p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let key = title_key(tag);
        let tags = p.tags.into_iter().filter(|t| title_key(t) != key).collect();
        self.set_page_tags(page, tags)
    }

    /// Pages carrying `tag` (listed or implied by a type), most recently updated first.
    pub fn pages_with_tag(&self, tag: &str) -> Result<Vec<PageId>> {
        let mut st = self.conn().prepare_cached(
            "SELECT p.id FROM page_tags t JOIN pages p ON p.id = t.page_id
             WHERE t.tag_key=?1 ORDER BY p.updated_at DESC, p.id",
        )?;
        let rows = st.query_map([title_key(tag)], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    /// Tags of every tagged page: the listed ones, then the type and the hub title if not
    /// already among them (each implies a tag), names as stored.
    pub fn all_page_tags(&self) -> Result<HashMap<PageId, Vec<String>>> {
        let mut out = HashMap::new();
        for src in schema::tag_sources(self.conn(), None)? {
            let mut all: Vec<String> = Vec::new();
            for t in src.names() {
                if !all.iter().any(|x| title_key(x) == title_key(t)) {
                    all.push(t.clone());
                }
            }
            if !all.is_empty() {
                out.insert(parse_id(&src.id)?, all);
            }
        }
        Ok(out)
    }

    /// Every tag in use (listed, implied by a type or by the page's hub): spelling of its first
    /// page, with its page count. Most used first, then by name.
    pub fn tag_catalog(&self) -> Result<Vec<TagInfo>> {
        let mut used: HashMap<String, (String, u32)> = HashMap::new();
        let mut sources = schema::tag_sources(self.conn(), None)?;
        sources.sort_by(|a, b| a.id.cmp(&b.id));
        for src in sources {
            let mut seen = HashSet::new();
            for t in src.names() {
                let key = title_key(t);
                if seen.insert(key.clone()) {
                    used.entry(key).or_insert((t.clone(), 0)).1 += 1;
                }
            }
        }
        let mut out: Vec<TagInfo> = used
            .into_values()
            .map(|(name, count)| TagInfo {
                name,
                count,
                suggested: false,
                icon: None,
            })
            .collect();
        out.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| title_key(&a.name).cmp(&title_key(&b.name)))
        });
        Ok(out)
    }

    /// Renames a tag everywhere: in the `tags` and `types` of every page carrying it (one
    /// transaction; each page is rewritten and logged, so it syncs). When a page already lists
    /// `new`, the two merge. A type's definition (its icon) moves to the new name, keeping the
    /// icon `new` already had. Returns the number of pages changed. A hub's implicit tag is never renamed (the hub page
    /// is not touched): only explicit tags and types change.
    pub fn rename_tag(&self, old: &str, new: &str) -> Result<usize> {
        let new = normalize_tag(new).ok_or_else(|| Error::Invalid("empty tag name".into()))?;
        if new.chars().count() > MAX_TAG_CHARS {
            return Err(Error::Invalid(format!(
                "tag {new:?} is longer than {MAX_TAG_CHARS} characters"
            )));
        }
        let key = title_key(old);
        let tx = self.tx()?;
        let mut changed = 0;
        for id in self.pages_with_tag(old)? {
            let Some(mut p) = self.get_page(id)? else {
                continue;
            };
            let (tags, types) = (p.tags.clone(), p.types.clone());
            p.tags = rename_in(&p.tags, &key, &new);
            p.tags.sort_by_cached_key(|t| title_key(t));
            p.types = rename_in(&p.types, &key, &new);
            if p.tags != tags || p.types != types {
                p.updated_at = self.now_ms();
                self.write_page(&p, &self.tick())?;
                changed += 1;
            }
        }
        self.rename_type_def(&key, &new)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Removes the explicit tag `name` from every page (types of that name stay, and keep
    /// implying it). Returns the number of pages changed.
    pub fn remove_tag(&self, name: &str) -> Result<usize> {
        let key = title_key(name);
        let tx = self.tx()?;
        let mut changed = 0;
        for id in self.pages_with_tag(name)? {
            let Some(mut p) = self.get_page(id)? else {
                continue;
            };
            let before = p.tags.len();
            p.tags.retain(|t| title_key(t) != key);
            if p.tags.len() != before {
                p.updated_at = self.now_ms();
                self.write_page(&p, &self.tick())?;
                changed += 1;
            }
        }
        tx.commit()?;
        Ok(changed)
    }
}

/// `names` with the entries matching `key` replaced by `new` (later duplicates dropped).
fn rename_in(names: &[String], key: &str, new: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for n in names {
        let n = if title_key(n) == key { new } else { n };
        if !out.iter().any(|x| title_key(x) == title_key(n)) {
            out.push(n.to_string());
        }
    }
    out
}
