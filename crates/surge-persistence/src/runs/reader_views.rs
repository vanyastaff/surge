//! Synchronous SQL adapters for materialized view tables.
//!
//! Called from `RunReader` async methods through `spawn_blocking`.

use std::path::PathBuf;
use std::str::FromStr;

use r2d2::PooledConnection;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use surge_core::{ContentHash, NodeKey};

use crate::runs::error::StorageError;
use crate::runs::seq::EventSeq;
use crate::runs::types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};

/// Read all rows of the `stage_executions` view ordered by `started_seq`.
pub fn stage_executions(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<StageExecution>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT node_id, attempt, started_seq, ended_seq, started_at, ended_at,
                outcome, cost_usd, tokens_in, tokens_out
         FROM stage_executions ORDER BY started_seq",
    )?;
    let iter = stmt.query_map([], |row| {
        let node_str: String = row.get(0)?;
        let node = NodeKey::from_str(&node_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(StageExecution {
            node_id: node,
            attempt: row.get::<_, i64>(1)? as u32,
            started_seq: EventSeq(row.get::<_, i64>(2)? as u64),
            ended_seq: row.get::<_, Option<i64>>(3)?.map(|v| EventSeq(v as u64)),
            started_at_ms: row.get(4)?,
            ended_at_ms: row.get(5)?,
            outcome: row.get(6)?,
            cost_usd: row.get(7)?,
            tokens_in: row.get::<_, i64>(8)? as u64,
            tokens_out: row.get::<_, i64>(9)? as u64,
        })
    })?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

/// Read all rows of the `artifacts` view ordered by `produced_at_seq`.
pub fn artifacts(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<ArtifactRecord>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash
         FROM artifacts ORDER BY produced_at_seq",
    )?;
    let iter = stmt.query_map([], |row| {
        let id_str: String = row.get(0)?;
        let id = ContentHash::from_str(&id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let hash_str: String = row.get(6)?;
        let content_hash = ContentHash::from_str(&hash_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let produced_by_node = match row.get::<_, Option<String>>(1)? {
            Some(s) => Some(NodeKey::from_str(&s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?),
            None => None,
        };
        Ok(ArtifactRecord {
            id,
            produced_by_node,
            produced_at_seq: EventSeq(row.get::<_, i64>(2)? as u64),
            name: row.get(3)?,
            path: PathBuf::from(row.get::<_, String>(4)?),
            size_bytes: row.get::<_, i64>(5)? as u64,
            content_hash,
        })
    })?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

/// Read all rows of the `pending_approvals` view ordered by `seq`.
pub fn pending_approvals(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<PendingApproval>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT seq, node_id, channel, requested_at, payload_hash, delivered, message_id
         FROM pending_approvals ORDER BY seq",
    )?;
    let iter = stmt.query_map([], |row| {
        let node_str: String = row.get(1)?;
        let node = NodeKey::from_str(&node_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(PendingApproval {
            seq: EventSeq(row.get::<_, i64>(0)? as u64),
            node_id: node,
            channel: row.get(2)?,
            requested_at_ms: row.get(3)?,
            payload_hash: row.get(4)?,
            delivered: row.get::<_, i64>(5)? != 0,
            message_id: row.get(6)?,
        })
    })?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

/// Aggregate the `cost_summary` view into a single `CostSummary`.
pub fn cost_summary(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<CostSummary, StorageError> {
    let mut summary = CostSummary::default();
    let mut stmt = conn.prepare("SELECT metric, value FROM cost_summary")?;
    let iter = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    for r in iter {
        let (metric, value) = r?;
        match metric.as_str() {
            "tokens_in" => summary.tokens_in = value as u64,
            "tokens_out" => summary.tokens_out = value as u64,
            "cache_hits" => summary.cache_hits = value as u64,
            "cost_usd" => summary.cost_usd = value,
            _ => {},
        }
    }
    Ok(summary)
}

/// List the seqs of all written graph snapshots.
pub fn list_snapshots(
    conn: &PooledConnection<SqliteConnectionManager>,
) -> Result<Vec<EventSeq>, StorageError> {
    let mut stmt = conn.prepare("SELECT at_seq FROM graph_snapshots ORDER BY at_seq")?;
    let iter = stmt.query_map([], |row| Ok(EventSeq(row.get::<_, i64>(0)? as u64)))?;
    iter.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

/// Find the most recent snapshot at or before `seq`, returning the seq and raw blob.
pub fn latest_snapshot_at_or_before(
    conn: &PooledConnection<SqliteConnectionManager>,
    seq: EventSeq,
) -> Result<Option<(EventSeq, Vec<u8>)>, StorageError> {
    match conn.query_row(
        "SELECT at_seq, snapshot FROM graph_snapshots WHERE at_seq <= ? ORDER BY at_seq DESC LIMIT 1",
        params![seq.0 as i64],
        |row| Ok((EventSeq(row.get::<_, i64>(0)? as u64), row.get::<_, Vec<u8>>(1)?)),
    ) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
