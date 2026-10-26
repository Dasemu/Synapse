//! Types: what kind of thing a page is, one per page ("persona", "lugar", "empresa").
//!
//! Types are plain names from a small vocabulary, not pages: a page carries its own
//! ([`Page::types`], at most one name, kept as a list for compatibility; synced with the page, last writer wins). Topics are a different
//! mechanism: see `tags`.
//!
//! Graph-wide metadata of a type (its emoji icon) is a [`TypeDef`] in the `type_defs` table,
//! keyed by [`title_key`] and synced with its own op ([`Op::PutTypeDef`], last writer wins per
//! name). A definition also keeps an unused type in the vocabulary; a removed one is a
//! tombstone that hides the type (even a seeded suggestion).

use crate::error::{Error, Result};
use crate::model::{Op, Page, PageId, TypeDef, title_key};
use crate::store::Graph;
use crate::tags::{MAX_TAG_CHARS, TagInfo, normalize_tag};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

/// Types offered in pickers before they are used anywhere (the seed of the vocabulary).
pub const SUGGESTED_TYPES: [&str; 10] = [
    "persona", "lugar", "empresa", "software", "sueño", "concepto", "idea", "evento", "recurso",
    "receta",
];

/// Longest icon, in characters (emoji sequences such as families or flags take several).
pub const MAX_ICON_CHARS: usize = 16;

/// Cleans a type icon: trimmed; `None` when empty. Refused when longer than
/// [`MAX_ICON_CHARS`], when it contains whitespace or control characters, or when it is plain
/// ASCII (an icon is an emoji or symbol, not a word).
pub fn normalize_icon(raw: &str) -> Result<Option<String>> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > MAX_ICON_CHARS
        || t.chars().any(|c| c.is_whitespace() || c.is_control())
        || t.is_ascii()
    {
        return Err(Error::Invalid(format!("{t:?} is not an emoji icon")));
    }
    Ok(Some(t.to_string()))
}

