//! Tool parameters. Doc comments become the JSON-schema descriptions the model reads.

use rmcp::schemars;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// Words to find (all must match, case- and accent-insensitive). Use "double quotes" for
    /// an exact phrase.
    pub query: String,
    /// What to search: "all" (default), "pages" (titles and aliases) or "blocks".
    #[serde(default)]
    pub scope: Option<String>,
    /// Journals: "any" (default), "only" or "exclude".
    #[serde(default)]
    pub journals: Option<String>,
    /// Only pages carrying this tag (explicit, implied by their type or by their hub).
    #[serde(default)]
    pub tag: Option<String>,
    /// Only pages of this type.
    #[serde(default, rename = "type")]
    pub page_type: Option<String>,
    /// Only direct members of this hub (title).
    #[serde(default)]
    pub hub: Option<String>,
    /// Only one page (title, alias, id or journal date).
    #[serde(default)]
    pub page: Option<String>,
    /// From date (YYYY-MM-DD, inclusive): journal date for journals, last edit for the rest.
    #[serde(default)]
    pub from: Option<String>,
    /// To date (YYYY-MM-DD, inclusive).
    #[serde(default)]
    pub to: Option<String>,
    /// Max results (default 20, max 100).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Cursor from a previous call ("next cursor").
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GetPageParams {
    /// Page title, alias, id, or a journal date (YYYY-MM-DD / "today").
    pub page: String,
    /// Show only the subtree under this block id.
    #[serde(default)]
    pub block: Option<String>,
    /// Max outline depth to show (0 = top-level blocks only).
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// Append each block's own id as a trailing `((id))` (default true; needed to edit blocks).
    #[serde(default)]
    pub ids: Option<bool>,
    /// Max blocks per call (default 400).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Cursor from a previous call ("next cursor").
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GetJournalParams {
    /// A day: YYYY-MM-DD, "today" or "yesterday". Default today. Ignored when from/to is set.
    #[serde(default)]
    pub date: Option<String>,
    /// Range start (YYYY-MM-DD). With `to`: every existing journal in the range (max 31 days).
    #[serde(default)]
    pub from: Option<String>,
    /// Range end (YYYY-MM-DD, default today).
    #[serde(default)]
    pub to: Option<String>,
    /// Append block ids as `((id))` (default true).
    #[serde(default)]
    pub ids: Option<bool>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListPagesParams {
    /// Only pages of this type.
    #[serde(default, rename = "type")]
    pub page_type: Option<String>,
    /// Only pages with this tag (explicit, from their type or their hub).
    #[serde(default)]
    pub tag: Option<String>,
    /// Only direct members of this hub (title).
    #[serde(default)]
    pub hub: Option<String>,
    /// Only notes without a type (journals and hubs excluded).
    #[serde(default)]
    pub untyped: Option<bool>,
    /// Only notes without explicit tags (journals and hubs excluded).
    #[serde(default)]
    pub untagged: Option<bool>,
    /// Only pages edited on or after this date (YYYY-MM-DD).
    #[serde(default)]
    pub updated_since: Option<String>,
    /// Only pages created on or after this date (YYYY-MM-DD).
    #[serde(default)]
    pub created_since: Option<String>,
    /// Journals: "exclude" (default), "include" or "only".
    #[serde(default)]
    pub journals: Option<String>,
    /// Sort: "updated" (default, newest first), "created" (newest first) or "title".
    #[serde(default)]
    pub sort: Option<String>,
    /// Max results (default 50, max 200).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Cursor from a previous call.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct PageRefParams {
    /// Page title, alias, id or journal date.
    pub page: String,
    /// Max items (default 50 for backlinks, 15 for related).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Cursor from a previous call (backlinks only).
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListParams {
    /// Max items (default 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct HubsParams {
    /// Show this hub's members instead of the whole hub tree.
    #[serde(default)]
    pub hub: Option<String>,
    /// In the tree, also list each hub's member notes (default false).
    #[serde(default)]
    pub members: Option<bool>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct RangeParams {
    /// Start date (YYYY-MM-DD, default 7 days before `to`).
    #[serde(default)]
    pub from: Option<String>,
    /// End date (YYYY-MM-DD or "today", default today).
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CreatePageParams {
    /// Title of the new page (not a date; must not exist yet).
    pub title: String,
    /// Page type (one per page), e.g. "Book". Reuse names from list_types when possible.
    #[serde(default, rename = "type")]
    pub page_type: Option<String>,
    /// Tags. Reuse names from list_tags when possible.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Hub (title of an existing hub) to file the page under.
    #[serde(default)]
    pub hub: Option<String>,
    /// Body as a Markdown outline ("- " bullets, 2-space indents for children; `key:: value`
    /// lines are properties). Plain paragraphs become top-level blocks.
    #[serde(default)]
    pub content: Option<String>,
    /// Journal day (YYYY-MM-DD) the page comes from; it is recorded as the day the page was
    /// born. The journal must exist. Default: today.
    #[serde(default)]
    pub source_journal: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct AppendBlocksParams {
    /// Target page (title, alias or id). Journals are read-only.
    pub page: String,
    /// Markdown outline to append ("- " bullets, 2-space indents for children).
    pub content: String,
    /// Append as children of this block id (default: at the end of the page).
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct UpdateBlockParams {
    /// Block id.
    pub block: String,
    /// New Markdown content of the block (replaces the old text; children are kept).
    pub content: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct DeleteBlockParams {
    /// Block id (its children are deleted too).
    pub block: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SetPageTypeParams {
    /// Page title, alias or id.
    pub page: String,
    /// New type, or null / "" to clear it.
    #[serde(default, rename = "type")]
    pub page_type: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SetPageTagsParams {
    /// Page title, alias or id.
    pub page: String,
    /// Replace all explicit tags with this list (omit to keep them and use add/remove).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Tags to add.
    #[serde(default)]
    pub add: Option<Vec<String>>,
    /// Tags to remove.
    #[serde(default)]
    pub remove: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SetPageHubParams {
    /// Page title, alias or id.
    pub page: String,
    /// Hub title to move the page into (a page has at most one hub), or null to remove it
    /// from its hub.
    #[serde(default)]
    pub hub: Option<String>,
}
