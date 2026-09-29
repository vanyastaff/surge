//! Queueing a steer message on a run hosted by the daemon.

use surge_core::RunId;

use crate::engine::daemon_facade::DaemonEngineFacade;
use crate::engine::facade::EngineFacade;
use crate::operator::error::OperatorError;

/// Queue `message` as a steer on `run_id` and return the steer id.
///
/// # Errors
/// Returns [`OperatorError::BlankSteer`] if `message` is blank and
/// [`OperatorError::SteerFailed`] if the daemon rejects the steer (run not
/// active in that daemon).
pub async fn queue_steer(
    daemon: &DaemonEngineFacade,
    run_id: RunId,
    message: &str,
) -> Result<String, OperatorError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(OperatorError::BlankSteer);
    }
    daemon
        .submit_steer(run_id, message.to_owned())
        .await
        .map_err(|cause| OperatorError::SteerFailed { cause })
}
