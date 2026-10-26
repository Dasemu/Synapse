//! The page-level graph behind the graph views: every page as a node and every relation
//! between pages as an edge (links, tags, property refs, block refs and embeds aggregated per
//! page, unlinked mentions and synapses), plus degree, backlinks, clusters and primary tags.
//!
//! Built with a few read-only queries over the core tables and cached; the cache key is the
//! op-log sequence plus the synapse table's state (visits fire synapses without logging ops).

use super::dto::{GraphData, GraphNode, edge_kind};
use super::graph::SynapseGraph;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use synapse_core::synapse::HALF_LIFE_MS;
use synapse_core::{Graph, title_key};

/// What invalidates the cached graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheKey {
    seq: i64,
    synapses: i64,
    synapse_last: i64,
    /// Synapse weights decay with time: rebuild at least hourly.
    hour: i64,
}

/// Synapses weaker than this are not drawn.
const MIN_SYNAPSE: f32 = 0.05;
/// Titles shorter than this are too ambiguous to detect as mentions.
const MIN_MENTION_LEN: usize = 3;

impl SynapseGraph {
    /// The whole page-level graph (journals included; the UI filters). Cached until the
    /// graph changes.
    pub fn graph_data(&self) -> Result<GraphData> {
        let g = self.lock();
        let key = cache_key(&g)?;
        let mut cache = self.graph_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, data)) = cache.as_ref()
            && *k == key
        {
            return Ok(GraphData::clone(data));
        }
        let data = Arc::new(build(&g, key.seq)?);
        *cache = Some((key, Arc::clone(&data)));
        Ok(GraphData::clone(&data))
    }
}

