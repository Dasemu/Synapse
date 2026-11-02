use rusqlite::OptionalExtension;

pub const VERSION: i64 = 9;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS pages (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    title_key TEXT NOT NULL UNIQUE,
    journal_date TEXT UNIQUE,
    origin_journal TEXT,
    properties TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    hlc TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'note',
    description TEXT NOT NULL DEFAULT '',
    hubs TEXT NOT NULL DEFAULT '[]',
    tags TEXT NOT NULL DEFAULT '[]',
    types TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS pages_updated ON pages(updated_at);
CREATE INDEX IF NOT EXISTS pages_origin ON pages(origin_journal);

CREATE TABLE IF NOT EXISTS aliases (
    alias_key TEXT PRIMARY KEY,
    alias TEXT NOT NULL,
    page_id TEXT NOT NULL,
    hlc TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS aliases_page ON aliases(page_id);

-- Derived index of `pages.hubs` (hub membership lives on the member page), kept in step on
-- every page write like `refs`.
CREATE TABLE IF NOT EXISTS page_hubs (
    page_id TEXT NOT NULL,
    hub_id TEXT NOT NULL,
    PRIMARY KEY (page_id, hub_id)
);
CREATE INDEX IF NOT EXISTS page_hubs_hub ON page_hubs(hub_id);

-- Derived index of `pages.tags` (normalised tag key per page), kept in step on every page write.
CREATE TABLE IF NOT EXISTS page_tags (
    page_id TEXT NOT NULL,
    tag_key TEXT NOT NULL,
    PRIMARY KEY (page_id, tag_key)
);
CREATE INDEX IF NOT EXISTS page_tags_key ON page_tags(tag_key);

CREATE TABLE IF NOT EXISTS blocks (
    id TEXT PRIMARY KEY,
    page_id TEXT NOT NULL,
    parent_id TEXT,
    position TEXT NOT NULL,
    content TEXT NOT NULL,
    properties TEXT NOT NULL DEFAULT '{}',
    collapsed INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    hlc TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS blocks_tree ON blocks(page_id, parent_id, position);

-- Page refs are stored by normalised title so they never dangle and survive sync.
CREATE TABLE IF NOT EXISTS refs (
    block_id TEXT NOT NULL,
    target_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (block_id, target_key, kind)
);
CREATE INDEX IF NOT EXISTS refs_target ON refs(target_key);

CREATE TABLE IF NOT EXISTS block_refs (
    block_id TEXT NOT NULL,
    target_block TEXT NOT NULL,
    embed INTEGER NOT NULL,
    PRIMARY KEY (block_id, target_block, embed)
);
CREATE INDEX IF NOT EXISTS block_refs_target ON block_refs(target_block);

-- Days on which a page was edited (links pages to journals of those days).
CREATE TABLE IF NOT EXISTS page_activity (
    page_id TEXT NOT NULL,
    day TEXT NOT NULL,
    edits INTEGER NOT NULL,
    PRIMARY KEY (page_id, day)
);
CREATE INDEX IF NOT EXISTS page_activity_day ON page_activity(day);

-- Weighted, undirected associations between pages (a < b). Local to the device.
CREATE TABLE IF NOT EXISTS synapses (
    a TEXT NOT NULL,
    b TEXT NOT NULL,
    weight REAL NOT NULL,
    last_fired INTEGER NOT NULL,
    PRIMARY KEY (a, b)
);
CREATE INDEX IF NOT EXISTS synapses_b ON synapses(b);

CREATE TABLE IF NOT EXISTS assets (
    hash TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mime TEXT,
    original_name TEXT,
    created_at INTEGER NOT NULL
);

-- v8: type metadata (icon), one row per type name key; `removed` rows are tombstones.
CREATE TABLE IF NOT EXISTS type_defs (
    type_key TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    icon TEXT,
    removed INTEGER NOT NULL DEFAULT 0,
    hlc TEXT NOT NULL
);

-- v9: per-module key-value data; a NULL value is a tombstone.
CREATE TABLE IF NOT EXISTS module_data (
    module TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT,
    hlc TEXT NOT NULL,
    PRIMARY KEY (module, key)
);

CREATE TABLE IF NOT EXISTS tombstones (id TEXT PRIMARY KEY, hlc TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS ops (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    hlc TEXT NOT NULL,
    payload TEXT NOT NULL
);
"#;

/// Every page's tag sources: `tags`, `types` and the title of its direct hub (an implicit tag,
/// only for notes: hubs, templates and journals get none). Joined through `page_hubs`, which
/// holds at most one row per page. `?1` limits the result to one page id (NULL: all pages).
pub const TAG_SOURCES_SQL: &str = "SELECT p.id, p.tags, p.types, h.title FROM pages p
     LEFT JOIN page_hubs ph ON ph.page_id = p.id AND p.kind = 'note' AND p.journal_date IS NULL
     LEFT JOIN pages h ON h.id = ph.hub_id AND h.kind = 'hub'
     WHERE (?1 IS NULL OR p.id = ?1)";

/// One page's tag sources, see [`TAG_SOURCES_SQL`]. Malformed JSON values count as empty.
pub struct TagSource {
    pub id: String,
    pub tags: Vec<String>,
    pub types: Vec<String>,
    pub hub: Option<String>,
}

/// Reads [`TagSource`]s for one page or (`None`) all of them.
pub fn tag_sources(
    conn: &rusqlite::Connection,
    only: Option<&str>,
) -> rusqlite::Result<Vec<TagSource>> {
    let mut st = conn.prepare_cached(TAG_SOURCES_SQL)?;
    let rows = st.query_map([only], |r| {
        let tags: String = r.get(1)?;
        let types: String = r.get(2)?;
        Ok(TagSource {
            id: r.get(0)?,
            tags: serde_json::from_str(&tags).unwrap_or_default(),
            types: serde_json::from_str(&types).unwrap_or_default(),
            hub: r.get(3)?,
        })
    })?;
    rows.collect()
}

impl TagSource {
    /// Every tag name: listed tags, the first type and the hub title (one type per page).
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.tags
            .iter()
            .chain(self.types.iter().take(1))
            .chain(self.hub.iter())
    }
}

fn insert_tag_rows(conn: &rusqlite::Connection, sources: &[TagSource]) -> rusqlite::Result<()> {
    let mut ins =
        conn.prepare_cached("INSERT OR IGNORE INTO page_tags(page_id, tag_key) VALUES (?1, ?2)")?;
    for src in sources {
        for t in src.names() {
            ins.execute([&src.id, &crate::model::title_key(t)])?;
        }
    }
    Ok(())
}

/// Rebuilds the `page_tags` index from every page's tags, first type and hub.
pub fn rebuild_page_tags(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM page_tags", [])?;
    insert_tag_rows(conn, &tag_sources(conn, None)?)
}

/// Rewrites the `page_tags` rows of one page (derived data: no ops needed).
pub fn reindex_page_tags(conn: &rusqlite::Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM page_tags WHERE page_id=?1", [id])?;
    insert_tag_rows(conn, &tag_sources(conn, Some(id))?)
}

/// v6: tag pages become notes, tags are sorted and deduplicated, a page keeps one type.
/// Malformed `tags`/`types` values are repaired to empty lists.
fn migrate_v6(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute("UPDATE pages SET kind='note' WHERE kind='tag'", [])?;
    let rows: Vec<(String, String, String)> = {
        let mut st = conn.prepare("SELECT id, tags, types FROM pages")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut upd = conn.prepare("UPDATE pages SET tags=?2, types=?3 WHERE id=?1")?;
    for (id, tags, types) in rows {
        let tags_v: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
        let mut types_v: Vec<String> = serde_json::from_str(&types).unwrap_or_default();
        types_v.truncate(1);
        let tags_n = serde_json::to_string(&crate::tags::sorted_tags(&tags_v)).unwrap_or_default();
        let types_n = serde_json::to_string(&types_v).unwrap_or_default();
        if tags_n != tags || types_n != types {
            upd.execute([&id, &tags_n, &types_n])?;
        }
    }
    rebuild_page_tags(conn)
}

/// Rebuilds the `page_hubs` index from every page's `hubs` column.
pub fn rebuild_page_hubs(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM page_hubs", [])?;
    let rows: Vec<(String, String)> = {
        let mut st = conn.prepare("SELECT id, hubs FROM pages WHERE hubs != '[]'")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut ins =
        conn.prepare("INSERT OR IGNORE INTO page_hubs(page_id, hub_id) VALUES (?1, ?2)")?;
    for (id, hubs) in rows {
        let hubs: Vec<uuid::Uuid> = serde_json::from_str(&hubs).unwrap_or_default();
        for h in hubs.into_iter().take(1) {
            ins.execute([&id, &h.to_string()])?;
        }
    }
    Ok(())
}

/// Upgrades databases created by older versions in place.
pub fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_alias_hlc: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('aliases') WHERE name='hlc'")?
        .exists([])?;
    if !has_alias_hlc {
        conn.execute_batch("ALTER TABLE aliases ADD COLUMN hlc TEXT NOT NULL DEFAULT ''")?;
    }
    // v3: page kinds (hubs), descriptions and hub membership; `page_hubs` is created by
    // `SCHEMA` and rebuilt from `pages.hubs`.
    for (col, ddl) in [
        ("kind", "kind TEXT NOT NULL DEFAULT 'note'"),
        ("description", "description TEXT NOT NULL DEFAULT ''"),
        ("hubs", "hubs TEXT NOT NULL DEFAULT '[]'"),
        // v4: free-form tags per page; `page_tags` is created by `SCHEMA`.
        ("tags", "tags TEXT NOT NULL DEFAULT '[]'"),
        // v5: tag pages (kind 'tag') and plain-name types per page.
        ("types", "types TEXT NOT NULL DEFAULT '[]'"),
    ] {
        let has: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('pages') WHERE name=?1")?
            .exists([col])?;
        if !has {
            conn.execute_batch(&format!("ALTER TABLE pages ADD COLUMN {ddl}"))?;
        }
    }
    let version: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key='schema'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .and_then(|v| v.parse().ok());
    // One transaction: much faster, and an interrupted migration leaves the old version
    // stamp, so it simply runs again (every step is idempotent).
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let run = || -> rusqlite::Result<()> {
        if version.is_some_and(|v| v < 3) {
            rebuild_page_hubs(conn)?;
        }
        // v6 rebuilds `page_tags` too, which covers the v4 and v7 rebuilds.
        if version.is_some_and(|v| v < 6) {
            migrate_v6(conn)?;
        } else if version.is_some_and(|v| v < 7) {
            // v7: a page's hub title is an implicit tag, so the index changes.
            rebuild_page_tags(conn)?;
        }
        // v8: `type_defs` (type icons) and v9: `module_data` are created empty by `SCHEMA`;
        // nothing to convert.
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema', ?1)",
            [VERSION.to_string()],
        )?;
        Ok(())
    };
    match run() {
        Ok(()) => conn.execute_batch("COMMIT"),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
