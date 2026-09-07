//! Shared helper: read a run's full event log, either as raw
//! `surge_core::RunEvent`s or folded into the authoritative `RunState`
//! (blocked-on-human state included). Used by `surge inbox`, `surge
//! resolve`, and `surge run report`.

use anyhow::{Context, Result, anyhow};
use surge_core::RunId;
use surge_core::run_event::RunEvent;
use surge_core::run_state::{RunState, fold};
use surge_persistence::runs::RunReader;
use surge_persistence::runs::seq::EventSeq;

/// Read the full event log for `run_id` via `reader`, converted to
/// `surge_core::RunEvent` — the one place `surge-persistence`'s
/// `EventSeq`/`timestamp_ms` shape gets turned into `surge-core`'s own event
/// type, so every caller that needs a plain `Vec<RunEvent>` (a pure
/// `surge-core` compiler, e.g. `RunReport::compile`, cannot depend on
/// `surge-persistence`'s `ReadEvent` at all) shares this one conversion
/// instead of re-deriving it.
pub(crate) async fn read_run_events(reader: &RunReader, run_id: RunId) -> Result<Vec<RunEvent>> {
    let events = reader
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .with_context(|| format!("read events for {run_id}"))?;
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

/// Read the full event log for `run_id` via `reader` and fold it into a
/// `RunState`. The fold ignores event timestamps, so the ms→`DateTime`
/// conversion `read_run_events` applies is lossy-safe.
pub(crate) async fn fold_run_state(reader: &RunReader, run_id: RunId) -> Result<RunState> {
    let run_events = read_run_events(reader, run_id).await?;
    fold(&run_events).map_err(|e| anyhow!("fold run {run_id}: {e}"))
}
