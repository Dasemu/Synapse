//! Search and auto-link suggestions over a [`synapse_core::Graph`].
//!
//! * Full-text: tantivy index in `<graph_dir>/index/`, kept up to date from the op log
//!   ([`Search::sync`]).
//! * Vectors: int8-quantised embeddings in the graph's SQLite (`search_vectors`), exact scan.
//! * Hybrid: reciprocal rank fusion of both ([`Search::hybrid_search`]).
//! * Auto-links: [`LinkMatcher`] (unlinked references) and [`Search::semantic_suggestions`].

pub mod autolink;
mod error;
mod fulltext;
pub mod testing;
mod vectors;

pub use autolink::{LinkMatcher, Match, Reason, Suggestion, apply_suggestion};
pub use error::{Error, Result};
pub use fulltext::Stemming;

use chrono::NaiveDate;
use fulltext::{FullText, day_of_date, day_of_ms};
use rusqlite::{OptionalExtension, params};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use synapse_core::embed::Embedder;
use synapse_core::{BlockId, Graph, Op, PageId, parse};
use uuid::Uuid;
use vectors::{VecIndex, normalize, quantize};

const SCHEMA_VERSION: &str = "1";
const RRF_K: f32 = 60.0;
const MAX_CHUNK_CHARS: usize = 2000;

const SEARCH_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS search_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS search_vectors (
    id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    page_id TEXT NOT NULL,
    stamp INTEGER NOT NULL,
    dims INTEGER NOT NULL,
    scale REAL NOT NULL,
    vec BLOB NOT NULL,
    PRIMARY KEY (id, model_id)
);
CREATE INDEX IF NOT EXISTS search_vectors_kind ON search_vectors(model_id, kind);
"#;

#[derive(Debug, Clone)]
pub struct Config {
    pub stemming: Stemming,
    /// tantivy indexing heap budget (bytes). Lower it on phones.
    pub heap_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            stemming: Stemming::None,
            heap_bytes: 32 << 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Pages,
    Blocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JournalFilter {
    #[default]
    Any,
    Only,
    Exclude,
}

/// Search filters. The date range applies to the journal date of journal content and to the
/// last edit day of everything else.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub scope: Scope,
    pub journals: JournalFilter,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// Restrict to one page (its blocks, or the page itself).
    pub page: Option<PageId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Page,
    Block,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub kind: HitKind,
    /// Block id for blocks, page id for pages.
    pub id: Uuid,
    pub page_id: PageId,
    pub page_title: String,
    /// Block content (truncated) or the page title.
    pub snippet: String,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VectorKind {
    Page,
    Block,
}

impl VectorKind {
    fn as_str(self) -> &'static str {
        match self {
            VectorKind::Page => "p",
            VectorKind::Block => "b",
        }
    }
}

/// Text waiting to be embedded. Pass it back to [`Search::store_embeddings`] with its vector.
#[derive(Debug, Clone)]
pub struct PendingEmbedding {
    pub id: Uuid,
    pub kind: VectorKind,
    pub page_id: PageId,
    pub stamp: i64,
    /// Chunk to embed: page title context + block text.
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub ops_processed: usize,
    pub blocks_indexed: usize,
    pub docs_deleted: usize,
}

#[derive(Default)]
struct ModelVectors {
    blocks: VecIndex,
    pages: VecIndex,
}

pub struct Search {
    ft: FullText,
    vectors: RefCell<HashMap<String, ModelVectors>>,
}

