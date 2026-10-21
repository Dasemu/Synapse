//! Automatic git-based backup and version history of a Synapse graph. See `docs/history.md`.

mod error;
pub mod export;
mod history;
pub mod parse;
mod push;
mod repo;
mod restore;

pub use error::{Error, Result};
pub use history::{History, HistoryOptions, PageVersion, SnapshotInfo, SnapshotPolicy};
pub use push::{PushAuth, PushReport};
pub use restore::RestoreReport;
