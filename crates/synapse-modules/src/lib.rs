//! Sandboxed WebAssembly modules that extend Synapse.
//!
//! A module is a single `.wasm` file with its [`Manifest`] embedded in a custom section. It
//! runs in a wasmi interpreter (no JIT, so it works on every platform the app targets) with a
//! fuel budget and a memory cap, and reaches the app only through host functions gated by the
//! permissions the user granted at install time. Its UI is data: views and block macros are
//! JSON trees the app renders. See `docs/modules.md`.

pub mod crypto;
mod host;
pub mod manifest;
mod registry;

pub use host::{
    FUEL_PER_BYTE, FUEL_PER_HOST_CALL, FUEL_PER_REQUEST, MAX_DERIVES, MAX_HOST_CALLS, MAX_MEMORY,
    MAX_MESSAGE, MAX_STORAGE_BYTES, MAX_STORAGE_ENTRIES, MAX_TABLE_ELEMENTS, MAX_WRITE_BYTES,
    Running,
};
pub use manifest::{API_VERSION, MANIFEST_SECTION, Manifest, Permission, ViewDecl};
pub use registry::{ModuleInfo, Registry};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("permission denied: {0}")]
    Denied(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("decryption failed (wrong key or tampered data)")]
    Decrypt,
    /// The module misbehaved (trap, out of fuel, bad response); its instance is discarded.
    #[error("module error: {0}")]
    Module(String),
    #[error("host error: {0}")]
    Host(String),
    #[error(transparent)]
    Core(#[from] synapse_core::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
