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
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        self.reader.run_id()
    }

    /// Worktree path for this run.
    #[must_use]
    pub fn worktree_path(&self) -> &Path {
        self.reader.worktree_path()
    }

    /// Returns true if `close()` has been called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

// Read methods delegated to embedded RunReader (avoids ~40 lines of boilerplate).
use crate::runs::macros::delegate_to_reader;
impl RunWriter {
    delegate_to_reader! {
        async {
            /// Highest-assigned event seq for this run. Forwarded to the embedded `RunReader`.
            pub current_seq() -> Result<EventSeq, StorageError>;
            /// All rows of the `stage_executions` materialized view. Forwarded to the embedded `RunReader`.
            pub stage_executions() -> Result<Vec<StageExecution>, StorageError>;
            /// All rows of the `artifacts` materialized view. Forwarded to the embedded `RunReader`.
            pub artifacts() -> Result<Vec<ArtifactRecord>, StorageError>;
            /// All rows of the `pending_approvals` materialized view. Forwarded to the embedded `RunReader`.
            pub pending_approvals() -> Result<Vec<PendingApproval>, StorageError>;
            /// Aggregate cost / token / cache-hit totals for this run. Forwarded to the embedded `RunReader`.
            pub cost_summary() -> Result<CostSummary, StorageError>;
            /// All snapshot seqs recorded for this run, ascending. Forwarded to the embedded `RunReader`.
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
    pub async fn read_artifact(&self, content_hash: &ContentHash) -> Result<Vec<u8>, StorageError> {
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

use surge_core::VersionedEventPayload;
use tokio::sync::oneshot;

use crate::runs::error::{CloseError, WriterError};

impl RunWriter {
    /// Append a single event. Returns the assigned `EventSeq`.
    pub async fn append_event(
        &self,
        payload: VersionedEventPayload,
    ) -> Result<EventSeq, StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::AppendEvent {
                payload,
                reply: reply_tx,
            })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx
            .await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Append multiple events atomically (single transaction). Returns assigned seqs in order.
    pub async fn append_events(
        &self,
        payloads: Vec<VersionedEventPayload>,
    ) -> Result<Vec<EventSeq>, StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::AppendBatch {
                payloads,
                reply: reply_tx,
            })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx
            .await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Store an artifact (atomic FS write + content-addressed dedup).
    ///
    /// `produced_at_seq` is set to the writer's current seq at call time
    /// (best-effort association with the latest event).
    pub async fn store_artifact(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<ArtifactRecord, StorageError> {
        self.ensure_open()?;
        let current_seq = self.current_seq().await?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::StoreArtifact {
                name: name.to_string(),
                content: content.to_vec(),
                produced_by: None,
                produced_at_seq: current_seq,
                reply: reply_tx,
            })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx
            .await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Write a graph snapshot.
    ///
    /// `blob` is the caller-encoded representation of the run state (typically
    /// `serde_json::to_vec(&run_state)`). M2 storage is agnostic to the
    /// snapshot's encoding; readers see only raw bytes via
    /// `latest_snapshot_at_or_before`. (RunState lacks serde derives in M1.)
    pub async fn write_graph_snapshot(
        &self,
        at_seq: EventSeq,
        blob: Vec<u8>,
    ) -> Result<(), StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::WriteSnapshot {
                at_seq,
                blob,
                reply: reply_tx,
            })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx
            .await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Truncate all materialized view tables and rebuild from events.
    ///
    /// Runs inside a single transaction; readers see pre-rebuild state until
    /// commit (WAL gives them a snapshot view), so there is no transient empty-
    /// view window from a reader perspective.
    pub async fn rebuild_views(&self) -> Result<(), StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::RebuildViews { reply: reply_tx })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx
            .await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Wait for all previously-issued commands to be processed by the writer task.
    ///
    /// Strict ordering of the mpsc channel + sequential command processing
    /// means once Flush is dequeued, all prior commands are already committed.
    /// Does NOT trigger WAL checkpoint or fsync — those are managed by the
    /// writer task via periodic `wal_checkpoint(TRUNCATE)`.
    pub async fn flush(&self) -> Result<(), StorageError> {
        self.ensure_open()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.writer_tx
            .send(WriterCommand::Flush { reply: reply_tx })
            .await
            .map_err(|_| StorageError::WriterTaskDied)?;
        reply_rx
            .await
            .map_err(|_| StorageError::WriterTaskDied)?
            .map_err(map_writer_err)
    }

    /// Explicit clean shutdown — sends Shutdown to the writer task, waits for
    /// it to drain in-flight commands and exit. Releases file lock and in-
    /// process token.
    ///
    /// Prefer over `Drop` for clean shutdown — `Drop` is a fire-and-forget
    /// fallback that warns via tracing.
    pub async fn close(mut self) -> Result<(), CloseError> {
        if self.closed {
            return Ok(());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .writer_tx
            .send(WriterCommand::Shutdown { reply: reply_tx })
            .await
            .is_ok()
        {
            let _ = reply_rx.await;
        }
        self.closed = true;
        if let Some(join) = self.writer_join.take() {
            join.await
                .map_err(|e| CloseError::JoinFailed(e.to_string()))?
                .map_err(CloseError::Writer)?;
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), StorageError> {
        if self.closed {
            return Err(StorageError::WriterTaskDied);
        }
        Ok(())
    }
}

fn map_writer_err(e: WriterError) -> StorageError {
    match e {
        WriterError::Sqlite(s) => StorageError::Sqlite(s),
        WriterError::Io(i) => StorageError::Io(i),
        WriterError::Serialization(j) => StorageError::SerializationFailed(j),
        WriterError::Internal(_) => StorageError::WriterTaskDied,
    }
}

impl Drop for RunWriter {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // Best-effort fire-and-forget shutdown. Drop is sync — we can't await join.
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = self
            .writer_tx
            .try_send(WriterCommand::Shutdown { reply: reply_tx });
        tracing::warn!(
            run_id = %self.reader.run_id(),
            "RunWriter dropped without close() — pending writes may be lost. \
             Prefer RunWriter::close().await for clean shutdown."
        );
    }
}

impl RunWriter {
    /// Polling-based event subscription stream (delegates to embedded `RunReader`).
    pub fn subscribe_events(
        &self,
    ) -> impl futures_core::Stream<Item = Result<ReadEvent, StorageError>> + Send + 'static {
        self.reader.subscribe_events()
    }
}
