//! Hubs (folder-like pages), namespaces and the legacy page purge. Rules and membership
//! live in `synapse_core::structure`; this module shapes them for the UI.

use super::dto::{
    HubDto, HubPreviewDto, HubsDto, LegacyCandidate, NamespaceEntry, PageBrief, PageDto,
    PageStructure,
};
use super::graph::{SynapseGraph, parse_id};
use anyhow::{Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use synapse_core::{
    Graph, HubConversion, PageId, Structure, is_legacy_page, namespace_parent, title_key,
};

impl SynapseGraph {
    /// Every hub with its members, and the root hubs.
    pub fn hubs(&self) -> Result<HubsDto> {
        let g = self.lock();
        let s = self.structure_of(&g)?;
        let hub_ids: HashSet<PageId> = s.hubs.iter().map(|h| h.page).collect();
        let member_of_hub: HashSet<PageId> = s
            .hubs
            .iter()
            .flat_map(|h| h.members.iter().copied())
            .collect();
        let mut briefs: HashMap<PageId, PageBrief> = HashMap::new();
        let mut hubs = Vec::with_capacity(s.hubs.len());
        for h in &s.hubs {
            let members = h
                .members
                .iter()
                .map(|&m| brief(&g, &mut briefs, m))
                .collect::<Result<Vec<_>>>()?;
            hubs.push(HubDto {
                id: h.page.to_string(),
                title: h.title.clone(),
                description: h.description.clone(),
                depth: h.depth,
                sub_hubs: h
                    .members
                    .iter()
                    .filter(|m| hub_ids.contains(m))
                    .map(|m| m.to_string())
                    .collect(),
                members,
            });
        }
        let roots = s
            .hubs
            .iter()
            .filter(|h| !member_of_hub.contains(&h.page))
            .map(|h| h.page.to_string())
            .collect();
        Ok(HubsDto { hubs, roots })
    }

    /// Hub and namespace context of a page.
    pub fn page_structure(&self, page_id: String) -> Result<PageStructure> {
        let g = self.lock();
        let id = parse_id(&page_id)?;
        let page = g
            .get_page(id)?
            .ok_or_else(|| anyhow!("page {page_id} not found"))?;
        let mut briefs = HashMap::new();
        let members = if page.is_hub() {
            g.hub_members(id)?
                .into_iter()
                .map(|m| brief(&g, &mut briefs, m))
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let in_hubs = g
            .hubs_of(id)?
            .into_iter()
            .map(|h| brief(&g, &mut briefs, h))
            .collect::<Result<Vec<_>>>()?;

        let mut path = Vec::new();
        let mut title = page.title.as_str();
        while let Some(parent) = namespace_parent(title) {
            path.push(parent);
            title = parent;
        }
        let ns_path = path
            .into_iter()
            .rev()
            .map(|t| entry(&g, t.to_string(), None))
            .collect::<Result<Vec<_>>>()?;
        let ns_children = g
            .namespace_children(&page.title)?
            .into_iter()
            .map(|(t, id)| entry(&g, t, id))
            .collect::<Result<Vec<_>>>()?;
        Ok(PageStructure {
            is_hub: page.is_hub(),
            description: page.description.clone(),
            members,
            in_hubs,
            ns_path,
            ns_children,
        })
    }

    /// Creates an empty hub. Fails when the title is taken.
    pub fn create_hub(&self, title: String, description: String) -> Result<PageDto> {
        Ok(PageDto::from(
            &self.lock().create_hub(&title, &description)?,
        ))
    }

    /// Id of the hub's template page (created empty when missing). Open it like any note to
    /// edit the outline new notes of the hub start from.
    pub fn hub_template(&self, hub_id: String) -> Result<String> {
        Ok(self.lock().hub_template(parse_id(&hub_id)?)?.to_string())
    }

    /// Creates a note in the hub, starting from a copy of the hub's template.
    pub fn create_page_from_template(&self, hub_id: String, title: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(
            &g.create_page_from_template(parse_id(&hub_id)?, &title)?,
        ))
    }

    pub fn set_hub_description(&self, hub_id: String, description: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(
            &g.set_hub_description(parse_id(&hub_id)?, &description)?,
        ))
    }

    /// Puts a page in a hub (no-op when it already is). Fails for journals, non-hub targets,
    /// the hub itself and memberships that would create a cycle.
    pub fn add_to_hub(&self, page_id: String, hub_id: String) -> Result<()> {
        self.lock()
            .add_to_hub(parse_id(&page_id)?, parse_id(&hub_id)?)?;
        Ok(())
    }

    /// Takes a page out of a hub (no-op when it is not in it).
    pub fn remove_from_hub(&self, page_id: String, hub_id: String) -> Result<()> {
        self.lock()
            .remove_from_hub(parse_id(&page_id)?, parse_id(&hub_id)?)?;
        Ok(())
    }

    /// Defaults for converting a note into a hub.
    pub fn hub_conversion_preview(&self, page_id: String) -> Result<HubPreviewDto> {
        let g = self.lock();
        let p = g.hub_conversion_preview(parse_id(&page_id)?)?;
        let mut briefs = HashMap::new();
        let members = p
            .members
            .iter()
            .map(|&m| brief(&g, &mut briefs, m))
            .collect::<Result<Vec<_>>>()?;
        Ok(HubPreviewDto {
            suggested_note_title: format!("{} (notes)", p.suggested_title),
            suggested_title: p.suggested_title,
            suggested_description: p.suggested_description,
            members,
            prose_blocks: p.prose_blocks,
            blocks: p.blocks,
        })
    }

    /// Turns a note into a hub in one transaction: renames it to `title` (links rewritten),
    /// moves its outline to a new note `keep_outline_as` (or deletes it when `None`; a blank title is an error), sets the
    /// description and adds `member_ids` (invalid members are skipped).
    pub fn convert_to_hub(
        &self,
        page_id: String,
        title: String,
        description: String,
        member_ids: Vec<String>,
        keep_outline_as: Option<String>,
    ) -> Result<PageDto> {
        let members = member_ids
            .iter()
            .map(|i| parse_id(i))
            .collect::<Result<Vec<_>>>()?;
        if keep_outline_as
            .as_ref()
            .is_some_and(|t| t.trim().is_empty())
        {
            return Err(anyhow!("the note that keeps the outline needs a title"));
        }
        let c = HubConversion {
            title,
            description,
            members,
            keep_outline_as,
        };
        let g = self.lock();
        Ok(PageDto::from(&g.convert_to_hub(parse_id(&page_id)?, &c)?))
    }

    /// Turns a hub back into a note: description, then one `[[Member]]` block per member.
    pub fn convert_to_note(&self, page_id: String) -> Result<PageDto> {
        Ok(PageDto::from(
            &self.lock().convert_to_note(parse_id(&page_id)?)?,
        ))
    }

    /// Pages worth cleaning up: leftovers of Logseq (its built-ins and pages with
    /// `type:: system`) and empty pages (no text in any block, not a hub, no type, tags or
    /// description). Of the Logseq ones only the organisational files (`contents`, `card`,
    /// `favorites`, `hls__*`) and empty pages are preselected, and never a hub: a built-in
    /// title such as `todo` may hold real content. Empty pages are all preselected.
    pub fn legacy_candidates(&self) -> Result<Vec<LegacyCandidate>> {
        let g = self.lock();
        let mut out = Vec::new();
        for p in g.list_pages(false, u32::MAX, 0)? {
            let key = title_key(&p.title);
            let tree = g.page_tree(p.id)?;
            let blocks = tree.len() as u32;
            let blank = tree.iter().all(|(_, b)| b.content.trim().is_empty());
            let empty = blank
                && !p.is_hub()
                && p.page_type().is_none()
                && p.tags.is_empty()
                && p.description.trim().is_empty();
            let reason = if is_legacy_page(&p.title) {
                if key.starts_with("hls__") {
                    "Logseq PDF highlights"
                } else {
                    "Logseq built-in"
                }
            } else if p.properties.get("type").and_then(|v| v.as_str()) == Some("system") {
                "type:: system"
            } else if empty {
                "Empty"
            } else {
                continue;
            };
            let organisational = matches!(key.as_str(), "contents" | "card" | "favorites")
                || key.starts_with("hls__");
            out.push(LegacyCandidate {
                id: p.id.to_string(),
                title: p.title.clone(),
                reason: reason.into(),
                blocks,
                preselected: !p.is_hub() && (empty || blocks == 0 || organisational),
            });
        }
        out.sort_by_key(|c| title_key(&c.title));
        Ok(out)
    }

    /// Deletes pages and their blocks in one transaction. Returns how many existed.
    pub fn purge_pages(&self, ids: Vec<String>) -> Result<u32> {
        let ids = ids
            .iter()
            .map(|i| parse_id(i))
            .collect::<Result<Vec<_>>>()?;
        let n = self.lock().transaction(|g| {
            let mut n = 0;
            for id in ids {
                if g.get_page(id)?.is_some() {
                    g.delete_page(id)?;
                    n += 1;
                }
            }
            Ok(n)
        })?;
        Ok(n)
    }

    /// Structure of the graph, cached until it changes.
    fn structure_of(&self, g: &Graph) -> Result<Arc<Structure>> {
        let seq = g.last_seq()?;
        let mut cache = self
            .structure_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((s, data)) = cache.as_ref()
            && *s == seq
        {
            return Ok(Arc::clone(data));
        }
        let data = Arc::new(g.structure()?);
        *cache = Some((seq, Arc::clone(&data)));
        Ok(data)
    }
}

fn brief(g: &Graph, cache: &mut HashMap<PageId, PageBrief>, id: PageId) -> Result<PageBrief> {
    if let Some(b) = cache.get(&id) {
        return Ok(b.clone());
    }
    let p = g
        .get_page(id)?
        .ok_or_else(|| anyhow!("page {id} not found"))?;
    let b = PageBrief::from(&p);
    cache.insert(id, b.clone());
    Ok(b)
}

/// Namespace level; looks the page up when the caller does not know it.
fn entry(g: &Graph, title: String, id: Option<PageId>) -> Result<NamespaceEntry> {
    let id = match id {
        Some(id) => Some(id),
        None => g
            .page_by_title(&title)?
            .filter(|p| !p.is_journal())
            .map(|p| p.id),
    };
    Ok(NamespaceEntry {
        title,
        id: id.map(|i| i.to_string()),
    })
}
