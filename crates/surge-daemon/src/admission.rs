//! `AdmissionController` — caps concurrent runs hosted by the daemon.
//! FIFO when over capacity. No aging, no preemption (M8 if needed).

use std::collections::{HashSet, VecDeque};
use surge_core::id::RunId;
use tokio::sync::Mutex;

/// Decision returned by `try_admit`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Run admitted; daemon may proceed to call `engine.start_run`.
    Admitted,
    /// Run queued at the given 0-based position. Daemon should send
    /// `StartRunQueued` to the client and admit it later when
    /// `notify_completed` makes a slot.
    Queued {
        /// Zero-based position in the queue at the moment of the call.
        position: usize,
    },
    /// Both `active` and `queue` are at their respective caps; the run
    /// was neither admitted nor queued. Daemon should reply with an
    /// `Error { code: QueueFull }` so the client can back off and
    /// retry instead of having an unbounded amount of work piled up
    /// inside the daemon process.
    QueueFull {
        /// Current queue length at the moment of the rejection.
        queue_len: usize,
        /// Configured queue cap (the value `try_admit` was about to exceed).
        max_queue: usize,
    },
}

/// Concurrent admission policy.
pub struct AdmissionController {
    inner: Mutex<Inner>,
    notify: tokio::sync::Notify,
    max_active: usize,
    max_queue: usize,
}

struct Inner {
    active: HashSet<RunId>,
    queue: VecDeque<RunId>,
}

impl AdmissionController {
    /// Construct with a hard cap on concurrent active runs and on the
    /// FIFO admission queue. When both are saturated, [`Self::try_admit`]
    /// returns [`AdmissionDecision::QueueFull`] instead of growing the
    /// queue without bound.
    #[must_use]
    pub fn new(max_active: usize, max_queue: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                active: HashSet::new(),
                queue: VecDeque::new(),
            }),
            notify: tokio::sync::Notify::new(),
            max_active,
            max_queue,
        }
    }

    /// Attempt to admit a run. Returns [`AdmissionDecision::Admitted`]
    /// if a slot was free, [`AdmissionDecision::Queued`] if the run
    /// joined the FIFO queue, or [`AdmissionDecision::QueueFull`] if
    /// both the active set and the queue are at their respective caps.
    pub async fn try_admit(&self, run_id: RunId) -> AdmissionDecision {
        let mut inner = self.inner.lock().await;
        if inner.active.len() < self.max_active {
            inner.active.insert(run_id);
            AdmissionDecision::Admitted
        } else if inner.queue.len() >= self.max_queue {
            AdmissionDecision::QueueFull {
                queue_len: inner.queue.len(),
                max_queue: self.max_queue,
            }
        } else {
            inner.queue.push_back(run_id);
            AdmissionDecision::Queued {
                position: inner.queue.len() - 1,
            }
        }
    }

    /// Mark a run as finished. Frees its slot and wakes any waiter
    /// blocked on [`AdmissionController::wait_changed`].
    pub async fn notify_completed(&self, run_id: RunId) {
        let mut inner = self.inner.lock().await;
        inner.active.remove(&run_id);
        self.notify.notify_waiters();
    }

    /// If a slot is free and a run is queued, dequeue + admit it.
    /// Returns the admitted [`RunId`].
    pub async fn pop_queued(&self) -> Option<RunId> {
        let mut inner = self.inner.lock().await;
        if inner.active.len() < self.max_active {
            if let Some(id) = inner.queue.pop_front() {
                inner.active.insert(id);
                return Some(id);
            }
        }
        None
    }

    /// Like [`Self::try_admit`], but rejects (returns `false`) instead of
    /// queueing when the cap is hit. Used by operations like
    /// `resume_run` that don't want to be deferred — the caller
    /// should propagate an error to the client and let them retry.
    pub async fn try_admit_no_queue(&self, run_id: RunId) -> bool {
        let mut inner = self.inner.lock().await;
        if inner.active.len() < self.max_active {
            inner.active.insert(run_id);
            true
        } else {
            false
        }
    }

    /// Snapshot counts for `surge daemon status`.
    pub async fn snapshot(&self) -> AdmissionSnapshot {
        let inner = self.inner.lock().await;
        AdmissionSnapshot {
            active: inner.active.len(),
            max_active: self.max_active,
            queued: inner.queue.len(),
            max_queue: self.max_queue,
        }
    }

    /// Block until something changes (a slot frees, a queue empties).
    /// Useful for the server loop's "drain queue" task.
    pub async fn wait_changed(&self) {
        self.notify.notified().await;
    }
}

