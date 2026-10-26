//! Search used by the quick switcher and the editor's autocompletes.
//!
//! Extension point: these are simple in-process scans meant to be replaced by
//! `synapse-search` (tantivy full-text + vectors) once it is wired in. Keep the signatures;
//! swap the bodies.

use super::dto::*;
use super::graph::SynapseGraph;
use anyhow::Result;
use synapse_core::title_key;

impl SynapseGraph {
    /// Page titles and aliases matching `query`: exact, then prefix, then word-prefix, then
    /// substring; shorter titles first within a tier. Empty query returns recent pages.
    pub fn search_titles(&self, query: String, limit: u32) -> Result<Vec<TitleHit>> {
        let g = self.lock();
        let q = title_key(&query);
        if q.is_empty() {
            return Ok(g
                .list_pages(false, limit, 0)?
                .iter()
                .map(|p| TitleHit {
                    page_id: p.id.to_string(),
                    title: p.title.clone(),
                    is_alias: false,
                })
                .collect());
        }
        let mut hits: Vec<(u8, usize, String, synapse_core::PageId)> = g
            .all_titles()?
            .into_iter()
            .filter_map(|(t, id)| {
                let k = title_key(&t);
                let tier = if k == q {
                    0
                } else if k.starts_with(&q) {
                    1
                } else if k
                    .match_indices(&q)
                    .any(|(i, _)| !k[..i].ends_with(|c: char| c.is_alphanumeric()))
                {
                    2
                } else if k.contains(&q) {
                    3
                } else {
                    return None;
                };
                Some((tier, k.chars().count(), t, id))
            })
            .collect();
        hits.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
        hits.truncate(limit as usize);
        let mut out = Vec::with_capacity(hits.len());
        for (_, _, title, id) in hits {
            let is_alias = g
                .get_page(id)?
                .is_some_and(|p| title_key(&p.title) != title_key(&title));
            out.push(TitleHit {
                page_id: id.to_string(),
                title,
                is_alias,
            });
        }
        Ok(out)
    }

    /// Blocks whose text contains `query` (for `((` autocomplete), most recent first.
    pub fn search_blocks(&self, query: String, limit: u32) -> Result<Vec<BlockHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let g = self.lock();
        // Read-only stopgap over the core tables until synapse-search lands.
        let mut st = g.conn().prepare_cached(
            "SELECT b.id, b.content, p.id, p.title FROM blocks b JOIN pages p ON p.id = b.page_id
             WHERE b.content LIKE ?1 ESCAPE '\\' ORDER BY b.updated_at DESC LIMIT ?2",
        )?;
        let rows = st.query_map((pattern, limit), |r| {
            Ok(BlockHit {
                id: r.get(0)?,
                content: r.get(1)?,
                page_id: r.get(2)?,
                page_title: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
