//! Exclusive write handle for a per-run database.
//!
//! Owns the `WriterToken` (in-process slot), the `FileLock` (cross-process
//! slot), the `mpsc::Sender<WriterCommand>`, and an embedded `RunReader` for
//! read methods. Reads delegate to the reader; writes go through the writer
//! task via the channel.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use surge_core::{ContentHash, RunId};
use tokio::sync::mpsc;

use crate::runs::error::StorageError;
use crate::runs::file_lock::FileLock;
use crate::runs::reader::{ReadEvent, RunReader};
use crate::runs::seq::EventSeq;
use crate::runs::types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};
use crate::runs::writer::WriterCommand;
use crate::runs::writer_slot::WriterToken;

/// Exclusive write handle for a per-run database.
///
/// One `RunWriter` per `RunId` per process; cross-process exclusion via
/// `FileLock` prevents any other process from opening another writer.
///
/// Drop is a best-effort fire-and-forget shutdown that emits `tracing::warn!`.
/// Prefer `RunWriter::close().await` for clean shutdown that joins the
/// background writer task.
pub struct RunWriter {
    pub(crate) reader: RunReader,
    pub(crate) writer_tx: mpsc::Sender<WriterCommand>,
    pub(crate) writer_join:
        Option<tokio::task::JoinHandle<Result<(), crate::runs::error::WriterError>>>,
    pub(crate) _token: Arc<WriterToken>,
    pub(crate) _file_lock: FileLock,
    pub(crate) closed: bool,
}

impl RunWriter {
    /// Run id this writer is bound to.
    pub fn run_id(&self) -> &RunId {
        self.reader.run_id()
    }

    /// Worktree path for this run.
    pub fn worktree_path(&self) -> &Path {
        self.reader.worktree_path()
    }

    /// Returns true if `close()` has been called.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

// Read methods delegated to embedded RunReader (avoids ~40 lines of boilerplate).
use crate::runs::macros::delegate_to_reader;
impl RunWriter {
    delegate_to_reader! {
        async {
            pub current_seq() -> Result<EventSeq, StorageError>;
            pub stage_executions() -> Result<Vec<StageExecution>, StorageError>;
            pub artifacts() -> Result<Vec<ArtifactRecord>, StorageError>;
            pub pending_approvals() -> Result<Vec<PendingApproval>, StorageError>;
            pub cost_summary() -> Result<CostSummary, StorageError>;
            pub list_snapshots() -> Result<Vec<EventSeq>, StorageError>;
        }
    }

    // Methods with non-trivial signatures (range, by-id, by-hash) are forwarded explicitly.

    /// Read one event by seq. Forwarded to the embedded `RunReader`.
    pub async fn read_event(&self, seq: EventSeq) -> Result<Option<ReadEvent>, StorageError> {
        self.reader.read_event(seq).await
    }

    /// Read events in a half-open range `[start, end)`. Forwarded to the embedded `RunReader`.
    pub async fn read_events(
        &self,
        range: Range<EventSeq>,
    ) -> Result<Vec<ReadEvent>, StorageError> {
        self.reader.read_events(range).await
    }

    /// Read the bytes of an artifact stored by content hash. Forwarded to the embedded `RunReader`.
    pub async fn read_artifact(
        &self,
        content_hash: &ContentHash,
    ) -> Result<Vec<u8>, StorageError> {
        self.reader.read_artifact(content_hash).await
    }

    /// Find the most recent snapshot at or before `seq`. Returns `(seq, raw blob)`.
    ///
    /// Forwarded to the embedded `RunReader`. The blob is caller-encoded (see
    /// `RunReader::latest_snapshot_at_or_before`); decoding is the caller's
    /// responsibility because `RunState` lacks serde derives in M1.
    pub async fn latest_snapshot_at_or_before(
        &self,
        seq: EventSeq,
    ) -> Result<Option<(EventSeq, Vec<u8>)>, StorageError> {
        self.reader.latest_snapshot_at_or_before(seq).await
    }
}
