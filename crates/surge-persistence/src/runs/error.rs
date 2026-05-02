//! Error types for the run storage layer.
//!
//! Distinct from the legacy `PersistenceError` — there is no `From` between
//! them by design; the two domains are independent.

use surge_core::RunId;
use thiserror::Error;

/// Failure modes for opening or creating a `Storage`, run reader, or run writer.
#[derive(Debug, Error)]
pub enum OpenError {
    /// Another writer (this process or another) currently holds the slot for this run.
    #[error("writer already held for run {run_id}")]
    WriterAlreadyHeld {
        /// The run whose writer slot is already held.
        run_id: RunId,
    },

    /// The requested run does not exist in the registry.
    #[error("run not found: {0}")]
    RunNotFound(RunId),

    /// A schema migration failed.
    #[error("migration failed: {0}")]
    MigrationFailed(String),

    /// Filesystem I/O error during open.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// SQLite-level error during open.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Configuration file invalid.
    #[error("config error: {0}")]
    Config(String),

    /// Storage requires the multi-thread tokio runtime; current is single-thread.
    #[error("single-threaded tokio runtime not supported by Storage; use multi-threaded runtime")]
    SingleThreadedRuntime,

    /// Connection pool initialization failed.
    #[error("pool init error: {0}")]
    Pool(String),
}

/// Failure modes for reads and writes against an open run.
#[derive(Debug, Error)]
pub enum StorageError {
    /// SQLite-level error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Filesystem I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization or deserialization failed.
    #[error("serialization failed: {0}")]
    SerializationFailed(#[from] serde_json::Error),

    /// The writer task ended before the request could be served.
    #[error("writer task died unexpectedly")]
    WriterTaskDied,

    /// Operation refused because a live writer holds the run.
    #[error("writer still active for run {run_id}")]
    WriterStillActive {
        /// The run whose writer slot is still held.
        run_id: RunId,
    },

    /// Connection pool error.
    #[error("pool error: {0}")]
    Pool(String),
}

/// Failure modes inside the writer task itself.
///
/// Wrapped into `StorageError` on the public surface; this is the writer-task
/// internal type carried over the oneshot reply channel.
#[derive(Debug, Error)]
pub enum WriterError {
    /// SQLite-level error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Filesystem I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization or deserialization failed.
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Internal invariant violation in the writer task.
    #[error("internal: {0}")]
    Internal(String),
}

/// Failure modes for `RunWriter::close`.
#[derive(Debug, Error)]
pub enum CloseError {
    /// A writer-level error while flushing or closing.
    #[error(transparent)]
    Writer(#[from] WriterError),

    /// The writer task panicked or otherwise failed to join.
    #[error("writer task join failed: {0}")]
    JoinFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::RunId;

    #[test]
    fn writer_already_held_displays_run_id() {
        let id = RunId::new();
        let err = OpenError::WriterAlreadyHeld { run_id: id };
        assert!(err.to_string().contains("run-"));
    }
}
