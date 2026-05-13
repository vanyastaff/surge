//! L3 (`surge:auto`) auto-merge gate.
//!
//! Subscribes to [`GlobalDaemonEvent::RunFinished`] and reacts ONLY to
//! `RunOutcome::Completed`. For each completion:
//!
//! 1. Look up the originating ticket via `IntakeRepo::lookup_ticket_by_run_id`.
//! 2. Re-fetch the ticket's current labels via `TaskSource::fetch_task`
//!    (labels are not stored on `ticket_index`, and the user may have
//!    flipped `surge:auto` off mid-run).
//! 3. Resolve the [`AutomationPolicy`]. Only `Auto { merge_when_clean: true }`
//!    enters the gate; other tiers are no-ops here (the
//!    `intake_completion` consumer already posted the completion
//!    comment).
//! 4. Idempotency precheck via `intake_emit_log` — a re-fired
//!    `RunFinished` does not duplicate the merge action.
//! 5. Evaluate merge readiness (currently always blocked — see
//!    [`check_merge_readiness`] for the deferred-work note).
//! 6. Post a `merge-proposed` or `merge-blocked` comment + apply the
//!    matching `surge:*` label.
//! 7. Record the side-effect into `intake_emit_log` so a retry no-ops.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::Connection;
use surge_intake::types::TaskId;
use surge_intake::{AutomationPolicy, TaskSource, resolve_policy};
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::IntakeRepo;
use surge_persistence::intake_emit_log::{EmitEventKind, EmitKey, has, record};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Result of a merge-readiness check against an external PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeReadiness {
    /// All required checks are green and the PR has an approved review.
    Ready,
    /// One or more requirements failed; the human-readable reason is
    /// posted to the tracker.
    Blocked(String),
}

/// Stable label literals applied to the tracker ticket on each
/// decision. Public for the docs renderer.
pub mod labels {
    /// Applied when [`MergeReadiness::Ready`] fired and the auto-merge
    /// comment was posted.
    pub const MERGE_PROPOSED: &str = "surge:merge-proposed";
    /// Applied when [`MergeReadiness::Blocked`] fired.
    pub const MERGE_BLOCKED: &str = "surge:merge-blocked";
}

/// Spawn the merge gate. The returned handle can be aborted on
/// shutdown; the gate also exits cleanly when the broadcast channel
/// closes.
#[allow(clippy::implicit_hasher)]
#[must_use]
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
                info!(target: "intake::merge_gate", "broadcast closed; exiting");
                return;
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    target: "intake::merge_gate",
                    skipped = n,
                    "merge gate lagged; some RunFinished events dropped"
                );
                continue;
            },
        };

        if let GlobalDaemonEvent::RunFinished { run_id, outcome } = event
            && let RunOutcome::Completed { .. } = outcome
        {
            handle_completion(&run_id.to_string(), &source_map, &conn).await;
        }
    }
}

async fn handle_completion(
    run_id_str: &str,
    source_map: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
) {
    let Some(row) = lookup_ticket(conn, run_id_str).await else {
        return;
    };
    let Some(source) = source_map.get(&row.source_id).cloned() else {
        warn!(
            target: "intake::merge_gate",
            source_id = %row.source_id,
            run_id = %run_id_str,
            "no source registered; cannot evaluate merge gate"
        );
        return;
    };
    let Some((task_id, policy)) = fetch_policy_for_completion(&row, &source).await else {
        return;
    };
    if !is_l3_with_merge(&policy) {
        info!(
            target: "intake::merge_gate",
            task_id = %row.task_id,
            run_id = %run_id_str,
            tier = policy.tier_code(),
            "non-L3 (or merge_when_clean=false); merge gate no-op"
        );
        return;
    }
    if already_emitted(conn, &row.source_id, &row.task_id, run_id_str).await {
        info!(
            target: "intake::merge_gate",
            task_id = %row.task_id,
            run_id = %run_id_str,
            "merge gate already emitted for this run; skipping"
        );
        return;
    }

    let readiness = check_merge_readiness(&source, &task_id).await;
    apply_merge_decision(
        &source,
        &task_id,
        &row.source_id,
        &row.task_id,
        run_id_str,
        readiness,
        conn,
    )
    .await;
}

async fn lookup_ticket(
    conn: &Arc<Mutex<Connection>>,
    run_id_str: &str,
) -> Option<surge_persistence::intake::IntakeRow> {
    let guard = conn.lock().await;
    match IntakeRepo::new(&guard).lookup_ticket_by_run_id(run_id_str) {
        Ok(Some(r)) => Some(r),
        Ok(None) => None,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                run_id = %run_id_str,
                "lookup_ticket_by_run_id failed"
            );
            None
        },
    }
}

