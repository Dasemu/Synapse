//! Outliner editing. Structural operations return the page's refreshed outline so the UI can
//! re-sync to the authoritative order after its optimistic update.
//!
//! Compound edits (indent, outdent, merge) are sequences of core primitives run inside one
//! `Graph::transaction`, so a failure part-way leaves the outline untouched.

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
        let b = get(&g, parse_id(&id)?)?;
        let at = placement(&g, page, &place)?;
        g.move_block(b.id, page, at)?;
        outline(&g, page)
    }

    /// Tab: the block becomes the last child of its previous sibling (no-op for a first child).
    pub fn indent_block(&self, id: String) -> Result<Vec<BlockDto>> {
        let g = self.lock();
        let b = get(&g, parse_id(&id)?)?;
        g.transaction(|g| {
            let sibs = siblings(g, &b)?;
            let i = sibs.iter().position(|s| s.id == b.id).unwrap_or(0);
            if i > 0 {
                let prev = &sibs[i - 1];
                if prev.collapsed {
                    g.set_collapsed(prev.id, false)?;
                }
                fail_point("indent")?;
                g.move_block(b.id, b.page_id, Placement::LastChildOf(Some(prev.id)))?;
            }
            Ok(())
        })?;
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
        g.transaction(|g| {
            let sibs = siblings(g, &b)?;
            let i = sibs.iter().position(|s| s.id == b.id).unwrap_or(0);
            g.move_block(b.id, b.page_id, Placement::After(parent))?;
            for s in &sibs[i + 1..] {
                fail_point("outdent")?;
                g.move_block(s.id, b.page_id, Placement::LastChildOf(Some(b.id)))?;
            }
            if b.collapsed && i + 1 < sibs.len() {
                g.set_collapsed(b.id, false)?;
            }
            Ok(())
        })?;
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
        g.transaction(|g| {
            if !b.content.is_empty() {
                g.update_block(target.id, &format!("{}{}", target.content, b.content))?;
            }
            fail_point("merge")?;
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
            g.delete_block(b.id)
        })?;
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
fn siblings(g: &Graph, b: &Block) -> synapse_core::Result<Vec<Block>> {
    Ok(g.page_tree(b.page_id)?
        .into_iter()
        .filter(|(_, s)| s.parent_id == b.parent_id)
        .map(|(_, s)| s)
        .collect())
}

/// Resolves a UI placement, checking that the anchor block exists and lives on `page`.
fn placement(g: &Graph, page: PageId, place: &Place) -> Result<Placement> {
    let anchor = place.anchor.as_deref().map(parse_id).transpose()?;
    if let Some(a) = anchor {
        let b = get(g, a)?;
        if b.page_id != page {
            bail!("anchor block {a} is not on page {page}");
        }
    }
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

/// Test hook: lets tests make a compound edit fail part-way. A no-op in normal builds.
#[cfg(not(test))]
#[inline(always)]
fn fail_point(_: &str) -> synapse_core::Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_AT: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn fail_point(name: &str) -> synapse_core::Result<()> {
    if FAIL_AT.with(|f| *f.borrow() == Some(name)) {
        return Err(synapse_core::Error::Invalid(format!(
            "injected failure at {name}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> (tempfile::TempDir, SynapseGraph) {
        let dir = tempfile::tempdir().unwrap();
        let g = SynapseGraph::open(dir.path().to_string_lossy().into_owned()).unwrap();
        (dir, g)
    }

    fn page(g: &SynapseGraph, rows: &[&str]) -> (String, Vec<String>) {
        let p = g.open_page("P".into()).unwrap().page.id;
        let ids = rows
            .iter()
            .map(|c| {
                let place = Place {
                    kind: PlaceKind::LastChildOf,
                    anchor: None,
                };
                g.insert_block(p.clone(), place, (*c).into()).unwrap().id
            })
            .collect();
        (p, ids)
    }

    fn shape(g: &SynapseGraph, p: &str) -> Vec<(u32, String)> {
        g.outline(p.into())
            .unwrap()
            .into_iter()
            .map(|b| (b.depth, b.content))
            .collect()
    }

    fn failing<T>(at: &'static str, f: impl FnOnce() -> T) -> T {
        FAIL_AT.with(|x| *x.borrow_mut() = Some(at));
        let out = f();
        FAIL_AT.with(|x| *x.borrow_mut() = None);
        out
    }

    #[test]
    fn failed_merge_leaves_text_and_children_untouched() {
        let (_d, g) = open();
        let (p, ids) = page(&g, &["a", "b", "kid"]);
        g.indent_block(ids[2].clone()).unwrap();
        let before = shape(&g, &p);
        assert!(failing("merge", || g.merge_block(ids[1].clone(), ids[0].clone())).is_err());
        assert_eq!(shape(&g, &p), before, "no duplicated text, no lost block");
        g.merge_block(ids[1].clone(), ids[0].clone()).unwrap();
        assert_eq!(shape(&g, &p), vec![(0, "ab".into()), (1, "kid".into())]);
    }

    #[test]
    fn failed_outdent_does_not_strand_siblings() {
        let (_d, g) = open();
        let (p, ids) = page(&g, &["p", "a", "b", "c"]);
        for id in &ids[1..] {
            g.indent_block(id.clone()).unwrap();
        }
        let before = shape(&g, &p);
        assert!(failing("outdent", || g.outdent_block(ids[1].clone())).is_err());
        assert_eq!(shape(&g, &p), before);
    }

    #[test]
    fn failed_indent_keeps_collapsed_state() {
        let (_d, g) = open();
        let (p, ids) = page(&g, &["a", "a1", "b"]);
        g.indent_block(ids[1].clone()).unwrap();
        g.set_collapsed(ids[0].clone(), true).unwrap();
        assert!(failing("indent", || g.indent_block(ids[2].clone())).is_err());
        let rows = g.outline(p).unwrap();
        assert!(rows[0].collapsed, "collapse change rolled back");
        assert_eq!(rows[2].depth, 0);
    }

    #[test]
    fn placement_rejects_missing_or_foreign_anchors() {
        let (_d, g) = open();
        let (p, ids) = page(&g, &["a"]);
        let other = g.open_page("Q".into()).unwrap().page.id;
        let after = |anchor: &str| Place {
            kind: PlaceKind::After,
            anchor: Some(anchor.into()),
        };
        assert!(
            g.insert_block(other.clone(), after(&ids[0]), "x".into())
                .is_err()
        );
        let missing = uuid::Uuid::now_v7().to_string();
        assert!(
            g.insert_block(p.clone(), after(&missing), "x".into())
                .is_err()
        );
        let child = Place {
            kind: PlaceKind::LastChildOf,
            anchor: Some(missing),
        };
        assert!(g.move_block(ids[0].clone(), p.clone(), child).is_err());
        assert_eq!(shape(&g, &p), vec![(0, "a".into())]);
    }
}
