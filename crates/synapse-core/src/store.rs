//! The graph store: pages, blocks, refs, journals, assets and the op log on SQLite.

use crate::blob::BlobStore;
use crate::hlc::HlcClock;
use crate::model::*;
use crate::parse::{self, extract_refs};
use crate::position::between;
use crate::{Error, Result, schema, synapse};
use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Namespace for deterministic journal ids (same day ⇒ same id on every device).
const JOURNAL_NS: Uuid = Uuid::from_u128(0x5f1e_7a2c_93b4_4d61_a0c8_6e2f_1b3d_9c47);

pub fn journal_id(date: NaiveDate) -> PageId {
    Uuid::new_v5(&JOURNAL_NS, journal_title(date).as_bytes())
}

const PAGE_COLS: &str = "id, title, journal_date, origin_journal, properties, created_at, updated_at, kind, description, hubs, tags, types";
const BLOCK_COLS: &str =
    "id, page_id, parent_id, position, content, properties, collapsed, created_at, updated_at";

pub struct Graph {
    conn: Connection,
    blobs: BlobStore,
    clock: HlcClock,
    dir: PathBuf,
    now_override: Cell<Option<i64>>,
}

impl Graph {
    /// Opens (or creates) a graph stored in `dir` (`synapse.db` + `blobs/`).
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let conn = Connection::open(dir.join("synapse.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        // Large transactions (imports, restores) spill a 2 MB default cache to disk and write
        // savepoint sub-journals to temp files; keep both in memory (32 MB cap).
        conn.pragma_update(None, "cache_size", -32_768)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.set_prepared_statement_cache_capacity(128);
        conn.execute_batch(schema::SCHEMA)?;
        schema::migrate(&conn)?;
        let actor: String = match conn
            .query_row("SELECT value FROM meta WHERE key='actor'", [], |r| r.get(0))
            .optional()?
        {
            Some(a) => a,
            None => {
                let a = new_id().simple().to_string()[..12].to_string();
                conn.execute("INSERT INTO meta(key, value) VALUES ('actor', ?1)", [&a])?;
                conn.execute(
                    "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema', ?1)",
                    [schema::VERSION.to_string()],
                )?;
                a
            }
        };
        let clock = HlcClock::new(actor);
        if let Some(last) = conn
            .query_row("SELECT hlc FROM ops ORDER BY seq DESC LIMIT 1", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
        {
            clock.observe(&last);
        }
        let blobs = BlobStore::open(dir.join("blobs"))?;
        let graph = Self {
            conn,
            blobs,
            clock,
            dir,
            now_override: Cell::new(None),
        };
        Ok(graph)
    }

    /// Runs `f` atomically. Calls nest (SQLite savepoints), so callers such as importers can
    /// group many graph operations into one transaction and roll back an inner one.
    ///
    /// Inside a `transaction`, single graph operations do not open their own savepoint (that
    /// would cost one per call on bulk imports); if one fails, return the error so the
    /// enclosing `transaction` rolls back, or wrap it in its own `transaction`.
    pub fn transaction<T>(&self, f: impl FnOnce(&Graph) -> Result<T>) -> Result<T> {
        let tx = self.savepoint()?;
        let out = f(self)?;
        tx.commit()?;
        Ok(out)
    }

    /// `execute` through the statement cache (hot paths run millions of times on imports).
    pub(crate) fn exec<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> rusqlite::Result<usize> {
        self.conn.prepare_cached(sql)?.execute(params)
    }

    /// `query_row` through the statement cache.
    pub(crate) fn row<T, P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
        f: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        self.conn.prepare_cached(sql)?.query_row(params, f)
    }

    fn savepoint(&self) -> Result<Tx<'_>> {
        self.conn.execute_batch("SAVEPOINT graph")?;
        Ok(Tx {
            conn: &self.conn,
            active: true,
            done: false,
        })
    }

