use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("core: {0}")]
    Core(#[from] synapse_core::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server answered {status} for {what}")]
    Status { status: u16, what: String },
    #[error("etag mismatch for {0}")]
    PreconditionFailed(String),
    #[error("xml: {0}")]
    Xml(String),
    #[error("icalendar: {0}")]
    Ical(String),
    #[error("discovery failed: {0}")]
    Discovery(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
