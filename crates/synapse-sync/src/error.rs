use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("graph: {0}")]
    Core(#[from] synapse_core::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unauthorized: check the auth token")]
    Unauthorized,
    #[error("server returned {status}: {message}")]
    Status { status: u16, message: String },
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("background task failed: {0}")]
    Task(String),
}

impl Error {
    /// True when the server could not be reached (as opposed to rejecting a request).
    pub fn is_offline(&self) -> bool {
        match self {
            Error::Http(e) => e.is_connect() || e.is_timeout() || e.is_request() && !e.is_status(),
            Error::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::UnexpectedEof
            ),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
