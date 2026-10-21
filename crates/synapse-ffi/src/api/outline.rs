//! Outliner editing. Structural operations return the page's refreshed outline so the UI can
//! re-sync to the authoritative order after its optimistic update.
//!
//! The core exposes single-block primitives, each in its own transaction; compound edits here
//! (indent, outdent, merge) are sequences of them. See the crate docs for the core helper that
//! would make them atomic.

use super::dto::*;
use super::graph::{SynapseGraph, outline, parse_id};
use anyhow::{Result, bail};
use synapse_core::{Block, BlockId, Graph, PageId, Placement};

impl SynapseGraph {
    pub fn insert_block(&self, page_id: String, place: Place, content: String) -> Result<BlockDto> {
        let g = self.lock();
        let page = parse_id(&page_id)?;
        let at = placement(&g, page, &place)?;
        let b = g.insert_block(page, at, &content)?;
        Ok(BlockDto::new(0, &b, false))
    }

    /// Replaces a block's Markdown (the debounced save while typing).
    pub fn update_block(&self, id: String, content: String) -> Result<()> {
        self.lock().update_block(parse_id(&id)?, &content)?;
        Ok(())
    }

    pub fn set_collapsed(&self, id: String, collapsed: bool) -> Result<()> {
        Ok(self.lock().set_collapsed(parse_id(&id)?, collapsed)?)
    }

    /// Deletes a block and its subtree.
    pub fn delete_block(&self, id: String) -> Result<Vec<BlockDto>> {
        let g = self.lock();
        let b = get(&g, parse_id(&id)?)?;
        g.delete_block(b.id)?;
        outline(&g, b.page_id)
    }

    /// Moves a block with its subtree (drag and drop), possibly to another page.
    pub fn move_block(&self, id: String, page_id: String, place: Place) -> Result<Vec<BlockDto>> {
        let g = self.lock();
        let page = parse_id(&page_id)?;
        let at = placement(&g, page, &place)?;
        g.move_block(parse_id(&id)?, page, at)?;
        outline(&g, page)
    }

    /// Tab: the block becomes the last child of its previous sibling (no-op for a first child).
    pub fn indent_block(&self, id: String) -> Result<Vec<BlockDto>> {
        let g = self.lock();
        let b = get(&g, parse_id(&id)?)?;
        let sibs = siblings(&g, &b)?;
        let i = sibs.iter().position(|s| s.id == b.id).unwrap_or(0);
        if i > 0 {
            let prev = &sibs[i - 1];
            if prev.collapsed {
                g.set_collapsed(prev.id, false)?;
            }
            g.move_block(b.id, b.page_id, Placement::LastChildOf(Some(prev.id)))?;
        }
        outline(&g, b.page_id)
    }

    /// Shift-Tab: the block moves right after its parent, adopting its following siblings as
    /// children so it keeps its visual place (Logseq's default outdenting).
    pub fn outdent_block(&self, id: String) -> Result<Vec<BlockDto>> {
        let g = self.lock();
        let b = get(&g, parse_id(&id)?)?;
        let Some(parent) = b.parent_id else {
            return outline(&g, b.page_id);
        };
        let sibs = siblings(&g, &b)?;
        let i = sibs.iter().position(|s| s.id == b.id).unwrap_or(0);
        g.move_block(b.id, b.page_id, Placement::After(parent))?;
        for s in &sibs[i + 1..] {
            g.move_block(s.id, b.page_id, Placement::LastChildOf(Some(b.id)))?;
        }
        if b.collapsed && i + 1 < sibs.len() {
            g.set_collapsed(b.id, false)?;
        }
        outline(&g, b.page_id)
    }

    /// Backspace at the start of `id`: its text is appended to `into` (the previous visible
    /// block) and the block is deleted. Its children keep their visual place: they take the
    /// block's slot when `into` is its parent, otherwise they become last children of `into`.
    pub fn merge_block(&self, id: String, into: String) -> Result<Vec<BlockDto>> {
        let g = self.lock();
        let b = get(&g, parse_id(&id)?)?;
        let target = get(&g, parse_id(&into)?)?;
        if b.id == target.id {
            bail!("cannot merge a block into itself");
        }
        if !b.content.is_empty() {
            g.update_block(target.id, &format!("{}{}", target.content, b.content))?;
        }
        let children: Vec<BlockId> = g
            .page_tree(b.page_id)?
            .into_iter()
            .filter(|(_, c)| c.parent_id == Some(b.id))
            .map(|(_, c)| c.id)
            .collect();
        if !children.is_empty() && target.collapsed && b.parent_id != Some(target.id) {
            g.set_collapsed(target.id, false)?;
        }
        let at = if b.parent_id == Some(target.id) {
            Placement::Before(b.id)
        } else {
            Placement::LastChildOf(Some(target.id))
        };
        for c in children {
            g.move_block(c, target.page_id, at)?;
        }
        g.delete_block(b.id)?;
        outline(&g, target.page_id)
    }

    /// Current outline of a page.
    pub fn outline(&self, page_id: String) -> Result<Vec<BlockDto>> {
        outline(&self.lock(), parse_id(&page_id)?)
    }
}

fn get(g: &Graph, id: BlockId) -> Result<Block> {
    match g.get_block(id)? {
        Some(b) => Ok(b),
        None => bail!("block {id} not found"),
    }
}

/// Siblings of `b` (including itself) in order.
fn siblings(g: &Graph, b: &Block) -> Result<Vec<Block>> {
    Ok(g.page_tree(b.page_id)?
        .into_iter()
        .filter(|(_, s)| s.parent_id == b.parent_id)
        .map(|(_, s)| s)
        .collect())
}

fn placement(g: &Graph, page: PageId, place: &Place) -> Result<Placement> {
    let anchor = place.anchor.as_deref().map(parse_id).transpose()?;
    let need = |a: Option<BlockId>| match a {
        Some(a) => Ok(a),
        None => Err(anyhow::anyhow!("{:?} needs an anchor block", place.kind)),
    };
    Ok(match place.kind {
        PlaceKind::After => Placement::After(need(anchor)?),
        PlaceKind::Before => Placement::Before(need(anchor)?),
        PlaceKind::LastChildOf => Placement::LastChildOf(anchor),
        PlaceKind::FirstChildOf => {
            let first = g
                .page_tree(page)?
                .into_iter()
                .find(|(_, b)| b.parent_id == anchor)
                .map(|(_, b)| b.id);
            match first {
                Some(f) => Placement::Before(f),
                None => Placement::LastChildOf(anchor),
            }
        }
    })
}
