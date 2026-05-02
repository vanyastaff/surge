//! Engine-side materialized view maintenance.
//!
//! Called from the writer task inside the same transaction as the event INSERT.
//! Each `EventPayload` variant updates the affected view tables.
//!
//! Phase 5 ships a stub that no-ops on every variant; Phase 6 fills in the
//! actual SQL per-variant in one task (single big match).

use rusqlite::Transaction;
use surge_core::run_event::EventPayload;

use crate::runs::error::WriterError;
use crate::runs::seq::EventSeq;

/// Update materialized view tables in response to a single event being appended.
///
/// Runs inside the same transaction as the originating `INSERT INTO events`.
/// Phase 5 stub — Phase 6 implements per-variant maintenance.
#[allow(clippy::needless_pass_by_value)]
#[allow(unused_variables)]
pub fn maintain(
    tx: &Transaction<'_>,
    seq: EventSeq,
    timestamp_ms: i64,
    payload: &EventPayload,
) -> Result<(), WriterError> {
    // Phase 5 stub — Phase 6 implements per-variant maintenance.
    let _ = (tx, seq, timestamp_ms, payload);
    Ok(())
}

/// Truncate all materialized view tables. Called at the start of `RebuildViews`.
pub fn rebuild(tx: &Transaction<'_>) -> Result<(), WriterError> {
    tx.execute_batch(
        "DELETE FROM stage_executions;
         DELETE FROM artifacts;
         DELETE FROM pending_approvals;
         DELETE FROM cost_summary;
         DELETE FROM graph_snapshots;",
    )?;
    Ok(())
}
