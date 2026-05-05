//! `AdmissionController` — caps concurrent runs hosted by the daemon.
//! FIFO when over capacity. No aging, no preemption (M8 if needed).

use std::collections::{HashSet, VecDeque};
use surge_core::id::RunId;
use tokio::sync::Mutex;

/// Decision returned by `try_admit`.
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
}

/// Concurrent admission policy.
pub struct AdmissionController {
    inner: Mutex<Inner>,
    notify: tokio::sync::Notify,
    max_active: usize,
}

struct Inner {
    active: HashSet<RunId>,
    queue: VecDeque<RunId>,
}

impl AdmissionController {
    /// Construct with a hard cap on concurrent active runs.
    #[must_use]
    pub fn new(max_active: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                active: HashSet::new(),
                queue: VecDeque::new(),
            }),
            notify: tokio::sync::Notify::new(),
            max_active,
        }
    }

    /// Attempt to admit a run. Returns [`AdmissionDecision::Admitted`]
    /// if a slot was free or [`AdmissionDecision::Queued`] if the run
    /// joined the FIFO queue.
    pub async fn try_admit(&self, run_id: RunId) -> AdmissionDecision {
        let mut inner = self.inner.lock().await;
        if inner.active.len() < self.max_active {
            inner.active.insert(run_id);
            AdmissionDecision::Admitted
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

    /// Like [`try_admit`], but rejects (returns `false`) instead of
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn under_cap_admits() {
        let a = AdmissionController::new(2);
        let r1 = RunId::new();
        let r2 = RunId::new();
        assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
        assert_eq!(a.try_admit(r2).await, AdmissionDecision::Admitted);
    }

    #[tokio::test]
    async fn over_cap_queues_in_fifo() {
        let a = AdmissionController::new(1);
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
        let a = AdmissionController::new(1);
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
        let a = AdmissionController::new(1);
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
        let a = AdmissionController::new(2);
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
    }
}
