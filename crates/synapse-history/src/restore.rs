//! Restore pages (or the whole graph) from a snapshot by re-importing the Markdown through the
//! `Graph` API. Restoring appends new ops; git history is never rewritten.

use crate::Result;
use crate::export;
use crate::history::{History, SnapshotInfo};
use crate::parse::{ParsedPage, parse_page};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use synapse_core::position::between;
use synapse_core::{Block, BlockId, Graph, NewPage, Page, PageId, PageKind, new_id};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Pages whose content was (re)written from the snapshot.
    pub restored: usize,
    /// Pages removed because they did not exist in the snapshot.
    pub removed: usize,
    /// Snapshot of the pre-restore state, taken so the restore itself is undoable.
    pub safety_snapshot: Option<SnapshotInfo>,
}

/// Test hook: fail after this many block writes/deletes.
type Fault<'a> = &'a Cell<Option<usize>>;

fn tick(fault: Fault) -> Result<()> {
    if let Some(n) = fault.get() {
        if n == 0 {
            return Err(crate::Error::NotFound("injected fault".into()));
        }
        fault.set(Some(n - 1));
    }
    Ok(())
}

/// Runs `f` in one graph transaction: any error rolls every change back.
fn atomic<T>(graph: &Graph, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut failure = None;
    let out = graph.transaction(|_| match f() {
        Ok(v) => Ok(v),
        Err(e) => {
            failure = Some(e);
            Err(synapse_core::Error::Invalid("restore aborted".into()))
        }
    });
    match (out, failure) {
        (Ok(v), _) => Ok(v),
        (Err(_), Some(e)) => Err(e),
        (Err(e), None) => Err(e.into()),
    }
}

struct Plan {
    page: PageId,
    is_hub: bool,
    description: String,
    /// Titles of the hubs the page belongs to.
    hubs: Vec<String>,
    tags: Vec<String>,
    types: Vec<String>,
    blocks: Vec<Block>,
    delete: Vec<BlockId>,
}

fn find_or_create(graph: &Graph, title: &str) -> Result<Page> {
    if let Ok(d) = chrono::NaiveDate::parse_from_str(title, "%Y-%m-%d") {
        return Ok(graph.journal(d)?);
    }
    match graph.page_by_title(title)? {
        Some(p) => Ok(p),
        None => Ok(graph.create_page(NewPage::titled(title))?),
    }
}

/// Matches parsed blocks to existing ones and decides what to write and delete.
fn plan(graph: &Graph, title: &str, parsed: &ParsedPage, steal: bool) -> Result<Plan> {
    let mut page = find_or_create(graph, title)?;
    let now = graph.now_ms();
    // A hub cannot hold blocks: turn it back into a note before any block is written.
    if page.is_hub() && !parsed.is_hub {
        page.kind = PageKind::Note;
        page.updated_at = now;
        graph.put_page(&page)?;
        // Like `convert_to_note`: members leave it, so it never regains them by surprise.
        for m in graph.hub_members(page.id)? {
            graph.remove_from_hub(m, page.id)?;
        }
    }
    // A hub file has no blocks; any left over in the page are deleted.
    let parsed_blocks: &[_] = if parsed.is_hub { &[] } else { &parsed.blocks };

    if page.properties != parsed.properties {
        graph.set_page_properties(page.id, parsed.properties.clone())?;
    }
    let current: HashSet<String> = graph.aliases_of(page.id)?.into_iter().collect();
    let wanted: HashSet<String> = parsed.aliases.iter().cloned().collect();
    for a in current.difference(&wanted) {
        graph.set_alias(a, None)?;
    }
    for a in wanted.difference(&current) {
        graph.set_alias(a, Some(page.id))?;
    }

    let existing: Vec<Block> = graph
        .page_tree(page.id)?
        .into_iter()
        .map(|(_, b)| b)
        .collect();
    let by_id: HashMap<BlockId, &Block> = existing.iter().map(|b| (b.id, b)).collect();
    let mut claimed: HashSet<BlockId> = HashSet::new();
    let mut ids: Vec<Option<BlockId>> = vec![None; parsed_blocks.len()];

    for (i, pb) in parsed_blocks.iter().enumerate() {
        let Some(id) = pb.id else { continue };
        let usable = by_id.contains_key(&id)
            || match graph.get_block(id)? {
                None => true,
                Some(_) => steal,
            };
        if usable && claimed.insert(id) {
            ids[i] = Some(id);
        }
    }
    for (i, pb) in parsed_blocks.iter().enumerate() {
        if ids[i].is_some() {
            continue;
        }
        let hit = existing
            .iter()
            .find(|b| !claimed.contains(&b.id) && b.content == pb.content);
        let id = hit.map(|b| b.id).unwrap_or_else(new_id);
        claimed.insert(id);
        ids[i] = Some(id);
    }

    // Build the tree: parents from depth, positions kept when still ordered.
    let mut blocks: Vec<Block> = Vec::with_capacity(parsed_blocks.len());
    let mut stack: Vec<BlockId> = Vec::new();
    let mut last_pos: HashMap<Option<BlockId>, String> = HashMap::new();
    for (i, pb) in parsed_blocks.iter().enumerate() {
        let id = ids[i].unwrap();
        stack.truncate(pb.depth);
        let parent = stack.last().copied();
        let prev = last_pos.get(&parent).cloned();
        let old = by_id.get(&id);
        let keep = old
            .filter(|o| {
                o.parent_id == parent && prev.as_deref().is_none_or(|p| o.position.as_str() > p)
            })
            .map(|o| o.position.clone());
        let position = keep.unwrap_or_else(|| between(prev.as_deref(), None));
        last_pos.insert(parent, position.clone());
        blocks.push(Block {
            id,
            page_id: page.id,
            parent_id: parent,
            position,
            content: pb.content.clone(),
            properties: pb.properties.clone(),
            collapsed: pb.collapsed,
            created_at: old.map_or(now, |o| o.created_at),
            updated_at: now,
        });
        stack.push(id);
    }
    let delete = existing
        .iter()
        .filter(|b| !claimed.contains(&b.id))
        .map(|b| b.id)
        .collect();
    Ok(Plan {
        page: page.id,
        is_hub: parsed.is_hub,
        description: parsed.description.clone(),
        hubs: parsed.hubs.clone(),
        tags: parsed.tags.clone(),
        types: parsed.types.clone(),
        blocks,
        delete,
    })
}

