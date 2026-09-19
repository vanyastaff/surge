//! Run-level pause: halt a live run at its next stage boundary (ADR-0020 T14).
//!
//! Pausing a *project* (`surge project pause`) stops new dispatches. Pausing
//! a *task* whose run is already live needs something else: the run must stop
//! advancing at a safe boundary, without losing in-flight work and without
//! killing the agent mid-turn. ACP has no mid-turn injection channel, so the
//! run task checks this gate at the top of its loop — every graph stage is a
//! boundary — and waits there until resumed or cancelled.
//!
//! The gate is in-memory, shared between the engine's `ActiveRun` (operator
//! pause/unpause) and the run task (wait). Like the steer queue, an
//! undelivered pause does not survive a daemon restart: after a restart the
//! run is either recovered and running, or not running at all — there is no
//! paused-run concept in the registry, and inventing one would duplicate
//! `RunStatus::Parked`'s machine (which is for provider rate limits, with a
//! `wake_at` and a wake scheduler this feature does not want).
//!
//! Resuming wakes the run task with `Notify`; a wait that loses the race to
//! the gate staying paused simply loops. Cancellation wins over pause: an
//! operator who stops a paused run gets the stop, not a hang.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Shared pause state for one run.
#[derive(Debug)]
pub struct PauseGate {
    paused: AtomicBool,
    resumed: Notify,
}

/// Shared handle to a run's [`PauseGate`].
pub type PauseHandle = Arc<PauseGate>;

/// Create a run's pause gate, initially unpaused.
#[must_use]
pub fn new_gate() -> PauseHandle {
    Arc::new(PauseGate {
        paused: AtomicBool::new(false),
        resumed: Notify::new(),
    })
}

impl PauseGate {
    /// Pause the run. Idempotent.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume the run and wake a waiting run task. Idempotent.
    pub fn unpause(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resumed.notify_waiters();
    }

    /// Whether the gate is currently paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Wait while paused, returning early if `cancel` fires.
    ///
    /// Called by the run task at the top of its stage loop. Returns `true`
    /// when this call actually waited (the run was paused at entry), so the
    /// caller can log the boundary it stopped at; `false` when the gate was
    /// open and the run continues immediately.
    pub async fn wait_if_paused(&self, cancel: &CancellationToken) -> bool {
        if !self.is_paused() {
            return false;
        }
        // `Notify::notified()` is created *before* the re-check so a resume
        // that lands between the check and the await is not lost.
        loop {
            let notified = self.resumed.notified();
            if !self.is_paused() {
                return true;
            }
            tokio::select! {
                () = cancel.cancelled() => return true,
                () = notified => {},
            }
            if cancel.is_cancelled() {
                return true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_gate_does_not_wait() {
        let gate = new_gate();
        let cancel = CancellationToken::new();
        assert!(!gate.wait_if_paused(&cancel).await);
    }

    #[tokio::test]
    async fn paused_gate_waits_until_resumed() {
        let gate = new_gate();
        let cancel = CancellationToken::new();
        gate.pause();

        let waiter = {
            let gate = gate.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { gate.wait_if_paused(&cancel).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "the run task must be parked at the boundary"
        );

        gate.unpause();
        let did_wait = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("resume must wake the waiter")
            .unwrap();
        assert!(did_wait, "the call waited while paused");
        assert!(!gate.is_paused());
    }

    #[tokio::test]
    async fn cancellation_wins_over_pause() {
        let gate = new_gate();
        let cancel = CancellationToken::new();
        gate.pause();

        let waiter = {
            let gate = gate.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { gate.wait_if_paused(&cancel).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("cancel must wake the waiter")
            .unwrap();
    }

    #[tokio::test]
    async fn pause_is_idempotent_and_resume_wakes_every_waiter() {
        let gate = new_gate();
        let a = CancellationToken::new();
        let b = CancellationToken::new();
        gate.pause();
        gate.pause();
        let wa = {
            let gate = gate.clone();
            let a = a.clone();
            tokio::spawn(async move { gate.wait_if_paused(&a).await })
        };
        let wb = {
            let gate = gate.clone();
            let b = b.clone();
            tokio::spawn(async move { gate.wait_if_paused(&b).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        gate.unpause();
        gate.unpause();
        for w in [wa, wb] {
            tokio::time::timeout(std::time::Duration::from_secs(2), w)
                .await
                .expect("resume wakes every waiter")
                .unwrap();
        }
    }
}
