//! Tags: topic pages ("neurociencia", "finanzas") that notes are filed under.
//!
//! A tag is a page of kind [`PageKind::Tag`]: it has an outline, backlinks and a place in the
//! graph like any note. A page lists the titles of its tag pages in [`Page::tags`] (synced
//! with the page, last writer wins), any number of them, and a derived `page_tags` index
//! answers "which pages have this tag". Tagging with a title that has no page creates the tag
//! page; tagging with an existing note turns that note into a tag page. Renaming or deleting
//! a tag page keeps its members in step. Names compare case-insensitively ([`title_key`]).
//! What a page *is* ("persona", "lugar") is a different, name-only mechanism: see `types`.

use crate::error::{Error, Result};
use crate::model::{NewPage, Page, PageId, PageKind, title_key};
use crate::store::{Graph, parse_id};
use std::collections::HashMap;

/// Longest tag (or type) name, in characters.
pub const MAX_TAG_CHARS: usize = 48;

/// A tag with the number of pages filed under it (tag pages with no members count 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagInfo {
    pub name: String,
    pub count: u32,
    /// Part of the seeded suggestions (types only; tag pages are never seeded).
    pub suggested: bool,
}

/// Cleans a tag name: leading `#` dropped, whitespace collapsed. `None` when nothing is left.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches('#');
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then_some(t)
}

impl Graph {
    /// The tag page called `name`, created (or made out of a note of that name) on demand.
    fn ensure_tag_page(&self, name: &str) -> Result<Page> {
        if let Some(mut p) = self.page_by_title(name)? {
            return match p.kind {
                PageKind::Tag => Ok(p),
                PageKind::Note if !p.is_journal() => {
                    p.kind = PageKind::Tag;
                    p.updated_at = self.now_ms();
                    self.write_page(&p, &self.tick())?;
                    Ok(p)
                }
                _ => Err(Error::Invalid(format!(
                    "{name:?} is a hub, template or journal and cannot be a tag"
                ))),
            };
        }
        self.create_page_kind_tx(NewPage::titled(name), PageKind::Tag, String::new())
    }

    /// Replaces the tags of `page` (names cleaned, duplicates dropped case-insensitively,
    /// order kept; each becomes the title of a tag page). Journals, templates and tag pages
    /// cannot be tagged, nor can a page be tagged with itself.
    pub fn set_page_tags(&self, page: PageId, tags: Vec<String>) -> Result<Page> {
        let tx = self.tx()?;
        let mut p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let mut wanted: Vec<String> = Vec::with_capacity(tags.len());
        for raw in tags {
            let Some(t) = normalize_tag(&raw) else {
                continue;
            };
            if t.chars().count() > MAX_TAG_CHARS {
                return Err(Error::Invalid(format!(
                    "tag {t:?} is longer than {MAX_TAG_CHARS} characters"
                )));
            }
            if !wanted.iter().any(|x| title_key(x) == title_key(&t)) {
                wanted.push(t);
            }
        }
        if (p.is_journal() || p.is_template() || p.is_tag()) && !wanted.is_empty() {
            return Err(Error::Invalid(
                "journals, templates and tag pages cannot have tags".into(),
            ));
        }
        if wanted.iter().any(|t| title_key(t) == title_key(&p.title)) {
            return Err(Error::Invalid("a page cannot be its own tag".into()));
        }
        let mut next = Vec::with_capacity(wanted.len());
        for t in wanted {
            next.push(self.ensure_tag_page(&t)?.title);
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

    /// Removes a tag (no-op when the page does not have it). The tag page stays.
    pub fn remove_page_tag(&self, page: PageId, tag: &str) -> Result<Page> {
        let p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let key = title_key(tag);
        let tags = p.tags.into_iter().filter(|t| title_key(t) != key).collect();
        self.set_page_tags(page, tags)
    }

    /// Pages carrying `tag`, most recently updated first.
    pub fn pages_with_tag(&self, tag: &str) -> Result<Vec<PageId>> {
        let mut st = self.conn().prepare_cached(
            "SELECT p.id FROM page_tags t JOIN pages p ON p.id = t.page_id
             WHERE t.tag_key=?1 ORDER BY p.updated_at DESC, p.id",
        )?;
        let rows = st.query_map([title_key(tag)], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    /// Tags of every tagged page, as stored.
    pub fn all_page_tags(&self) -> Result<HashMap<PageId, Vec<String>>> {
        let mut st = self
            .conn()
            .prepare_cached("SELECT id, tags FROM pages WHERE tags != '[]'")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, tags) = row?;
            out.insert(
                parse_id(&id)?,
                serde_json::from_str(&tags).unwrap_or_default(),
            );
        }
        Ok(out)
    }

    /// Every tag page with the number of pages under it: most used first, then by name.
    pub fn tag_catalog(&self) -> Result<Vec<TagInfo>> {
        let mut st = self.conn().prepare_cached(
            "SELECT p.title, (SELECT COUNT(*) FROM page_tags t WHERE t.tag_key = p.title_key)
             FROM pages p WHERE p.kind = 'tag'",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (name, count) = row?;
            out.push(TagInfo {
                name,
                count,
                suggested: false,
            });
        }
        out.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| title_key(&a.name).cmp(&title_key(&b.name)))
        });
        Ok(out)
    }

    /// A tag page was renamed: its members now list the new title.
    pub(crate) fn retitle_tag(&self, old: &str, new: &str) -> Result<()> {
        for id in self.pages_with_tag(old)? {
            if let Some(mut p) = self.get_page(id)? {
                let key = title_key(old);
                for t in &mut p.tags {
                    if title_key(t) == key {
                        *t = new.to_string();
                    }
                }
                self.write_page(&p, &self.tick())?;
            }
        }
        Ok(())
    }

    /// A tag page was deleted: its members stop listing it.
    pub(crate) fn drop_tag_from_members(&self, title: &str) -> Result<()> {
        let key = title_key(title);
        for id in self.pages_with_tag(title)? {
            if let Some(mut p) = self.get_page(id)? {
                p.tags.retain(|t| title_key(t) != key);
                self.write_page(&p, &self.tick())?;
            }
        }
        Ok(())
    }

    /// Databases from before tag pages (or snapshots of them) list tags that have no page:
    /// create the missing pages, keeping the spelling first used.
    pub(crate) fn repair_tag_pages(&self) -> Result<()> {
        let missing: Vec<String> = {
            let mut st = self.conn().prepare_cached(
                "SELECT DISTINCT tag_key FROM page_tags
                 WHERE tag_key NOT IN (SELECT title_key FROM pages)",
            )?;
            st.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        if missing.is_empty() {
            return Ok(());
        }
        let tx = self.tx()?;
        let mut done = std::collections::HashSet::new();
        for (_, tags) in self.all_page_tags()? {
            for t in tags {
                let key = title_key(&t);
                if missing.contains(&key) && done.insert(key) {
                    self.ensure_tag_page(&t)?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}