/// Lightweight snapshot of admission counts; surfaced via
/// `surge daemon status`.
#[derive(Debug, Clone, Copy)]
pub struct AdmissionSnapshot {
    /// Currently-active runs.
    pub active: usize,
    /// Configured cap on concurrent active runs.
    pub max_active: usize,
    /// Runs currently waiting in the FIFO queue.
    pub queued: usize,
    /// Configured cap on the FIFO queue length.
    pub max_queue: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn under_cap_admits() {
        let a = AdmissionController::new(2, 4);
        let r1 = RunId::new();
        let r2 = RunId::new();
        assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
        assert_eq!(a.try_admit(r2).await, AdmissionDecision::Admitted);
    }

    #[tokio::test]
    async fn over_cap_queues_in_fifo() {
        let a = AdmissionController::new(1, 4);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let r3 = RunId::new();
        assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
        assert_eq!(
            a.try_admit(r2).await,
            AdmissionDecision::Queued { position: 0 }
        );
        assert_eq!(
            a.try_admit(r3).await,
            AdmissionDecision::Queued { position: 1 }
        );
    }

    #[tokio::test]
    async fn complete_then_pop_admits_next() {
        let a = AdmissionController::new(1, 4);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let _ = a.try_admit(r1).await;
        let _ = a.try_admit(r2).await; // queued at 0
        assert!(a.pop_queued().await.is_none()); // still no slot
        a.notify_completed(r1).await;
        let popped = a.pop_queued().await;
        assert_eq!(popped, Some(r2));
    }

    #[tokio::test]
    async fn try_admit_no_queue_rejects_at_cap() {
        let a = AdmissionController::new(1, 4);
        let r1 = RunId::new();
        let r2 = RunId::new();
        assert!(a.try_admit_no_queue(r1).await);
        assert!(!a.try_admit_no_queue(r2).await);
        // No queueing happened — snapshot.queued should be 0.
        let s = a.snapshot().await;
        assert_eq!(s.queued, 0);
    }

    #[tokio::test]
    async fn snapshot_counts() {
        let a = AdmissionController::new(2, 4);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let r3 = RunId::new();
        let _ = a.try_admit(r1).await;
        let _ = a.try_admit(r2).await;
        let _ = a.try_admit(r3).await;
        let s = a.snapshot().await;
        assert_eq!(s.active, 2);
        assert_eq!(s.max_active, 2);
        assert_eq!(s.queued, 1);
        assert_eq!(s.max_queue, 4);
    }

    #[tokio::test]
    async fn rejects_with_queue_full_when_both_caps_hit() {
        // max_active=1, max_queue=1: first admits, second queues, third
        // is rejected as QueueFull. The rejected run is NOT enqueued,
        // so the queue length stays at the cap, not above it.
        let a = AdmissionController::new(1, 1);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let r3 = RunId::new();
        assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
        assert_eq!(
            a.try_admit(r2).await,
            AdmissionDecision::Queued { position: 0 }
        );
        assert_eq!(
            a.try_admit(r3).await,
            AdmissionDecision::QueueFull {
                queue_len: 1,
                max_queue: 1,
            }
        );
        let s = a.snapshot().await;
        assert_eq!(s.active, 1);
        assert_eq!(s.queued, 1, "queue must not grow past max_queue");
    }

    #[tokio::test]
    async fn queue_full_does_not_block_admission_after_drain() {
        // After the queued run is drained, the previously-rejected run
        // can be re-admitted — the rejection is not sticky.
        let a = AdmissionController::new(1, 1);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let r3 = RunId::new();
        let _ = a.try_admit(r1).await;
        let _ = a.try_admit(r2).await;
        // r3 is rejected: queue is at cap.
        assert!(matches!(
            a.try_admit(r3).await,
            AdmissionDecision::QueueFull { .. }
        ));
        // Free a slot, drain r2 from the queue, then retrying r3 should
        // queue (queue back to empty, active back at cap from r2).
        a.notify_completed(r1).await;
        assert_eq!(a.pop_queued().await, Some(r2));
        assert_eq!(
            a.try_admit(r3).await,
            AdmissionDecision::Queued { position: 0 }
        );
    }
}
