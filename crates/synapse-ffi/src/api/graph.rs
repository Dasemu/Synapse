//! The graph handle and read-side queries (pages, journals, links, timeline, assets).

use super::dto::*;
use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;
use flutter_rust_bridge::frb;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use synapse_core::{Graph, NewPage, Page, PageId, journal_title};
use uuid::Uuid;

/// An open graph. Opaque to Dart; every call locks the graph for its duration, so calls are
/// short and coarse-grained. The app serialises writes on its side to keep their order.
///
/// The graph is held as `Arc<Mutex<Graph>>` (the core `Graph` is `Send` but not `Sync`), the
/// same shape `synapse-sync`'s `SyncClient` takes, so a sync client can share it.
pub struct SynapseGraph {
    graph: Arc<Mutex<Graph>>,
    /// Page-level graph for the graph views, keyed by what invalidates it.
    pub(crate) graph_cache: Mutex<Option<(super::graph_view::CacheKey, Arc<GraphData>)>>,
    /// Hubs and home hubs, keyed by op-log sequence.
    pub(crate) structure_cache: Mutex<Option<(i64, Arc<synapse_core::Structure>)>>,
    /// Installed modules (`<dir>/modules/`), started on first use.
    pub(crate) modules: synapse_modules::Registry,
}

impl SynapseGraph {
    /// Opens (or creates) the graph stored in `dir`.
    pub fn open(dir: String) -> Result<SynapseGraph> {
        let graph = Graph::open(&dir).with_context(|| format!("opening graph at {dir}"))?;
        let graph = Arc::new(Mutex::new(graph));
        let modules = synapse_modules::Registry::open(std::path::Path::new(&dir), graph.clone());
        Ok(SynapseGraph {
            graph,
            graph_cache: Mutex::new(None),
            structure_cache: Mutex::new(None),
            modules,
        })
    }

