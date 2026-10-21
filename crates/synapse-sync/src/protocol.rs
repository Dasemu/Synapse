//! Wire types shared by `synapse-sync` (client) and `synapse-server`. See `docs/sync.md`.

use serde::{Deserialize, Serialize};
use synapse_core::{Backlink, Block, Op, OpRecord, Page};

/// Bumped on incompatible changes. Sent as `X-Synapse-Protocol` by clients.
pub const PROTOCOL_VERSION: u32 = 1;
pub const HEADER_PROTOCOL: &str = "x-synapse-protocol";
/// Bytes already stored of a partially uploaded blob (response header).
pub const HEADER_UPLOAD_OFFSET: &str = "upload-offset";
/// Total size of the blob being uploaded (request header).
pub const HEADER_UPLOAD_LENGTH: &str = "upload-length";

/// `GET/PUT /v1/graphs/{id}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub protocol: u32,
    /// Stable identity of the server-side graph (its actor id). A change means the graph was
    /// recreated and clients must restart their cursors.
    pub server_id: String,
    pub last_seq: i64,
}

/// An op to push: the local HLC plus the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireOp {
    pub hlc: String,
    pub op: Op,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    pub ops: Vec<WireOp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushResponse {
    /// Ops that changed server state (the rest lost LWW or were already known).
    pub applied: u32,
    pub last_seq: i64,
}

/// `GET /v1/graphs/{id}/ops?since=&limit=&exclude_actor=`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResponse {
    pub server_id: String,
    /// Ops after `since`, minus those from `exclude_actor`.
    pub ops: Vec<OpRecord>,
    /// Cursor to use for the next pull: the seq of the last op *scanned* (even if filtered).
    pub next_cursor: i64,
    pub last_seq: i64,
    pub has_more: bool,
}

/// Payload of each `data:` line of the SSE stream `GET /v1/graphs/{id}/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncEvent {
    pub last_seq: i64,
}

// ---- read API (thin clients) ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub depth: usize,
    pub block: Block,
}

/// A page with its aliases and block tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageView {
    pub page: Page,
    pub aliases: Vec<String>,
    pub tree: Vec<TreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacklinksView {
    pub backlinks: Vec<Backlink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

/// Actor part of an HLC (`{millis:013}-{counter:05}-{actor}`).
pub fn hlc_actor(hlc: &str) -> Option<&str> {
    hlc.splitn(3, '-').nth(2).filter(|a| !a.is_empty())
}

/// Graph ids are directory names on the server: restrict them to a safe alphabet.
pub fn is_valid_graph_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// blake3 hex digest (64 lowercase hex chars).
pub fn is_valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_and_validation() {
        assert_eq!(hlc_actor("0000000000001-00000-abc123"), Some("abc123"));
        assert_eq!(hlc_actor("garbage"), None);
        assert!(is_valid_graph_id("my-graph_1"));
        assert!(!is_valid_graph_id("../x"));
        assert!(!is_valid_graph_id(""));
        assert!(is_valid_hash(&"a".repeat(64)));
        assert!(!is_valid_hash("ab/../"));
    }
}
