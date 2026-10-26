//! Tags (topic pages that notes are filed under) and types (plain names saying what a page
//! is). Rules live in `synapse_core::{tags, types}`; this module shapes them for the UI.

use super::dto::{PageBrief, PageDto, TagDto};
use super::graph::{SynapseGraph, parse_id};
use anyhow::Result;

impl SynapseGraph {
    /// Replaces the tags of a page (each title gets a tag page if it has none). Journals,
    /// templates and tag pages cannot be tagged.
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

    /// Every tag page with its number of pages, most used first.
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

    /// Pages carrying a tag, most recently updated first.
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

    /// Replaces the types of a page. Journals and templates cannot be typed.
    pub fn set_page_types(&self, page_id: String, types: Vec<String>) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(
            &g.set_page_types(parse_id(&page_id)?, types)?,
        ))
    }

    pub fn add_page_type(&self, page_id: String, ty: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(&g.add_page_type(parse_id(&page_id)?, &ty)?))
    }

    pub fn remove_page_type(&self, page_id: String, ty: String) -> Result<PageDto> {
        let g = self.lock();
        Ok(PageDto::from(
            &g.remove_page_type(parse_id(&page_id)?, &ty)?,
        ))
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
