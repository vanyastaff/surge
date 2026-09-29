//! Event-log reading and folding shared by every operator service.

use surge_core::RunId;
use surge_core::run_event::RunEvent;
use surge_core::run_state::{RunState, fold};
use surge_persistence::runs::RunReader;

use crate::operator::error::OperatorError;

/// Read the full event log via `reader`, converted to `surge_core::RunEvent`.
///
/// A typed alias for [`RunReader::read_run_events`]. Takes no separate run id
/// — `reader` already knows its own via [`RunReader::run_id`].
///
/// # Errors
/// Returns [`OperatorError::ReadEvents`] if the log cannot be read.
pub async fn read_run_events(reader: &RunReader) -> Result<Vec<RunEvent>, OperatorError> {
    reader
        .read_run_events()
        .await
        .map_err(|source| OperatorError::ReadEvents {
            run_id: *reader.run_id(),
            source,
        })
}

/// Read the full event log for `run_id` via `reader` and fold it into a
/// [`RunState`]. The fold ignores event timestamps, so the ms→`DateTime`
/// conversion `read_run_events` applies is lossy-safe.
///
/// # Errors
/// Returns [`OperatorError`] if the log cannot be read or does not fold.
pub async fn fold_run_state(reader: &RunReader, run_id: RunId) -> Result<RunState, OperatorError> {
    let run_events = read_run_events(reader).await?;
    fold(&run_events).map_err(|cause| OperatorError::Fold { run_id, cause })
}
