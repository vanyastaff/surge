//! Error types for the run storage layer.
//!
//! Distinct from the legacy `PersistenceError` — there is no `From` between
//! them by design; the two domains are independent.

use surge_core::RunId;
use thiserror::Error;

/// Failure modes for opening or creating a `Storage`, run reader, or run writer.
#[derive(Debug, Error)]
pub enum OpenError {
    /// Durable run authority refused an opening without journal mutation.
    #[error("run opening refused: {0}")]
    Authority(String),
    /// Native ownership refused before a pathname-based consumer could run.
    #[error("state-home {category}: {message}")]
    StateHome {
        /// Stable failure category without private object content.
        category: &'static str,
        /// SDK status or structural refusal reason.
        message: String,
    },
    /// The in-process writer ownership registry is poisoned.
    #[error("writer ownership registry is poisoned")]
    WriterOwnershipPoisoned,
    /// The supplied writer was not issued by this storage instance.
    #[error("writer belongs to another storage for run {run_id}")]
    WriterOwnerMismatch {
        /// The run whose writer association did not match.
        run_id: RunId,
    },
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
    /// The in-process writer ownership registry is poisoned.
    #[error("writer ownership registry is poisoned")]
    WriterOwnershipPoisoned,
    /// A durable operation was rejected without terminating the writer.
    #[error("journal operation rejected: {0}")]
    OperationRejected(String),
    /// Persistent-task ownership could not be read; generic hosts fail closed.
    #[error("task ownership journal: {0}")]
    WorkItemJournal(#[from] crate::work_items::WorkItemError),
    /// Durable bootstrap ownership could not be read; legacy launchers must fail closed.
    #[error("bootstrap ownership journal: {0}")]
    BootstrapJournal(#[from] super::bootstrap_operations::BootstrapStoreError),
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

    /// Event sequence lies outside the positive SQLite sequence domain.
    #[error("invalid event sequence bound or row")]
    InvalidEventSequence,

    /// A persisted event's timestamp is not a valid instant (#100).
    #[error(transparent)]
    CorruptTimestamp(#[from] super::reader::CorruptTimestamp),

    /// Connection pool error.
    #[error("pool error: {0}")]
    Pool(String),

    /// Schema-version migration of an event payload failed during read
    /// (e.g. payload bytes do not match the chain's expected layout for
    /// the recorded `schema_version`, or the version is out of range).
    /// Distinct from [`Self::Pool`] / [`Self::Sqlite`] so callers and
    /// operators can tell migration drift apart from raw I/O / pool
    /// faults at a glance — log filters, dashboards, and tests no
    /// longer need to pattern-match on string contents to discriminate.
    #[error("schema migration failed: {0}")]
    MigrationFailed(String),
}

/// Failure modes inside the writer task itself.
///
/// Wrapped into `StorageError` on the public surface; this is the writer-task
/// internal type carried over the oneshot reply channel.
#[derive(Debug, Error)]
pub enum WriterError {
    /// A stale or invalid durable operation was rejected before commit.
    #[error("operation rejected: {0}")]
    OperationRejected(String),
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
    /// Graceful shutdown could not obtain its actor-issued seal receipt.
    #[error("writer seal receipt failed: {0}")]
    Receipt(SealReceiptError),
    /// Receipt loss and actor failure were both observed after joining the actor.
    #[error("writer seal receipt failed: {receipt}; actor settlement failed: {actor}")]
    ReceiptAndActor {
        /// First receipt failure.
        receipt: SealReceiptError,
        /// Independently collected actual actor join result.
        actor: Box<CloseError>,
    },
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

/// Failure obtaining a serialized seal acknowledgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SealReceiptError {
    /// The actor no longer accepts commands.
    #[error("command channel closed")]
    CommandChannelClosed,
    /// The actor did not send a successful close/prefix receipt.
    #[error("reply channel closed")]
    ReplyChannelClosed,
}

/// Finite observation of an actual writer failure; carries no private SQL details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterFailure {
    /// The owning writer requested priority interruption or was dropped.
    Interrupted,
    /// The actor returned an error while opening, querying or closing.
    ActorFailed,
}

/// A lifecycle observation channel lost its actor without a terminal receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FeedLifecycleError {
    /// A closed channel still reported Running; settlement is unconfirmed.
    #[error("writer lifecycle owner lost before terminal receipt")]
    WriterLost,
}
