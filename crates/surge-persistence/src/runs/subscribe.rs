//! Polling-based event subscription stream.
//!
//! Subscriber polls `events` table every 100 ms and yields new rows since the
//! last seen `seq`. Per-tick batch capped at `SUBSCRIBE_BATCH_MAX` to bound
//! memory if the consumer lags behind the writer.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::SqliteConnectionManager;
use async_stream::try_stream;
use futures_core::Stream;
use r2d2::Pool;
use rusqlite::params;
use surge_core::VersionedEventPayload;
use tokio::time::{MissedTickBehavior, interval};

use crate::runs::error::StorageError;
use crate::runs::reader::ReadEvent;
use crate::runs::seq::EventSeq;

/// Maximum events yielded per polling tick. Bounds memory if consumer lags.
pub const SUBSCRIBE_BATCH_MAX: usize = 256;

/// Polling interval in ms. 100 ms is below human perception threshold.
pub const POLL_INTERVAL_MS: u64 = 100;

/// Build the polling-based event stream.
///
/// `_artifacts_dir` is reserved for future per-event artifact lookups; not
/// used in M2.
pub fn subscribe(
    pool: Pool<SqliteConnectionManager>,
    _artifacts_dir: Arc<PathBuf>,
) -> impl Stream<Item = Result<ReadEvent, StorageError>> + Send + 'static {
    try_stream! {
        let mut last_seq = EventSeq::ZERO;
        let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let pool_clone = pool.clone();
            let next_batch = tokio::task::spawn_blocking(move || -> Result<Vec<ReadEvent>, StorageError> {
                read_batch(&pool_clone, last_seq, EventSeq(i64::MAX as u64))
            })
            .await
            .map_err(|e| StorageError::Pool(e.to_string()))??;

            for ev in next_batch {
                last_seq = ev.seq;
                yield ev;
            }
        }
    }
}

/// One complete synchronous SQL batch, shared without changing public stream lifetime.
pub(crate) fn read_batch(
    pool: &Pool<SqliteConnectionManager>,
    after: EventSeq,
    through: EventSeq,
) -> Result<Vec<ReadEvent>, StorageError> {
    let conn = pool
        .get()
        .map_err(|error| StorageError::Pool(error.to_string()))?;
    read_rows(&conn, after, through)
}

pub(crate) fn read_rows(
    conn: &rusqlite::Connection,
    after: EventSeq,
    through: EventSeq,
) -> Result<Vec<ReadEvent>, StorageError> {
    let lower = i64::try_from(after.0).map_err(|_| StorageError::InvalidEventSequence)?;
    let upper = i64::try_from(through.0).map_err(|_| StorageError::InvalidEventSequence)?;
    if lower > upper {
        return Err(StorageError::InvalidEventSequence);
    }
    let mut statement = conn.prepare(
        "SELECT seq, timestamp, kind, payload FROM events
         WHERE seq > ? AND seq <= ? ORDER BY seq LIMIT ?",
    )?;
    let rows = statement.query_map(params![lower, upper, SUBSCRIBE_BATCH_MAX as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (seq, timestamp_ms, kind, blob) = row?;
        if seq <= 0 {
            return Err(StorageError::InvalidEventSequence);
        }
        let seq = EventSeq(u64::try_from(seq).map_err(|_| StorageError::InvalidEventSequence)?);
        let payload: VersionedEventPayload = serde_json::from_slice(&blob)?;
        result.push(ReadEvent {
            seq,
            timestamp_ms,
            kind,
            payload,
        });
    }
    Ok(result)
}
