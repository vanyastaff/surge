//! Read-only handle for a per-run database.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use surge_core::{ContentHash, RunId, VersionedEventPayload};

use crate::runs::error::StorageError;
use crate::runs::reader_views as views;
use crate::runs::seq::EventSeq;
use crate::runs::types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};

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
                "SELECT seq, timestamp, kind, payload FROM events WHERE seq = ?",
                params![seq.0 as i64],
                |row| {
                    let blob: Vec<u8> = row.get(3)?;
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        blob,
                    ))
                },
            );
            match row {
                Ok((seq, ts, kind, blob)) => {
                    let payload: VersionedEventPayload = serde_json::from_slice(&blob)?;
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
                "SELECT seq, timestamp, kind, payload
                 FROM events WHERE seq >= ? AND seq < ? ORDER BY seq",
            )?;
            let iter =
                stmt.query_map(params![range.start.0 as i64, range.end.0 as i64], |row| {
                    let blob: Vec<u8> = row.get(3)?;
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        blob,
                    ))
                })?;
            let mut out = Vec::new();
            for r in iter {
                let (seq, ts, kind, blob) = r?;
                let payload: VersionedEventPayload = serde_json::from_slice(&blob)?;
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
