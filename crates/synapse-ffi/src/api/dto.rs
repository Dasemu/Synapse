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
    /// The page is a hub (no blocks; members list instead of an outline).
    pub is_hub: bool,
    /// Hub description (empty for notes).
    pub description: String,
    /// Tag names (no pages) this page lists, sorted by name; its type implies a tag too.
    pub tags: Vec<String>,
    /// The page's single type ("persona", "lugar"...: a name, not a page); it implies a tag of
    /// the same name.
    pub page_type: Option<String>,
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
            is_hub: p.is_hub(),
            description: p.description.clone(),
            tags: p.tags.clone(),
            page_type: p.page_type().map(String::from),
        }
    }
}

/// A tag or type of the vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct TagDto {
    pub name: String,
    /// Pages using it.
    pub count: u32,
    /// One of the seeded suggestions.
    pub suggested: bool,
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
pub struct PageData {
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
    pub is_hub: bool,
}

impl From<&Page> for PageBrief {
    fn from(p: &Page) -> Self {
        Self {
            id: p.id.to_string(),
            title: p.title.clone(),
            is_journal: p.is_journal(),
            is_hub: p.is_hub(),
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

/// A block matching a search (`((` autocomplete, quick switcher content results).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockHit {
    pub id: String,
    pub content: String,
    pub page_id: String,
    pub page_title: String,
}

/// Kind codes of [`GraphData::kinds`].
pub mod edge_kind {
    pub const LINK: u8 = 0;
    pub const TAG: u8 = 1;
    pub const PROPERTY: u8 = 2;
    pub const BLOCK_REF: u8 = 3;
    pub const EMBED: u8 = 4;
    /// Unlinked reference: the target's title appears in the source's text.
    pub const MENTION: u8 = 5;
    /// Hebbian association (undirected), weight = decayed synapse strength.
    pub const SYNAPSE: u8 = 6;
    /// Hub membership, hub → member (structural, weight 1).
    pub const CONTAINS: u8 = 7;
    /// A page carrying a tag (page → virtual tag node, weight 1; the tag is listed in the
    /// page's `tags` or implied by one of its `types`). Unlike `TAG` (an inline `#tag` in the
    /// text, a link to a page) it comes from the page's own names.
    pub const TAGGED: u8 = 8;
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub journal_date: Option<String>,
    pub origin_journal: Option<String>,
    pub created_at: i64,
    /// Distinct neighbours over structural edges (links, tags, properties, block refs, hub
    /// membership).
    pub degree: u32,
    /// Distinct pages referencing this one.
    pub backlinks: u32,
    /// Community (Louvain), numbered by size: 0 is the largest.
    pub cluster: u32,
    /// Node index of the tag (page or virtual tag node) this page uses most, if any.
    pub primary_tag: Option<u32>,
    /// The page is a hub (`PageKind::Hub`).
    pub is_hub: bool,
    /// Node index of the deepest hub containing this page (a hub: itself).
    pub home_hub: Option<u32>,
    /// Tag names of the page, listed or implied by its types, for grouping by tag.
    pub tags: Vec<String>,
    /// A virtual tag node (not a page, no outline): `id` is `tag:<title_key>`, `title` the
    /// display name, `degree` the number of pages carrying the tag.
    pub is_tag: bool,
}

/// The whole page-level graph, edges as parallel arrays (cheap to move across the FFI).
/// Edges are directed source → target, except synapses (stored once, `source < target`).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub sources: Vec<u32>,
    pub targets: Vec<u32>,
    /// See [`edge_kind`].
    pub kinds: Vec<u8>,
    /// Number of references (structural edges) or synapse strength.
    pub weights: Vec<f32>,
    /// Op-log sequence the data was built at.
    pub seq: i64,
}

/// One batch of layout iterations.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutFrame {
    /// `x0, y0, x1, y1, ...`
    pub positions: Vec<f32>,
    /// Remaining "temperature"; the layout is settled when it reaches 0.
    pub alpha: f32,
}

/// A hub (folder-like page) with its members.
#[derive(Debug, Clone, PartialEq)]
pub struct HubDto {
    pub id: String,
    pub title: String,
    pub description: String,
    /// 0 for root hubs, one more per level of nesting.
    pub depth: u32,
    /// Sub-hubs first, then notes, each by title.
    pub members: Vec<PageBrief>,
    /// Ids of the members that are hubs themselves.
    pub sub_hubs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HubsDto {
    /// Sorted by title.
    pub hubs: Vec<HubDto>,
    /// Ids of the hubs that are not a member of any hub, by title.
    pub roots: Vec<String>,
}

/// A level of a namespace (`A/B` under `A`); `id` is `None` when only deeper pages exist.
#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceEntry {
    pub title: String,
    pub id: Option<String>,
}

/// Hub and namespace context of one page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageStructure {
    pub is_hub: bool,
    /// Hub description (empty for notes).
    pub description: String,
    /// Empty unless the page is a hub; sub-hubs first, then notes.
    pub members: Vec<PageBrief>,
    /// Hubs the page belongs to (its own membership list), by title.
    pub in_hubs: Vec<PageBrief>,
    /// Ancestor namespaces from the root: `A`, `A/B` for `A/B/C` (they may not exist).
    pub ns_path: Vec<NamespaceEntry>,
    pub ns_children: Vec<NamespaceEntry>,
}

/// Defaults for the "convert to hub" dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct HubPreviewDto {
    /// Last namespace segment of the title.
    pub suggested_title: String,
    /// First prose block as plain text (<= 280 chars).
    pub suggested_description: String,
    /// Candidate members: pages the outline links or tags (outline order), then pages
    /// referencing the note from properties or tags.
    pub members: Vec<PageBrief>,
    /// Blocks that are prose rather than links (what "keep the outline" would save).
    pub prose_blocks: u32,
    pub blocks: u32,
    /// `"<suggested_title> (notes)"`, for the note receiving the outline.
    pub suggested_note_title: String,
}

/// A page the legacy purge offers to delete.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyCandidate {
    pub id: String,
    pub title: String,
    pub reason: String,
    pub blocks: u32,
    pub preselected: bool,
}
