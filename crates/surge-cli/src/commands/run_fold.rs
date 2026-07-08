//! Shared helper: read a run's full event log and fold it into the
//! authoritative `RunState` (blocked-on-human state included). Used by
//! `surge inbox` and `surge resolve`.

use anyhow::{Context, Result, anyhow};
use surge_core::RunId;
use surge_core::run_event::RunEvent;
use surge_core::run_state::{RunState, fold};
use surge_persistence::runs::RunReader;
use surge_persistence::runs::seq::EventSeq;

/// Read the full event log for `run_id` via `reader` and fold it into a
/// `RunState`. The fold ignores event timestamps, so the ms→`DateTime`
/// conversion here is lossy-safe.
pub(crate) async fn fold_run_state(reader: &RunReader, run_id: RunId) -> Result<RunState> {
    let events = reader
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .with_context(|| format!("read events for {run_id}"))?;
    let run_events: Vec<RunEvent> = events
        .into_iter()
        .map(|read| RunEvent {
            run_id,
            seq: read.seq.0,
            timestamp: chrono::DateTime::from_timestamp_millis(read.timestamp_ms)
                .unwrap_or_default(),
            payload: read.payload.payload,
        })
        .collect();
    fold(&run_events).map_err(|e| anyhow!("fold run {run_id}: {e}"))
}