fn cache_key(g: &Graph) -> Result<CacheKey> {
    let (synapses, synapse_last) = g.conn().query_row(
        "SELECT count(*), coalesce(max(last_fired), 0) FROM synapses",
        [],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    Ok(CacheKey {
        seq: g.last_seq()?,
        synapses,
        synapse_last,
        hour: g.now_ms() / 3_600_000,
    })
}

struct Builder {
    edges: HashMap<(u32, u32, u8), f32>,
}

impl Builder {
    fn add(&mut self, s: u32, t: u32, kind: u8, w: f32) {
        if s != t {
            *self.edges.entry((s, t, kind)).or_default() += w;
        }
    }
}

pub(crate) fn build(g: &Graph, seq: i64) -> Result<GraphData> {
    let conn = g.conn();

    // Nodes.
    let mut nodes = Vec::new();
    let mut index: HashMap<String, u32> = HashMap::new();
    let page_tags: HashMap<String, Vec<String>> = g
        .all_page_tags()?
        .into_iter()
        .map(|(id, t)| (id.to_string(), t))
        .collect();
    let mut page_types: HashMap<String, String> = g
        .all_page_types()?
        .into_iter()
        .map(|(id, t)| (id.to_string(), t))
        .collect();
    {
        let mut st = conn.prepare(
            "SELECT id, title, journal_date, origin_journal, created_at FROM pages
             WHERE kind != 'template'
             ORDER BY journal_date IS NOT NULL, created_at",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (id, title, journal_date, origin_journal, created_at) = row?;
            let page_type = page_types.remove(&id);
            let tags = page_tags.get(&id).cloned().unwrap_or_default();
            index.insert(id.clone(), nodes.len() as u32);
            nodes.push(GraphNode {
                id,
                title,
                journal_date,
                origin_journal,
                created_at,
                degree: 0,
                backlinks: 0,
                cluster: 0,
                primary_tag: None,
                is_hub: false,
                home_hub: None,
                tags,
                is_tag: false,
                page_type,
            });
        }
    }
    // Title and alias keys resolve page refs (refs are stored by normalised title).
    let mut by_key: HashMap<String, u32> = HashMap::new();
    for (t, id) in g.all_titles()? {
        if let Some(&i) = index.get(&id.to_string()) {
            by_key.entry(title_key(&t)).or_insert(i);
        }
    }

    let mut b = Builder {
        edges: HashMap::new(),
    };

    // Page refs: links, tags, property refs.
    {
        let mut st = conn.prepare(
            "SELECT b.page_id, r.target_key, r.kind FROM refs r JOIN blocks b ON b.id = r.block_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (page, key, kind) = row?;
            let (Some(&s), Some(&t)) = (index.get(&page), by_key.get(&key)) else {
                continue;
            };
            let kind = match kind.as_str() {
                "tag" => edge_kind::TAG,
                "property" => edge_kind::PROPERTY,
                _ => edge_kind::LINK,
            };
            b.add(s, t, kind, 1.0);
        }
    }

    // Block refs and embeds, aggregated to the pages of both blocks.
    {
        let mut st = conn.prepare(
            "SELECT src.page_id, dst.page_id, r.embed FROM block_refs r
             JOIN blocks src ON src.id = r.block_id JOIN blocks dst ON dst.id = r.target_block",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?;
        for row in rows {
            let (sp, tp, embed) = row?;
            if let (Some(&s), Some(&t)) = (index.get(&sp), index.get(&tp)) {
                let kind = if embed {
                    edge_kind::EMBED
                } else {
                    edge_kind::BLOCK_REF
                };
                b.add(s, t, kind, 1.0);
            }
        }
    }

    // Explicitly related pairs (any direction), so mentions only add new information.
    let linked: HashSet<(u32, u32)> = b
        .edges
        .keys()
        .flat_map(|&(s, t, _)| [(s, t), (t, s)])
        .collect();
    mentions(g, &nodes, &index, &linked, &mut b)?;

    // Synapses (undirected), decayed to now.
    {
        let now = g.now_ms();
        let mut st = conn.prepare("SELECT a, b, weight, last_fired FROM synapses")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (a, bb, w, last) = row?;
            let w = (w * 0.5f64.powf((now - last).max(0) as f64 / HALF_LIFE_MS)) as f32;
            if let (Some(&x), Some(&y)) = (index.get(&a), index.get(&bb))
                && w >= MIN_SYNAPSE
            {
                b.add(x.min(y), x.max(y), edge_kind::SYNAPSE, w);
            }
        }
    }

    // Hub membership (hub → member), cycles already broken by `structure()`.
    let structure = g.structure()?;
    for h in &structure.hubs {
        let Some(&hi) = index.get(&h.page.to_string()) else {
            continue;
        };
        nodes[hi as usize].is_hub = true;
        for m in &h.members {
            if let Some(&mi) = index.get(&m.to_string()) {
                b.add(hi, mi, edge_kind::CONTAINS, 1.0);
            }
        }
    }

    tagged(g, &index, &mut nodes, &mut b)?;

    // Stable edge order (sources, then targets, then kind).
    let mut edges: Vec<((u32, u32, u8), f32)> = b.edges.into_iter().collect();
    edges.sort_by_key(|&(k, _)| k);

    // Degree, backlinks and primary tags over structural edges.
    let n = nodes.len();
    let mut neigh: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    let mut referrers: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    let mut tag_use: Vec<HashMap<u32, f32>> = vec![HashMap::new(); n];
    for &((s, t, k), w) in &edges {
        if k == edge_kind::MENTION || k == edge_kind::SYNAPSE {
            continue;
        }
        neigh[s as usize].insert(t);
        neigh[t as usize].insert(s);
        referrers[t as usize].insert(s);
        if k == edge_kind::TAG || k == edge_kind::TAGGED {
            *tag_use[s as usize].entry(t).or_default() += w;
        }
    }
    let clusters = communities(&neigh);
    for (i, node) in nodes.iter_mut().enumerate() {
        node.degree = neigh[i].len() as u32;
        node.backlinks = referrers[i].len() as u32;
        node.cluster = clusters[i];
        node.primary_tag = tag_use[i]
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(&t, _)| t);
    }

    for (page, home) in &structure.home {
        if let (Some(&i), Some(&h)) = (index.get(&page.to_string()), index.get(&home.to_string())) {
            nodes[i as usize].home_hub = Some(h);
        }
    }

    let mut out = GraphData {
        nodes,
        sources: Vec::with_capacity(edges.len()),
        targets: Vec::with_capacity(edges.len()),
        kinds: Vec::with_capacity(edges.len()),
        weights: Vec::with_capacity(edges.len()),
        seq,
    };
    for ((s, t, k), w) in edges {
        out.sources.push(s);
        out.targets.push(t);
        out.kinds.push(k);
        out.weights.push(w);
    }
    Ok(out)
}

