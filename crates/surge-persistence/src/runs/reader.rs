//! Read-only handle for a per-run database.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::SqliteConnectionManager;
use r2d2::Pool;
use rusqlite::params;
use surge_core::run_event::RunEvent;
use surge_core::{ContentHash, RoadmapPatchId, RunId, VersionedEventPayload, migrate_payload};

use crate::runs::error::StorageError;
use crate::runs::reader_views as views;
use crate::runs::seq::EventSeq;
use crate::runs::types::{
    ArtifactRecord, CostSummary, PendingApproval, RoadmapPatchRecord, StageExecution, TaskLedgerRow,
};

/// Read-only handle on a per-run database.
///
/// Holds a small `r2d2_sqlite` pool of read connections (default 4). Async
/// methods route blocking SQL through `tokio::task::spawn_blocking`. Cloneable
/// (cheap — pool and Arcs).
#[derive(Clone)]
pub struct RunReader {
    pub(crate) run_id: RunId,
    pub(crate) pool: Pool<SqliteConnectionManager>,
    pub(crate) artifacts_dir: Arc<PathBuf>,
    pub(crate) worktree_path: Arc<PathBuf>,
}

/// Decoded event row.
#[derive(Debug, Clone)]
pub struct ReadEvent {
    /// Monotonic event sequence number.
    pub seq: EventSeq,
    /// Unix epoch ms timestamp recorded when the event was appended.
    pub timestamp_ms: i64,
    /// Event kind tag (matches `EventPayload` variant name).
    pub kind: String,
    /// Decoded versioned payload.
    pub payload: VersionedEventPayload,
}

impl RunReader {
    /// Run id this reader is bound to.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Worktree directory for this run.
    #[must_use]
    pub fn worktree_path(&self) -> &Path {
        &self.worktree_path
    }

