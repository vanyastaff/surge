//! Run-completion → tracker-comment hook (RFC-0010 acceptance criterion #5).
//!
//! Subscribes to the daemon's [`GlobalDaemonEvent`] stream and reacts to
//! [`GlobalDaemonEvent::RunFinished`]. For each finished run that originated
//! from an external task tracker (i.e., its `run_id` matches a row in the
//! `ticket_index` table), this module:
//!
//! 1. Looks up the originating ticket via [`IntakeRepo::lookup_ticket_by_run_id`].
//! 2. Posts a status comment to the tracker via [`TaskSource::post_comment`]:
//!    - `RunOutcome::Completed` → `"✅ Surge run completed (terminal node: <node>)."`
//!    - `RunOutcome::Failed`    → `"❌ Run failed: <error>"`
//!    - `RunOutcome::Aborted`   → `"Run aborted: <reason>"`
//!    - `RunOutcome::Parked`    → `"⏸ ... paused until <wake_at> ..."` (Task 12,
//!      R37/R37.1) — not a failure; the run resumes on its own.
//! 3. Transitions the ticket FSM (`Active`/`RunStarted` → `Completed` / `Failed`
//!    / `Aborted`). `Parked` is the one outcome that does **not** transition the
//!    FSM — no `TicketState` represents "paused, resumes on its own" today (see
//!    [`format_completion`]'s doc), so the ticket is left in whatever state it
//!    already had rather than mislabeled.
//!
//! Comment posting is best-effort: a failed `post_comment` is logged via
//! `tracing::warn` but the ticket FSM still transitions (when it transitions at
//! all) — the on-disk state is authoritative regardless of whether the tracker
//! received the cosmetic note.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::Connection;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::{IntakeRepo, TicketState};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Spawn the run-completion consumer. Returns the `JoinHandle` so the caller
/// can `abort()` on shutdown if needed.
///
/// Runs forever, draining `rx` until the broadcast channel closes (i.e., the
/// last `BroadcastRegistry` clone is dropped) or the process exits. Lagged
/// receives are logged and dropped: the ticket then stays stuck in `Active`
/// until manual intervention. RFC-0010 § Crash recovery designs a startup
/// sweep that would also catch this case, but that sweep is not implemented
/// yet — see the RFC for the planned reconciliation rules.
// `clippy::implicit_hasher`: the daemon owns the source map; consumers don't
// need to be generic over `BuildHasher`. The map uses the default
// `RandomState` everywhere, including the daemon's `main.rs` call site.
#[allow(clippy::implicit_hasher)]
pub fn spawn(
    rx: broadcast::Receiver<GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
) -> JoinHandle<()> {
    tokio::spawn(run(rx, source_map, conn))
}

async fn run(
    mut rx: broadcast::Receiver<GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
) {
    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Closed) => {
                info!("run-completion consumer: broadcast closed; exiting");
                return;
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    skipped = n,
                    "run-completion consumer lagged; some RunFinished events dropped"
                );
                continue;
            },
        };

        let GlobalDaemonEvent::RunFinished { run_id, outcome } = event else {
            continue;
        };

        handle_run_finished(&run_id.to_string(), &outcome, &source_map, &conn).await;
    }
}

async fn handle_run_finished(
    run_id_str: &str,
    outcome: &RunOutcome,
    source_map: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
) {
    let row = {
        let guard = conn.lock().await;
        IntakeRepo::new(&guard).lookup_ticket_by_run_id(run_id_str)
    };
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => {
            warn!(error = %e, run_id = %run_id_str, "lookup_ticket_by_run_id failed");
            return;
        },
    };

    let (body, new_state, purpose) = format_completion(outcome);

    // Best-effort comment post: a malformed `task_id` string in the DB row
    // (e.g., from a future migration that loosened validation, or manual
    // edits) skips the cosmetic tracker note but MUST NOT skip the FSM
    // transition below — the on-disk state is authoritative regardless.
    let task_id = match TaskId::try_new(row.task_id.clone()) {
        Ok(id) => Some(id),
        Err(e) => {
            warn!(error = %e, task_id = %row.task_id, "invalid task_id; skipping comment post");
            None
        },
    };

    if let Some(task_id) = task_id {
        if let Some(source) = source_map.get(&row.source_id) {
            match source.post_comment(&task_id, &body).await {
                Ok(()) => info!(
                    task_id = %row.task_id,
                    purpose = %purpose,
                    "posted run-completion comment"
                ),
                Err(e) => warn!(
                    error = %e,
                    task_id = %row.task_id,
                    "failed to post run-completion comment"
                ),
            }
        } else {
            warn!(
                source_id = %row.source_id,
                task_id = %row.task_id,
                "no source registered; cannot post run-completion comment"
            );
        }
    }

    if let Some(new_state) = new_state {
        let guard = conn.lock().await;
        if let Err(e) = IntakeRepo::new(&guard).update_state(&row.task_id, new_state) {
            warn!(error = %e, task_id = %row.task_id, "failed to update ticket state");
        }
    }
}

