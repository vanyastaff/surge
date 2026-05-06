//! Error type for `surge-intake`.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("invalid task id: {0}")]
    InvalidTaskId(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    #[error("internal: {0}")]
    Internal(String),
}