fn same(a: &Block, b: &Block) -> bool {
    a.page_id == b.page_id
        && a.parent_id == b.parent_id
        && a.position == b.position
        && a.content == b.content
        && a.properties == b.properties
        && a.collapsed == b.collapsed
}

fn apply_deletes(graph: &Graph, plan: &Plan, fault: Fault) -> Result<()> {
    for id in &plan.delete {
        tick(fault)?;
        // Deleting a parent already removed its subtree.
        if graph.get_block(*id)?.is_some() {
            graph.delete_block(*id)?;
        }
    }
    Ok(())
}

/// Writes changed blocks; returns whether anything was written.
fn apply_puts(graph: &Graph, plan: &Plan, fault: Fault) -> Result<bool> {
    let mut wrote = false;
    for b in &plan.blocks {
        if graph.get_block(b.id)?.is_some_and(|cur| same(&cur, b)) {
            continue;
        }
        tick(fault)?;
        graph.put_block(b)?;
        wrote = true;
    }
    Ok(wrote || !plan.delete.is_empty())
}

/// Sets kind and description once the page's blocks are gone; returns whether it changed.
fn apply_kind(graph: &Graph, plan: &Plan) -> Result<bool> {
    let Some(mut page) = graph.get_page(plan.page)? else {
        return Ok(false);
    };
    let kind = if plan.is_hub {
        PageKind::Hub
    } else {
        PageKind::Note
    };
    if page.kind == kind
        && page.description == plan.description
        && page.tags == plan.tags
        && page.types == plan.types
    {
        return Ok(false);
    }
    page.kind = kind;
    page.tags = plan.tags.clone();
    page.types = plan.types.clone();
    page.description = plan.description.clone();
    page.updated_at = graph.now_ms();
    graph.put_page(&page)?;
    Ok(true)
}

/// Puts the page back in the hubs named by the snapshot (once every page has its kind).
/// Hubs that no longer exist or would now be invalid (e.g. a cycle) are skipped.
fn apply_hubs(graph: &Graph, plan: &Plan) -> Result<bool> {
    let Some(page) = graph.get_page(plan.page)? else {
        return Ok(false);
    };
    let mut wanted = Vec::new();
    for t in &plan.hubs {
        if let Some(h) = graph.page_by_title(t)?
            && h.is_hub()
            && !wanted.contains(&h.id)
        {
            wanted.push(h.id);
        }
    }
    wanted.truncate(1);
    if page.hubs == wanted {
        return Ok(false);
    }
    if skip_invalid(graph.set_page_hubs(page.id, wanted.clone()))?.is_none() {
        graph.set_page_hubs(page.id, Vec::new())?;
        for h in wanted {
            skip_invalid(graph.add_to_hub(page.id, h))?;
        }
    }
    Ok(true)
}

