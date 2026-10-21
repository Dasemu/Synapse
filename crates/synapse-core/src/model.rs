use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type PageId = Uuid;
pub type BlockId = Uuid;
/// Free-form properties (`key:: value` in Logseq).
pub type Properties = serde_json::Map<String, serde_json::Value>;

pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub id: PageId,
    pub title: String,
    /// Set for journal pages: the day they represent.
    pub journal_date: Option<NaiveDate>,
    /// Journal day in which this page was born (episodic anchor).
    pub origin_journal: Option<NaiveDate>,
    pub properties: Properties,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Page {
    pub fn is_journal(&self) -> bool {
        self.journal_date.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub page_id: PageId,
    pub parent_id: Option<BlockId>,
    /// Fractional index among siblings (see [`crate::position`]).
    pub position: String,
    /// Logseq-flavoured Markdown (rich text source of truth).
    pub content: String,
    pub properties: Properties,
    pub collapsed: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Kind of a link from a block to a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// `[[Page]]`
    Link,
    /// `#tag` / `#[[tag]]`
    Tag,
    /// `key:: [[Page]]` inside properties
    Property,
}

impl RefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Link => "link",
            RefKind::Tag => "tag",
            RefKind::Property => "property",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "link" => RefKind::Link,
            "tag" => RefKind::Tag,
            "property" => RefKind::Property,
            _ => return None,
        })
    }
}

/// A backlink: `block` (on `page`) references the queried page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backlink {
    pub block: Block,
    pub page_title: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// blake3 hex of the content.
    pub hash: String,
    pub size: u64,
    pub mime: Option<String>,
    pub original_name: Option<String>,
    pub created_at: i64,
}

/// One day on the timeline view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineDay {
    pub date: NaiveDate,
    pub journal: Option<PageId>,
    /// Pages born on this day.
    pub created: Vec<PageId>,
    /// Pages edited on this day (excluding the journal itself), with edit counts.
    pub edited: Vec<(PageId, u32)>,
}

/// Journal neighbours (episodic chain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalNeighbors {
    pub prev: Option<NaiveDate>,
    pub next: Option<NaiveDate>,
}

/// Entry of the op log, the change feed for indexing, backup and sync.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpRecord {
    pub seq: i64,
    pub hlc: String,
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    UpsertPage { page: Page },
    DeletePage { id: PageId },
    UpsertBlock { block: Block },
    DeleteBlock { id: BlockId },
    PutAsset { asset: Asset },
    SetAlias { alias: String, page: Option<PageId> },
}

/// Fields for creating a page; defaults come from the graph clock.
#[derive(Debug, Clone, Default)]
pub struct NewPage {
    pub title: String,
    pub origin_journal: Option<NaiveDate>,
    pub properties: Properties,
    /// Override timestamps (importer). Millis since epoch.
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

impl NewPage {
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }
}

/// Where to insert a new block.
#[derive(Debug, Clone, Copy)]
pub enum Placement {
    /// Last child of `parent` (or last root block when `None`).
    LastChildOf(Option<BlockId>),
    /// Right after a sibling.
    After(BlockId),
    /// Right before a sibling.
    Before(BlockId),
}

/// Journal page title for a date (ISO; the UI formats it for display).
pub fn journal_title(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Normalised key used for title/alias lookups.
pub fn title_key(title: &str) -> String {
    title.trim().to_lowercase()
}
