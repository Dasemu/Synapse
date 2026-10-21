//! Plain data transferred to the UI. Ids are UUID strings and days are `YYYY-MM-DD`, which
//! keeps the generated Dart side free of extra codecs.

use synapse_core::{Block, Page, journal_title};

/// Page header data.
#[derive(Debug, Clone, PartialEq)]
pub struct PageDto {
    pub id: String,
    pub title: String,
    /// Set for journal pages.
    pub journal_date: Option<String>,
    /// Journal day the page was born in.
    pub origin_journal: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&Page> for PageDto {
    fn from(p: &Page) -> Self {
        Self {
            id: p.id.to_string(),
            title: p.title.clone(),
            journal_date: p.journal_date.map(journal_title),
            origin_journal: p.origin_journal.map(journal_title),
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

/// One row of the outline, in pre-order.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDto {
    pub id: String,
    pub parent_id: Option<String>,
    pub depth: u32,
    pub content: String,
    pub collapsed: bool,
    pub has_children: bool,
}

impl BlockDto {
    pub(crate) fn new(depth: usize, b: &Block, has_children: bool) -> Self {
        Self {
            id: b.id.to_string(),
            parent_id: b.parent_id.map(|p| p.to_string()),
            depth: depth as u32,
            content: b.content.clone(),
            collapsed: b.collapsed,
            has_children,
        }
    }
}

/// Everything the page view needs in one call.
#[derive(Debug, Clone, PartialEq)]
pub struct PageView {
    pub page: PageDto,
    pub blocks: Vec<BlockDto>,
    pub aliases: Vec<String>,
    /// Previous/next existing journal (journal pages only).
    pub prev_journal: Option<String>,
    pub next_journal: Option<String>,
}

/// Where a block goes, relative to `anchor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    After,
    Before,
    /// Last child of `anchor`, or last root block when `anchor` is `None`.
    LastChildOf,
    /// First child of `anchor`, or first root block when `anchor` is `None`.
    FirstChildOf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub kind: PlaceKind,
    pub anchor: Option<String>,
}

/// Blocks of one page referencing the queried page.
#[derive(Debug, Clone, PartialEq)]
pub struct BacklinkGroup {
    pub page_id: String,
    pub page_title: String,
    pub is_journal: bool,
    pub blocks: Vec<BlockDto>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageBrief {
    pub id: String,
    pub title: String,
    pub is_journal: bool,
}

impl From<&Page> for PageBrief {
    fn from(p: &Page) -> Self {
        Self {
            id: p.id.to_string(),
            title: p.title.clone(),
            is_journal: p.is_journal(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelatedDto {
    pub page: PageBrief,
    pub score: f32,
}

/// Target of a `((block ref))`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockRefDto {
    pub id: String,
    pub content: String,
    pub page_id: String,
    pub page_title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditDto {
    pub page_id: String,
    pub edits: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineDayDto {
    pub date: String,
    pub journal_id: Option<String>,
    /// Pages born on this day.
    pub created: Vec<String>,
    /// Pages edited on this day.
    pub edited: Vec<EditDto>,
}

/// Timeline slice plus the titles of every page it mentions.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineDto {
    pub days: Vec<TimelineDayDto>,
    pub pages: Vec<PageBrief>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetDto {
    pub hash: String,
    pub size: u64,
    pub mime: Option<String>,
    pub original_name: Option<String>,
}

/// Quick switcher / `[[` autocomplete hit.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleHit {
    pub page_id: String,
    /// The matched title or alias.
    pub title: String,
    pub is_alias: bool,
}

/// `((` autocomplete hit.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockHit {
    pub id: String,
    pub content: String,
    pub page_title: String,
}
