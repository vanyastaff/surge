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

/// A queued operator steer message awaiting delivery to the run's next agent
/// stage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QueuedSteer {
    /// Short unique id, shown by `surge steer --list` and used by `--cancel`.
    pub id: String,
    /// The operator's steer message.
    pub message: String,
}

/// Shared FIFO queue of pending steers for one run. Held by the engine's
/// `ActiveRun` (submit / list / cancel) and the run task (drain-at-boundary).
pub type SteerQueue = Arc<Mutex<Vec<QueuedSteer>>>;

/// Create an empty steer queue.
#[must_use]
pub fn new_queue() -> SteerQueue {
    Arc::new(Mutex::new(Vec::new()))
}

/// Mint a short, human-friendly steer id (the last 10 chars of a ULID).
#[must_use]
pub fn new_steer_id() -> String {
    let full = ulid::Ulid::new().to_string();
    full[full.len().saturating_sub(10)..].to_string()
}
