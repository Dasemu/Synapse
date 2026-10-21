//! Synapse sync: wire protocol (`protocol`), HTTP transport and thin-client read API
//! (`Remote`), and the replicating `SyncClient` with a background loop. See `docs/sync.md`.

mod client;
mod error;
pub mod protocol;
mod remote;
mod state;

pub use client::{BlobPolicy, SyncClient, SyncConfig, SyncHandle};
pub use error::{Error, Result};
pub use remote::{Remote, RemoteConfig};
pub use state::{Cursors, SyncState, SyncStatus};
