//! Persistent sync cursors (table `sync_state`) and the UI-facing state.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use synapse_core::Graph;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Idle,
    Syncing,
    /// Server unreachable; local edits are kept and pushed once it is back.
    Offline,
    /// The server answered but something failed (auth, protocol, disk...). See `message`.
    Error,
}

/// What the UI shows about sync.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncState {
    pub status: SyncStatus,
    pub message: Option<String>,
    /// Local ops not yet pushed to the server.
    pub pending_ops: u64,
    /// Wall-clock millis of the last fully successful cycle.
    pub last_sync_ms: Option<i64>,
    /// Ops pushed / pulled(applied) in the last successful cycle.
    pub last_pushed: u64,
    pub last_pulled: u64,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            status: SyncStatus::Idle,
            message: None,
            pending_ops: 0,
            last_sync_ms: None,
            last_pushed: 0,
            last_pulled: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cursors {
    /// Highest local op seq already pushed (or skipped because it came from the server).
    pub pushed_seq: i64,
    /// Highest server op seq already pulled.
    pub pull_cursor: i64,
}

pub(crate) fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
}

fn get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM sync_state WHERE key=?1", [key], |r| {
        r.get(0)
    })
    .optional()
}

fn set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sync_state(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=?2",
        params![key, value],
    )?;
    Ok(())
}

/// Loads the cursors for `(remote_key, server_id)`. If the graph is now synced with another
/// server/graph, or the server graph was recreated, cursors restart from zero (re-pushing is
/// safe: the server merges with last-writer-wins and ignores what it already has).
pub(crate) fn load(g: &Graph, remote_key: &str, server_id: &str) -> rusqlite::Result<Cursors> {
    let c = g.conn();
    init(c)?;
    let same = get(c, "remote_key")?.as_deref() == Some(remote_key)
        && get(c, "server_id")?.as_deref() == Some(server_id);
    if !same {
        set(c, "remote_key", remote_key)?;
        set(c, "server_id", server_id)?;
        set(c, "pushed_seq", "0")?;
        set(c, "pull_cursor", "0")?;
        return Ok(Cursors::default());
    }
    let num =
        |k| -> rusqlite::Result<i64> { Ok(get(c, k)?.and_then(|v| v.parse().ok()).unwrap_or(0)) };
    Ok(Cursors {
        pushed_seq: num("pushed_seq")?,
        pull_cursor: num("pull_cursor")?,
    })
}

pub(crate) fn save_pushed(g: &Graph, seq: i64) -> rusqlite::Result<()> {
    set(g.conn(), "pushed_seq", &seq.to_string())
}

pub(crate) fn save_pulled(g: &Graph, cursor: i64) -> rusqlite::Result<()> {
    set(g.conn(), "pull_cursor", &cursor.to_string())
}

/// Number of local-origin ops after `pushed_seq`.
pub(crate) fn pending(g: &Graph, pushed_seq: i64) -> rusqlite::Result<u64> {
    let pattern = format!("%-{}", g.actor());
    g.conn()
        .query_row(
            "SELECT count(*) FROM ops WHERE seq > ?1 AND hlc LIKE ?2",
            params![pushed_seq, pattern],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n as u64)
}

/// Pending count using the persisted cursor (works before the first successful sync too).
pub(crate) fn pending_now(g: &Graph) -> rusqlite::Result<u64> {
    init(g.conn())?;
    let pushed = get(g.conn(), "pushed_seq")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pending(g, pushed)
}
