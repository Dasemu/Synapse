//! Bookkeeping of what the importer wrote, so re-imports never clobber native Synapse content.
//! The tables are local to the graph database and not part of the op log.

use serde_json::Value;
use std::collections::HashMap;
use synapse_core::{Block, BlockId, Graph, PageId, Properties, Result};
use uuid::Uuid;

pub fn init(g: &Graph) -> Result<()> {
    g.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS import_pages(
           source_path TEXT PRIMARY KEY, page_id TEXT NOT NULL, props_hash TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS import_blocks(
           block_id TEXT PRIMARY KEY, page_id TEXT NOT NULL, source_path TEXT NOT NULL,
           content_hash TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS import_blocks_page ON import_blocks(page_id);",
    )?;
    Ok(())
}

fn fnv(parts: &[&[u8]]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.iter().chain(&[0u8]) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

fn json(props: &Properties) -> String {
    serde_json::to_string(&Value::Object(props.clone())).unwrap_or_default()
}

/// Hash used for deriving fallback ids (ignores `collapsed`).
pub fn id_hash(content: &str, props: &Properties) -> String {
    fnv(&[content.as_bytes(), json(props).as_bytes()])
}

/// Hash of the user-editable state of a block.
pub fn state_hash(b: &Block) -> String {
    fnv(&[
        b.content.as_bytes(),
        json(&b.properties).as_bytes(),
        &[u8::from(b.collapsed)],
    ])
}

pub fn props_hash(p: &Properties) -> String {
    fnv(&[json(p).as_bytes()])
}

pub fn page_of(g: &Graph, source: &str) -> Result<Option<PageId>> {
    let mut st = g
        .conn()
        .prepare_cached("SELECT page_id FROM import_pages WHERE source_path=?1")?;
    let rows: Vec<String> = st
        .query_map([source], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows.first().and_then(|s| Uuid::parse_str(s).ok()))
}

pub fn page_props_hash(g: &Graph, source: &str) -> Result<Option<String>> {
    let mut st = g
        .conn()
        .prepare_cached("SELECT props_hash FROM import_pages WHERE source_path=?1")?;
    let rows: Vec<String> = st
        .query_map([source], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows.into_iter().next())
}

pub fn set_page(g: &Graph, source: &str, page: PageId, props_hash: &str) -> Result<()> {
    g.conn().execute(
        "INSERT OR REPLACE INTO import_pages(source_path, page_id, props_hash) VALUES (?1, ?2, ?3)",
        [source, &page.to_string(), props_hash],
    )?;
    Ok(())
}

/// `block id → state hash at import` for the blocks this importer wrote on `page`.
pub fn blocks_of(g: &Graph, page: PageId) -> Result<HashMap<BlockId, String>> {
    let mut st = g
        .conn()
        .prepare_cached("SELECT block_id, content_hash FROM import_blocks WHERE page_id=?1")?;
    let rows = st.query_map([page.to_string()], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = HashMap::new();
    for r in rows {
        let (id, h) = r?;
        if let Ok(id) = Uuid::parse_str(&id) {
            out.insert(id, h);
        }
    }
    Ok(out)
}

pub fn set_block(g: &Graph, b: &Block, source: &str, hash: &str) -> Result<()> {
    g.conn()
        .prepare_cached(
            "INSERT OR REPLACE INTO import_blocks(block_id, page_id, source_path, content_hash)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute([&b.id.to_string(), &b.page_id.to_string(), source, hash])?;
    Ok(())
}

pub fn drop_block(g: &Graph, id: BlockId) -> Result<()> {
    g.conn().execute(
        "DELETE FROM import_blocks WHERE block_id=?1",
        [id.to_string()],
    )?;
    Ok(())
}
