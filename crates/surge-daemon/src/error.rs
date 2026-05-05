//! Daemon-side error type. Mapped to `ErrorCode` on the IPC wire by
//! `server.rs` (Phase 6).

use surge_orchestrator::engine::ipc::ErrorCode;

/// Errors produced by the daemon's IPC server and admission machinery.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Underlying I/O error (socket reads / writes, fs operations).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// IPC framing layer error (oversize frame, malformed JSON, etc.).
    #[error("framing: {0}")]
    Framing(#[from] surge_orchestrator::engine::ipc::FramingError),
    /// PID file or socket discovery failed.
    #[error("pidfile: {0}")]
    Pidfile(#[from] crate::pidfile::PidfileError),
    /// Admission queue refused new work.
    #[error("admission queue full ({active}/{max} active, {queued} queued)")]
    AdmissionFull {
        /// Number of currently-active runs.
        active: usize,
        /// Maximum allowed concurrent active runs.
        max: usize,
        /// Number of runs waiting in the queue.
        queued: usize,
    },
    /// FIFO admission queue is at its configured cap. The active set
    /// may also be at cap, but the load-bearing condition is the queue
    /// length: even if a slot freed instantly, accepting more work
    /// would still grow the daemon's pending state without bound.
    #[error("admission queue is full ({queue_len}/{max_queue})")]
    QueueFull {
        /// Current queue length.
        queue_len: usize,
        /// Configured queue cap.
        max_queue: usize,
    },
    /// Lookup of a run id the daemon does not currently host.
    #[error("run not active: {0}")]
    RunNotActive(surge_core::id::RunId),
    /// Underlying storage error (sqlite I/O, etc.).
    #[error("storage: {0}")]
    Storage(String),
    /// Engine-level error propagated through the daemon.
    #[error("engine: {0}")]
    Engine(#[from] surge_orchestrator::engine::EngineError),
    /// The IPC client disconnected while a request was in flight.
    #[error("client disconnected mid-request")]
    ClientGone,
    /// The daemon is in graceful shutdown and refusing new work.
    #[error("shutdown in progress")]
    ShuttingDown,
}

impl DaemonError {
    /// Map this error to a stable IPC error code that clients can
    /// react to programmatically.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Framing(_) => ErrorCode::BadRequest,
            Self::Io(_) | Self::Pidfile(_) | Self::ClientGone => ErrorCode::Internal,
            Self::AdmissionFull { .. } => ErrorCode::AdmissionFull,
            Self::QueueFull { .. } => ErrorCode::QueueFull,
            Self::RunNotActive(_) => ErrorCode::RunNotActive,
            Self::Storage(_) => ErrorCode::StorageError,
            Self::Engine(_) => ErrorCode::EngineError,
            Self::ShuttingDown => ErrorCode::ShuttingDown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_full_maps_to_admission_full() {
        let e = DaemonError::AdmissionFull {
            active: 8,
            max: 8,
            queued: 3,
        };
        assert_eq!(e.code(), ErrorCode::AdmissionFull);
    }

    #[test]
    fn queue_full_maps_to_queue_full() {
        let e = DaemonError::QueueFull {
            queue_len: 4,
            max_queue: 4,
        };
        assert_eq!(e.code(), ErrorCode::QueueFull);
        assert!(format!("{e}").contains("4/4"));
    }

    #[test]
    fn shutting_down_maps_to_shutting_down() {
        assert_eq!(DaemonError::ShuttingDown.code(), ErrorCode::ShuttingDown);
    }
}
