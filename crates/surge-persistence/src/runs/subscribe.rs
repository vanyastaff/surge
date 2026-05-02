//! Polling-based event subscription stream.
//!
//! Subscriber polls `events` table every 100 ms and yields new rows since the
//! last seen `seq`. Per-tick batch capped at `SUBSCRIBE_BATCH_MAX` to bound
//! memory if the consumer lags behind the writer.

use std::sync::Arc;
use std::path::PathBuf;
use std::time::Duration;

use async_stream::try_stream;
use futures_core::Stream;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use surge_core::VersionedEventPayload;
use tokio::time::{interval, MissedTickBehavior};

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
                let conn = pool_clone.get().map_err(|e| StorageError::Pool(e.to_string()))?;
                let mut stmt = conn.prepare(
                    "SELECT seq, timestamp, kind, payload
                     FROM events WHERE seq > ?
                     ORDER BY seq LIMIT ?",
                )?;
                let iter = stmt.query_map(
                    params![last_seq.0 as i64, SUBSCRIBE_BATCH_MAX as i64],
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
                    out.push(ReadEvent { seq, timestamp_ms: ts, kind, payload });
                }
                Ok(out)
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
