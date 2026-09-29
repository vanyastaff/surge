//! Queueing a steer message on a run hosted by the daemon.

use surge_core::RunId;

use crate::engine::daemon_facade::DaemonEngineFacade;
use crate::engine::facade::EngineFacade;
use crate::engine::steer::QueuedSteer;
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

/// The steer messages currently queued for `run_id`, oldest first.
///
/// # Errors
/// Returns [`OperatorError::SteerFailed`] if the daemon does not host the run.
pub async fn list_steers(
    daemon: &DaemonEngineFacade,
    run_id: RunId,
) -> Result<Vec<QueuedSteer>, OperatorError> {
    daemon
        .list_steers(run_id)
        .await
        .map_err(|cause| OperatorError::SteerFailed { cause })
}

/// Drop a queued steer by id. Returns whether it was still pending; `false`
/// means it was already delivered or in flight (the cancel is still recorded,
/// so a retry will not re-deliver it).
///
/// # Errors
/// Returns [`OperatorError::SteerFailed`] if the daemon does not host the run.
pub async fn cancel_steer(
    daemon: &DaemonEngineFacade,
    run_id: RunId,
    steer_id: &str,
) -> Result<bool, OperatorError> {
    daemon
        .cancel_steer(run_id, steer_id.to_owned())
        .await
        .map_err(|cause| OperatorError::SteerFailed { cause })
}
