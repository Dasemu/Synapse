use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("core: {0}")]
    Core(#[from] synapse_core::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("git: {0}")]
    Git(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("push: {0}")]
    Push(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn git<E: std::fmt::Display>(e: E) -> Error {
    Error::Git(e.to_string())
}