/// Tags are virtual nodes (`tag:<key>`, appended after the pages): every page carrying a tag,
/// listed or implied by a type, gets one `TAGGED` edge to it. A tag named like a hub already
/// in the graph (a hub's implicit tag) uses the hub's node instead of a second one.
fn tagged(
    g: &Graph,
    index: &HashMap<String, u32>,
    nodes: &mut Vec<GraphNode>,
    b: &mut Builder,
) -> Result<()> {
    let display: HashMap<String, String> = g
        .tag_catalog()?
        .into_iter()
        .map(|t| (title_key(&t.name), t.name))
        .collect();
    let mut st = g
        .conn()
        .prepare("SELECT tag_key, page_id FROM page_tags ORDER BY tag_key, page_id")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut tag_index: HashMap<String, u32> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.is_hub)
        .map(|(i, n)| (title_key(&n.title), i as u32))
        .collect();
    for row in rows {
        let (key, page) = row?;
        let Some(&s) = index.get(&page) else {
            continue;
        };
        let t = *tag_index.entry(key.clone()).or_insert_with(|| {
            nodes.push(GraphNode {
                id: format!("tag:{key}"),
                title: display.get(&key).cloned().unwrap_or_else(|| key.clone()),
                journal_date: None,
                origin_journal: None,
                created_at: 0,
                degree: 0,
                backlinks: 0,
                cluster: 0,
                primary_tag: None,
                is_hub: false,
                home_hub: None,
                tags: Vec::new(),
                is_tag: true,
                page_type: None,
            });
            nodes.len() as u32 - 1
        });
        // A hub's members already hang from it by a CONTAINS edge.
        if !b.edges.contains_key(&(t, s, edge_kind::CONTAINS)) {
            b.add(s, t, edge_kind::TAGGED, 1.0);
        }
    }
    Ok(())
}

