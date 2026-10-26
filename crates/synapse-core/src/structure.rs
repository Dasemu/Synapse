//! Organisation of pages: namespaces (title kinship, `A/B` is a child of `A`) and hubs.
//!
//! A hub is a page *kind* ([`PageKind::Hub`]): a container of pages (notes and other hubs)
//! with a short description and no blocks. Membership lives on the member side, like tags:
//! [`Page::hubs`] lists the hubs a page belongs to (synced with the page, last writer wins), and
//! the local `page_hubs` table indexes it for "members of a hub". A page belongs to at most one hub;
//! hub nesting is a DAG (cycles are refused locally and ignored on read, see
//! [`Graph::structure`]). Notes become hubs only by explicit conversion
//! ([`Graph::convert_to_hub`]).

use crate::error::{Error, Result};
use crate::model::{BlockId, NewPage, Page, PageId, PageKind, Placement, RefKind, title_key};
use crate::parse::{extract_refs, find_close, property_split};
use crate::store::{Graph, parse_id};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Longest suggested hub description, in characters.
const MAX_DESCRIPTION_CHARS: usize = 280;

#[derive(Debug, Clone, PartialEq)]
pub struct HubInfo {
    pub page: PageId,
    pub title: String,
    pub description: String,
    /// Member pages: sub-hubs first, then notes, by title key (existing, non-journal pages).
    pub members: Vec<PageId>,
    /// Nesting level: 0 for root hubs (not a member of any hub).
    pub depth: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Structure {
    /// Every hub, sorted by title key.
    pub hubs: Vec<HubInfo>,
    /// Page → its deepest hub (a hub maps to itself). Pages in no hub are absent.
    pub home: HashMap<PageId, PageId>,
}

/// What converting a note into a hub would produce (defaults for the conversion dialog).
#[derive(Debug, Clone, PartialEq)]
pub struct HubPreview {
    /// Last namespace segment of the title (`A/B/C` → `C`), trimmed.
    pub suggested_title: String,
    /// First prose block as plain text (refs rendered as titles), at most 280 characters.
    pub suggested_description: String,
    /// Candidate members in order: pages linked or tagged from the outline (outline order),
    /// then pages whose page properties reference this page, then pages with blocks tagging
    /// it or referencing it in a block property. Existing non-journal pages, no duplicates.
    pub members: Vec<PageId>,
    /// Blocks with text beyond links, headings and properties.
    pub prose_blocks: u32,
    pub blocks: u32,
}

/// Parameters of [`Graph::convert_to_hub`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HubConversion {
    /// New title (the page is renamed, links rewritten, when it differs).
    pub title: String,
    pub description: String,
    pub members: Vec<PageId>,
    /// Title of a new note that receives the whole original outline (moved, structure kept)
    /// and becomes a member of the hub. `None` deletes the blocks.
    pub keep_outline_as: Option<String>,
}

/// Parent namespace of a title: everything before the last `/`. Titles with leading, trailing
/// or empty segments have none.
pub fn namespace_parent(title: &str) -> Option<&str> {
    let title = title.trim();
    let (parent, _) = title.rsplit_once('/')?;
    title
        .split('/')
        .all(|s| !s.trim().is_empty())
        .then(|| parent.trim_end())
}

/// Logseq's organisational built-in pages (and PDF highlight pages), useless in Synapse.
pub fn is_legacy_page(title: &str) -> bool {
    const BUILTIN: [&str; 15] = [
        "contents",
        "journals",
        "favorites",
        "card",
        "all pages",
        "now",
        "later",
        "todo",
        "doing",
        "done",
        "canceled",
        "cancelled",
        "in-progress",
        "wait",
        "waiting",
    ];
    let key = title_key(title);
    key.starts_with("hls__") || BUILTIN.contains(&key.as_str())
}

/// Strings found anywhere inside a property value.
fn value_strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| value_strings(x, out)),
        _ => {}
    }
}

/// Block text without its `key:: value` lines.
fn content_lines(content: &str) -> String {
    content
        .lines()
        .filter(|l| property_split(l).is_none())
        .collect::<Vec<_>>()
        .join("\n")
}

