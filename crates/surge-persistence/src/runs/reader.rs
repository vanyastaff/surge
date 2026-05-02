//! Read-only handle for a per-run database.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use surge_core::{RunId, VersionedEventPayload};

use crate::runs::error::StorageError;
use crate::runs::seq::EventSeq;

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
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Worktree directory for this run.
    pub fn worktree_path(&self) -> &Path {
        &self.worktree_path
    }

    /// Latest event seq written so far. Returns `EventSeq::ZERO` if the table is empty.
    pub async fn current_seq(&self) -> Result<EventSeq, StorageError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || -> Result<EventSeq, StorageError> {
            let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
            let max: Option<i64> = conn.query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
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
                }
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
            let iter = stmt.query_map(
                params![range.start.0 as i64, range.end.0 as i64],
                |row| {
                    let blob: Vec<u8> = row.get(3)?;
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        blob,
                    ))
                },
            )?;
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
}