/// Map a finished run's outcome to a tracker comment, an optional FSM
/// transition, and a log `purpose` tag.
///
/// The FSM transition is `Option` for exactly one reason:
/// [`RunOutcome::Parked`] (Task 12, R37/R37.1) is not a failure — the run
/// paused on a provider rate-limit window and resumes on its own — but no
/// `TicketState` variant represents "paused, will resume on its own" today.
/// `TicketState::Snoozed` is *not* it: that is a different mechanism
/// (`SnoozeScheduler`, driven by a ticket-level `snooze_until` an
/// operator/automation set), not `RunStatus::Parked`/`wake_at` — the Task 12
/// plan names this exact non-overlap explicitly. Collapsing `Parked` onto
/// `Aborted` (this function's existing fallback for a genuinely unknown
/// future variant) would misrepresent the ticket the same way `Failed`
/// would — the run is not stopped. `None` here means "leave the ticket's
/// state exactly as it already is"; introducing a real variant for this is
/// an M4 decision, not this call site's.
fn format_completion(outcome: &RunOutcome) -> (String, Option<TicketState>, &'static str) {
    match outcome {
        RunOutcome::Completed { terminal } => (
            format!("✅ Surge run completed (terminal node: `{terminal}`)."),
            Some(TicketState::Completed),
            "run_completed",
        ),
        RunOutcome::Failed { error } => (
            format!("❌ Run failed: {error}"),
            Some(TicketState::Failed),
            "run_failed",
        ),
        RunOutcome::Aborted { reason } => (
            format!("Run aborted: {reason}"),
            Some(TicketState::Aborted),
            "run_aborted",
        ),
        RunOutcome::Parked { wake_at } => (
            format!(
                "⏸ Surge run paused until {wake_at} (provider rate limit reached); it will \
                 resume automatically once the window resets."
            ),
            None,
            "run_parked",
        ),
        // Policy, stated once, shared with `inbox::state_sync`'s identical
        // wildcard (Task 12 M3 review, "two tails now diverge"): a
        // genuinely unrecognized future `RunOutcome` variant is treated
        // the same way `Parked` is above — `None`, leaving the ticket's
        // state untouched, rather than guessing a terminal one. This
        // replaces an earlier choice (Aborted-shaped messaging "so the
        // ticket FSM still progresses to a terminal state instead of
        // staying Active forever") that the `Parked` arm just above
        // already contradicts: guessing `Aborted` for a fact this
        // function has no way to know is exactly the same wrong-direction
        // mistake `Parked`'s own fix exists to avoid, and leaving a ticket
        // stuck `Active` a little longer is the lesser cost of the two.
        _ => (
            "Run finished with an outcome this build does not recognize yet.".to_string(),
            None,
            "run_unknown",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::keys::NodeKey;

    #[test]
    fn format_completion_completed_phrasing() {
        let outcome = RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        };
        let (body, state, purpose) = format_completion(&outcome);
        assert!(body.starts_with("✅"));
        assert!(body.contains("end"));
        assert_eq!(state, Some(TicketState::Completed));
        assert_eq!(purpose, "run_completed");
    }

    #[test]
    fn format_completion_failed_phrasing() {
        let outcome = RunOutcome::Failed {
            error: "graph validation".into(),
        };
        let (body, state, purpose) = format_completion(&outcome);
        assert!(body.starts_with("❌ Run failed:"));
        assert!(body.contains("graph validation"));
        assert_eq!(state, Some(TicketState::Failed));
        assert_eq!(purpose, "run_failed");
    }

    #[test]
    fn format_completion_aborted_phrasing() {
        let outcome = RunOutcome::Aborted {
            reason: "user pressed Stop".into(),
        };
        let (body, state, purpose) = format_completion(&outcome);
        assert!(body.starts_with("Run aborted:"));
        assert!(body.contains("user pressed Stop"));
        assert_eq!(state, Some(TicketState::Aborted));
        assert_eq!(purpose, "run_aborted");
    }

    /// Task 12 M3: the behavioral point of this whole change — a parked run
    /// must not transition the ticket FSM at all (`None`, not `Aborted` or
    /// `Failed`), and its comment must read as a pause, not a failure.
    #[test]
    fn format_completion_parked_does_not_transition_state_and_is_not_worded_as_a_failure() {
        let wake_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:05:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let outcome = RunOutcome::Parked { wake_at };
        let (body, state, purpose) = format_completion(&outcome);
        assert_eq!(
            state, None,
            "a parked run must leave the ticket's existing state untouched — no TicketState \
             variant represents \"paused, resumes on its own\" today, and mislabeling it \
             Aborted/Failed would misrepresent a run that is working as designed"
        );
        assert!(
            body.contains("2026-01-01"),
            "comment must name the wake time, got: {body}"
        );
        assert!(
            !body.to_lowercase().contains("fail") && !body.to_lowercase().contains("abort"),
            "comment must not read as a failure, got: {body}"
        );
        assert_eq!(purpose, "run_parked");
    }
}
