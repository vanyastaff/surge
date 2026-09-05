//! Operator steering — queued, non-destructive (Phase 2 B2).
//!
//! ACP v1 has no mid-turn injection channel: a `session/prompt` turn runs until
//! the agent returns a `StopReason`, and a second concurrent prompt is
//! unspecified (real agents error). So Surge steers at the **next stage
//! boundary**: `surge steer` enqueues a message here, the run task drains the
//! queue before it opens the next agent stage, and the message is prepended to
//! that stage's prompt (recorded as a `SteerDelivered` event).
//!
//! The queue is in-memory (held by the engine's `ActiveRun` and shared with the
//! run task). A daemon restart before delivery drops undelivered steers — the
//! operator re-issues after restart. Delivery is persisted as an event; the
//! queue itself is not.

use std::sync::Arc;

use tokio::sync::Mutex;

/// Maximum byte length of a single steer message. Guards against an oversize
/// message blowing up the next agent prompt (the CLI also checks, but the
/// `SubmitSteer` IPC reaches the engine directly).
pub const MAX_STEER_LEN: usize = 4096;

/// Maximum number of undelivered steers a run may hold. A run parked at a
/// non-agent node never drains, so an uncapped queue would grow without bound
/// in daemon memory.
pub const MAX_QUEUED_STEERS: usize = 32;

/// A queued operator steer message awaiting delivery to the run's next agent
/// stage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QueuedSteer {
    /// Short unique id, shown by `surge steer --list` and used by `--cancel`.
    pub id: String,
    /// The operator's steer message.
    pub message: String,
}

/// Steer state for one run: the pending (undelivered) queue plus the ids the
/// operator has cancelled. The cancelled set tombstones ids so that a steer
/// cancelled while it is already in-flight (drained into a stage) is not
/// resurrected by the run task's re-queue-on-error path.
#[derive(Debug, Default)]
pub struct SteerState {
    /// Undelivered steers in FIFO order.
    pub pending: Vec<QueuedSteer>,
    /// Ids the operator cancelled; delivery/re-queue must skip these.
    pub cancelled: std::collections::HashSet<String>,
}

/// Shared steer state for one run. Held by the engine's `ActiveRun` (submit /
/// list / cancel) and the run task (drain-at-boundary, re-queue-on-error).
pub type SteerQueue = Arc<Mutex<SteerState>>;

/// Create an empty steer queue.
#[must_use]
pub fn new_queue() -> SteerQueue {
    Arc::new(Mutex::new(SteerState::default()))
}

/// Mint a short, human-friendly steer id (the last 10 chars of a ULID).
#[must_use]
pub fn new_steer_id() -> String {
    let full = ulid::Ulid::new().to_string();
    full[full.len().saturating_sub(10)..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_id_is_dropped_by_the_requeue_filter() {
        // Mirrors run_task's re-queue-on-error: a steer cancelled while it was
        // in-flight (its id tombstoned in `cancelled`) must NOT be restored to
        // the pending queue, so the cancel isn't reversed.
        let mut state = SteerState::default();
        state.cancelled.insert("s1".to_owned());
        let in_flight_backup = vec![
            QueuedSteer {
                id: "s1".to_owned(),
                message: "cancelled".to_owned(),
            },
            QueuedSteer {
                id: "s2".to_owned(),
                message: "keep".to_owned(),
            },
        ];
        let restored: Vec<_> = in_flight_backup
            .into_iter()
            .filter(|steer| !state.cancelled.contains(&steer.id))
            .collect();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, "s2");
    }
}