/// Unlinked references: a non-journal page title (or alias) appearing as whole words in a
/// block of another page that does not already reference it. Titles are indexed by their
/// first word so each block is scanned once.
fn mentions(
    g: &Graph,
    nodes: &[GraphNode],
    index: &HashMap<String, u32>,
    linked: &HashSet<(u32, u32)>,
    b: &mut Builder,
) -> Result<()> {
    let mut by_first: HashMap<String, Vec<(String, u32)>> = HashMap::new();
    for (t, id) in g.all_titles()? {
        let Some(&i) = index.get(&id.to_string()) else {
            continue;
        };
        if nodes[i as usize].journal_date.is_some() {
            continue;
        }
        let key = title_key(&t);
        if key.chars().count() < MIN_MENTION_LEN {
            continue;
        }
        let first = key
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("");
        if first.is_empty() {
            continue;
        }
        by_first
            .entry(first.to_string())
            .or_default()
            .push((key, i));
    }
    if by_first.is_empty() {
        return Ok(());
    }
    let mut st = g.conn().prepare("SELECT page_id, content FROM blocks")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut found: HashSet<(u32, u32)> = HashSet::new();
    for row in rows {
        let (page, content) = row?;
        let Some(&s) = index.get(&page) else { continue };
        let text = strip_markup(&content.to_lowercase());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < text.len() {
            let start_ok = i == 0 || !is_word(text[..i].chars().next_back().unwrap());
            if start_ok && text.is_char_boundary(i) {
                let rest = &text[i..];
                let word_end = rest
                    .find(|c: char| !c.is_alphanumeric())
                    .unwrap_or(rest.len());
                if word_end > 0
                    && let Some(cands) = by_first.get(&rest[..word_end])
                {
                    for (key, t) in cands {
                        if *t == s || !rest.starts_with(key.as_str()) {
                            continue;
                        }
                        let after = rest[key.len()..].chars().next();
                        if after.is_some_and(is_word) {
                            continue;
                        }
                        if !linked.contains(&(s, *t)) && found.insert((s, *t)) {
                            b.add(s, *t, edge_kind::MENTION, 1.0);
                        }
                    }
                }
            }
            // Next char boundary.
            i += 1;
            while i < bytes.len() && !text.is_char_boundary(i) {
                i += 1;
            }
        }
    }
    Ok(())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// Blanks out explicit references (`[[..]]`, `#tag`, `((..))`, code) so their text is not
/// counted again as a mention. Keeps byte offsets irrelevant: output is only scanned.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("#[[") {
            let end = r.find("]]").map(|e| e + 2).unwrap_or(r.len());
            rest = &r[end..];
            out.push(' ');
        } else if let Some(r) = rest.strip_prefix("[[") {
            let end = r.find("]]").map(|e| e + 2).unwrap_or(r.len());
            rest = &r[end..];
            out.push(' ');
        } else if let Some(r) = rest.strip_prefix("((") {
            let end = r.find("))").map(|e| e + 2).unwrap_or(r.len());
            rest = &r[end..];
            out.push(' ');
        } else if let Some(r) = rest.strip_prefix('`') {
            let end = r.find('`').map(|e| e + 1).unwrap_or(r.len());
            rest = &r[end..];
            out.push(' ');
        } else if rest.starts_with('#') && out.chars().next_back().is_none_or(|c| c.is_whitespace())
        {
            let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
            rest = &rest[end..];
            out.push(' ');
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Communities by Louvain modularity optimisation (local moving + aggregation, in node
/// index order, so results are deterministic). Clusters are renumbered by size, largest
/// first; isolated pages are singleton clusters.
fn communities(neigh: &[HashSet<u32>]) -> Vec<u32> {
    let n = neigh.len();
    // Current level graph: weighted adjacency (no self loops) and node strength.
    let mut adj: Vec<HashMap<u32, f64>> = neigh
        .iter()
        .map(|s| s.iter().map(|&j| (j, 1.0)).collect())
        .collect();
    let mut strength: Vec<f64> = adj.iter().map(|a| a.values().sum()).collect();
    let m2: f64 = strength.iter().sum();
    // Community of each original node.
    let mut member: Vec<u32> = (0..n as u32).collect();
    if m2 == 0.0 {
        return rank_by_size(&member);
    }
    loop {
        let size = adj.len();
        let mut comm: Vec<u32> = (0..size as u32).collect();
        let mut tot: Vec<f64> = strength.clone();
        let mut moved_any = false;
        for _ in 0..20 {
            let mut moved = false;
            for i in 0..size {
                let ki = strength[i];
                let old = comm[i];
                tot[old as usize] -= ki;
                let mut links: HashMap<u32, f64> = HashMap::new();
                for (&j, &w) in &adj[i] {
                    *links.entry(comm[j as usize]).or_default() += w;
                }
                let gain = |c: u32, kin: f64| kin - tot[c as usize] * ki / m2;
                let mut best = old;
                let mut best_gain = gain(old, links.get(&old).copied().unwrap_or(0.0));
                let mut cands: Vec<(u32, f64)> = links.into_iter().collect();
                cands.sort_by_key(|&(c, _)| c);
                for (c, kin) in cands {
                    let g = gain(c, kin);
                    if g > best_gain + 1e-12 {
                        best = c;
                        best_gain = g;
                    }
                }
                comm[i] = best;
                tot[best as usize] += ki;
                if best != old {
                    moved = true;
                    moved_any = true;
                }
            }
            if !moved {
                break;
            }
        }
        if !moved_any {
            break;
        }
        // Aggregate communities into nodes.
        let mut renum: HashMap<u32, u32> = HashMap::new();
        for &c in &comm {
            let next = renum.len() as u32;
            renum.entry(c).or_insert(next);
        }
        let k = renum.len();
        let mut next_adj: Vec<HashMap<u32, f64>> = vec![HashMap::new(); k];
        let mut next_strength = vec![0.0; k];
        for i in 0..size {
            let ci = renum[&comm[i]];
            next_strength[ci as usize] += strength[i];
            for (&j, &w) in &adj[i] {
                let cj = renum[&comm[j as usize]];
                if ci != cj {
                    *next_adj[ci as usize].entry(cj).or_default() += w;
                }
            }
        }
        for m in member.iter_mut() {
            *m = renum[&comm[*m as usize]];
        }
        if k == size {
            break;
        }
        adj = next_adj;
        strength = next_strength;
    }
    rank_by_size(&member)
}

fn rank_by_size(label: &[u32]) -> Vec<u32> {
    let mut size: HashMap<u32, u32> = HashMap::new();
    for &l in label {
        *size.entry(l).or_default() += 1;
    }
    let mut order: Vec<(u32, u32)> = size.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let rank: HashMap<u32, u32> = order
        .iter()
        .enumerate()
        .map(|(r, &(l, _))| (l, r as u32))
        .collect();
    label.iter().map(|l| rank[l]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::{Place, PlaceKind};

    fn open() -> (tempfile::TempDir, SynapseGraph) {
        let dir = tempfile::tempdir().unwrap();
        let g = SynapseGraph::open(dir.path().to_string_lossy().into_owned()).unwrap();
        (dir, g)
    }

    fn add(g: &SynapseGraph, page: &str, content: &str) -> String {
        let p = g.open_page(page.into()).unwrap().page.id;
        let place = Place {
            kind: PlaceKind::LastChildOf,
            anchor: None,
        };
        g.insert_block(p, place, content.into()).unwrap().id
    }

    fn edges(d: &GraphData) -> Vec<(String, String, u8)> {
        let t = |i: u32| d.nodes[i as usize].title.clone();
        let mut v: Vec<_> = (0..d.kinds.len())
            .map(|e| (t(d.sources[e]), t(d.targets[e]), d.kinds[e]))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn extracts_every_relation_kind_at_page_level() {
        let (_d, g) = open();
        let target = add(&g, "Rust", "a systems language");
        add(&g, "Notes", "learning [[Rust]] and #lang");
        add(&g, "Notes", "type:: [[Book]]");
        add(&g, "Notes", &format!("see (({target}))"));
        add(&g, "Notes", &format!("{{{{embed (({target}))}}}}"));
        add(&g, "Diary", "talked about rust and lang today");
        let d = g.graph_data().unwrap();
        let e = edges(&d);
        let has = |s: &str, t: &str, k: u8| e.contains(&(s.into(), t.into(), k));
        assert!(has("Notes", "Rust", edge_kind::LINK));
        assert!(has("Notes", "lang", edge_kind::TAG));
        assert!(has("Notes", "Book", edge_kind::PROPERTY));
        assert!(has("Notes", "Rust", edge_kind::BLOCK_REF));
        assert!(has("Notes", "Rust", edge_kind::EMBED));
        assert!(has("Diary", "Rust", edge_kind::MENTION));
        assert!(has("Diary", "lang", edge_kind::MENTION));
        // Explicit links are not repeated as mentions; titles do not mention themselves.
        assert!(!has("Notes", "Rust", edge_kind::MENTION));
        assert!(!has("Rust", "Rust", edge_kind::MENTION));

        let node = |t: &str| d.nodes.iter().find(|n| n.title == t).unwrap();
        assert_eq!(node("Rust").backlinks, 1);
        assert_eq!(node("Notes").degree, 3);
        let lang = d.nodes.iter().position(|n| n.title == "lang").unwrap() as u32;
        assert_eq!(node("Notes").primary_tag, Some(lang));
        assert!(node("Notes").journal_date.is_none());
    }

    #[test]
    fn mentions_need_whole_words_and_skip_code_and_links() {
        let (_d, g) = open();
        add(&g, "Go", "language");
        add(&g, "Gopher", "");
        add(&g, "A", "going to `gopher` and [[x gopher y]] gopherish");
        let d = g.graph_data().unwrap();
        assert!(
            !edges(&d)
                .iter()
                .any(|(s, _, k)| s == "A" && *k == edge_kind::MENTION)
        );
    }

    #[test]
    fn mentions_skip_reverse_links_and_multiword_tags() {
        let (_d, g) = open();
        add(&g, "Alpha", "about beta things");
        add(&g, "Beta", "back to [[Alpha]]");
        add(&g, "Gamma Ray", "");
        add(&g, "Delta", "#[[Gamma Ray]] burst");
        add(&g, "Ray", "");
        let d = g.graph_data().unwrap();
        let e = edges(&d);
        let mention = |s: &str, t: &str| e.contains(&(s.into(), t.into(), edge_kind::MENTION));
        assert!(!mention("Alpha", "Beta"), "already linked the other way");
        assert!(!mention("Delta", "Ray"), "tag text is not a mention");
        assert!(!mention("Delta", "Gamma Ray"));
    }

    #[test]
    fn cache_is_invalidated_by_edits_and_visits() {
        let (_d, g) = open();
        add(&g, "A", "[[B]]");
        let d1 = g.graph_data().unwrap();
        assert_eq!(g.graph_data().unwrap(), d1, "cached");
        add(&g, "A", "[[C]]");
        let d2 = g.graph_data().unwrap();
        assert!(d2.seq > d1.seq);
        assert!(d2.nodes.iter().any(|n| n.title == "C"));

        let a = g.page_by_title("A".into()).unwrap().unwrap().page.id;
        let c = g.page_by_title("C".into()).unwrap().unwrap().page.id;
        let syn = |d: &GraphData| {
            (0..d.kinds.len())
                .filter(|&e| d.kinds[e] == edge_kind::SYNAPSE)
                .map(|e| d.weights[e])
                .sum::<f32>()
        };
        let before = syn(&d2);
        g.record_visit(Some(a), c).unwrap();
        let d3 = g.graph_data().unwrap();
        assert!(syn(&d3) > before, "visits strengthen synapses");
    }

    #[test]
    fn clusters_follow_communities() {
        let (_d, g) = open();
        for (p, links) in [
            ("a1", "[[a2]] [[a3]]"),
            ("a2", "[[a3]]"),
            ("b1", "[[b2]] [[b3]] [[b4]]"),
            ("b2", "[[b3]] [[b4]]"),
            ("b3", "[[b4]]"),
        ] {
            add(&g, p, links);
        }
        let d = g.graph_data().unwrap();
        let c = |t: &str| d.nodes.iter().find(|n| n.title == t).unwrap().cluster;
        assert_eq!(c("a1"), c("a2"));
        assert_eq!(c("a2"), c("a3"));
        assert_eq!(c("b1"), c("b4"));
        assert_ne!(c("a1"), c("b1"));
        assert_eq!(c("b1"), 0, "largest community first");
    }
}