    /// Atomic scope for one graph operation: a savepoint at top level, a no-op when an
    /// enclosing transaction already provides atomicity.
    pub(crate) fn tx(&self) -> Result<Tx<'_>> {
        if self.conn.is_autocommit() {
            return self.savepoint();
        }
        Ok(Tx {
            conn: &self.conn,
            active: false,
            done: false,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Device/actor id used in HLC timestamps.
    pub fn actor(&self) -> &str {
        self.clock.actor()
    }

    /// Raw connection for sibling crates that keep their own tables (read-mostly).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Fixes "now" (millis since epoch). Used by tests and importers.
    pub fn set_now(&self, ms: Option<i64>) {
        self.now_override.set(ms);
    }

    pub fn now_ms(&self) -> i64 {
        self.now_override
            .get()
            .unwrap_or_else(|| Local::now().timestamp_millis())
    }

    pub fn today(&self) -> NaiveDate {
        day_of(self.now_ms())
    }

    // ---------------------------------------------------------------- pages

    pub fn get_page(&self, id: PageId) -> Result<Option<Page>> {
        Ok(self
            .row(
                &format!("SELECT {PAGE_COLS} FROM pages WHERE id=?1"),
                [id.to_string()],
                page_row,
            )
            .optional()?)
    }

    /// Looks a page up by title or alias (case-insensitive).
    pub fn page_by_title(&self, title: &str) -> Result<Option<Page>> {
        let key = title_key(title);
        let page = self
            .row(
                &format!("SELECT {PAGE_COLS} FROM pages WHERE title_key=?1"),
                [&key],
                page_row,
            )
            .optional()?;
        if page.is_some() {
            return Ok(page);
        }
        let id: Option<String> = self
            .row(
                "SELECT page_id FROM aliases WHERE alias_key=?1",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.get_page(parse_id(&id)?),
            None => Ok(None),
        }
    }

    /// Creates a page. Its origin defaults to today's journal (pages are born in journals).
    pub fn create_page(&self, new: NewPage) -> Result<Page> {
        let tx = self.tx()?;
        let page = self.create_page_tx(new)?;
        tx.commit()?;
        Ok(page)
    }

    pub(crate) fn create_page_tx(&self, new: NewPage) -> Result<Page> {
        self.create_page_kind_tx(new, PageKind::Note, String::new())
    }

    pub(crate) fn create_page_kind_tx(
        &self,
        new: NewPage,
        kind: PageKind,
        description: String,
    ) -> Result<Page> {
        let title = new.title.trim().to_string();
        if title.is_empty() {
            return Err(Error::Invalid("empty page title".into()));
        }
        if parse_day(&title).is_some() {
            return Err(Error::Invalid(format!(
                "{title:?} is a journal date; use journal()"
            )));
        }
        if self.page_by_title(&title)?.is_some() {
            return Err(Error::Conflict(format!("page {title:?} already exists")));
        }
        let now = self.now_ms();
        let origin = new.origin_journal.unwrap_or_else(|| self.today());
        self.ensure_journal_tx(origin)?;
        let page = Page {
            id: new_id(),
            title,
            journal_date: None,
            origin_journal: Some(origin),
            properties: new.properties,
            created_at: new.created_at.unwrap_or(now),
            updated_at: new.updated_at.unwrap_or(now),
            kind,
            description,
            hubs: Vec::new(),
            tags: Vec::new(),
            types: Vec::new(),
        };
        self.write_page(&page, &self.tick())?;
        Ok(page)
    }

    /// Writes a fully specified page as-is, keeping its timestamps (importers, restore).
    pub fn put_page(&self, page: &Page) -> Result<()> {
        let tx = self.tx()?;
        if let Some(o) = page.origin_journal {
            self.ensure_journal_tx(o)?;
        }
        self.write_page(page, &self.tick())?;
        tx.commit()
    }

    /// Returns the page with this title, creating it (born in `origin`) if missing.
    pub fn ensure_page(&self, title: &str, origin: Option<NaiveDate>) -> Result<Page> {
        if let Some(p) = self.page_by_title(title)? {
            return Ok(p);
        }
        if let Some(date) = parse_day(title) {
            return self.journal(date);
        }
        self.create_page(NewPage {
            title: title.into(),
            origin_journal: origin,
            ..Default::default()
        })
    }

    /// Pages ordered by last update (most recent first).
    pub fn list_pages(&self, include_journals: bool, limit: u32, offset: u32) -> Result<Vec<Page>> {
        // Templates are reached through their hub, never listed as pages.
        let filter = if include_journals {
            "WHERE kind != 'template'"
        } else {
            "WHERE journal_date IS NULL AND kind != 'template'"
        };
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {PAGE_COLS} FROM pages {filter} ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = st.query_map(params![limit, offset], page_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// All page titles and aliases with their page id (for auto-link matching).
    pub fn all_titles(&self) -> Result<Vec<(String, PageId)>> {
        let mut st = self
            .conn
            .prepare("SELECT title, id FROM pages UNION ALL SELECT alias, page_id FROM aliases")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| {
            let (t, id) = r?;
            Ok((t, parse_id(&id)?))
        })
        .collect()
    }

    pub fn set_page_properties(&self, id: PageId, properties: Properties) -> Result<Page> {
        let mut page = self
            .get_page(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let tx = self.tx()?;
        page.properties = properties;
        page.updated_at = self.now_ms();
        self.write_page(&page, &self.tick())?;
        tx.commit()?;
        Ok(page)
    }

    /// Renames a page and rewrites `[[Old]]` / `#Old` in every referencing block.
    pub fn rename_page(&self, id: PageId, new_title: &str) -> Result<Page> {
        let tx = self.tx()?;
        let mut page = self
            .get_page(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        if page.is_journal() {
            return Err(Error::Invalid("journal pages cannot be renamed".into()));
        }
        let new_title = new_title.trim();
        if new_title.is_empty() {
            return Err(Error::Invalid("empty page title".into()));
        }
        if parse_day(new_title).is_some() {
            return Err(Error::Invalid(format!("{new_title:?} is a journal date")));
        }
        if let Some(other) = self.page_by_title(new_title)?
            && other.id != id
        {
            return Err(Error::Conflict(format!(
                "page {new_title:?} already exists"
            )));
        }
        let old_key = title_key(&page.title);
        let referencing: Vec<String> = {
            let mut st = self
                .conn
                .prepare("SELECT DISTINCT block_id FROM refs WHERE target_key=?1")?;
            st.query_map([&old_key], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        page.title = new_title.to_string();
        page.updated_at = self.now_ms();
        self.write_page(&page, &self.tick())?;
        for bid in referencing {
            if let Some(mut b) = self.get_block(parse_id(&bid)?)? {
                let rewritten = parse::rewrite_refs(&b.content, &old_key, new_title);
                let mut props_changed = false;
                for v in b.properties.values_mut() {
                    if let Some(text) = v.as_str() {
                        let r = parse::rewrite_refs(text, &old_key, new_title);
                        if r != text {
                            *v = serde_json::Value::String(r);
                            props_changed = true;
                        }
                    }
                }
                if rewritten != b.content || props_changed {
                    b.content = rewritten;
                    b.updated_at = self.now_ms();
                    self.write_block(&b, &self.tick())?;
                }
            }
        }
        tx.commit()?;
        Ok(page)
    }

    /// Deletes a page and all its blocks; when it is a hub, its members leave it.
    pub fn delete_page(&self, id: PageId) -> Result<()> {
        let tx = self.tx()?;
        if let Some(p) = self.get_page(id)? {
            if p.is_template() {
                return Err(Error::Invalid(
                    "a template cannot be deleted on its own; it goes with its hub".into(),
                ));
            }
            if p.is_hub() {
                for t in self.hub_template_ids(id)? {
                    self.delete_page_rows(t)?;
                }
            }
        }
        self.delete_page_rows(id)?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn delete_page_rows(&self, id: PageId) -> Result<()> {
        // Members of a deleted hub leave it (each member page is rewritten and logged).
        self.drop_hub_from_members(id)?;
        let ids: Vec<String> = {
            let mut st = self
                .conn
                .prepare("SELECT id FROM blocks WHERE page_id=?1")?;
            st.query_map([id.to_string()], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for b in ids {
            self.remove_block_row(parse_id(&b)?, &self.tick())?;
        }
        self.remove_page_row(id, &self.tick())?;
        Ok(())
    }

    /// Points `alias` at `page` (or removes it with `None`).
    pub fn set_alias(&self, alias: &str, page: Option<PageId>) -> Result<()> {
        let tx = self.tx()?;
        let hlc = self.tick();
        self.write_alias(alias, page, &hlc)?;
        self.log(
            &hlc,
            &Op::SetAlias {
                alias: alias.to_string(),
                page,
            },
        )?;
        tx.commit()
    }

    pub fn aliases_of(&self, page: PageId) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare("SELECT alias FROM aliases WHERE page_id=?1 ORDER BY alias")?;
        let rows = st.query_map([page.to_string()], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ------------------------------------------------------------- journals

    /// The journal page for `date`, created if missing.
    pub fn journal(&self, date: NaiveDate) -> Result<Page> {
        let tx = self.tx()?;
        let p = self.ensure_journal_tx(date)?;
        tx.commit()?;
        Ok(p)
    }

    pub fn today_journal(&self) -> Result<Page> {
        self.journal(self.today())
    }

    fn ensure_journal_tx(&self, date: NaiveDate) -> Result<Page> {
        if let Some(p) = self.get_page(journal_id(date))? {
            return Ok(p);
        }
        let ms = Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .map(|d| d.timestamp_millis())
            .unwrap_or_else(|| self.now_ms());
        let page = Page {
            id: journal_id(date),
            title: journal_title(date),
            journal_date: Some(date),
            origin_journal: None,
            properties: Properties::new(),
            created_at: ms,
            updated_at: ms,
            kind: PageKind::Note,
            description: String::new(),
            hubs: Vec::new(),
            tags: Vec::new(),
            types: Vec::new(),
        };
        self.write_page(&page, &self.tick())?;
        Ok(page)
    }

    /// Previous and next existing journals around `date` (the episodic chain).
    pub fn journal_neighbors(&self, date: NaiveDate) -> Result<JournalNeighbors> {
        let d = journal_title(date);
        let prev: Option<String> = self.row(
            "SELECT max(journal_date) FROM pages WHERE journal_date < ?1",
            [&d],
            |r| r.get(0),
        )?;
        let next: Option<String> = self.row(
            "SELECT min(journal_date) FROM pages WHERE journal_date > ?1",
            [&d],
            |r| r.get(0),
        )?;
        Ok(JournalNeighbors {
            prev: prev.and_then(|s| parse_day(&s)),
            next: next.and_then(|s| parse_day(&s)),
        })
    }

    /// Up to `limit` journals strictly before `before` (or the newest ones), newest first.
    pub fn journals_before(&self, before: Option<NaiveDate>, limit: u32) -> Result<Vec<Page>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {PAGE_COLS} FROM pages WHERE journal_date IS NOT NULL AND (?1 IS NULL OR journal_date < ?1)
             ORDER BY journal_date DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(params![before.map(journal_title), limit], page_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Journals in `[from, to]`, newest first.
    pub fn journals_between(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Page>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {PAGE_COLS} FROM pages WHERE journal_date BETWEEN ?1 AND ?2 ORDER BY journal_date DESC"
        ))?;
        let rows = st.query_map(params![journal_title(from), journal_title(to)], page_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Timeline view data for `[from, to]`, oldest first.
    pub fn timeline(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<TimelineDay>> {
        let (f, t) = (journal_title(from), journal_title(to));
        let mut days: std::collections::BTreeMap<String, TimelineDay> = Default::default();
        let mut st = self
            .conn
            .prepare("SELECT journal_date, id FROM pages WHERE journal_date BETWEEN ?1 AND ?2")?;
        for r in st.query_map(params![f, t], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (d, id) = r?;
            entry(&mut days, &d)?.journal = Some(parse_id(&id)?);
        }
        let mut st = self.conn.prepare(
            "SELECT origin_journal, id FROM pages WHERE origin_journal BETWEEN ?1 AND ?2 ORDER BY created_at",
        )?;
        for r in st.query_map(params![f, t], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (d, id) = r?;
            entry(&mut days, &d)?.created.push(parse_id(&id)?);
        }
        let mut st = self.conn.prepare(
            "SELECT a.day, a.page_id, a.edits FROM page_activity a JOIN pages p ON p.id = a.page_id
             WHERE a.day BETWEEN ?1 AND ?2 AND p.journal_date IS NULL ORDER BY a.edits DESC",
        )?;
        for r in st.query_map(params![f, t], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })? {
            let (d, id, n) = r?;
            entry(&mut days, &d)?.edited.push((parse_id(&id)?, n));
        }
        Ok(days.into_values().collect())
    }

    /// Days on which `page` was edited, with edit counts (oldest first).
    pub fn edit_days(&self, page: PageId) -> Result<Vec<(NaiveDate, u32)>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT day, edits FROM page_activity WHERE page_id=?1 ORDER BY day")?;
        let rows = st.query_map([page.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?))
        })?;
        rows.map(|r| {
            let (d, n) = r?;
            Ok((parse_day(&d).ok_or_else(|| Error::Invalid(d))?, n))
        })
        .collect()
    }

    // --------------------------------------------------------------- blocks

    pub fn get_block(&self, id: BlockId) -> Result<Option<Block>> {
        Ok(self
            .row(
                &format!("SELECT {BLOCK_COLS} FROM blocks WHERE id=?1"),
                [id.to_string()],
                block_row,
            )
            .optional()?)
    }

    /// Blocks of a page in outline (pre-order) order, with their depth.
    pub fn page_tree(&self, page: PageId) -> Result<Vec<(usize, Block)>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {BLOCK_COLS} FROM blocks WHERE page_id=?1 ORDER BY position"
        ))?;
        let all: Vec<Block> = st
            .query_map([page.to_string()], block_row)?
            .collect::<std::result::Result<_, _>>()?;
        let mut children: HashMap<Option<BlockId>, Vec<Block>> = HashMap::new();
        for b in all {
            children.entry(b.parent_id).or_default().push(b);
        }
        let mut out = Vec::new();
        let mut stack: Vec<(usize, Block)> = children
            .remove(&None)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .map(|b| (0, b))
            .collect();
        while let Some((d, b)) = stack.pop() {
            if let Some(kids) = children.remove(&Some(b.id)) {
                stack.extend(kids.into_iter().rev().map(|k| (d + 1, k)));
            }
            out.push((d, b));
        }
        // Blocks not reachable from the roots (parent missing after a concurrent delete, or a
        // parent cycle from concurrent moves) are shown as roots, deterministically ordered.
        let mut parents: Vec<Option<BlockId>> = children.keys().copied().collect();
        parents.sort();
        for parent in parents {
            let mut stack: Vec<(usize, Block)> = match children.remove(&parent) {
                Some(blocks) => blocks.into_iter().rev().map(|b| (0, b)).collect(),
                None => continue,
            };
            while let Some((d, b)) = stack.pop() {
                if let Some(kids) = children.remove(&Some(b.id)) {
                    stack.extend(kids.into_iter().rev().map(|k| (d + 1, k)));
                }
                out.push((d, b));
            }
        }
        Ok(out)
    }

    /// Inserts a new block with `content` and indexes its refs.
    pub fn insert_block(&self, page: PageId, at: Placement, content: &str) -> Result<Block> {
        self.ensure_not_hub(page)?;
        let tx = self.tx()?;
        let (parent_id, position) = self.resolve_placement(page, at)?;
        let now = self.now_ms();
        let block = Block {
            id: new_id(),
            page_id: page,
            parent_id,
            position,
            content: content.to_string(),
            properties: Properties::new(),
            collapsed: false,
            created_at: now,
            updated_at: now,
        };
        self.local_block_write(&block)?;
        tx.commit()?;
        Ok(block)
    }

    /// Writes a fully specified block (importer). Pages referenced by it are created, born in
    /// the block's journal when it lives in one. Does not record edit activity.
    pub fn put_block(&self, block: &Block) -> Result<()> {
        self.ensure_not_hub(block.page_id)?;
        let tx = self.tx()?;
        self.write_block(block, &self.tick())?;
        self.ensure_ref_pages(block)?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_block(&self, id: BlockId, content: &str) -> Result<Block> {
        let tx = self.tx()?;
        let mut b = self
            .get_block(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        if b.content != content {
            b.content = content.to_string();
            b.updated_at = self.now_ms();
            self.local_block_write(&b)?;
        }
        tx.commit()?;
        Ok(b)
    }

    pub fn set_block_properties(&self, id: BlockId, properties: Properties) -> Result<Block> {
        let tx = self.tx()?;
        let mut b = self
            .get_block(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        b.properties = properties;
        b.updated_at = self.now_ms();
        self.local_block_write(&b)?;
        tx.commit()?;
        Ok(b)
    }

    pub fn set_collapsed(&self, id: BlockId, collapsed: bool) -> Result<()> {
        let mut b = self
            .get_block(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        b.collapsed = collapsed;
        let tx = self.tx()?;
        self.write_block(&b, &self.tick())?;
        tx.commit()
    }

    /// Moves a block (with its subtree) to a new place, possibly on another page.
    pub fn move_block(&self, id: BlockId, page: PageId, at: Placement) -> Result<Block> {
        self.ensure_not_hub(page)?;
        let tx = self.tx()?;
        let mut b = self
            .get_block(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let (parent_id, position) = self.resolve_placement(page, at)?;
        let mut p = parent_id;
        let mut seen = std::collections::HashSet::new();
        while let Some(pid) = p {
            if !seen.insert(pid) {
                break; // pre-existing cycle (from concurrent moves); page_tree tolerates it
            }
            if pid == id {
                return Err(Error::Invalid(
                    "cannot move a block into its own subtree".into(),
                ));
            }
            p = self.get_block(pid)?.and_then(|x| x.parent_id);
        }
        let old_page = b.page_id;
        b.parent_id = parent_id;
        b.position = position;
        b.page_id = page;
        b.updated_at = self.now_ms();
        self.local_block_write(&b)?;
        if old_page != page {
            for d in self.descendants(id)? {
                let mut c = self.get_block(d)?.unwrap();
                c.page_id = page;
                self.write_block(&c, &self.tick())?;
            }
            self.touch_page(old_page)?;
        }
        tx.commit()?;
        Ok(b)
    }

    /// Deletes a block and its subtree.
    pub fn delete_block(&self, id: BlockId) -> Result<()> {
        let tx = self.tx()?;
        let b = self
            .get_block(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let mut ids = self.descendants(id)?;
        ids.push(id);
        for d in ids {
            self.remove_block_row(d, &self.tick())?;
        }
        self.touch_page(b.page_id)?;
        tx.commit()?;
        Ok(())
    }

    /// Hubs have no outline: block writes into them are refused.
    fn ensure_not_hub(&self, page: PageId) -> Result<()> {
        let kind: Option<String> = self
            .row(
                "SELECT kind FROM pages WHERE id=?1",
                [page.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if kind.as_deref() == Some(PageKind::Hub.as_str()) {
            return Err(Error::Invalid("hubs have no blocks".into()));
        }
        Ok(())
    }

    fn descendants(&self, id: BlockId) -> Result<Vec<BlockId>> {
        let mut st = self.conn.prepare_cached(
            "WITH RECURSIVE d(id) AS (SELECT id FROM blocks WHERE parent_id=?1
               UNION SELECT b.id FROM blocks b JOIN d ON b.parent_id = d.id)
             SELECT id FROM d",
        )?;
        let rows = st.query_map([id.to_string()], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    /// Blocks referencing `page` (by title or alias), newest first.
    pub fn backlinks(&self, page: PageId) -> Result<Vec<Backlink>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {}, p.title, r.kind FROM refs r
             JOIN blocks b ON b.id = r.block_id JOIN pages p ON p.id = b.page_id
             WHERE r.target_key IN (SELECT title_key FROM pages WHERE id=?1
                                    UNION SELECT alias_key FROM aliases WHERE page_id=?1)
               AND b.page_id != ?1
             ORDER BY b.updated_at DESC",
            BLOCK_COLS
                .split(", ")
                .map(|c| format!("b.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let rows = st.query_map([page.to_string()], |r| {
            Ok(Backlink {
                block: block_row(r)?,
                page_title: r.get(9)?,
                kind: RefKind::parse(&r.get::<_, String>(10)?).unwrap_or(RefKind::Link),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Blocks that reference or embed block `id`.
    pub fn block_backrefs(&self, id: BlockId) -> Result<Vec<Block>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {} FROM block_refs r JOIN blocks b ON b.id = r.block_id WHERE r.target_block=?1",
            BLOCK_COLS.split(", ").map(|c| format!("b.{c}")).collect::<Vec<_>>().join(", ")
        ))?;
        let rows = st.query_map([id.to_string()], block_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Pages that blocks of `page` link to.
    pub fn outgoing_pages(&self, page: PageId) -> Result<Vec<PageId>> {
        let mut st = self.conn.prepare_cached(
            "SELECT DISTINCT p.id FROM refs r JOIN blocks b ON b.id = r.block_id
             JOIN pages p ON p.title_key = r.target_key WHERE b.page_id=?1",
        )?;
        let rows = st.query_map([page.to_string()], |r| r.get::<_, String>(0))?;
        rows.map(|r| parse_id(&r?)).collect()
    }

    fn resolve_placement(&self, page: PageId, at: Placement) -> Result<(Option<BlockId>, String)> {
        let sib = |id: BlockId| -> Result<Block> {
            let b = self
                .get_block(id)?
                .ok_or_else(|| Error::NotFound(id.to_string()))?;
            if b.page_id != page {
                return Err(Error::Invalid("sibling is on another page".into()));
            }
            Ok(b)
        };
        let parent_sql = "page_id=?1 AND parent_id IS ?2";
        Ok(match at {
            Placement::LastChildOf(parent) => {
                if let Some(p) = parent {
                    sib(p)?;
                }
                let max: Option<String> = self.row(
                    &format!("SELECT max(position) FROM blocks WHERE {parent_sql}"),
                    params![page.to_string(), parent.map(|p| p.to_string())],
                    |r| r.get(0),
                )?;
                (parent, between(max.as_deref(), None))
            }
            Placement::After(s) => {
                let s = sib(s)?;
                let next: Option<String> = self.row(
                    &format!(
                        "SELECT min(position) FROM blocks WHERE {parent_sql} AND position > ?3"
                    ),
                    params![
                        page.to_string(),
                        s.parent_id.map(|p| p.to_string()),
                        s.position
                    ],
                    |r| r.get(0),
                )?;
                (s.parent_id, between(Some(&s.position), next.as_deref()))
            }
            Placement::Before(s) => {
                let s = sib(s)?;
                let prev: Option<String> = self.row(
                    &format!(
                        "SELECT max(position) FROM blocks WHERE {parent_sql} AND position < ?3"
                    ),
                    params![
                        page.to_string(),
                        s.parent_id.map(|p| p.to_string()),
                        s.position
                    ],
                    |r| r.get(0),
                )?;
                (s.parent_id, between(prev.as_deref(), Some(&s.position)))
            }
        })
    }

    /// A local edit: write, create referenced pages, record activity and fire synapses.
    fn local_block_write(&self, block: &Block) -> Result<()> {
        let before: Vec<String> = self.ref_keys(block.id)?;
        self.write_block(block, &self.tick())?;
        self.ensure_ref_pages(block)?;
        let after = self.ref_keys(block.id)?;
        for key in after.iter().filter(|k| !before.contains(k)) {
            if let Some(target) = self.page_by_title(key)? {
                synapse::fire(
                    &self.conn,
                    block.page_id,
                    target.id,
                    synapse::LINK_STRENGTH,
                    self.now_ms(),
                )?;
            }
        }
        self.touch_page(block.page_id)
    }

    fn ref_keys(&self, block: BlockId) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT target_key FROM refs WHERE block_id=?1")?;
        let rows = st.query_map([block.to_string()], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Creates pages referenced by `block` that do not exist yet. Returns how many.
    fn ensure_ref_pages(&self, block: &Block) -> Result<usize> {
        let origin = match self.get_page(block.page_id)? {
            Some(p) if p.is_journal() => p.journal_date,
            _ => Some(day_of(block.created_at)),
        };
        let mut n = 0;
        for (title, _) in extract_refs(&block.content).pages {
            if self.page_by_title(&title)?.is_none() {
                if let Some(date) = parse_day(&title) {
                    self.ensure_journal_tx(date)?;
                } else {
                    self.create_page_tx(NewPage {
                        title,
                        origin_journal: origin,
                        created_at: Some(block.created_at),
                        updated_at: Some(block.created_at),
                        ..Default::default()
                    })?;
                }
                n += 1;
            }
        }
        Ok(n)
    }

    /// Bumps `updated_at` and records today's activity for a page.
    pub(crate) fn touch_page(&self, page: PageId) -> Result<()> {
        let now = self.now_ms();
        let day = journal_title(day_of(now));
        self.exec(
            "UPDATE pages SET updated_at=?2 WHERE id=?1",
            params![page.to_string(), now],
        )?;
        let is_journal: bool = self
            .row(
                "SELECT journal_date IS NOT NULL FROM pages WHERE id=?1",
                [page.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(true);
        if !is_journal {
            self.ensure_journal_tx(day_of(now))?;
            self.exec(
                "INSERT INTO page_activity(page_id, day, edits) VALUES (?1, ?2, 1)
                 ON CONFLICT(page_id, day) DO UPDATE SET edits = edits + 1",
                params![page.to_string(), day],
            )?;
        }
        Ok(())
    }

    // --------------------------------------------------------------- assets

    /// Streams a file into the blob store and registers it.
    pub fn put_asset_file(&self, path: &Path, mime: Option<&str>) -> Result<Asset> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        self.put_asset_reader(std::fs::File::open(path)?, name.as_deref(), mime)
    }

    pub fn put_asset_reader(
        &self,
        reader: impl Read,
        name: Option<&str>,
        mime: Option<&str>,
    ) -> Result<Asset> {
        let (hash, size) = self.blobs.put_reader(reader)?;
        if let Some(a) = self.asset(&hash)? {
            return Ok(a);
        }
        let asset = Asset {
            hash,
            size,
            mime: mime.map(str::to_string),
            original_name: name.map(str::to_string),
            created_at: self.now_ms(),
        };
        let tx = self.tx()?;
        let hlc = self.tick();
        self.write_asset(&asset)?;
        self.log(
            &hlc,
            &Op::PutAsset {
                asset: asset.clone(),
            },
        )?;
        tx.commit()?;
        Ok(asset)
    }

    pub fn asset(&self, hash: &str) -> Result<Option<Asset>> {
        Ok(self
            .row(
                "SELECT hash, size, mime, original_name, created_at FROM assets WHERE hash=?1",
                [hash],
                |r| {
                    Ok(Asset {
                        hash: r.get(0)?,
                        size: r.get::<_, i64>(1)? as u64,
                        mime: r.get(2)?,
                        original_name: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    // ------------------------------------------------------- synapses/brain

    /// Records navigation `from` → `to`; co-visits strengthen the pair (Hebbian).
    pub fn record_visit(&self, from: Option<PageId>, to: PageId) -> Result<()> {
        if let Some(f) = from
            && f != to
        {
            synapse::fire(&self.conn, f, to, synapse::VISIT_STRENGTH, self.now_ms())?;
        }
        Ok(())
    }

    /// Notes related to `page` by spreading activation over refs, synapses and time.
    pub fn related(&self, page: PageId, limit: usize) -> Result<Vec<(PageId, f32)>> {
        synapse::spread(self, page, limit)
    }

    /// Current (decayed) synapse weight between two pages.
    pub fn synapse_weight(&self, a: PageId, b: PageId) -> Result<f32> {
        synapse::weight(&self.conn, a, b, self.now_ms())
    }

    // ------------------------------------------------------------------ ops

    /// Op log entries with `seq > after`, oldest first.
    pub fn ops_since(&self, after: i64, limit: u32) -> Result<Vec<OpRecord>> {
        let mut st = self.conn.prepare_cached(
            "SELECT seq, hlc, payload FROM ops WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = st.query_map(params![after, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (seq, hlc, payload) = r?;
            Ok(OpRecord {
                seq,
                hlc,
                op: serde_json::from_str(&payload)?,
            })
        })
        .collect()
    }

    pub fn last_seq(&self) -> Result<i64> {
        Ok(self.row("SELECT coalesce(max(seq), 0) FROM ops", [], |r| r.get(0))?)
    }

    /// Applies an op from another replica with last-writer-wins per entity.
    /// Returns whether it changed local state. Applied ops are re-logged locally.
    pub fn apply_remote(&self, hlc: &str, op: &Op) -> Result<bool> {
        self.clock.observe(hlc);
        let tx = self.tx()?;
        let newer = |table: &str, id: &str| -> Result<bool> {
            let local: Option<String> = self
                .row(&format!("SELECT hlc FROM {table} WHERE id=?1"), [id], |r| {
                    r.get(0)
                })
                .optional()?;
            let tomb: Option<String> = self
                .row("SELECT hlc FROM tombstones WHERE id=?1", [id], |r| r.get(0))
                .optional()?;
            Ok(local.is_none_or(|l| hlc > l.as_str()) && tomb.is_none_or(|t| hlc > t.as_str()))
        };
        let applied = match op {
            Op::UpsertPage { page } => {
                if !newer("pages", &page.id.to_string())? {
                    false
                } else {
                    let mut page = page.clone();
                    // Two replicas created the same title offline. Journals always keep their
                    // date title; otherwise the smaller id keeps it. Every replica applies the
                    // same rule, so they converge.
                    if let Some(other) = self.page_by_exact_title(&page.title)?
                        && other.id != page.id
                    {
                        let incoming_wins = match (page.is_journal(), other.is_journal()) {
                            (true, false) => true,
                            (false, true) => false,
                            _ => page.id < other.id,
                        };
                        if incoming_wins {
                            let mut o = other.clone();
                            o.title = self.conflict_title(&o.title, o.id)?;
                            self.write_page(&o, &self.tick())?;
                        } else {
                            page.title = self.conflict_title(&page.title, page.id)?;
                        }
                    }
                    // A page title beats a colliding alias.
                    self.exec(
                        "DELETE FROM aliases WHERE alias_key=?1 AND page_id != ?2",
                        params![title_key(&page.title), page.id.to_string()],
                    )?;
                    self.write_page_row(&page, hlc)?;
                    true
                }
            }
            Op::DeletePage { id } => {
                newer("pages", &id.to_string())? && {
                    self.remove_page_row_raw(*id, hlc)?;
                    true
                }
            }
            Op::UpsertBlock { block } => {
                newer("blocks", &block.id.to_string())? && {
                    self.write_block_row(block, hlc)?;
                    true
                }
            }
            Op::DeleteBlock { id } => {
                newer("blocks", &id.to_string())? && {
                    self.remove_block_row_raw(*id, hlc)?;
                    true
                }
            }
            Op::PutAsset { asset } => {
                crate::blob::is_valid_hash(&asset.hash) && self.asset(&asset.hash)?.is_none() && {
                    self.write_asset(asset)?;
                    true
                }
            }
            Op::SetAlias { alias, page } => {
                let key = title_key(alias);
                let local: Option<String> = self
                    .row(
                        "SELECT hlc FROM aliases WHERE alias_key=?1
                         UNION ALL SELECT hlc FROM tombstones WHERE id=?2",
                        params![key, format!("alias:{key}")],
                        |r| r.get(0),
                    )
                    .optional()?;
                local.is_none_or(|l| hlc > l.as_str()) && {
                    self.write_alias(alias, *page, hlc)?;
                    true
                }
            }
        };
        if applied {
            self.log(hlc, op)?;
        }
        tx.commit()?;
        Ok(applied)
    }

    // ----------------------------------------------------------- raw writes

    pub(crate) fn tick(&self) -> String {
        self.clock.tick(self.now_ms())
    }

    pub(crate) fn log(&self, hlc: &str, op: &Op) -> Result<()> {
        self.exec(
            "INSERT INTO ops(hlc, payload) VALUES (?1, ?2)",
            params![hlc, serde_json::to_string(op)?],
        )?;
        Ok(())
    }

    pub(crate) fn write_page(&self, page: &Page, hlc: &str) -> Result<()> {
        self.write_page_row(page, hlc)?;
        self.log(hlc, &Op::UpsertPage { page: page.clone() })
    }

    fn write_page_row(&self, p: &Page, hlc: &str) -> Result<()> {
        self.exec(
            "INSERT INTO pages(id, title, title_key, journal_date, origin_journal, properties, created_at, updated_at, hlc, kind, description, hubs, tags, types)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET title=?2, title_key=?3, journal_date=?4, origin_journal=?5,
               properties=?6, created_at=?7, updated_at=?8, hlc=?9, kind=?10, description=?11, hubs=?12, tags=?13, types=?14",
            params![
                p.id.to_string(),
                p.title,
                title_key(&p.title),
                p.journal_date.map(journal_title),
                p.origin_journal.map(journal_title),
                serde_json::to_string(&p.properties)?,
                p.created_at,
                p.updated_at,
                hlc,
                p.kind.as_str(),
                p.description,
                serde_json::to_string(&p.hubs)?,
                serde_json::to_string(&p.tags)?,
                serde_json::to_string(&p.types)?
            ],
        )?;
        let id = p.id.to_string();
        self.exec("DELETE FROM page_tags WHERE page_id=?1", [&id])?;
        // The type implies a tag of the same name, so the index covers both (a page has one
        // type: older replicas may sync more, and only the first counts).
        for t in p.tags.iter().chain(p.types.iter().take(1)) {
            self.exec(
                "INSERT OR IGNORE INTO page_tags(page_id, tag_key) VALUES (?1, ?2)",
                params![id, title_key(t)],
            )?;
        }
        self.exec("DELETE FROM page_hubs WHERE page_id=?1", [&id])?;
        // A page belongs to one hub: only the first entry counts (older replicas may sync more).
        for h in p.hubs.iter().take(1) {
            self.exec(
                "INSERT OR IGNORE INTO page_hubs(page_id, hub_id) VALUES (?1, ?2)",
                params![id, h.to_string()],
            )?;
        }
        Ok(())
    }

    fn remove_page_row(&self, id: PageId, hlc: &str) -> Result<()> {
        self.remove_page_row_raw(id, hlc)?;
        self.log(hlc, &Op::DeletePage { id })
    }

    fn remove_page_row_raw(&self, id: PageId, hlc: &str) -> Result<()> {
        let s = id.to_string();
        // Cascade: blocks inserted concurrently on another replica must not linger orphaned.
        self.exec(
            "INSERT OR REPLACE INTO tombstones(id, hlc) SELECT id, ?2 FROM blocks WHERE page_id=?1",
            params![s, hlc],
        )?;
        self.exec(
            "DELETE FROM refs WHERE block_id IN (SELECT id FROM blocks WHERE page_id=?1)",
            [&s],
        )?;
        self.exec(
            "DELETE FROM block_refs WHERE block_id IN (SELECT id FROM blocks WHERE page_id=?1)",
            [&s],
        )?;
        self.conn
            .execute("DELETE FROM blocks WHERE page_id=?1", [&s])?;
        self.exec("DELETE FROM pages WHERE id=?1", [&s])?;
        self.conn
            .execute("DELETE FROM aliases WHERE page_id=?1", [&s])?;
        self.conn
            .execute("DELETE FROM page_activity WHERE page_id=?1", [&s])?;
        self.conn
            .execute("DELETE FROM synapses WHERE a=?1 OR b=?1", [&s])?;
        // Only the page's own memberships: rows naming it as a hub mirror other pages'
        // `hubs` and dangle until those pages are rewritten.
        self.exec("DELETE FROM page_hubs WHERE page_id=?1", [&s])?;
        self.exec("DELETE FROM page_tags WHERE page_id=?1", [&s])?;
        self.exec(
            "INSERT OR REPLACE INTO tombstones(id, hlc) VALUES (?1, ?2)",
            params![s, hlc],
        )?;
        Ok(())
    }

    fn write_block(&self, block: &Block, hlc: &str) -> Result<()> {
        self.write_block_row(block, hlc)?;
        self.log(
            hlc,
            &Op::UpsertBlock {
                block: block.clone(),
            },
        )
    }

    fn write_block_row(&self, b: &Block, hlc: &str) -> Result<()> {
        let id = b.id.to_string();
        self.exec(
            "INSERT INTO blocks(id, page_id, parent_id, position, content, properties, collapsed, created_at, updated_at, hlc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET page_id=?2, parent_id=?3, position=?4, content=?5,
               properties=?6, collapsed=?7, created_at=?8, updated_at=?9, hlc=?10",
            params![
                id,
                b.page_id.to_string(),
                b.parent_id.map(|p| p.to_string()),
                b.position,
                b.content,
                serde_json::to_string(&b.properties)?,
                b.collapsed,
                b.created_at,
                b.updated_at,
                hlc
            ],
        )?;
        self.conn
            .execute("DELETE FROM refs WHERE block_id=?1", [&id])?;
        self.conn
            .execute("DELETE FROM block_refs WHERE block_id=?1", [&id])?;
        let refs = extract_refs(&b.content);
        for (title, kind) in &refs.pages {
            self.exec(
                "INSERT OR IGNORE INTO refs(block_id, target_key, kind) VALUES (?1, ?2, ?3)",
                params![id, title_key(title), kind.as_str()],
            )?;
        }
        for v in b.properties.values() {
            for (title, _) in extract_refs(
                &v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
            )
            .pages
            {
                self.exec(
                    "INSERT OR IGNORE INTO refs(block_id, target_key, kind) VALUES (?1, ?2, 'property')",
                    params![id, title_key(&title)],
                )?;
            }
        }
        for (list, embed) in [(&refs.blocks, false), (&refs.embeds, true)] {
            for t in list {
                self.exec(
                    "INSERT OR IGNORE INTO block_refs(block_id, target_block, embed) VALUES (?1, ?2, ?3)",
                    params![id, t.to_string(), embed],
                )?;
            }
        }
        Ok(())
    }

    fn remove_block_row(&self, id: BlockId, hlc: &str) -> Result<()> {
        self.remove_block_row_raw(id, hlc)?;
        self.log(hlc, &Op::DeleteBlock { id })
    }

    fn remove_block_row_raw(&self, id: BlockId, hlc: &str) -> Result<()> {
        let s = id.to_string();
        self.exec("DELETE FROM blocks WHERE id=?1", [&s])?;
        self.conn
            .execute("DELETE FROM refs WHERE block_id=?1", [&s])?;
        self.conn
            .execute("DELETE FROM block_refs WHERE block_id=?1", [&s])?;
        self.exec(
            "INSERT OR REPLACE INTO tombstones(id, hlc) VALUES (?1, ?2)",
            params![s, hlc],
        )?;
        Ok(())
    }

    fn write_asset(&self, a: &Asset) -> Result<()> {
        self.exec(
            "INSERT OR IGNORE INTO assets(hash, size, mime, original_name, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![a.hash, a.size as i64, a.mime, a.original_name, a.created_at],
        )?;
        Ok(())
    }

    fn write_alias(&self, alias: &str, page: Option<PageId>, hlc: &str) -> Result<()> {
        let key = title_key(alias);
        match page {
            Some(p) => {
                self.exec(
                    "INSERT OR REPLACE INTO aliases(alias_key, alias, page_id, hlc) VALUES (?1, ?2, ?3, ?4)",
                    params![key, alias.trim(), p.to_string(), hlc],
                )?;
                self.exec(
                    "DELETE FROM tombstones WHERE id=?1",
                    [format!("alias:{key}")],
                )?;
            }
            None => {
                self.conn
                    .execute("DELETE FROM aliases WHERE alias_key=?1", [&key])?;
                self.exec(
                    "INSERT OR REPLACE INTO tombstones(id, hlc) VALUES (?1, ?2)",
                    params![format!("alias:{key}"), hlc],
                )?;
            }
        }
        Ok(())
    }

    /// Page whose own title (not alias) matches, case-insensitively.
    fn page_by_exact_title(&self, title: &str) -> Result<Option<Page>> {
        Ok(self
            .row(
                &format!("SELECT {PAGE_COLS} FROM pages WHERE title_key=?1"),
                [title_key(title)],
                page_row,
            )
            .optional()?)
    }

    /// A free title for the loser of a title conflict, using the random tail of its id.
    fn conflict_title(&self, title: &str, id: PageId) -> Result<String> {
        let hex = id.simple().to_string();
        for n in [8, 16, 32] {
            let t = format!("{title} ~{}", &hex[32 - n..]);
            if self.page_by_exact_title(&t)?.is_none() {
                return Ok(t);
            }
        }
        Err(Error::Conflict(format!(
            "no free conflict title for {title:?}"
        )))
    }
}

/// Savepoint guard: rolled back on drop unless committed.
pub(crate) struct Tx<'a> {
    conn: &'a Connection,
    /// Whether this guard opened a savepoint (false when nested in a transaction).
    active: bool,
    done: bool,
}

impl Tx<'_> {
    pub(crate) fn commit(mut self) -> Result<()> {
        if self.active {
            self.conn.execute_batch("RELEASE graph")?;
        }
        self.done = true;
        Ok(())
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if self.active && !self.done {
            let _ = self.conn.execute_batch("ROLLBACK TO graph; RELEASE graph");
        }
    }
}

fn entry<'a>(
    days: &'a mut std::collections::BTreeMap<String, TimelineDay>,
    d: &str,
) -> Result<&'a mut TimelineDay> {
    let date = parse_day(d).ok_or_else(|| Error::Invalid(d.into()))?;
    Ok(days.entry(d.to_string()).or_insert_with(|| TimelineDay {
        date,
        journal: None,
        created: vec![],
        edited: vec![],
    }))
}

pub(crate) fn day_of(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

pub(crate) fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

pub(crate) fn parse_id(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| Error::Invalid(format!("bad id {s}: {e}")))
}

fn props(s: String) -> Properties {
    serde_json::from_str(&s).unwrap_or_default()
}

fn page_row(r: &Row) -> rusqlite::Result<Page> {
    let id: String = r.get(0)?;
    Ok(Page {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        title: r.get(1)?,
        journal_date: r.get::<_, Option<String>>(2)?.and_then(|s| parse_day(&s)),
        origin_journal: r.get::<_, Option<String>>(3)?.and_then(|s| parse_day(&s)),
        properties: props(r.get(4)?),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        kind: PageKind::parse(&r.get::<_, String>(7)?),
        description: r.get(8)?,
        hubs: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
        tags: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or_default(),
        types: serde_json::from_str(&r.get::<_, String>(11)?).unwrap_or_default(),
    })
}

fn block_row(r: &Row) -> rusqlite::Result<Block> {
    let id = |s: String| Uuid::parse_str(&s).unwrap_or_default();
    Ok(Block {
        id: id(r.get(0)?),
        page_id: id(r.get(1)?),
        parent_id: r.get::<_, Option<String>>(2)?.map(id),
        position: r.get(3)?,
        content: r.get(4)?,
        properties: props(r.get(5)?),
        collapsed: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}
