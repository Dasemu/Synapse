//! Tags (names a page is filed under) and the single type (a name saying what a page is). Rules live in `synapse_core::{tags, types}`; this module shapes them for the UI.

use super::dto::{PageBrief, PageDto, TagDto};
use super::graph::{SynapseGraph, parse_id};
use anyhow::Result;

impl SynapseGraph {
    /// Replaces the tags of a page (names only, stored sorted). Journals and templates cannot
    /// be tagged.
    pub fn set_page_tags(&self, page_id: String, tags: Vec<String>) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(&g.set_page_tags(parse_id(&page_id)?, tags)?))
    }

    pub fn add_page_tag(&self, page_id: String, tag: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(&g.add_page_tag(parse_id(&page_id)?, &tag)?))
    }

    pub fn remove_page_tag(&self, page_id: String, tag: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(
            &g.remove_page_tag(parse_id(&page_id)?, &tag)?,
        ))
    }

    /// Renames a tag in the `tags` and `types` of every page carrying it; returns the number of
    /// pages changed.
    pub fn rename_tag(&self, old: String, new: String) -> Result<u32> {
        Ok(self.lock().rename_tag(&old, &new)? as u32)
    }

    /// Removes the explicit tag from every page (types of that name stay); returns the number
    /// of pages changed.
    pub fn remove_tag(&self, name: String) -> Result<u32> {
        Ok(self.lock().remove_tag(&name)? as u32)
    }

    /// Every tag in use (listed or implied by a type) with its number of pages, most used first.
    pub fn tag_catalog(&self) -> Result<Vec<TagDto>> {
        Ok(self
            .lock()
            .tag_catalog()?
            .into_iter()
            .map(|t| TagDto {
                name: t.name,
                count: t.count,
                suggested: t.suggested,
            })
            .collect())
    }

    /// Pages carrying a tag (listed or via their type), most recently updated first.
    pub fn pages_with_tag(&self, tag: String) -> Result<Vec<PageBrief>> {
        let g = self.lock();
        let mut out = Vec::new();
        for id in g.pages_with_tag(&tag)? {
            if let Some(p) = g.get_page(id)? {
                out.push(PageBrief::from(&p));
            }
        }
        Ok(out)
    }

    /// Sets (or with `None` clears) the single type of a page. Journals and templates cannot be
    /// typed.
    pub fn set_page_type(&self, page_id: String, page_type: Option<String>) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(&g.set_page_type(
            parse_id(&page_id)?,
            page_type.as_deref(),
        )?))
    }

    /// Types in use (most used first) followed by the seeded suggestions not used yet.
    pub fn type_catalog(&self) -> Result<Vec<TagDto>> {
        Ok(self
            .lock()
            .type_catalog()?
            .into_iter()
            .map(|t| TagDto {
                name: t.name,
                count: t.count,
                suggested: t.suggested,
            })
            .collect())
    }
}
