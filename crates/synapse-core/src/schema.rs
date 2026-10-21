pub const VERSION: i64 = 1;

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
    hlc TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS pages_updated ON pages(updated_at);
CREATE INDEX IF NOT EXISTS pages_origin ON pages(origin_journal);

CREATE TABLE IF NOT EXISTS aliases (
    alias_key TEXT PRIMARY KEY,
    alias TEXT NOT NULL,
    page_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS aliases_page ON aliases(page_id);

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

CREATE TABLE IF NOT EXISTS tombstones (id TEXT PRIMARY KEY, hlc TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS ops (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    hlc TEXT NOT NULL,
    payload TEXT NOT NULL
);
"#;
