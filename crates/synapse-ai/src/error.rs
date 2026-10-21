use std::time::Duration;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no API key available for `{0}`")]
    MissingKey(String),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("network error: {0}")]
    Network(String),
    #[error("API error {status}: {message}")]
    Api {
        status: u16,
        message: String,
        retry_after: Option<Duration>,
    },
    #[error("the model declined to answer: {0}")]
    Refused(String),
    #[error("could not parse provider response: {0}")]
    Parse(String),
    #[error("could not parse model output as JSON: {0}")]
    BadModelOutput(String),
    #[error(transparent)]
    Graph(#[from] synapse_core::Error),
}

impl Error {
    pub(crate) fn is_retryable(&self) -> bool {
        match self {
            Error::Network(_) => true,
            Error::Api { status, .. } => matches!(status, 408 | 409 | 429 | 500..=599),
            _ => false,
        }
    }
}
