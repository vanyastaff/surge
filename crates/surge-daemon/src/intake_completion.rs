//! Tracker completion ingestion and durable terminal comment delivery.
//!
//! Startup, periodic and lag-triggered sweeps repair Active/RunStarted tickets
//! from existing terminal journals. Missing, invalid and nonterminal histories
//! preserve ticket state. Every terminal transition atomically enqueues an
//! immutable run-specific comment. A separate leased worker retries delivery,
//! including for terminal tickets after restart, and records acknowledgment.
//!
//! Parked/unknown outcomes remain bounded best effort. External success before
//! durable acknowledgment can be delivered again; exactly-once is not promised.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::Connection;
use std::time::Duration;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::{IntakeRepo, TicketState};
use surge_persistence::intake_outbox::{self, TerminalCommentKind};
use surge_persistence::runs::Storage;
use surge_persistence::runs::inspection::RunDatabaseInspection;
use surge_persistence::runs::{Clock, SystemClock};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Spawn live completion delivery and durable ticket reconciliation.
/// A startup sweep repairs lost events before receiving; later sweeps run every minute.
/// Lag triggers a sweep, and channel closure stops the worker.
// `clippy::implicit_hasher`: the daemon owns the source map; consumers don't
// need to be generic over `BuildHasher`. The map uses the default
// `RandomState` everywhere, including the daemon's `main.rs` call site.
#[allow(clippy::implicit_hasher)]
pub fn spawn(
    rx: broadcast::Receiver<GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
    storage: Arc<Storage>,
) -> JoinHandle<()> {
    tokio::spawn(run(rx, source_map, conn, storage))
}

async fn run(
    mut rx: broadcast::Receiver<GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
    storage: Arc<Storage>,
) {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let delivery = tokio::spawn(crate::intake_delivery::run(
        Arc::clone(&source_map),
        Arc::clone(&conn),
        Arc::new(SystemClock),
        shutdown_rx,
    ));
    reconcile_pending(&storage, &source_map, &conn).await;
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let received = tokio::select! {
            received = rx.recv() => received,
            _ = interval.tick() => {
                reconcile_pending(&storage, &source_map, &conn).await;
                continue;
            }
        };
        let event = match received {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Closed) => {
                info!("run-completion consumer: broadcast closed; exiting");
                let _ = shutdown_tx.send(true);
                let _ = delivery.await;
                return;
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    skipped = n,
                    "run-completion consumer lagged; some RunFinished events dropped"
                );
                reconcile_pending(&storage, &source_map, &conn).await;
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

    let (body, _state, purpose) = format_completion(outcome);
    if let Some(kind) = terminal_kind(outcome) {
        let changed = {
            let guard = conn.lock().await;
            intake_outbox::enqueue_terminal(
                &guard,
                &row.task_id,
                run_id_str,
                kind,
                &body,
                SystemClock.now_ms(),
            )
        };
        match changed {
            Ok(_) => return,
            Err(error) => {
                warn!(%error, task_id = %row.task_id, "failed to mirror terminal ticket state");
                return;
            },
        }
    } else if !matches!(row.state, TicketState::Active | TicketState::RunStarted) {
        return;
    }

    // Best-effort comment post: a malformed `task_id` string in the DB row
    // (e.g., from a future migration that loosened validation, or manual
    // edits) skips the cosmetic tracker note but MUST NOT skip the FSM
    // transition — the on-disk state is authoritative regardless.
    let task_id = match TaskId::try_new(row.task_id.clone()) {
        Ok(id) => Some(id),
        Err(e) => {
            warn!(error = %e, task_id = %row.task_id, "invalid task_id; skipping comment post");
            None
        },
    };

    if let Some(task_id) = task_id {
        if let Some(source) = source_map.get(&row.source_id) {
            match post_comment_bounded(source.as_ref(), &task_id, &body).await {
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
}

/// Shared bounded tracker delivery. Local state is committed before this await.
pub(crate) async fn post_comment_bounded(
    source: &dyn TaskSource,
    task_id: &TaskId,
    body: &str,
) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(5), source.post_comment(task_id, body))
        .await
        .map_err(|_| "tracker comment timed out".to_string())?
        .map_err(|error| error.to_string())
}

pub(crate) fn terminal_kind(outcome: &RunOutcome) -> Option<TerminalCommentKind> {
    match outcome {
        RunOutcome::Completed { .. } => Some(TerminalCommentKind::Completed),
        RunOutcome::Failed { .. } => Some(TerminalCommentKind::Failed),
        RunOutcome::Aborted { .. } => Some(TerminalCommentKind::Aborted),
        _ => None,
    }
}

/// Read only existing journal evidence; absence/corruption never guesses a result.
pub(crate) async fn durable_outcome(
    storage: &Arc<Storage>,
    run_id: surge_core::RunId,
) -> Option<RunOutcome> {
    let inspection = match storage.inspect_run(run_id).await {
        Ok(inspection) => inspection,
        Err(error) => {
            warn!(%error, %run_id, "cannot inspect ticket run journal");
            return None;
        },
    };
    let RunDatabaseInspection::Present { events } = inspection.database else {
        warn!(%run_id, "ticket run journal absent; preserving ticket state");
        return None;
    };
    let events: Vec<_> = events
        .into_iter()
        .map(|event| surge_core::run_event::RunEvent {
            run_id,
            seq: event.seq.0,
            timestamp: chrono::DateTime::from_timestamp_millis(event.timestamp_ms)
                .unwrap_or_default(),
            payload: event.payload.payload,
        })
        .collect();
    match surge_core::run_state::fold(&events) {
        Ok(surge_core::RunState::Terminal { .. }) => {},
        Ok(_) => return None,
        Err(error) => {
            warn!(%error, %run_id, "invalid ticket run history; preserving ticket state");
            return None;
        },
    }
    events.iter().rev().find_map(|event| match &event.payload {
        surge_core::run_event::EventPayload::RunCompleted { terminal_node } => {
            Some(RunOutcome::Completed {
                terminal: terminal_node.clone(),
            })
        },
        surge_core::run_event::EventPayload::RunFailed { error } => Some(RunOutcome::Failed {
            error: error.clone(),
        }),
        surge_core::run_event::EventPayload::RunAborted { reason } => Some(RunOutcome::Aborted {
            reason: reason.clone(),
        }),
        _ => None,
    })
}

async fn reconcile_pending(
    storage: &Arc<Storage>,
    sources: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
) {
    let candidates = {
        let guard = conn.lock().await;
        IntakeRepo::new(&guard).pending_run_tickets()
    };
    let candidates = match candidates {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, "cannot list pending ticket runs");
            return;
        },
    };
    for (task_id, run_id) in candidates {
        let parsed = match run_id.parse() {
            Ok(parsed) => parsed,
            Err(error) => {
                warn!(%error, %task_id, %run_id, "invalid correlated run id");
                continue;
            },
        };
        if let Some(outcome) = durable_outcome(storage, parsed).await {
            handle_run_finished(&run_id, &outcome, sources, conn).await;
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
pub(crate) fn format_completion(
    outcome: &RunOutcome,
) -> (String, Option<TicketState>, &'static str) {
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