fn state_get(graph: &Graph, key: &str) -> Result<Option<String>> {
    Ok(graph
        .conn()
        .query_row("SELECT value FROM search_state WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

fn state_set(graph: &Graph, key: &str, value: &str) -> Result<()> {
    graph.conn().execute(
        "INSERT OR REPLACE INTO search_state(key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

fn fnv(text: &str) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h as i64
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

fn clean_for_embedding(content: &str) -> String {
    truncate(
        &content.replace("[[", "").replace("]]", ""),
        MAX_CHUNK_CHARS,
    )
}

impl Search {
    /// Opens the index at `<graph_dir>/index/`, creating it if needed. Call [`Search::sync`]
    /// afterwards to catch up with the op log.
    pub fn open(graph: &Graph) -> Result<Self> {
        Self::open_with(graph, Config::default())
    }

    pub fn open_with(graph: &Graph, config: Config) -> Result<Self> {
        graph.conn().execute_batch(SEARCH_SCHEMA)?;
        let dir = graph.dir().join("index");
        let want = format!("{SCHEMA_VERSION}/{}", config.stemming.as_str());
        let mismatch = state_get(graph, "index_config")?.as_deref() != Some(&want);
        if mismatch && dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        if mismatch {
            state_set(graph, "last_seq", "0")?;
            state_set(graph, "index_config", &want)?;
        }
        let ft = FullText::open(&dir, config.stemming, config.heap_bytes)?;
        // The index directory was lost or never committed: start over from the op log.
        if ft.doc_count() == 0 && last_seq(graph)? != 0 && graph.last_seq()? != 0 {
            state_set(graph, "last_seq", "0")?;
        }
        Ok(Self {
            ft,
            vectors: RefCell::new(HashMap::new()),
        })
    }

    /// Number of documents in the full-text index.
    pub fn doc_count(&self) -> u64 {
        self.ft.doc_count()
    }

    // ------------------------------------------------------------ indexing

    /// Applies op-log entries newer than the persisted sequence to the index.
    pub fn sync(&mut self, graph: &Graph) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let mut last = last_seq(graph)?;
        let start = last;
        loop {
            let ops = graph.ops_since(last, 5000)?;
            let Some(tail) = ops.last() else { break };
            let next = tail.seq;
            let mut blocks: HashSet<BlockId> = HashSet::new();
            let mut pages: HashSet<PageId> = HashSet::new();
            let mut all_pages = false;
            for rec in &ops {
                match &rec.op {
                    Op::UpsertBlock { block } => {
                        blocks.insert(block.id);
                    }
                    Op::DeleteBlock { id } => {
                        blocks.insert(*id);
                    }
                    Op::UpsertPage { page } => {
                        pages.insert(page.id);
                    }
                    Op::DeletePage { id } => {
                        pages.insert(*id);
                    }
                    // The previous owner of an alias is not in the op, so refresh all pages.
                    Op::SetAlias { .. } => all_pages = true,
                    Op::PutAsset { .. } => {}
                }
            }
            stats.ops_processed += ops.len();
            let mut cache: HashMap<PageId, Option<(Option<NaiveDate>, i64)>> = HashMap::new();
            for id in blocks {
                match graph.get_block(id)? {
                    Some(b) => {
                        let info = *cache.entry(b.page_id).or_insert_with(|| {
                            graph
                                .get_page(b.page_id)
                                .ok()
                                .flatten()
                                .map(|p| (p.journal_date, p.updated_at))
                        });
                        let (day, journal) = match info.and_then(|i| i.0) {
                            Some(d) => (day_of_date(d), true),
                            None => (day_of_ms(b.updated_at), false),
                        };
                        self.ft
                            .upsert_block(b.id, b.page_id, &b.content, day, journal)?;
                        stats.blocks_indexed += 1;
                    }
                    None => {
                        self.ft.delete(id);
                        self.drop_vectors(graph, id)?;
                        stats.docs_deleted += 1;
                    }
                }
            }
            if all_pages {
                self.index_all_pages(graph)?;
            } else {
                for id in pages {
                    self.index_page(graph, id)?;
                }
            }
            last = next;
        }
        if last != start {
            self.ft.commit()?;
            state_set(graph, "last_seq", &last.to_string())?;
        }
        Ok(stats)
    }

    /// Drops the index and rebuilds it from the current graph state.
    pub fn rebuild(&mut self, graph: &Graph) -> Result<SyncStats> {
        self.ft.clear()?;
        self.ft.commit()?;
        state_set(graph, "last_seq", "0")?;
        self.sync(graph)
    }

    fn index_page(&mut self, graph: &Graph, id: PageId) -> Result<()> {
        match graph.get_page(id)? {
            Some(p) => {
                let aliases = graph.aliases_of(id)?;
                let (day, journal) = match p.journal_date {
                    Some(d) => (day_of_date(d), true),
                    None => (day_of_ms(p.updated_at), false),
                };
                self.ft.upsert_page(id, &p.title, &aliases, day, journal)?;
            }
            None => {
                self.ft.delete(id);
                self.drop_vectors(graph, id)?;
            }
        }
        Ok(())
    }

    fn index_all_pages(&mut self, graph: &Graph) -> Result<()> {
        let mut aliases: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut st = graph.conn().prepare("SELECT page_id, alias FROM aliases")?;
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (p, a) = r?;
                aliases.entry(p).or_default().push(a);
            }
        }
        let mut offset = 0;
        loop {
            let pages = graph.list_pages(true, 1000, offset)?;
            if pages.is_empty() {
                break;
            }
            offset += pages.len() as u32;
            for p in pages {
                let al = aliases.remove(&p.id.to_string()).unwrap_or_default();
                let (day, journal) = match p.journal_date {
                    Some(d) => (day_of_date(d), true),
                    None => (day_of_ms(p.updated_at), false),
                };
                self.ft.upsert_page(p.id, &p.title, &al, day, journal)?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ full text

    /// Ranked full-text search over blocks and page titles.
    pub fn search(
        &self,
        graph: &Graph,
        query: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let raw = self.ft.search(query, filter, limit)?;
        hydrate(graph, raw.into_iter().map(|h| (h.is_page, h.id, h.score)))
    }

    /// Quick switcher: page titles/aliases, prefix and typo tolerant.
    pub fn quick_switch(&self, graph: &Graph, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let raw = self.ft.quick_switch(query, limit)?;
        hydrate(graph, raw.into_iter().map(|h| (true, h.id, h.score)))
    }

    // -------------------------------------------------------------- vectors

    /// Chunks (page titles, then blocks) whose vector for `model_id` is missing or stale.
    pub fn pending_embeddings(
        &self,
        graph: &Graph,
        model_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingEmbedding>> {
        let mut out = Vec::new();
        // Pages: embed "title; alias; alias". Stale when that text's hash changes.
        let mut aliases: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut st = graph.conn().prepare("SELECT page_id, alias FROM aliases")?;
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (p, a) = r?;
                aliases.entry(p).or_default().push(a);
            }
        }
        {
            let mut st = graph.conn().prepare(
                "SELECT p.id, p.title, v.stamp FROM pages p
                 LEFT JOIN search_vectors v ON v.id = p.id AND v.model_id = ?1
                 WHERE p.journal_date IS NULL",
            )?;
            let rows = st.query_map([model_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?;
            for r in rows {
                let (id, title, stamp) = r?;
                let mut text = title;
                for a in aliases.get(&id).into_iter().flatten() {
                    text.push_str("; ");
                    text.push_str(a);
                }
                let h = fnv(&text);
                if stamp != Some(h) {
                    let id = parse_uuid(&id)?;
                    out.push(PendingEmbedding {
                        id,
                        kind: VectorKind::Page,
                        page_id: id,
                        stamp: h,
                        text,
                    });
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
        }
        let mut st = graph.conn().prepare(
            "SELECT b.id, b.page_id, b.content, b.updated_at, p.title FROM blocks b
             JOIN pages p ON p.id = b.page_id
             LEFT JOIN search_vectors v ON v.id = b.id AND v.model_id = ?1
             WHERE trim(b.content) <> '' AND (v.id IS NULL OR v.stamp <> b.updated_at)
             LIMIT ?2",
        )?;
        let rows = st.query_map(params![model_id, (limit - out.len()) as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for r in rows {
            let (id, page, content, stamp, title) = r?;
            out.push(PendingEmbedding {
                id: parse_uuid(&id)?,
                kind: VectorKind::Block,
                page_id: parse_uuid(&page)?,
                stamp,
                text: format!("{title}\n{}", clean_for_embedding(&content)),
            });
        }
        Ok(out)
    }

    /// Stores vectors for previously pending chunks (L2-normalised, int8-quantised).
    pub fn store_embeddings(
        &self,
        graph: &Graph,
        model_id: &str,
        items: &[(PendingEmbedding, Vec<f32>)],
    ) -> Result<()> {
        graph.conn().execute_batch("SAVEPOINT search_store")?;
        let r = self.store_rows(graph, model_id, items);
        graph.conn().execute_batch(if r.is_ok() {
            "RELEASE search_store"
        } else {
            "ROLLBACK TO search_store; RELEASE search_store"
        })?;
        r
    }

    fn store_rows(
        &self,
        graph: &Graph,
        model_id: &str,
        items: &[(PendingEmbedding, Vec<f32>)],
    ) -> Result<()> {
        self.ensure_loaded(graph, model_id)?;
        let mut vectors = self.vectors.borrow_mut();
        let mv = vectors.get_mut(model_id).expect("loaded");
        for (p, v) in items {
            let (q, scale) = quantize(&normalize(v));
            let bytes: Vec<u8> = q.iter().map(|&x| x as u8).collect();
            graph.conn().execute(
                "INSERT OR REPLACE INTO search_vectors
                 (id, model_id, kind, page_id, stamp, dims, scale, vec)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    p.id.to_string(),
                    model_id,
                    p.kind.as_str(),
                    p.page_id.to_string(),
                    p.stamp,
                    q.len() as i64,
                    scale,
                    bytes
                ],
            )?;
            let ix = match p.kind {
                VectorKind::Page => &mut mv.pages,
                VectorKind::Block => &mut mv.blocks,
            };
            ix.upsert_quantized(p.id, p.page_id, p.stamp, &q, scale);
        }
        Ok(())
    }

    /// Embeds up to `max_items` pending chunks with `embedder`, `batch` texts per call.
    /// Returns how many vectors were stored.
    pub fn embed_pending(
        &self,
        graph: &Graph,
        embedder: &dyn Embedder,
        batch: usize,
        max_items: usize,
    ) -> Result<usize> {
        let model = embedder.model_id();
        let mut done = 0;
        while done < max_items {
            let pending =
                self.pending_embeddings(graph, &model, batch.min(max_items - done).max(1))?;
            if pending.is_empty() {
                break;
            }
            let texts: Vec<String> = pending.iter().map(|p| p.text.clone()).collect();
            let vecs = embedder
                .embed(&texts)
                .map_err(|e| Error::Embed(e.to_string()))?;
            if vecs.len() != pending.len() {
                return Err(Error::Embed(
                    "embedder returned a wrong vector count".into(),
                ));
            }
            let items: Vec<_> = pending.into_iter().zip(vecs).collect();
            done += items.len();
            self.store_embeddings(graph, &model, &items)?;
        }
        Ok(done)
    }

    /// Number of stored vectors `(pages, blocks)` for a model.
    pub fn vector_counts(&self, graph: &Graph, model_id: &str) -> Result<(usize, usize)> {
        self.ensure_loaded(graph, model_id)?;
        let v = self.vectors.borrow();
        let m = &v[model_id];
        Ok((m.pages.len(), m.blocks.len()))
    }

    fn ensure_loaded(&self, graph: &Graph, model_id: &str) -> Result<()> {
        if !self.vectors.borrow().contains_key(model_id) {
            self.load_model(graph, model_id)?;
        }
        Ok(())
    }

    fn load_model(&self, graph: &Graph, model_id: &str) -> Result<()> {
        let mut mv = ModelVectors::default();
        let mut st = graph.conn().prepare(
            "SELECT id, kind, page_id, stamp, scale, vec FROM search_vectors WHERE model_id=?1",
        )?;
        let rows = st.query_map([model_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, f32>(4)?,
                r.get::<_, Vec<u8>>(5)?,
            ))
        })?;
        for r in rows {
            let (id, kind, page, stamp, scale, bytes) = r?;
            let q: Vec<i8> = bytes.into_iter().map(|b| b as i8).collect();
            let ix = if kind == "p" {
                &mut mv.pages
            } else {
                &mut mv.blocks
            };
            ix.upsert_quantized(parse_uuid(&id)?, parse_uuid(&page)?, stamp, &q, scale);
        }
        self.vectors.borrow_mut().insert(model_id.to_string(), mv);
        Ok(())
    }

    fn drop_vectors(&self, graph: &Graph, id: Uuid) -> Result<()> {
        graph
            .conn()
            .execute("DELETE FROM search_vectors WHERE id=?1", [id.to_string()])?;
        for mv in self.vectors.borrow_mut().values_mut() {
            mv.blocks.remove(id);
            mv.pages.remove(id);
        }
        Ok(())
    }

    /// Pure semantic search. `query` is any vector from the model `model_id`.
    pub fn vector_search(
        &self,
        graph: &Graph,
        model_id: &str,
        query: &[f32],
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let ranked = self.vector_candidates(graph, model_id, query, filter, limit)?;
        hydrate(graph, ranked.into_iter())
    }

    /// `(is_page, id, cosine)` ranked, post-filtered.
    fn vector_candidates(
        &self,
        graph: &Graph,
        model_id: &str,
        query: &[f32],
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<(bool, Uuid, f32)>> {
        self.ensure_loaded(graph, model_id)?;
        let q = normalize(query);
        let vectors = self.vectors.borrow();
        let mv = &vectors[model_id];
        // Over-fetch because journal/date filters are applied after the scan.
        let fetch = if filter.journals == JournalFilter::Any
            && filter.from.is_none()
            && filter.to.is_none()
        {
            limit
        } else {
            limit * 5 + 50
        };
        let mut all: Vec<(bool, vectors::Neighbor)> = Vec::new();
        if filter.scope != Scope::Pages {
            all.extend(
                mv.blocks
                    .top_k(&q, fetch, filter.page)
                    .into_iter()
                    .map(|n| (false, n)),
            );
        }
        if filter.scope != Scope::Blocks {
            all.extend(
                mv.pages
                    .top_k(&q, fetch, filter.page)
                    .into_iter()
                    .map(|n| (true, n)),
            );
        }
        all.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
        let mut pages: HashMap<PageId, Option<Option<NaiveDate>>> = HashMap::new();
        let mut out = Vec::new();
        for (is_page, n) in all {
            let jd = *pages.entry(n.page).or_insert_with(|| {
                graph
                    .get_page(n.page)
                    .ok()
                    .flatten()
                    .map(|p| p.journal_date)
            });
            let Some(jd) = jd else { continue };
            match (filter.journals, jd.is_some()) {
                (JournalFilter::Only, false) | (JournalFilter::Exclude, true) => continue,
                _ => {}
            }
            let day = jd.map(day_of_date).unwrap_or_else(|| day_of_ms(n.stamp));
            if filter.from.is_some_and(|d| day < day_of_date(d))
                || filter.to.is_some_and(|d| day > day_of_date(d))
            {
                continue;
            }
            out.push((is_page, n.id, n.score));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    // --------------------------------------------------------------- hybrid

    /// Hybrid search: embeds `query` with `embedder` (may block on the network) and fuses
    /// full-text and vector rankings with reciprocal rank fusion. Without an embedder, or when
    /// no vectors are stored yet, this is plain full-text search.
    pub fn hybrid_search(
        &self,
        graph: &Graph,
        query: &str,
        embedder: Option<&dyn Embedder>,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        match embedder {
            Some(e) => {
                let v = e
                    .embed(&[query.to_string()])
                    .map_err(|e| Error::Embed(e.to_string()))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::Embed("empty embedding response".into()))?;
                self.hybrid_with_vector(graph, query, Some((&e.model_id(), &v)), filter, limit)
            }
            None => self.hybrid_with_vector(graph, query, None, filter, limit),
        }
    }

    /// Like [`Search::hybrid_search`] with a precomputed query vector `(model_id, vector)`.
    pub fn hybrid_with_vector(
        &self,
        graph: &Graph,
        query: &str,
        vector: Option<(&str, &[f32])>,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let pool = (limit * 3).max(50);
        let text = self.ft.search(query, filter, pool)?;
        let sem = match vector {
            Some((model, v)) => self.vector_candidates(graph, model, v, filter, pool)?,
            None => Vec::new(),
        };
        let mut fused: HashMap<(bool, Uuid), f32> = HashMap::new();
        for (rank, h) in text.iter().enumerate() {
            *fused.entry((h.is_page, h.id)).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        for (rank, (is_page, id, _)) in sem.iter().enumerate() {
            *fused.entry((*is_page, *id)).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        let mut ranked: Vec<_> = fused.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.1.cmp(&b.0.1)));
        ranked.truncate(limit);
        hydrate(graph, ranked.into_iter().map(|((p, id), s)| (p, id, s)))
    }

    // ------------------------------------------------------------ auto-link

    /// Blocks that mention `page` by title or alias without linking it, best first.
    pub fn unlinked_references(
        &self,
        graph: &Graph,
        page: PageId,
        limit: usize,
    ) -> Result<Vec<Suggestion>> {
        let Some(p) = graph.get_page(page)? else {
            return Ok(Vec::new());
        };
        let mut names = vec![(p.title.clone(), page, false)];
        names.extend(graph.aliases_of(page)?.into_iter().map(|a| (a, page, true)));
        let matcher = LinkMatcher::from_titles(names.clone());
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (name, _, _) in names {
            for h in self.ft.phrase_blocks(&name, 1000)? {
                if h.page_id == page || !seen.insert(h.id) {
                    continue;
                }
                let Some(b) = graph.get_block(h.id)? else {
                    continue;
                };
                out.extend(matcher.suggest_block(&b, true));
            }
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(limit);
        Ok(out)
    }

    /// Pages whose embedding is close to the block's, not yet linked from it.
    pub fn semantic_suggestions(
        &self,
        graph: &Graph,
        model_id: &str,
        block: BlockId,
        limit: usize,
        min_similarity: f32,
    ) -> Result<Vec<Suggestion>> {
        let Some(b) = graph.get_block(block)? else {
            return Ok(Vec::new());
        };
        self.ensure_loaded(graph, model_id)?;
        let vectors = self.vectors.borrow();
        let mv = &vectors[model_id];
        let Some(v) = mv.blocks.get(block) else {
            return Ok(Vec::new());
        };
        let linked: HashSet<String> = parse::extract_refs(&b.content)
            .pages
            .iter()
            .map(|(t, _)| synapse_core::title_key(t))
            .collect();
        let mut out = Vec::new();
        for n in mv
            .pages
            .top_k(&normalize(&v), limit + linked.len() + 1, None)
        {
            if n.score < min_similarity || n.id == b.page_id {
                continue;
            }
            let Some(p) = graph.get_page(n.id)? else {
                continue;
            };
            if linked.contains(&synapse_core::title_key(&p.title)) {
                continue;
            }
            out.push(Suggestion {
                block,
                span: None,
                matched: String::new(),
                target_page: n.id,
                score: n.score,
                reason: Reason::Semantic {
                    similarity: n.score,
                },
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Lexical suggestions first, then semantic ones for pages not already suggested.
    pub fn suggestions_for_block(
        &self,
        graph: &Graph,
        matcher: &LinkMatcher,
        model_id: Option<&str>,
        block: BlockId,
        limit: usize,
        min_similarity: f32,
    ) -> Result<Vec<Suggestion>> {
        let Some(b) = graph.get_block(block)? else {
            return Ok(Vec::new());
        };
        let mut out = matcher.suggest_block(&b, true);
        if let Some(m) = model_id {
            let have: HashSet<PageId> = out.iter().map(|s| s.target_page).collect();
            out.extend(
                self.semantic_suggestions(graph, m, block, limit, min_similarity)?
                    .into_iter()
                    .filter(|s| !have.contains(&s.target_page)),
            );
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Pages most similar to `page` by title embedding.
    pub fn related_pages(
        &self,
        graph: &Graph,
        model_id: &str,
        page: PageId,
        limit: usize,
    ) -> Result<Vec<(PageId, f32)>> {
        self.ensure_loaded(graph, model_id)?;
        let vectors = self.vectors.borrow();
        let mv = &vectors[model_id];
        let Some(v) = mv.pages.get(page) else {
            return Ok(Vec::new());
        };
        Ok(mv
            .pages
            .top_k(&normalize(&v), limit + 1, None)
            .into_iter()
            .filter(|n| n.id != page)
            .take(limit)
            .map(|n| (n.id, n.score))
            .collect())
    }
}

fn last_seq(graph: &Graph) -> Result<i64> {
    Ok(state_get(graph, "last_seq")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| Error::Invalid(e.to_string()))
}

fn hydrate(graph: &Graph, raw: impl Iterator<Item = (bool, Uuid, f32)>) -> Result<Vec<SearchHit>> {
    let mut titles: HashMap<PageId, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    for (is_page, id, score) in raw {
        if is_page {
            let Some(p) = graph.get_page(id)? else {
                continue;
            };
            out.push(SearchHit {
                kind: HitKind::Page,
                id,
                page_id: id,
                snippet: p.title.clone(),
                page_title: p.title,
                score,
            });
        } else {
            let Some(b) = graph.get_block(id)? else {
                continue;
            };
            let title = titles
                .entry(b.page_id)
                .or_insert_with(|| graph.get_page(b.page_id).ok().flatten().map(|p| p.title))
                .clone();
            let Some(page_title) = title else { continue };
            out.push(SearchHit {
                kind: HitKind::Block,
                id,
                page_id: b.page_id,
                page_title,
                snippet: truncate(&b.content, 300),
                score,
            });
        }
    }
    Ok(out)
}
