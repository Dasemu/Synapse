use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("core: {0}")]
    Core(#[from] synapse_core::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("tantivy: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    #[error("tantivy directory: {0}")]
    Directory(#[from] tantivy::directory::error::OpenDirectoryError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("embedder: {0}")]
    Embed(String),
    #[error("invalid: {0}")]
    Invalid(String),
    /// A suggestion no longer matches the block text (the block changed since).
    #[error("stale suggestion: {0}")]
    Stale(String),
}

pub type Result<T> = std::result::Result<T, Error>;