    /// Latest event seq written so far. Returns `EventSeq::ZERO` if the table is empty.
    pub async fn current_seq(&self) -> Result<EventSeq, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<EventSeq, StorageError> {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let max: Option<i64> =
                conn.query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
            Ok(EventSeq(max.unwrap_or(0) as u64))
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read one event by seq.
    pub async fn read_event(&self, seq: EventSeq) -> Result<Option<ReadEvent>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<ReadEvent>, StorageError> {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let row = conn.query_row(
                "SELECT seq, timestamp, kind, payload, schema_version FROM events WHERE seq = ?",
                params![seq.0 as i64],
                |row| {
                    let blob: Vec<u8> = row.get(3)?;
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        blob,
                        row.get::<_, i64>(4)? as u32,
                    ))
                },
            );
            match row {
                Ok((seq, ts, kind, blob, schema_version)) => {
                    let inner = migrate_payload(schema_version, &blob).map_err(|e| {
                        StorageError::MigrationFailed(format!("seq={}: {e}", seq.0))
                    })?;
                    let payload = VersionedEventPayload {
                        schema_version,
                        payload: inner,
                    };
                    Ok(Some(ReadEvent {
                        seq,
                        timestamp_ms: ts,
                        kind,
                        payload,
                    }))
                },
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read events in a half-open range `[start, end)`.
    pub async fn read_events(
        &self,
        range: Range<EventSeq>,
    ) -> Result<Vec<ReadEvent>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<ReadEvent>, StorageError> {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let mut stmt = conn.prepare(
                "SELECT seq, timestamp, kind, payload, schema_version
                 FROM events WHERE seq >= ? AND seq < ? ORDER BY seq",
            )?;
            // SQLite stores seq as i64. A naive `range.end.0 as i64` wraps
            // the common open-ended sentinel `EventSeq(u64::MAX)` to -1,
            // making `seq < -1` match nothing. Clamp instead so an
            // out-of-i64-range bound becomes i64::MAX (every real seq fits
            // in i64).
            let start_bind = i64::try_from(range.start.0).unwrap_or(i64::MAX);
            let end_bind = i64::try_from(range.end.0).unwrap_or(i64::MAX);
            let iter = stmt.query_map(params![start_bind, end_bind], |row| {
                let blob: Vec<u8> = row.get(3)?;
                Ok((
                    EventSeq(row.get::<_, i64>(0)? as u64),
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    blob,
                    row.get::<_, i64>(4)? as u32,
                ))
            })?;
            let mut out = Vec::new();
            for r in iter {
                let (seq, ts, kind, blob, schema_version) = r?;
                let inner = migrate_payload(schema_version, &blob)
                    .map_err(|e| StorageError::MigrationFailed(format!("seq={}: {e}", seq.0)))?;
                let payload = VersionedEventPayload {
                    schema_version,
                    payload: inner,
                };
                out.push(ReadEvent {
                    seq,
                    timestamp_ms: ts,
                    kind,
                    payload,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read this run's full event log as plain `surge_core::RunEvent`s — the
    /// one place this crate's `ReadEvent`/`EventSeq`/`timestamp_ms` shape gets
    /// turned into `surge-core`'s own event type, so every caller that needs
    /// a `Vec<RunEvent>` (a pure `surge-core` compiler, e.g.
    /// `RunReport::compile`, cannot depend on this crate's `ReadEvent` at
    /// all) shares this one conversion instead of re-deriving it. Used by
    /// `surge-cli` (`surge run report`, `surge inbox`, `surge resolve`) and
    /// `surge-daemon` (the L3 merge gate's optional Run Report attachment,
    /// spec §10/R31) alike.
    pub async fn read_run_events(&self) -> Result<Vec<RunEvent>, StorageError> {
        let run_id = self.run_id;
        let events = self.read_events(EventSeq(0)..EventSeq(u64::MAX)).await?;
        Ok(events
            .into_iter()
            .map(|read| RunEvent {
                run_id,
                seq: read.seq.0,
                timestamp: chrono::DateTime::from_timestamp_millis(read.timestamp_ms)
                    .unwrap_or_default(),
                payload: read.payload.payload,
            })
            .collect())
    }

    /// Read all rows of the `stage_executions` materialized view.
    pub async fn stage_executions(&self) -> Result<Vec<StageExecution>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::stage_executions(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read all rows of the `artifacts` materialized view.
    pub async fn artifacts(&self) -> Result<Vec<ArtifactRecord>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::artifacts(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read all rows of the `pending_approvals` materialized view.
    pub async fn pending_approvals(&self) -> Result<Vec<PendingApproval>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::pending_approvals(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Aggregate the `cost_summary` view into a single `CostSummary`.
    pub async fn cost_summary(&self) -> Result<CostSummary, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::cost_summary(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read all rows of the per-run `task_ledger` materialized view.
    pub async fn task_ledger(&self) -> Result<Vec<TaskLedgerRow>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::task_ledger(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read all roadmap patch lifecycle records.
    pub async fn roadmap_patches(&self) -> Result<Vec<RoadmapPatchRecord>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::roadmap_patches(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read one roadmap patch lifecycle record by ID.
    pub async fn roadmap_patch(
        &self,
        patch_id: &RoadmapPatchId,
    ) -> Result<Option<RoadmapPatchRecord>, StorageError> {
        let pool = self.pool.clone();
        let patch_id = patch_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::roadmap_patch(&conn, &patch_id)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// List the seqs of all written graph snapshots.
    pub async fn list_snapshots(&self) -> Result<Vec<EventSeq>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::list_snapshots(&conn)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Find the most recent snapshot at or before `seq`, returning the seq and raw blob.
    ///
    /// The blob is the `serde_json` encoding of `RunState` (see M2 design §2.3).
    /// Decoding is intentionally left to the caller because `surge_core::RunState`
    /// does not yet implement `serde::Deserialize`; once it does, this method may
    /// be tightened to return a decoded `RunState`.
    pub async fn latest_snapshot_at_or_before(
        &self,
        seq: EventSeq,
    ) -> Result<Option<(EventSeq, Vec<u8>)>, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            views::latest_snapshot_at_or_before(&conn, seq)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }

    /// Read the bytes of an artifact stored by content hash.
    ///
    /// Looks up the stored file path in the `artifacts` table and reads the
    /// file from `artifacts_dir`.
    pub async fn read_artifact(&self, content_hash: &ContentHash) -> Result<Vec<u8>, StorageError> {
        let pool = self.pool.clone();
        let dir = self.artifacts_dir.clone();
        let hash_str = content_hash.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, StorageError> {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let path: String = conn.query_row(
                "SELECT path FROM artifacts WHERE content_hash = ? LIMIT 1",
                params![hash_str],
                |row| row.get(0),
            )?;
            let full = dir.join(path);
            let bytes = std::fs::read(&full)?;
            Ok(bytes)
        })
        .await
        .map_err(|e| StorageError::Pool(e.to_string()))?
    }
}

impl RunReader {
    /// Polling-based event subscription stream.
    ///
    /// Yields events with `seq > last_seq` every ~100 ms. Per-tick batch is
    /// capped at `SUBSCRIBE_BATCH_MAX` (256) to bound memory if the consumer
    /// lags behind the writer. Cancel-safe — dropping the stream releases the
    /// background polling task.
    pub fn subscribe_events(
        &self,
    ) -> impl futures_core::Stream<Item = Result<ReadEvent, StorageError>> + Send + 'static {
        crate::runs::subscribe::subscribe(self.pool.clone(), self.artifacts_dir.clone())
    }
}

/// Opaque finite-feed observation owned by the execution's event forwarder.
///
/// Holds the original writer exclusion through final drain. This capability
/// cannot write, close the actor, reveal SQL handles, or switch to another run.
/// Reader fields precede exclusion fields so resource release keeps that order.
pub struct RunEventFeed {
    reader: RunReader,
    lease: Arc<super::writer_slot::WriterLease>,
    lifecycle: tokio::sync::watch::Receiver<super::writer::WriterLifecycle>,
    #[cfg(any(test, feature = "test-support"))]
    read_gate: Option<SharedReadGate>,
    #[cfg(any(test, feature = "test-support"))]
    post_read_gate: Option<SharedReadGate>,
    #[cfg(any(test, feature = "test-support"))]
    checkout_gate: Option<SharedReadGate>,
}

/// Complete owner for a synchronous feed SQL worker.
///
/// Move this value into the actual blocking closure and join that closure before
/// starting another batch. Its reader and original writer exclusion remain owned
/// even when the asynchronous task awaiting the worker is cancelled.
#[cfg_attr(any(test, feature = "test-support"), derive(Clone))]
pub struct RunEventReadLease {
    reader: RunReader,
    lease: Arc<super::writer_slot::WriterLease>,
    #[cfg(any(test, feature = "test-support"))]
    read_gate: Option<SharedReadGate>,
    #[cfg(any(test, feature = "test-support"))]
    post_read_gate: Option<SharedReadGate>,
    #[cfg(any(test, feature = "test-support"))]
    checkout_gate: Option<SharedReadGate>,
}

impl RunEventFeed {
    pub(crate) fn from_writer(writer: &super::run_writer::RunWriter) -> Self {
        Self {
            reader: writer.reader.clone(),
            lease: writer.lease.clone(),
            lifecycle: writer.lifecycle.clone(),
            #[cfg(any(test, feature = "test-support"))]
            read_gate: None,
            #[cfg(any(test, feature = "test-support"))]
            post_read_gate: None,
            #[cfg(any(test, feature = "test-support"))]
            checkout_gate: None,
        }
    }
    /// Exact run to which this feed and every derived SQL worker belong.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        self.reader.run_id()
    }

    /// Latest actual actor observation; Running after channel loss is not a seal.
    #[must_use]
    pub fn current_lifecycle(&self) -> super::writer::WriterLifecycle {
        *self.lifecycle.borrow()
    }

    /// Wait for a new actor observation; distinguish owner loss from terminal state.
    pub async fn changed(
        &mut self,
    ) -> Result<super::writer::WriterLifecycle, super::error::FeedLifecycleError> {
        if self.lifecycle.changed().await.is_err() {
            let current = self.current_lifecycle();
            return match current {
                super::writer::WriterLifecycle::Running => {
                    Err(super::error::FeedLifecycleError::WriterLost)
                },
                _ => Ok(current),
            };
        }
        Ok(self.current_lifecycle())
    }

    /// Derive the complete owner to move into one real synchronous SQL worker.
    #[must_use]
    pub fn read_lease(&self) -> RunEventReadLease {
        RunEventReadLease {
            reader: self.reader.clone(),
            lease: self.lease.clone(),
            #[cfg(any(test, feature = "test-support"))]
            read_gate: self.read_gate.clone(),
            #[cfg(any(test, feature = "test-support"))]
            post_read_gate: self.post_read_gate.clone(),
            #[cfg(any(test, feature = "test-support"))]
            checkout_gate: self.checkout_gate.clone(),
        }
    }
}
impl RunEventReadLease {
    /// Read at most 256 events with `after < seq <= through`, in sequence order.
    ///
    /// This is synchronous; call it inside the retained blocking job, whose actual
    /// join must complete before a subsequent batch or a settlement claim.
    pub fn read_batch(
        &self,
        after: EventSeq,
        through: EventSeq,
    ) -> Result<Vec<ReadEvent>, StorageError> {
        self.lease
            .verify()
            .map_err(super::host_settlement::storage_error)?;
        #[cfg(any(test, feature = "test-support"))]
        let connection = if let Some(gate) = &self.checkout_gate {
            wait_read_gate(gate)?;
            self.reader.pool.get_timeout(std::time::Duration::ZERO)
        } else {
            self.reader.pool.get()
        }
        .map_err(|error| StorageError::Pool(error.to_string()))?;
        #[cfg(not(any(test, feature = "test-support")))]
        let connection = self
            .reader
            .pool
            .get()
            .map_err(|error| StorageError::Pool(error.to_string()))?;
        #[cfg(any(test, feature = "test-support"))]
        if let Some(gate) = &self.read_gate {
            wait_read_gate(gate)?;
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            let result = super::subscribe::read_rows(&connection, after, through);
            // The SQL statement has completed and dropped; preserve its actual
            // empty result while this worker still owns its connection and lease.
            if result.as_ref().is_ok_and(Vec::is_empty)
                && let Some(gate) = &self.post_read_gate
            {
                wait_read_gate(gate)?;
            }
            result
        }
        #[cfg(not(any(test, feature = "test-support")))]
        super::subscribe::read_rows(&connection, after, through)
    }
}

#[cfg(any(test, feature = "test-support"))]
type SharedReadGate = Arc<std::sync::Mutex<Option<ReadGateChannels>>>;
#[cfg(any(test, feature = "test-support"))]
struct ReadGateChannels {
    entered: tokio::sync::oneshot::Sender<()>,
    released: std::sync::mpsc::Receiver<()>,
}

/// One-shot test controller for an actual feed worker holding its real connection.
/// Dropping the controller releases the worker even when an assertion unwinds.
#[cfg(any(test, feature = "test-support"))]
pub struct FeedReadGate {
    entered: tokio::sync::oneshot::Receiver<()>,
    release: Option<std::sync::mpsc::Sender<()>>,
}
#[cfg(any(test, feature = "test-support"))]
impl FeedReadGate {
    /// Await the selected actual SQL boundary once; no synthetic result is supplied.
    pub async fn entered(&mut self) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        (&mut self.entered).await
    }
    /// Release the actual blocking worker; does not claim that it has joined.
    pub fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
impl Drop for FeedReadGate {
    fn drop(&mut self) {
        self.release();
    }
}
#[cfg(any(test, feature = "test-support"))]
impl RunEventFeed {
    fn has_test_gate(&self) -> bool {
        self.checkout_gate.is_some() || self.read_gate.is_some() || self.post_read_gate.is_some()
    }

    /// Gate the first successful empty SQL result before its worker returns it.
    ///
    /// Separate from the pre-query gate: the real query and statement teardown
    /// have already finished, while the original connection and lease remain held.
    /// Install before deriving the worker lease to exercise.
    pub fn install_post_read_gate_for_test(&mut self) -> Result<FeedReadGate, StorageError> {
        if self.has_test_gate() {
            return Err(StorageError::OperationRejected(
                "feed post-read gate already installed".into(),
            ));
        }
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        self.post_read_gate = Some(Arc::new(std::sync::Mutex::new(Some(ReadGateChannels {
            entered,
            released,
        }))));
        Ok(FeedReadGate {
            entered: entry,
            release: Some(release),
        })
    }

    /// Install one isolated test gate before deriving the worker lease to exercise.
    pub fn install_read_gate_for_test(&mut self) -> Result<FeedReadGate, StorageError> {
        if self.has_test_gate() {
            return Err(StorageError::OperationRejected(
                "feed read gate already installed".into(),
            ));
        }
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        self.read_gate = Some(Arc::new(std::sync::Mutex::new(Some(ReadGateChannels {
            entered,
            released,
        }))));
        Ok(FeedReadGate {
            entered: entry,
            release: Some(release),
        })
    }
}
#[cfg(any(test, feature = "test-support"))]
fn wait_read_gate(gate: &SharedReadGate) -> Result<(), StorageError> {
    let gate = gate
        .lock()
        .map_err(|_| StorageError::Pool("feed test gate poisoned".into()))?
        .take();
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        gate.released
            .recv()
            .map_err(|_| StorageError::Pool("feed test gate release lost".into()))?;
    }
    Ok(())
}

/// Isolated test owner of the selected feed's actual reader pool and lifecycle.
///
/// This deliberately retains the complete read lease. Drop it and any slot guard
/// before making an exclusion-release or fixture-cleanup assertion.
#[cfg(any(test, feature = "test-support"))]
pub struct FeedPoolProbe {
    owner: RunEventReadLease,
    lifecycle: tokio::sync::watch::Receiver<super::writer::WriterLifecycle>,
}

/// Actual checked-out pool slots followed by their independent complete owner.
/// No SQL connection or mutation capability is exposed through this test guard.
#[cfg(any(test, feature = "test-support"))]
pub struct FeedPoolSlots {
    connections: Vec<r2d2::PooledConnection<SqliteConnectionManager>>,
    capacity: usize,
    _owner: RunEventReadLease,
}
#[cfg(any(test, feature = "test-support"))]
impl FeedPoolSlots {
    /// Number of actual retained pooled connections.
    #[must_use]
    pub fn count(&self) -> usize {
        self.connections.len()
    }
    /// Capacity independently read from the selected real pool.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}
#[cfg(any(test, feature = "test-support"))]
impl RunEventFeed {
    /// Gate the first actual worker before checkout and expose bounded same-pool controls.
    ///
    /// Only this opt-in fixture selects real zero-timeout checkout in derived
    /// workers. Ordinary builds and unconfigured feeds retain the normal get path.
    pub fn install_pool_probe_for_test(
        &mut self,
    ) -> Result<(FeedReadGate, FeedPoolProbe), StorageError> {
        if self.has_test_gate() {
            return Err(StorageError::OperationRejected(
                "feed gate already installed".into(),
            ));
        }
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        self.checkout_gate = Some(Arc::new(std::sync::Mutex::new(Some(ReadGateChannels {
            entered,
            released,
        }))));
        let gate = FeedReadGate {
            entered: entry,
            release: Some(release),
        };
        let probe = FeedPoolProbe {
            owner: self.read_lease(),
            lifecycle: self.lifecycle.clone(),
        };
        Ok((gate, probe))
    }
}
#[cfg(any(test, feature = "test-support"))]
impl FeedPoolProbe {
    /// Await the actual actor's successful seal; failure or lost Running never succeeds.
    pub async fn wait_sealed(&mut self) -> Result<EventSeq, StorageError> {
        loop {
            let lifecycle = *self.lifecycle.borrow();
            match lifecycle {
                super::writer::WriterLifecycle::Sealed(prefix) => return Ok(prefix),
                super::writer::WriterLifecycle::Failed(_) => {
                    return Err(StorageError::WriterTaskDied);
                },
                super::writer::WriterLifecycle::Running => {},
            }
            if self.lifecycle.changed().await.is_err() {
                return match *self.lifecycle.borrow() {
                    super::writer::WriterLifecycle::Sealed(prefix) => Ok(prefix),
                    _ => Err(StorageError::WriterTaskDied),
                };
            }
        }
    }

    /// Blocking positive control: real zero-timeout checkout, SELECT 1, and return to pool.
    pub fn check_available_zero_timeout(&self) -> Result<(), StorageError> {
        let connection = self
            .owner
            .reader
            .pool
            .get_timeout(std::time::Duration::ZERO)
            .map_err(|error| StorageError::Pool(error.to_string()))?;
        let value: i64 = connection.query_row("SELECT 1", [], |row| row.get(0))?;
        if value != 1 {
            return Err(StorageError::OperationRejected(
                "pool availability query contradicted literal".into(),
            ));
        }
        Ok(())
    }

    /// Blocking bounded control: retain every real slot, then prove actual checkout exhaustion.
    /// Fixture pools above sixteen connections are rejected instead of unbounded acquisition.
    pub fn exhaust_all_slots(&self) -> Result<FeedPoolSlots, StorageError> {
        let maximum = self.owner.reader.pool.max_size();
        if !(1..=16).contains(&maximum) {
            return Err(StorageError::OperationRejected(
                "fixture pool capacity outside 1..=16".into(),
            ));
        }
        let capacity = usize::try_from(maximum).map_err(|_| {
            StorageError::OperationRejected("fixture pool capacity overflow".into())
        })?;
        let mut connections = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            connections.push(
                self.owner
                    .reader
                    .pool
                    .get_timeout(std::time::Duration::ZERO)
                    .map_err(|error| StorageError::Pool(error.to_string()))?,
            );
        }
        let guard = FeedPoolSlots {
            connections,
            capacity,
            _owner: self.owner.clone(),
        };
        self.verify_exhausted_zero_timeout()?;
        Ok(guard)
    }

    /// Blocking negative control: no idle slots and real zero-timeout checkout must fail.
    pub fn verify_exhausted_zero_timeout(&self) -> Result<(), StorageError> {
        let state = self.owner.reader.pool.state();
        if state.idle_connections != 0 || state.connections != self.owner.reader.pool.max_size() {
            return Err(StorageError::OperationRejected(
                "pool is not fully occupied".into(),
            ));
        }
        match self
            .owner
            .reader
            .pool
            .get_timeout(std::time::Duration::ZERO)
        {
            Err(_) => Ok(()),
            Ok(_) => Err(StorageError::OperationRejected(
                "exhausted pool unexpectedly allowed checkout".into(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "reader/feed_probe_tests.rs"]
mod feed_probe_tests;