fn checked_type_name(name: &str) -> Result<String> {
    let n = normalize_tag(name).ok_or_else(|| Error::Invalid("empty type name".into()))?;
    if n.chars().count() > MAX_TAG_CHARS {
        return Err(Error::Invalid(format!(
            "type {n:?} is longer than {MAX_TAG_CHARS} characters"
        )));
    }
    Ok(n)
}

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

    /// The vocabulary: every type in use (spelling of its first page), then types defined but
    /// unused (by name), then the suggestions not used yet in seed order (unless removed).
    /// Used types are sorted by count then name. Each carries its icon.
    pub fn type_catalog(&self) -> Result<Vec<TagInfo>> {
        let defs = self.type_defs()?;
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
        let live_icon = |k: &str| {
            defs.get(k)
                .filter(|d| !d.removed)
                .and_then(|d| d.icon.clone())
        };
        let mut out: Vec<TagInfo> = used
            .iter()
            .map(|(k, (name, count))| TagInfo {
                name: name.clone(),
                count: *count,
                suggested: suggested(k),
                icon: live_icon(k),
            })
            .collect();
        out.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| title_key(&a.name).cmp(&title_key(&b.name)))
        });
        // Defined but unused types (created explicitly, or given an icon), by name.
        let mut unused: Vec<&TypeDef> = defs
            .iter()
            .filter(|(k, d)| !d.removed && !used.contains_key(*k) && !suggested(k))
            .map(|(_, d)| d)
            .collect();
        unused.sort_by_cached_key(|d| title_key(&d.name));
        out.extend(unused.into_iter().map(|d| TagInfo {
            name: d.name.clone(),
            count: 0,
            suggested: false,
            icon: d.icon.clone(),
        }));
        for s in SUGGESTED_TYPES {
            let k = title_key(s);
            let def = defs.get(&k);
            if !used.contains_key(&k) && !def.is_some_and(|d| d.removed) {
                out.push(TagInfo {
                    name: def.map_or_else(|| s.to_string(), |d| d.name.clone()),
                    count: 0,
                    suggested: true,
                    icon: def.and_then(|d| d.icon.clone()),
                });
            }
        }
        Ok(out)
    }

    /// Every stored type definition (removed ones included), by name key.
    pub fn type_defs(&self) -> Result<HashMap<String, TypeDef>> {
        let mut st = self
            .conn()
            .prepare_cached("SELECT type_key, name, icon, removed FROM type_defs")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TypeDef {
                    name: r.get(1)?,
                    icon: r.get(2)?,
                    removed: r.get(3)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The definition of one type name (case-insensitive), removed ones included.
    pub fn type_def(&self, name: &str) -> Result<Option<TypeDef>> {
        Ok(self
            .row(
                "SELECT name, icon, removed FROM type_defs WHERE type_key=?1",
                [title_key(name)],
                |r| {
                    Ok(TypeDef {
                        name: r.get(0)?,
                        icon: r.get(1)?,
                        removed: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Writes and logs a type definition (synced as [`Op::PutTypeDef`]).
    fn put_type_def(&self, def: TypeDef) -> Result<()> {
        let hlc = self.tick();
        self.write_type_def_row(&def, &hlc)?;
        self.log(&hlc, &Op::PutTypeDef { def })
    }

    /// How a type name is spelled: its definition, else its first page, else `name`.
    fn type_spelling(&self, name: &str) -> Result<String> {
        if let Some(d) = self.type_def(name)?.filter(|d| !d.removed) {
            return Ok(d.name);
        }
        let key = title_key(name);
        Ok(self
            .type_catalog()?
            .into_iter()
            .find(|t| t.count > 0 && title_key(&t.name) == key)
            .map_or_else(|| name.to_string(), |t| t.name))
    }

    /// Adds a type to the vocabulary even if no page uses it (no-op when it is already
    /// defined). Returns its name as spelled in the graph.
    pub fn create_type(&self, name: &str) -> Result<String> {
        let name = checked_type_name(name)?;
        let tx = self.tx()?;
        if let Some(d) = self.type_def(&name)?.filter(|d| !d.removed) {
            tx.commit()?;
            return Ok(d.name);
        }
        let name = self.type_spelling(&name)?;
        self.put_type_def(TypeDef {
            name: name.clone(),
            icon: None,
            removed: false,
        })?;
        tx.commit()?;
        Ok(name)
    }

    /// Sets (or with `None` clears) the emoji of a type, defining the type if needed (see
    /// [`normalize_icon`] for what an icon may be).
    pub fn set_type_icon(&self, name: &str, icon: Option<&str>) -> Result<()> {
        let name = checked_type_name(name)?;
        let icon = match icon {
            Some(i) => normalize_icon(i)?,
            None => None,
        };
        let tx = self.tx()?;
        let current = self.type_def(&name)?.filter(|d| !d.removed);
        if current.as_ref().is_some_and(|d| d.icon == icon) {
            tx.commit()?;
            return Ok(());
        }
        let name = self.type_spelling(&name)?;
        self.put_type_def(TypeDef {
            name,
            icon,
            removed: false,
        })?;
        tx.commit()
    }

    /// Deletes a type: pages of that type lose it (their explicit tags stay) and its
    /// definition becomes a tombstone (icon dropped; a seeded suggestion of that name is
    /// hidden too). Returns the number of pages changed.
    pub fn remove_type(&self, name: &str) -> Result<usize> {
        let name = checked_type_name(name)?;
        let key = title_key(&name);
        let tx = self.tx()?;
        let mut changed = 0;
        for id in self.pages_with_tag(&name)? {
            let Some(mut p) = self.get_page(id)? else {
                continue;
            };
            if p.page_type().is_some_and(|t| title_key(t) == key) {
                p.types.clear();
                p.updated_at = self.now_ms();
                self.write_page(&p, &self.tick())?;
                changed += 1;
            }
        }
        let name = self.type_def(&name)?.map_or(name, |d| d.name);
        self.put_type_def(TypeDef {
            name,
            icon: None,
            removed: true,
        })?;
        tx.commit()?;
        Ok(changed)
    }

    /// Renames a type: like [`Graph::rename_tag`] (pages' types and same-name tags, merging
    /// duplicates), and the type stays in the vocabulary under `new` with its icon even when
    /// no page uses it (e.g. an unused suggestion). Returns the number of pages changed.
    pub fn rename_type(&self, old: &str, new: &str) -> Result<usize> {
        checked_type_name(new)?;
        let tx = self.tx()?;
        self.create_type(old)?;
        let n = self.rename_tag(old, new)?;
        tx.commit()?;
        Ok(n)
    }

    /// Moves the definition of type `old_key` to `new` (used by `rename_tag`): `new` keeps its
    /// own icon if it has one, else takes the old one; the old name becomes a tombstone (only
    /// the spelling changes when the keys match). No-op when `old_key` has no live definition.
    pub(crate) fn rename_type_def(&self, old_key: &str, new: &str) -> Result<()> {
        let Some(old) = self.type_def(old_key)?.filter(|d| !d.removed) else {
            return Ok(());
        };
        if title_key(new) == old_key {
            if old.name != new {
                self.put_type_def(TypeDef {
                    name: new.to_string(),
                    ..old
                })?;
            }
            return Ok(());
        }
        let target = self.type_def(new)?.filter(|d| !d.removed);
        let icon = target.and_then(|d| d.icon).or(old.icon);
        self.put_type_def(TypeDef {
            name: new.to_string(),
            icon,
            removed: false,
        })?;
        self.put_type_def(TypeDef {
            name: old.name,
            icon: None,
            removed: true,
        })
    }
}