async fn fetch_policy_for_completion(
    row: &surge_persistence::intake::IntakeRow,
    source: &Arc<dyn TaskSource>,
) -> Option<(TaskId, AutomationPolicy)> {
    let task_id = match TaskId::try_new(row.task_id.clone()) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                task_id = %row.task_id,
                "invalid task_id; skipping merge gate"
            );
            return None;
        },
    };
    let details = match source.fetch_task(&task_id).await {
        Ok(d) => d,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                task_id = %row.task_id,
                "fetch_task failed; skipping merge gate"
            );
            return None;
        },
    };
    Some((task_id, resolve_policy(&details.labels)))
}

fn is_l3_with_merge(policy: &AutomationPolicy) -> bool {
    matches!(
        policy,
        AutomationPolicy::Auto {
            merge_when_clean: true
        }
    )
}

async fn already_emitted(
    conn: &Arc<Mutex<Connection>>,
    source_id: &str,
    task_id: &str,
    run_id_str: &str,
) -> bool {
    let key_proposed = EmitKey {
        source_id,
        task_id,
        event_kind: EmitEventKind::MergeProposed,
        run_id: run_id_str,
    };
    let key_blocked = EmitKey {
        source_id,
        task_id,
        event_kind: EmitEventKind::MergeBlocked,
        run_id: run_id_str,
    };
    let guard = conn.lock().await;
    match (has(&guard, key_proposed), has(&guard, key_blocked)) {
        (Ok(p), Ok(b)) => p || b,
        (Err(e), _) | (_, Err(e)) => {
            warn!(target: "intake::merge_gate", error = %e, "intake_emit_log has() failed");
            false
        },
    }
}

async fn apply_merge_decision(
    source: &Arc<dyn TaskSource>,
    task_id: &TaskId,
    source_id: &str,
    task_id_str: &str,
    run_id_str: &str,
    readiness: MergeReadiness,
    conn: &Arc<Mutex<Connection>>,
) {
    let (event_kind, label, body) = match readiness {
        MergeReadiness::Ready => (
            EmitEventKind::MergeProposed,
            labels::MERGE_PROPOSED,
            "Surge L3 auto-merge: PR is ready (checks green + review approved). \
             Merge action proposed."
                .to_string(),
        ),
        MergeReadiness::Blocked(reason) => (
            EmitEventKind::MergeBlocked,
            labels::MERGE_BLOCKED,
            format!("Surge L3 auto-merge: blocked — {reason}"),
        ),
    };

    if let Err(e) = source.post_comment(task_id, &body).await {
        warn!(
            target: "intake::merge_gate",
            error = %e,
            task_id = %task_id_str,
            "merge gate comment post failed"
        );
    }
    if let Err(e) = source.set_label(task_id, label, true).await {
        warn!(
            target: "intake::merge_gate",
            error = %e,
            task_id = %task_id_str,
            label = label,
            "merge gate set_label failed"
        );
    }

    {
        let guard = conn.lock().await;
        let emit_key = EmitKey {
            source_id,
            task_id: task_id_str,
            event_kind,
            run_id: run_id_str,
        };
        if let Err(e) = record(&guard, emit_key) {
            warn!(target: "intake::merge_gate", error = %e, "intake_emit_log record failed");
        }
    }

    info!(
        target: "intake::merge_gate",
        task_id = %task_id_str,
        run_id = %run_id_str,
        decision = %event_kind.as_str(),
        "merge gate emitted decision"
    );
}

/// Decide whether an external PR is ready to auto-merge.
///
/// **Currently stubbed** — the implementation always returns
/// [`MergeReadiness::Blocked`] with a "not implemented" reason. Real
/// readiness requires querying:
///
/// - GitHub: `pull_request.merge_state_status == "clean"` plus the
///   `check_runs` API for required checks plus `reviews` API for
///   approval (via `octocrab`).
/// - Linear: PR linkage is informational; merge intent flows through
///   GitHub regardless.
///
/// A follow-up milestone adds a real `MergeReadinessCheck` plug-point.
/// Until then the gate produces explicit `merge-blocked` comments so
/// L3 runs never silently fall through.
///
/// The function is `async` so the real implementation can issue HTTP
/// calls without changing the call site.
#[allow(clippy::unused_async)]
async fn check_merge_readiness(_source: &Arc<dyn TaskSource>, _task_id: &TaskId) -> MergeReadiness {
    MergeReadiness::Blocked(
        "PR readiness check is not implemented yet — \
         this is a stub default that lands the L3 gate plumbing. \
         Merge manually after verifying checks + review."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_variants_compare() {
        let ready = MergeReadiness::Ready;
        let blocked = MergeReadiness::Blocked("anything".into());
        assert_ne!(ready, blocked);
        assert_eq!(ready, MergeReadiness::Ready);
    }

    #[test]
    fn label_constants_are_stable() {
        assert_eq!(labels::MERGE_PROPOSED, "surge:merge-proposed");
        assert_eq!(labels::MERGE_BLOCKED, "surge:merge-blocked");
    }

    #[test]
    fn label_constants_do_not_collide() {
        assert_ne!(labels::MERGE_PROPOSED, labels::MERGE_BLOCKED);
    }
}
