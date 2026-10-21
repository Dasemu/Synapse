//! Weighted associations between pages ("synapses") and spreading activation.
//!
//! Weights grow when two pages are linked, co-visited or co-edited (Hebbian) and decay with a
//! half-life unless reinforced. `spread` ranks related notes by propagating activation over
//! explicit refs, synapses and the temporal journal structure.

use crate::model::PageId;
use crate::store::{Graph, parse_id};
use crate::{Result, journal_title};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

pub const HALF_LIFE_MS: f64 = 30.0 * 24.0 * 3600.0 * 1000.0;
pub const LINK_STRENGTH: f32 = 0.5;
pub const VISIT_STRENGTH: f32 = 0.2;
pub const MAX_WEIGHT: f32 = 10.0;

fn ordered(a: PageId, b: PageId) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn decayed(w: f32, last: i64, now: i64) -> f32 {
    let dt = (now - last).max(0) as f64;
    (w as f64 * 0.5f64.powf(dt / HALF_LIFE_MS)) as f32
}

/// Strengthens the a–b synapse by `amount` (negative to weaken, e.g. rejected suggestion).
pub fn fire(conn: &Connection, a: PageId, b: PageId, amount: f32, now: i64) -> Result<()> {
    if a == b {
        return Ok(());
    }
    let w = (weight(conn, a, b, now)? + amount).clamp(0.0, MAX_WEIGHT);
    let (x, y) = ordered(a, b);
    conn.execute(
        "INSERT OR REPLACE INTO synapses(a, b, weight, last_fired) VALUES (?1, ?2, ?3, ?4)",
        params![x, y, w, now],
    )?;
    Ok(())
}

pub fn weight(conn: &Connection, a: PageId, b: PageId, now: i64) -> Result<f32> {
    let (x, y) = ordered(a, b);
    let row: Option<(f32, i64)> = conn
        .query_row(
            "SELECT weight, last_fired FROM synapses WHERE a=?1 AND b=?2",
            params![x, y],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(w, l)| decayed(w, l, now)).unwrap_or(0.0))
}

/// Weighted neighbours of a page: refs both ways, synapses, journal chain and anchors.
pub fn neighbors(g: &Graph, page: PageId) -> Result<Vec<(PageId, f32)>> {
    let conn = g.conn();
    let now = g.now_ms();
    let mut out: HashMap<PageId, f32> = HashMap::new();
    let mut add = |id: PageId, w: f32| *out.entry(id).or_default() += w;

    for p in g.outgoing_pages(page)? {
        add(p, 1.0);
    }
    for bl in g.backlinks(page)? {
        add(bl.block.page_id, 1.0);
    }
    let pid = page.to_string();
    let mut st = conn.prepare_cached(
        "SELECT b, weight, last_fired FROM synapses WHERE a=?1
         UNION ALL SELECT a, weight, last_fired FROM synapses WHERE b=?1",
    )?;
    for r in st.query_map([&pid], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, f32>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (other, w, last) = r?;
        add(parse_id(&other)?, decayed(w, last, now) / 2.0);
    }
    if let Some(p) = g.get_page(page)? {
        if let Some(d) = p.journal_date {
            let n = g.journal_neighbors(d)?;
            for j in [n.prev, n.next].into_iter().flatten() {
                add(crate::store::journal_id(j), 0.5);
            }
            let day = journal_title(d);
            let mut st = conn.prepare_cached(
                "SELECT id FROM pages WHERE origin_journal=?1
                 UNION SELECT page_id FROM page_activity WHERE day=?1",
            )?;
            for r in st.query_map([&day], |r| r.get::<_, String>(0))? {
                add(parse_id(&r?)?, 0.4);
            }
        } else {
            if let Some(o) = p.origin_journal {
                add(crate::store::journal_id(o), 0.4);
            }
            for (d, _) in g.edit_days(page)? {
                add(crate::store::journal_id(d), 0.3);
            }
        }
    }
    out.remove(&page);
    Ok(out.into_iter().collect())
}

/// Spreading activation from `seed`, three hops, normalised by fan-out.
pub fn spread(g: &Graph, seed: PageId, limit: usize) -> Result<Vec<(PageId, f32)>> {
    const DECAY: f32 = 0.5;
    let mut activation: HashMap<PageId, f32> = HashMap::new();
    let mut frontier = vec![(seed, 1.0f32)];
    let mut cache: HashMap<PageId, Vec<(PageId, f32)>> = HashMap::new();
    for _ in 0..3 {
        let mut next: HashMap<PageId, f32> = HashMap::new();
        for (node, a) in frontier {
            if a < 0.01 {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(node) {
                e.insert(neighbors(g, node)?);
            }
            let nbrs = &cache[&node];
            let fan = (nbrs.len().max(1) as f32).sqrt();
            for &(m, w) in nbrs {
                if m != seed {
                    *next.entry(m).or_default() += a * w * DECAY / fan;
                }
            }
        }
        for (m, a) in &next {
            *activation.entry(*m).or_default() += a;
        }
        frontier = next.into_iter().collect();
    }
    let mut ranked: Vec<_> = activation.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(limit);
    Ok(ranked)
}
