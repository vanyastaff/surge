//! Shared helper: read a run's full event log, either as raw
//! `surge_core::RunEvent`s or folded into the authoritative `RunState`
//! (blocked-on-human state included). Used by `surge inbox`, `surge
//! resolve`, and `surge run report`.

use anyhow::{Context, Result, anyhow};
use surge_core::RunId;
use surge_core::run_event::RunEvent;
use surge_core::run_state::{RunState, fold};
use surge_persistence::runs::RunReader;

/// Read the full event log via `reader`, converted to `surge_core::RunEvent`.
/// A thin, `anyhow`-wrapped alias for [`RunReader::read_run_events`] — that
/// method is this crate's actual home for the `surge-persistence` →
/// `surge-core` event conversion (also used by `surge-daemon`'s L3 merge
/// gate, spec §10/R31); kept here so this module's existing `pub(crate)` call
/// sites (`surge inbox`, `surge resolve`, `surge run report`) do not need to
/// change. Takes no separate `run_id` — `reader` already knows its own via
/// [`RunReader::run_id`], so there is nothing for a caller to keep in sync.
pub(crate) async fn read_run_events(reader: &RunReader) -> Result<Vec<RunEvent>> {
    reader
        .read_run_events()
        .await
        .with_context(|| format!("read events for {}", reader.run_id()))
}

/// Read the full event log for `run_id` via `reader` and fold it into a
/// `RunState`. The fold ignores event timestamps, so the ms→`DateTime`
/// conversion `read_run_events` applies is lossy-safe.
pub(crate) async fn fold_run_state(reader: &RunReader, run_id: RunId) -> Result<RunState> {
    let run_events = read_run_events(reader).await?;
    fold(&run_events).map_err(|e| anyhow!("fold run {run_id}: {e}"))
}