    /// The shared graph, for Rust-side services such as the sync client.
    #[frb(ignore)]
    pub fn shared(&self) -> Arc<Mutex<Graph>> {
        Arc::clone(&self.graph)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Graph> {
        self.graph.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Graph directory.
    #[frb(sync)]
    pub fn dir(&self) -> String {
        self.lock().dir().to_string_lossy().into_owned()
    }

    /// Today's date (graph clock) as `YYYY-MM-DD`.
    #[frb(sync)]
    pub fn today(&self) -> String {
        journal_title(self.lock().today())
    }

    // ------------------------------------------------------------- pages

    pub fn page(&self, id: String) -> Result<Option<PageData>> {
        let g = self.lock();
        match g.get_page(parse_id(&id)?)? {
            Some(p) => Ok(Some(page_view(&g, &p)?)),
            None => Ok(None),
        }
    }

    /// Page by title or alias, without creating it.
    pub fn page_by_title(&self, title: String) -> Result<Option<PageData>> {
        let g = self.lock();
        match g.page_by_title(&title)? {
            Some(p) => Ok(Some(page_view(&g, &p)?)),
            None => Ok(None),
        }
    }

    /// Page by title or alias, created if missing (following a `[[link]]`). Date titles open
    /// the journal of that day.
    pub fn open_page(&self, title: String) -> Result<PageData> {
        let g = self.lock();
        let page = match g.page_by_title(&title)? {
            Some(p) => p,
            None => match parse_day(&title) {
                Some(d) => g.journal(d)?,
                None => g.create_page(NewPage::titled(title))?,
            },
        };
        page_view(&g, &page)
    }

    /// Pages ordered by last update, most recent first.
    pub fn list_pages(
        &self,
        include_journals: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<PageDto>> {
        let g = self.lock();
        Ok(g.list_pages(include_journals, limit, offset)?
            .iter()
            .map(PageDto::from)
            .collect())
    }

    pub fn rename_page(&self, id: String, new_title: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(&g.rename_page(parse_id(&id)?, &new_title)?))
    }

    pub fn delete_page(&self, id: String) -> Result<()> {
        Ok(self.lock().delete_page(parse_id(&id)?)?)
    }

    pub fn aliases(&self, page_id: String) -> Result<Vec<String>> {
        Ok(self.lock().aliases_of(parse_id(&page_id)?)?)
    }

    pub fn add_alias(&self, page_id: String, alias: String) -> Result<()> {
        let alias = alias.trim();
        if alias.is_empty() {
            bail!("empty alias");
        }
        Ok(self.lock().set_alias(alias, Some(parse_id(&page_id)?))?)
    }

    pub fn remove_alias(&self, alias: String) -> Result<()> {
        Ok(self.lock().set_alias(&alias, None)?)
    }

    // ---------------------------------------------------------- journals

    /// The journal of `date` (created if missing).
    pub fn journal(&self, date: String) -> Result<PageData> {
        let g = self.lock();
        let p = g.journal(parse_date(&date)?)?;
        page_view(&g, &p)
    }

    /// Journals feed, newest first. With `before = None` it starts at today (creating today's
    /// journal); otherwise at the newest journal strictly before `before`.
    pub fn journals(&self, before: Option<String>, limit: u32) -> Result<Vec<PageData>> {
        let g = self.lock();
        let mut out = Vec::new();
        let cursor = match before {
            None => {
                let today = g.today_journal()?;
                let date = today.journal_date;
                out.push(page_view(&g, &today)?);
                date
            }
            Some(b) => Some(parse_date(&b)?),
        };
        if (out.len() as u32) < limit {
            for page in g.journals_before(cursor, limit - out.len() as u32)? {
                out.push(page_view(&g, &page)?);
            }
        }
        Ok(out)
    }

    pub fn journal_neighbors(&self, date: String) -> Result<(Option<String>, Option<String>)> {
        let n = self.lock().journal_neighbors(parse_date(&date)?)?;
        Ok((n.prev.map(journal_title), n.next.map(journal_title)))
    }

    /// Timeline data for `[from, to]` (oldest first) with titles of every page it mentions.
    pub fn timeline(&self, from: String, to: String) -> Result<TimelineDto> {
        let g = self.lock();
        let days = g.timeline(parse_date(&from)?, parse_date(&to)?)?;
        let mut ids: BTreeSet<PageId> = BTreeSet::new();
        let days = days
            .into_iter()
            .map(|d| {
                ids.extend(d.created.iter().copied());
                ids.extend(d.edited.iter().map(|(p, _)| *p));
                TimelineDayDto {
                    date: journal_title(d.date),
                    journal_id: d.journal.map(|j| j.to_string()),
                    created: d.created.iter().map(Uuid::to_string).collect(),
                    edited: d
                        .edited
                        .iter()
                        .map(|(p, n)| EditDto {
                            page_id: p.to_string(),
                            edits: *n,
                        })
                        .collect(),
                }
            })
            .collect();
        let mut pages = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(p) = g.get_page(id)? {
                pages.push(PageBrief::from(&p));
            }
        }
        Ok(TimelineDto { days, pages })
    }

    // ------------------------------------------------------------- links

    /// Linked references grouped by page (most recently edited first).
    pub fn backlinks(&self, page_id: String) -> Result<Vec<BacklinkGroup>> {
        let g = self.lock();
        let mut groups: Vec<BacklinkGroup> = Vec::new();
        let mut index: HashMap<PageId, usize> = HashMap::new();
        let mut seen: HashSet<Uuid> = HashSet::new();
        for bl in g.backlinks(parse_id(&page_id)?)? {
            if !seen.insert(bl.block.id) {
                continue;
            }
            let i = match index.get(&bl.block.page_id) {
                Some(&i) => i,
                None => {
                    let page_type = g
                        .get_page(bl.block.page_id)?
                        .and_then(|p| p.page_type().map(String::from));
                    groups.push(BacklinkGroup {
                        page_id: bl.block.page_id.to_string(),
                        page_title: bl.page_title.clone(),
                        is_journal: parse_day(&bl.page_title).is_some(),
                        page_type,
                        blocks: Vec::new(),
                    });
                    index.insert(bl.block.page_id, groups.len() - 1);
                    groups.len() - 1
                }
            };
            groups[i].blocks.push(BlockDto::new(0, &bl.block, false));
        }
        Ok(groups)
    }

    /// Notes related to `page_id` by spreading activation.
    pub fn related(&self, page_id: String, limit: u32) -> Result<Vec<RelatedDto>> {
        let g = self.lock();
        let mut out = Vec::new();
        for (id, score) in g.related(parse_id(&page_id)?, limit as usize)? {
            if let Some(p) = g.get_page(id)? {
                out.push(RelatedDto {
                    page: PageBrief::from(&p),
                    score,
                });
            }
        }
        Ok(out)
    }

    /// Records navigation between pages (strengthens their synapse).
    pub fn record_visit(&self, from: Option<String>, to: String) -> Result<()> {
        let from = from.as_deref().map(parse_id).transpose()?;
        Ok(self.lock().record_visit(from, parse_id(&to)?)?)
    }

    /// Target of a `((block ref))`.
    pub fn block_ref(&self, id: String) -> Result<Option<BlockRefDto>> {
        let g = self.lock();
        let Some(b) = g.get_block(parse_id(&id)?)? else {
            return Ok(None);
        };
        let page_title = g.get_page(b.page_id)?.map(|p| p.title).unwrap_or_default();
        Ok(Some(BlockRefDto {
            id: b.id.to_string(),
            content: b.content,
            page_id: b.page_id.to_string(),
            page_title,
        }))
    }

    // ------------------------------------------------------------ assets

    /// Copies a file into the blob store (streamed).
    pub fn put_asset_file(&self, path: String, mime: Option<String>) -> Result<AssetDto> {
        let a = self
            .lock()
            .put_asset_file(std::path::Path::new(&path), mime.as_deref())?;
        Ok(asset_dto(a))
    }

    /// Stores in-memory bytes (clipboard, Android content URIs).
    pub fn put_asset_bytes(
        &self,
        bytes: Vec<u8>,
        name: Option<String>,
        mime: Option<String>,
    ) -> Result<AssetDto> {
        let a = self
            .lock()
            .put_asset_reader(&bytes[..], name.as_deref(), mime.as_deref())?;
        Ok(asset_dto(a))
    }

    /// Local file path of an asset (for `asset://<hash>` images), if present.
    #[frb(sync)]
    pub fn asset_path(&self, hash: String) -> Option<String> {
        if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let g = self.lock();
        g.blobs()
            .contains(&hash)
            .then(|| g.blobs().path_of(&hash).to_string_lossy().into_owned())
    }
}

fn asset_dto(a: synapse_core::Asset) -> AssetDto {
    AssetDto {
        hash: a.hash,
        size: a.size,
        mime: a.mime,
        original_name: a.original_name,
    }
}

pub(crate) fn page_view(g: &Graph, p: &Page) -> Result<PageData> {
    let (prev, next) = match p.journal_date {
        Some(d) => {
            let n = g.journal_neighbors(d)?;
            (n.prev.map(journal_title), n.next.map(journal_title))
        }
        None => (None, None),
    };
    Ok(PageData {
        page: PageDto::from(p),
        blocks: outline(g, p.id)?,
        aliases: g.aliases_of(p.id)?,
        prev_journal: prev,
        next_journal: next,
    })
}

/// The page outline as UI rows.
pub(crate) fn outline(g: &Graph, page: PageId) -> Result<Vec<BlockDto>> {
    let tree = g.page_tree(page)?;
    Ok(tree
        .iter()
        .enumerate()
        .map(|(i, (d, b))| {
            let has_children = tree.get(i + 1).is_some_and(|(nd, _)| nd > d);
            BlockDto::new(*d, b, has_children)
        })
        .collect())
}

pub(crate) fn parse_id(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| anyhow!("bad id {s:?}: {e}"))
}

pub(crate) fn parse_date(s: &str) -> Result<NaiveDate> {
    parse_day(s).ok_or_else(|| anyhow!("bad date {s:?}, expected YYYY-MM-DD"))
}

fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}