/// Rule violations (cycles, pages that are no longer hubs) become `None`; storage errors
/// still abort the restore.
fn skip_invalid<T>(r: synapse_core::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(synapse_core::Error::Invalid(_) | synapse_core::Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn clear_or_delete(graph: &Graph, page: &Page) -> Result<bool> {
    if page.is_journal() {
        let roots: Vec<_> = graph
            .page_tree(page.id)?
            .into_iter()
            .filter(|(d, _)| *d == 0)
            .map(|(_, b)| b.id)
            .collect();
        let had = !roots.is_empty();
        for id in roots {
            graph.delete_block(id)?;
        }
        Ok(had)
    } else {
        graph.delete_page(page.id)?;
        Ok(true)
    }
}

impl History {
    /// Restores one page to its state at `rev`. A page that did not exist then is removed
    /// (a journal is emptied instead).
    pub fn restore_page(
        &mut self,
        graph: &Graph,
        title: &str,
        rev: &str,
        now_ms: i64,
    ) -> Result<RestoreReport> {
        let commit = self.resolve(rev)?;
        let text = self.file_at(commit, &Self::path_for_title(title))?;
        let safety = self.snapshot(graph, now_ms)?;
        let mut report = RestoreReport {
            safety_snapshot: safety,
            ..Default::default()
        };
        let fault = &self.fault;
        let this = &*self;
        atomic(graph, || {
            match text {
                Some(text) => {
                    let parsed = parse_page(&text);
                    let p = plan(graph, title, &parsed, false)?;
                    apply_deletes(graph, &p, fault)?;
                    apply_puts(graph, &p, fault)?;
                    apply_kind(graph, &p)?;
                    apply_hubs(graph, &p)?;
                    if parsed.is_hub {
                        this.rejoin_members(graph, commit, title, p.page)?;
                    }
                    report.restored = 1;
                }
                None => {
                    let page = graph
                        .page_by_title(title)?
                        .ok_or_else(|| Self::not_found(title))?;
                    if clear_or_delete(graph, &page)? {
                        report.removed = 1;
                    }
                }
            }
            Ok(())
        })?;
        Ok(report)
    }

    /// Membership lives on the members: after restoring a hub alone, puts back the existing
    /// pages that were in it at `commit` (other memberships are left alone).
    fn rejoin_members(
        &self,
        graph: &Graph,
        commit: gix::ObjectId,
        title: &str,
        hub: PageId,
    ) -> Result<()> {
        let key = synapse_core::title_key(title);
        for (path, oid) in &self.snapshot_files(&commit.to_string())? {
            let Some(member) = export::title_from_path(path) else {
                continue;
            };
            let parsed = parse_page(&self.read_blob(*oid)?);
            if !parsed
                .hubs
                .iter()
                .any(|h| synapse_core::title_key(h) == key)
            {
                continue;
            }
            if let Some(m) = graph.page_by_title(&member)?
                && m.id != hub
            {
                // Skipped when it would now be invalid (e.g. a cycle).
                skip_invalid(graph.add_to_hub(m.id, hub))?;
            }
        }
        Ok(())
    }

    /// Restores every page to its state at `rev`; pages created since are removed.
    pub fn restore_graph(
        &mut self,
        graph: &Graph,
        rev: &str,
        now_ms: i64,
    ) -> Result<RestoreReport> {
        let files = self.snapshot_files(rev)?;
        let safety = self.snapshot(graph, now_ms)?;
        let mut report = RestoreReport {
            safety_snapshot: safety,
            ..Default::default()
        };
        let fault = &self.fault;
        let this = &*self;
        atomic(graph, || {
            let mut plans = Vec::new();
            let mut wanted: HashSet<String> = HashSet::new();
            for (path, oid) in &files {
                let Some(title) = export::title_from_path(path) else {
                    continue;
                };
                wanted.insert(synapse_core::title_key(&title));
                let text = this.read_blob(*oid)?;
                plans.push(plan(graph, &title, &parse_page(&text), true)?);
            }
            // Delete first so blocks moved between pages can be re-created where they belong.
            for p in &plans {
                apply_deletes(graph, p, fault)?;
            }
            let mut changed = vec![false; plans.len()];
            for (i, p) in plans.iter().enumerate() {
                changed[i] = apply_puts(graph, p, fault)?;
            }
            for (i, p) in plans.iter().enumerate() {
                changed[i] |= apply_kind(graph, p)?;
            }
            for (i, p) in plans.iter().enumerate() {
                changed[i] |= apply_hubs(graph, p)?;
            }
            report.restored += changed.iter().filter(|c| **c).count();
            let mut offset = 0;
            let mut extra = Vec::new();
            loop {
                let batch = graph.list_pages(true, 500, offset)?;
                if batch.is_empty() {
                    break;
                }
                offset += batch.len() as u32;
                extra.extend(
                    batch
                        .into_iter()
                        .filter(|p| !wanted.contains(&synapse_core::title_key(&p.title))),
                );
            }
            for page in extra {
                if clear_or_delete(graph, &page)? {
                    report.removed += 1;
                }
            }
            Ok(())
        })?;
        Ok(report)
    }
}