fn heading_level(text: &str) -> usize {
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && text[hashes..].starts_with(char::is_whitespace) {
        hashes
    } else {
        0
    }
}

fn is_pure_heading(text: &str) -> bool {
    heading_level(text) > 0 && !text.contains('\n')
}

/// Number of words left in `text` once refs and separators are removed.
fn remaining_words(text: &str) -> usize {
    let mut plain = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let (skip, close) = if rest.starts_with("#[[") {
            (3, Some("]]"))
        } else if rest.starts_with("[[") {
            (2, Some("]]"))
        } else if rest.starts_with("((") {
            (2, Some("))"))
        } else {
            (0, None)
        };
        if let Some(close) = close {
            let end = if close == "]]" {
                find_close(text, i + skip)
            } else {
                text[i + skip..].find("))").map(|j| i + skip + j)
            };
            if let Some(end) = end {
                plain.push(' ');
                i = end + 2;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        let tag_start = c == '#'
            && !rest[1..].starts_with(['#', '[', ' '])
            && (i == 0 || text[..i].ends_with(char::is_whitespace));
        if tag_start {
            let body = &rest[1..];
            let len = body
                .find(|c: char| c.is_whitespace() || ",.;:!?()[]{}\"'".contains(c))
                .unwrap_or(body.len());
            plain.push(' ');
            i += 1 + len;
            continue;
        }
        plain.push(if ",;:-–—·|*>".contains(c) {
            ' '
        } else {
            c
        });
        i += c.len_utf8();
    }
    plain
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// A block with real text: not empty, not a bare heading, with words besides its refs.
fn is_prose(text: &str) -> bool {
    !text.is_empty()
        && !(is_pure_heading(text) && extract_refs(text).pages.is_empty())
        && remaining_words(&text[heading_level(text)..]) > 0
}

/// Plain text of a block: refs rendered as their titles, Markdown and Logseq markup removed,
/// whitespace collapsed.
fn plain_text(text: &str) -> String {
    const MARKERS: [&str; 11] = [
        "TODO ",
        "DOING ",
        "DONE ",
        "LATER ",
        "NOW ",
        "WAITING ",
        "WAIT ",
        "CANCELED ",
        "CANCELLED ",
        "IN-PROGRESS ",
        "[#",
    ];
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut line = line.trim();
        line = line[heading_level(line)..].trim_start();
        for m in MARKERS {
            if let Some(rest) = line.strip_prefix(m) {
                line = if m == "[#" {
                    rest.split_once(']').map_or(rest, |(_, r)| r).trim_start()
                } else {
                    rest.trim_start()
                };
            }
        }
        out.push(' ');
        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            if let Some(after) = rest.strip_prefix("![")
                && let Some(end) = after.find("](")
                && let Some(close) = after[end..].find(')')
            {
                i += 2 + end + close + 1;
                continue;
            }
            if rest.starts_with("#[[") || rest.starts_with("[[") {
                let skip = if rest.starts_with('#') { 3 } else { 2 };
                if let Some(end) = find_close(line, i + skip) {
                    out.push_str(&line[i + skip..end]);
                    i = end + 2;
                    continue;
                }
            }
            if let Some(after) = rest.strip_prefix('[')
                && let Some(end) = after.find("](")
                && let Some(close) = after[end..].find(')')
            {
                out.push_str(&after[..end]);
                i += 1 + end + close + 1;
                continue;
            }
            if let Some(after) = rest.strip_prefix("((")
                && let Some(end) = after.find("))")
            {
                i += 2 + end + 2;
                continue;
            }
            if let Some(after) = rest.strip_prefix("{{")
                && let Some(end) = after.find("}}")
            {
                i += 2 + end + 2;
                continue;
            }
            if rest.starts_with('#')
                && !rest[1..].starts_with(['#', ' '])
                && (i == 0 || line[..i].ends_with(char::is_whitespace))
            {
                i += 1; // `#tag` → `tag`
                continue;
            }
            if let Some(m) = ["**", "__", "~~", "==", "^^"]
                .iter()
                .find(|m| rest.starts_with(**m))
            {
                i += m.len();
                continue;
            }
            let c = rest.chars().next().unwrap_or(' ');
            if c != '`' && c != '*' {
                out.push(c);
            }
            i += c.len_utf8();
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cuts `text` to at most `max` characters at a word boundary, marking the cut with `…`.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max - 1).collect();
    let whole_word = text[head.len()..].starts_with(char::is_whitespace);
    let cut = match head.rfind(char::is_whitespace) {
        Some(i) if i > 0 && !whole_word => &head[..i],
        _ => head.as_str(),
    };
    format!(
        "{}…",
        cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:".contains(c))
    )
}

struct PageRow {
    title: String,
    key: String,
    journal: bool,
    kind: PageKind,
    description: String,
    hubs: Vec<PageId>,
    hlc: String,
}

impl Graph {
    // ------------------------------------------------------------- hubs

    /// Creates an empty hub.
    pub fn create_hub(&self, title: &str, description: &str) -> Result<Page> {
        let tx = self.tx()?;
        let page = self.create_page_kind_tx(
            NewPage::titled(title),
            PageKind::Hub,
            description.trim().to_string(),
        )?;
        self.ensure_hub_template(&page)?;
        tx.commit()?;
        Ok(page)
    }

    /// The template of `hub`: the page whose outline new notes of the hub start from. Every hub
    /// has exactly one (possibly empty); hubs created before templates existed, or whose
    /// template was lost to a concurrent edit, get a new one here.
    pub fn hub_template(&self, hub: PageId) -> Result<PageId> {
        let tx = self.tx()?;
        let page = self.hub_page(hub)?;
        let id = self.ensure_hub_template(&page)?;
        tx.commit()?;
        Ok(id)
    }

    /// Creates a note in `hub` titled `title`, with a copy of the hub's template outline.
    pub fn create_page_from_template(&self, hub: PageId, title: &str) -> Result<Page> {
        self.transaction(|g| {
            let hub_page = g.hub_page(hub)?;
            let template = g.ensure_hub_template(&hub_page)?;
            let page = g.create_page_tx(NewPage::titled(title))?;
            let mut copies: HashMap<BlockId, BlockId> = HashMap::new();
            for (_, b) in g.page_tree(template)? {
                let at = Placement::LastChildOf(b.parent_id.and_then(|p| copies.get(&p).copied()));
                let copy = g.insert_block(page.id, at, &b.content)?;
                if !b.properties.is_empty() {
                    g.set_block_properties(copy.id, b.properties.clone())?;
                }
                if b.collapsed {
                    g.set_collapsed(copy.id, true)?;
                }
                copies.insert(b.id, copy.id);
            }
            g.add_to_hub(page.id, hub)?;
            g.get_page(page.id)?
                .ok_or_else(|| Error::NotFound(page.id.to_string()))
        })
    }

    /// Ids of the template pages that belong to `hub` (one, unless replicas raced).
    pub(crate) fn hub_template_ids(&self, hub: PageId) -> Result<Vec<PageId>> {
        let mut st = self.conn().prepare_cached(
            "SELECT p.id FROM page_hubs ph JOIN pages p ON p.id = ph.page_id
             WHERE ph.hub_id=?1 AND p.kind='template' ORDER BY p.created_at, p.id",
        )?;
        let rows = st.query_map([hub.to_string()], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    /// Returns the template of `hub`, creating an empty one when there is none. When several
    /// exist the oldest wins.
    fn ensure_hub_template(&self, hub: &Page) -> Result<PageId> {
        if let Some(&t) = self.hub_template_ids(hub.id)?.first() {
            return Ok(t);
        }
        let base = format!("Template: {}", hub.title);
        let mut title = base.clone();
        let mut n = 2;
        while self.page_by_title(&title)?.is_some() {
            title = format!("{base} ({n})");
            n += 1;
        }
        let t =
            self.create_page_kind_tx(NewPage::titled(title), PageKind::Template, String::new())?;
        self.add_to_hub(t.id, hub.id)?;
        Ok(t.id)
    }

    pub fn set_hub_description(&self, hub: PageId, description: &str) -> Result<Page> {
        let tx = self.tx()?;
        let mut page = self.hub_page(hub)?;
        let description = description.trim();
        if page.description != description {
            page.description = description.to_string();
            page.updated_at = self.now_ms();
            self.write_page(&page, &self.tick())?;
        }
        tx.commit()?;
        Ok(page)
    }

    /// Members of `hub`: sub-hubs first, then notes, each by title key (existing, non-journal
    /// pages whose `hubs` contain it).
    pub fn hub_members(&self, hub: PageId) -> Result<Vec<PageId>> {
        let mut st = self.conn().prepare_cached(
            "SELECT p.id FROM page_hubs ph JOIN pages p ON p.id = ph.page_id
             WHERE ph.hub_id=?1 AND p.id != ?1 AND p.journal_date IS NULL AND p.kind != 'template'
             ORDER BY p.kind != 'hub', p.title_key",
        )?;
        let rows = st.query_map([hub.to_string()], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    /// Hubs containing `page` (its own `hubs` that still exist as hubs), by title key.
    pub fn hubs_of(&self, page: PageId) -> Result<Vec<PageId>> {
        let mut st = self.conn().prepare_cached(
            "SELECT h.id FROM page_hubs ph JOIN pages h ON h.id = ph.hub_id
             WHERE ph.page_id=?1 AND h.kind='hub' AND h.id != ?1 AND h.journal_date IS NULL
             ORDER BY h.title_key",
        )?;
        let rows = st.query_map([page.to_string()], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    /// Sets the hub `page` belongs to (at most one; duplicates dropped). Newly added hubs must
    /// be hub pages other than `page` and not nested inside it (no cycles); journals belong to
    /// no hub. Ids already in the list are kept as they are, even when they dangle.
    pub fn set_page_hubs(&self, page: PageId, hubs: Vec<PageId>) -> Result<Page> {
        let tx = self.tx()?;
        let mut p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let mut next: Vec<PageId> = Vec::with_capacity(hubs.len());
        for h in hubs {
            if !next.contains(&h) {
                next.push(h);
            }
        }
        if next == p.hubs {
            tx.commit()?;
            return Ok(p);
        }
        if next.len() > 1 {
            return Err(Error::Invalid("a page can belong to only one hub".into()));
        }
        if p.is_template() && !p.hubs.is_empty() {
            return Err(Error::Invalid("a template stays in its hub".into()));
        }
        if p.is_journal() && !next.is_empty() {
            return Err(Error::Invalid("journals cannot be in hubs".into()));
        }
        let mut inside: Option<HashSet<PageId>> = None;
        for &h in &next {
            if h == page {
                return Err(Error::Invalid("a hub cannot contain itself".into()));
            }
            if p.hubs.contains(&h) {
                continue;
            }
            let hub = self.hub_page(h)?;
            if p.is_hub() {
                let nested = match &inside {
                    Some(set) => set,
                    None => inside.insert(self.nested_in(page)?),
                };
                if nested.contains(&h) {
                    return Err(Error::Invalid(format!(
                        "{:?} is inside {:?}; adding it would create a cycle",
                        hub.title, p.title
                    )));
                }
            }
        }
        p.hubs = next;
        p.updated_at = self.now_ms();
        self.write_page(&p, &self.tick())?;
        tx.commit()?;
        Ok(p)
    }

    /// Puts `page` in `hub`, taking it out of the hub it was in (no-op when already there).
    pub fn add_to_hub(&self, page: PageId, hub: PageId) -> Result<Page> {
        self.set_page_hubs(page, vec![hub])
    }

    /// Takes `page` out of `hub` (no-op when it is not in it).
    pub fn remove_from_hub(&self, page: PageId, hub: PageId) -> Result<Page> {
        let p = self
            .get_page(page)?
            .ok_or_else(|| Error::NotFound(page.to_string()))?;
        let hubs = p.hubs.iter().copied().filter(|&h| h != hub).collect();
        self.set_page_hubs(page, hubs)
    }

    /// Suggested title, description and members for converting a note into a hub.
    pub fn hub_conversion_preview(&self, page: PageId) -> Result<HubPreview> {
        let p = self.note_page(page)?;
        let conn = self.conn();
        let mut pages: HashMap<PageId, (String, bool)> = HashMap::new();
        {
            let mut st =
                conn.prepare("SELECT id, title_key, journal_date IS NOT NULL FROM pages")?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            for row in rows {
                let (id, key, journal) = row?;
                pages.insert(parse_id(&id)?, (key, journal));
            }
        }
        // Titles first, so a title beats an alias with the same key; aliases may dangle.
        let mut by_key: HashMap<String, PageId> = HashMap::new();
        let mut own_keys: HashSet<String> = HashSet::new();
        for (t, id) in self.all_titles()? {
            if !pages.contains_key(&id) {
                continue;
            }
            let key = title_key(&t);
            if id == page {
                own_keys.insert(key.clone());
            }
            by_key.entry(key).or_insert(id);
        }

        let mut members = Vec::new();
        let mut seen: HashSet<PageId> = HashSet::from([page]);
        let mut add = |id: PageId, members: &mut Vec<PageId>| {
            if pages.get(&id).is_some_and(|(_, journal)| !journal) && seen.insert(id) {
                members.push(id);
            }
        };
        let tree = self.page_tree(page)?;
        let mut prose_blocks = 0u32;
        let mut description = None;
        for (_, b) in &tree {
            let text = content_lines(&b.content);
            let text = text.trim();
            for (t, kind) in extract_refs(text).pages {
                if kind != RefKind::Property
                    && let Some(&id) = by_key.get(&title_key(&t))
                {
                    add(id, &mut members);
                }
            }
            if is_prose(text) {
                prose_blocks += 1;
                if description.is_none() {
                    let plain = plain_text(text);
                    if !plain.is_empty() {
                        description = Some(shorten(&plain, MAX_DESCRIPTION_CHARS));
                    }
                }
            }
        }
        // Pages whose page properties reference this page, by title key.
        {
            let mut st = conn.prepare(
                "SELECT id, properties FROM pages WHERE journal_date IS NULL ORDER BY title_key",
            )?;
            let rows =
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, props) = row?;
                let props: serde_json::Map<String, Value> =
                    serde_json::from_str(&props).unwrap_or_default();
                let mut strings = Vec::new();
                props.values().for_each(|v| value_strings(v, &mut strings));
                let points = strings.iter().any(|s| {
                    extract_refs(s)
                        .pages
                        .iter()
                        .any(|(t, _)| own_keys.contains(&title_key(t)))
                });
                if points {
                    add(parse_id(&id)?, &mut members);
                }
            }
        }
        // Pages with blocks tagging this page or pointing at it from a block property.
        {
            let mut st = conn.prepare(
                "SELECT DISTINCT b.page_id, p.title_key, r.target_key FROM refs r
                 JOIN blocks b ON b.id = r.block_id JOIN pages p ON p.id = b.page_id
                 WHERE r.kind IN ('tag', 'property') AND p.journal_date IS NULL
                 ORDER BY p.title_key",
            )?;
            let rows =
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(2)?)))?;
            for row in rows {
                let (id, target) = row?;
                if own_keys.contains(&target) {
                    add(parse_id(&id)?, &mut members);
                }
            }
        }

        let suggested_title = match namespace_parent(&p.title) {
            Some(_) => p.title.rsplit('/').next().unwrap_or(&p.title).trim(),
            None => p.title.trim(),
        }
        .to_string();
        Ok(HubPreview {
            suggested_title,
            suggested_description: description.unwrap_or_default(),
            members,
            prose_blocks,
            blocks: tree.len() as u32,
        })
    }

    /// Turns a note into a hub in one transaction: rename (links rewritten) when the title
    /// differs, move the outline into a new note or delete it, set the description and add
    /// the members (invalid ones, such as journals or cycles, are skipped).
    pub fn convert_to_hub(&self, page: PageId, c: &HubConversion) -> Result<Page> {
        self.transaction(|g| {
            let mut p = g.note_page(page)?;
            let title = c.title.trim();
            if title.is_empty() {
                return Err(Error::Invalid("empty page title".into()));
            }
            if title != p.title {
                p = g.rename_page(page, title)?;
            }
            let mut members: Vec<PageId> = Vec::new();
            let roots: Vec<_> = g
                .page_tree(page)?
                .into_iter()
                .filter(|(d, _)| *d == 0)
                .map(|(_, b)| b.id)
                .collect();
            if let Some(note_title) = &c.keep_outline_as {
                let note = g.create_page_tx(NewPage {
                    title: note_title.trim().to_string(),
                    origin_journal: p.origin_journal,
                    created_at: Some(p.created_at),
                    ..Default::default()
                })?;
                for id in roots {
                    g.move_block(id, note.id, Placement::LastChildOf(None))?;
                }
                members.push(note.id);
            } else {
                for id in roots {
                    if g.get_block(id)?.is_some() {
                        g.delete_block(id)?;
                    }
                }
            }
            // Anything page_tree could not place (should not happen) goes too.
            let left: Vec<String> = {
                let mut st = g.conn().prepare("SELECT id FROM blocks WHERE page_id=?1")?;
                st.query_map([page.to_string()], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            for id in left {
                let id = parse_id(&id)?;
                if g.get_block(id)?.is_some() {
                    g.delete_block(id)?;
                }
            }
            let mut p = g
                .get_page(page)?
                .ok_or_else(|| Error::NotFound(page.to_string()))?;
            p.kind = PageKind::Hub;
            p.description = c.description.trim().to_string();
            p.updated_at = g.now_ms();
            g.write_page(&p, &g.tick())?;
            g.ensure_hub_template(&p)?;
            for &m in &c.members {
                if !members.contains(&m) {
                    members.push(m);
                }
            }
            for m in members {
                match g.add_to_hub(m, page) {
                    Ok(_) | Err(Error::Invalid(_) | Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(p)
        })
    }

    /// Turns a hub back into a note: the description becomes the first block, then one
    /// `[[Member]]` block per member; the members leave the hub.
    pub fn convert_to_note(&self, hub: PageId) -> Result<Page> {
        self.transaction(|g| {
            let mut p = g.hub_page(hub)?;
            let members = g.hub_members(hub)?;
            for t in g.hub_template_ids(hub)? {
                g.delete_page_rows(t)?;
            }
            let description = std::mem::take(&mut p.description);
            p.kind = PageKind::Note;
            p.updated_at = g.now_ms();
            g.write_page(&p, &g.tick())?;
            if !description.is_empty() {
                g.insert_block(hub, Placement::LastChildOf(None), &description)?;
            }
            for m in &members {
                if let Some(mp) = g.get_page(*m)? {
                    g.insert_block(
                        hub,
                        Placement::LastChildOf(None),
                        &format!("[[{}]]", mp.title),
                    )?;
                }
            }
            g.drop_hub_from_members(hub)?;
            g.get_page(hub)?
                .ok_or_else(|| Error::NotFound(hub.to_string()))
        })
    }

    /// Hubs of the whole graph with their members, nesting depth and every page's home hub.
    ///
    /// Hub ids that dangle (missing pages, pages that are no longer hubs) and journals are
    /// ignored. Concurrent edits on two replicas can store a cycle; memberships are then taken
    /// by the HLC of the member page's last write, oldest first, and the one closing a cycle
    /// is ignored, which every replica decides the same way.
    pub fn structure(&self) -> Result<Structure> {
        let conn = self.conn();
        let mut pages: HashMap<PageId, PageRow> = HashMap::new();
        {
            let mut st = conn.prepare(
                "SELECT id, title, title_key, journal_date IS NOT NULL, kind, description, hubs, hlc
                 FROM pages",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    PageRow {
                        title: r.get(1)?,
                        key: r.get(2)?,
                        journal: r.get(3)?,
                        kind: PageKind::parse(&r.get::<_, String>(4)?),
                        description: r.get(5)?,
                        hubs: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                        hlc: r.get(7)?,
                    },
                ))
            })?;
            for row in rows {
                let (id, p) = row?;
                pages.insert(parse_id(&id)?, p);
            }
        }
        let is_hub = |id: &PageId| {
            pages
                .get(id)
                .is_some_and(|p| p.kind == PageKind::Hub && !p.journal)
        };

        // Hub → members, from every page's own `hubs`, oldest page write first.
        let mut edges: Vec<(&str, PageId, PageId)> = Vec::new();
        for (&id, p) in &pages {
            if p.journal || p.kind == PageKind::Template {
                continue;
            }
            // One hub per page: the first entry counts.
            for &h in p.hubs.iter().take(1) {
                if h != id && is_hub(&h) {
                    edges.push((p.hlc.as_str(), id, h));
                }
            }
        }
        edges.sort();
        edges.dedup();
        let mut members: HashMap<PageId, Vec<PageId>> = HashMap::new();
        let mut sub_hubs: HashMap<PageId, Vec<PageId>> = HashMap::new();
        for (_, member, hub) in edges {
            if is_hub(&member) {
                if reaches(&sub_hubs, member, hub) {
                    continue; // closes a cycle
                }
                sub_hubs.entry(hub).or_default().push(member);
            }
            members.entry(hub).or_default().push(member);
        }

        let mut hubs: Vec<HubInfo> = pages
            .iter()
            .filter(|(id, _)| is_hub(id))
            .map(|(&id, p)| {
                let mut m = members.remove(&id).unwrap_or_default();
                m.sort_by_cached_key(|x| {
                    let row = pages.get(x);
                    (
                        !is_hub(x),
                        row.map(|r| r.key.clone()).unwrap_or_default(),
                        *x,
                    )
                });
                HubInfo {
                    page: id,
                    title: p.title.clone(),
                    description: p.description.clone(),
                    members: m,
                    depth: 0,
                }
            })
            .collect();
        let key = |id: &PageId| pages.get(id).map(|p| p.key.as_str()).unwrap_or("");
        hubs.sort_by(|a, b| key(&a.page).cmp(key(&b.page)).then(a.page.cmp(&b.page)));
        assign_depth(&mut hubs);
        let home = home_hubs(&hubs, |id| key(id).to_string());
        Ok(Structure { hubs, home })
    }

    /// Direct children of a namespace as `(full title, page id)`; levels that only exist as a
    /// prefix of deeper titles come with no id. Sorted by title key.
    pub fn namespace_children(&self, page_title: &str) -> Result<Vec<(String, Option<PageId>)>> {
        let parent: Vec<String> = page_title.split('/').map(title_key).collect();
        if parent.iter().any(|s| s.is_empty()) {
            return Ok(Vec::new());
        }
        let mut st = self.conn().prepare(
            "SELECT id, title FROM pages WHERE journal_date IS NULL AND title LIKE '%/%'",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        // Child key → (display title, page when the child exists as a page itself).
        let mut found: HashMap<String, (String, Option<PageId>)> = HashMap::new();
        for row in rows {
            let (id, title) = row?;
            let segs: Vec<&str> = title.split('/').map(str::trim).collect();
            if segs.len() <= parent.len()
                || segs.iter().any(|s| s.is_empty())
                || !segs.iter().zip(&parent).all(|(s, p)| title_key(s) == *p)
            {
                continue;
            }
            let depth = parent.len() + 1;
            let child = segs[..depth].join("/");
            let entry = found
                .entry(title_key(&child))
                .or_insert_with(|| (child.clone(), None));
            if segs.len() == depth {
                *entry = (title, Some(parse_id(&id)?));
            }
        }
        let mut out: Vec<(String, (String, Option<PageId>))> = found.into_iter().collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out.into_iter().map(|(_, v)| v).collect())
    }

    // ---------------------------------------------------------- helpers

    fn hub_page(&self, id: PageId) -> Result<Page> {
        let p = self
            .get_page(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        if !p.is_hub() {
            return Err(Error::Invalid(format!("{:?} is not a hub", p.title)));
        }
        Ok(p)
    }

    fn note_page(&self, id: PageId) -> Result<Page> {
        let p = self
            .get_page(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        if p.is_journal() {
            return Err(Error::Invalid("journals cannot be hubs".into()));
        }
        if p.is_hub() {
            return Err(Error::Invalid(format!("{:?} is already a hub", p.title)));
        }
        Ok(p)
    }

    /// Every page nested (at any depth) inside hub `hub`, from the `page_hubs` index.
    fn nested_in(&self, hub: PageId) -> Result<HashSet<PageId>> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([hub]);
        let mut st = self
            .conn()
            .prepare_cached("SELECT page_id FROM page_hubs WHERE hub_id=?1")?;
        while let Some(h) = queue.pop_front() {
            let kids: Vec<String> = st
                .query_map([h.to_string()], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            for k in kids {
                if let Ok(k) = parse_id(&k)
                    && seen.insert(k)
                {
                    queue.push_back(k);
                }
            }
        }
        Ok(seen)
    }

    /// Removes `hub` from the `hubs` of every page in it (each page rewritten and logged).
    pub(crate) fn drop_hub_from_members(&self, hub: PageId) -> Result<()> {
        let ids: Vec<String> = {
            let mut st = self
                .conn()
                .prepare_cached("SELECT page_id FROM page_hubs WHERE hub_id=?1 ORDER BY page_id")?;
            st.query_map([hub.to_string()], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for id in ids {
            if let Some(mut p) = self.get_page(parse_id(&id)?)? {
                p.hubs.retain(|&h| h != hub);
                self.write_page(&p, &self.tick())?;
            }
        }
        Ok(())
    }
}

/// Whether `to` is reachable from `from` over accepted hub → sub-hub edges.
fn reaches(sub_hubs: &HashMap<PageId, Vec<PageId>>, from: PageId, to: PageId) -> bool {
    let mut seen = HashSet::from([from]);
    let mut stack = vec![from];
    while let Some(h) = stack.pop() {
        if h == to {
            return true;
        }
        for &k in sub_hubs.get(&h).into_iter().flatten() {
            if seen.insert(k) {
                stack.push(k);
            }
        }
    }
    false
}

/// BFS from the root hubs (members of no hub) over hub → sub-hub membership.
fn assign_depth(hubs: &mut [HubInfo]) {
    let index: HashMap<PageId, usize> = hubs.iter().enumerate().map(|(i, h)| (h.page, i)).collect();
    let mut is_member = vec![false; hubs.len()];
    for h in hubs.iter() {
        for m in &h.members {
            if let Some(&i) = index.get(m) {
                is_member[i] = true;
            }
        }
    }
    let mut depth: Vec<Option<u32>> = vec![None; hubs.len()];
    let mut queue = VecDeque::new();
    for (i, &member) in is_member.iter().enumerate() {
        if !member {
            depth[i] = Some(0);
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let d = depth[i].unwrap_or(0);
        for m in &hubs[i].members {
            if let Some(&j) = index.get(m)
                && depth[j].is_none()
            {
                depth[j] = Some(d + 1);
                queue.push_back(j);
            }
        }
    }
    for (h, d) in hubs.iter_mut().zip(depth) {
        h.depth = d.unwrap_or(0);
    }
}

/// Deepest hub of every member; ties go to the hub with fewer members, then the smaller key.
fn home_hubs(hubs: &[HubInfo], key: impl Fn(&PageId) -> String) -> HashMap<PageId, PageId> {
    let rank = |h: &HubInfo| (std::cmp::Reverse(h.depth), h.members.len(), key(&h.page));
    let mut best: HashMap<PageId, &HubInfo> = HashMap::new();
    for h in hubs {
        for m in &h.members {
            if best.get(m).is_none_or(|b| rank(h) < rank(b)) {
                best.insert(*m, h);
            }
        }
    }
    let mut home: HashMap<PageId, PageId> = best.into_iter().map(|(p, h)| (p, h.page)).collect();
    for h in hubs {
        home.insert(h.page, h.page);
    }
    home
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_and_plain_text() {
        assert!(!is_prose("## Things"));
        assert!(!is_prose("[[Film]]"));
        assert!(!is_prose("#Music, [[Games]]"));
        assert!(is_prose("[[Books]] I want to read"));
        assert!(is_prose("## [[Film]] and more words"));
        assert_eq!(
            plain_text(
                "## TODO **Read** [[Big Book]] with #rust and [site](http://x) ![i](a.png) `x`"
            ),
            "Read Big Book with rust and site x"
        );
        assert_eq!(plain_text("line one\nline  two"), "line one line two");
    }

    #[test]
    fn shorten_cuts_at_words() {
        let long = "word ".repeat(100);
        let s = shorten(long.trim(), 20);
        assert!(s.chars().count() <= 20, "{s}");
        assert_eq!(s, "word word word word…");
        assert_eq!(shorten("short", 20), "short");
    }
}
