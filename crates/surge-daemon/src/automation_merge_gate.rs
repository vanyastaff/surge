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
//! 5. Evaluate merge readiness via [`TaskSource::check_merge_readiness`]
//!    (GitHub inspects PR state + approving reviews; PR-less providers
//!    stay `Blocked`).
//! 6. On `Ready`, execute the merge via [`TaskSource::merge_pr`] and post a
//!    `merged` comment + label; on `Blocked` (or a failed merge), post a
//!    `merge-blocked` comment + label and escalate to the operator so a
//!    stalled L3 run is never silent.
//! 7. Record the terminal decision into `intake_emit_log` so a re-fired
//!    completion no-ops — in particular, a recorded `Merged` row prevents a
//!    double-merge after recovery.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::Connection;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::notify_config::{NotifyChannel, NotifySeverity};
use surge_intake::types::TaskId;
use surge_intake::{AutomationPolicy, MergeOutcome, MergeReadiness, TaskSource, resolve_policy};
use surge_notify::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::IntakeRepo;
use surge_persistence::intake_emit_log::{EmitEventKind, EmitKey, has, record};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Stable label literals applied to the tracker ticket on each
/// decision. Public for the docs renderer.
pub mod labels {
    /// Legacy label applied when the gate only *proposed* a merge. No longer
    /// written (the gate now executes the merge), kept for the docs renderer
    /// and to read back tickets labelled before the gate merged for real.
    pub const MERGE_PROPOSED: &str = "surge:merge-proposed";
    /// Applied when [`MergeReadiness::Blocked`] fired, or a merge attempt
    /// could not proceed (conflict / API error).
    pub const MERGE_BLOCKED: &str = "surge:merge-blocked";
    /// Applied when the gate successfully merged the PR.
    pub const MERGED: &str = "surge:merged";
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
    notifier: Arc<dyn NotifyDeliverer>,
) -> JoinHandle<()> {
    tokio::spawn(run(rx, source_map, conn, notifier))
}

async fn run(
    mut rx: broadcast::Receiver<GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
    notifier: Arc<dyn NotifyDeliverer>,
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
            handle_completion(run_id, &source_map, &conn, &notifier).await;
        }
    }
}

async fn handle_completion(
    run_id: RunId,
    source_map: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
    notifier: &Arc<dyn NotifyDeliverer>,
) {
    let run_id_str = run_id.to_string();
    let run_id_str = run_id_str.as_str();
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
    let ctx = MergeCtx {
        source: &source,
        task_id: &task_id,
        source_id: &row.source_id,
        task_id_str: &row.task_id,
        run_id,
        run_id_str,
        conn,
        notifier,
    };
    apply_merge_decision(&ctx, readiness).await;
}

/// Bundle of the stable references the decision path threads through its
/// helpers. Collapses what would otherwise be 8–9 positional arguments per
/// function into one borrow.
struct MergeCtx<'a> {
    source: &'a Arc<dyn TaskSource>,
    task_id: &'a TaskId,
    source_id: &'a str,
    task_id_str: &'a str,
    run_id: RunId,
    run_id_str: &'a str,
    conn: &'a Arc<Mutex<Connection>>,
    notifier: &'a Arc<dyn NotifyDeliverer>,
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
    // Any prior terminal decision for this (source, task, run) makes the
    // gate a no-op on a re-fired completion. `Merged` is the critical one:
    // it guarantees a recovery re-emit never double-merges. `MergeBlocked`
    // (and the legacy `MergeProposed`) likewise suppress duplicate
    // comments / escalations.
    let kinds = [
        EmitEventKind::Merged,
        EmitEventKind::MergeBlocked,
        EmitEventKind::MergeProposed,
    ];
    let guard = conn.lock().await;
    for kind in kinds {
        let key = EmitKey {
            source_id,
            task_id,
            event_kind: kind,
            run_id: run_id_str,
        };
        match has(&guard, key) {
            Ok(true) => return true,
            Ok(false) => {},
            Err(e) => {
                warn!(target: "intake::merge_gate", error = %e, "intake_emit_log has() failed");
                return false;
            },
        }
    }
    false
}

async fn apply_merge_decision(ctx: &MergeCtx<'_>, readiness: MergeReadiness) {
    match readiness {
        // The readiness gate already confirmed the PR exists, is open,
        // mergeable, and approved — execute the real merge, pinned to the
        // head the verdict was computed against.
        MergeReadiness::Ready { head_ref } => attempt_merge(ctx, head_ref.as_deref()).await,
        // Red checks / missing review / errored readiness check. Comment +
        // label + escalate so an L3 run never silently fails to merge.
        MergeReadiness::Blocked(reason) => {
            record_blocked(ctx, &format!("Surge L3 auto-merge: blocked — {reason}")).await;
        },
    }
}

/// Execute the merge for a `Ready` PR and record the outcome. `expected_head`
/// pins the merge to the revision the readiness verdict approved.
async fn attempt_merge(ctx: &MergeCtx<'_>, expected_head: Option<&str>) {
    match ctx.source.merge_pr(ctx.task_id, expected_head).await {
        Ok(MergeOutcome::Merged | MergeOutcome::AlreadyMerged) => {
            // The merge is irreversible. Record the terminal `Merged` dedup
            // row FIRST: even if the follow-up comment/label fail, a
            // re-fired completion then no-ops instead of re-attempting the
            // merge (which would 405 against an already-merged PR).
            record_dedup(ctx, EmitEventKind::Merged).await;

            let body = "Surge L3 auto-merge: PR merged ✓ (checks green + review approved).";
            if let Err(e) = ctx.source.post_comment(ctx.task_id, body).await {
                warn!(
                    target: "intake::merge_gate",
                    error = %e,
                    task_id = %ctx.task_id_str,
                    "merged, but completion comment failed (merge already durable)"
                );
            }
            if let Err(e) = ctx
                .source
                .set_label(ctx.task_id, labels::MERGED, true)
                .await
            {
                warn!(
                    target: "intake::merge_gate",
                    error = %e,
                    task_id = %ctx.task_id_str,
                    "merged, but set_label failed"
                );
            }
            escalate(
                ctx,
                NotifySeverity::Success,
                "L3 auto-merge complete",
                &format!(
                    "Surge merged the PR for {} after run {}.",
                    ctx.task_id_str, ctx.run_id_str
                ),
            )
            .await;
            info!(
                target: "intake::merge_gate",
                task_id = %ctx.task_id_str,
                run_id = %ctx.run_id_str,
                "merge gate merged PR"
            );
        },
        Ok(MergeOutcome::Conflict(reason)) => {
            record_blocked(
                ctx,
                &format!("Surge L3 auto-merge: merge could not proceed — {reason}"),
            )
            .await;
        },
        Err(e) => {
            record_blocked(
                ctx,
                &format!("Surge L3 auto-merge: merge attempt errored — {e}"),
            )
            .await;
        },
    }
}

/// Post a `merge-blocked` comment + label, escalate to the operator, and
/// record the dedup row. Used for both readiness-blocked and merge-failed
/// paths so a stalled L3 run is always visible.
async fn record_blocked(ctx: &MergeCtx<'_>, body: &str) {
    // Escalate first: "never a silent stall" must hold even if the tracker
    // comment fails. The `already_emitted` precheck stops re-fires before
    // reaching here, so this fires at most once per run.
    escalate(ctx, NotifySeverity::Warn, "L3 auto-merge blocked", body).await;

    let comment_ok = match ctx.source.post_comment(ctx.task_id, body).await {
        Ok(()) => true,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                task_id = %ctx.task_id_str,
                "merge gate comment post failed; will retry on next RunFinished"
            );
            false
        },
    };
    let label_ok = match ctx
        .source
        .set_label(ctx.task_id, labels::MERGE_BLOCKED, true)
        .await
    {
        Ok(()) => true,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                task_id = %ctx.task_id_str,
                label = labels::MERGE_BLOCKED,
                "merge gate set_label failed; will retry on next RunFinished"
            );
            false
        },
    };

    if !(comment_ok && label_ok) {
        warn!(
            target: "intake::merge_gate",
            task_id = %ctx.task_id_str,
            run_id = %ctx.run_id_str,
            comment_ok,
            label_ok,
            "merge gate emission incomplete — skipping intake_emit_log record so retry can fire"
        );
        return;
    }
    record_dedup(ctx, EmitEventKind::MergeBlocked).await;
    info!(
        target: "intake::merge_gate",
        task_id = %ctx.task_id_str,
        run_id = %ctx.run_id_str,
        decision = %EmitEventKind::MergeBlocked.as_str(),
        "merge gate emitted decision"
    );
}

/// Insert the idempotency dedup row for a terminal gate decision.
///
/// Retries a few times on transient `SQLite` failure. A persistent failure on
/// the [`EmitEventKind::Merged`] row is logged at ERROR: the merge already
/// happened durably, so a missing dedup row means a re-fired completion could
/// mislabel the merged PR `merge-blocked`. It can never double-merge — the
/// readiness recheck on the re-fire returns "already merged" — but the
/// operator should know the dedup write failed.
async fn record_dedup(ctx: &MergeCtx<'_>, kind: EmitEventKind) {
    const ATTEMPTS: u8 = 3;
    let key = EmitKey {
        source_id: ctx.source_id,
        task_id: ctx.task_id_str,
        event_kind: kind,
        run_id: ctx.run_id_str,
    };
    for attempt in 1..=ATTEMPTS {
        let result = {
            let guard = ctx.conn.lock().await;
            record(&guard, key.clone())
        };
        match result {
            Ok(_) => return,
            Err(e) if attempt < ATTEMPTS => {
                warn!(
                    target: "intake::merge_gate",
                    error = %e,
                    kind = %kind.as_str(),
                    attempt,
                    "intake_emit_log record failed; retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            },
            Err(e) if matches!(kind, EmitEventKind::Merged) => {
                error!(
                    target: "intake::merge_gate",
                    error = %e,
                    run_id = %ctx.run_id_str,
                    task_id = %ctx.task_id_str,
                    "CRITICAL: PR merged but `merged` dedup row not persisted after retries; \
                     a re-fired completion may mislabel the merged PR (cannot double-merge — \
                     the readiness recheck blocks it). Manual check advised."
                );
            },
            Err(e) => {
                warn!(
                    target: "intake::merge_gate",
                    error = %e,
                    kind = %kind.as_str(),
                    "intake_emit_log record failed after retries"
                );
            },
        }
    }
}

/// Deliver an operator escalation for the merge decision. A missing desktop
/// channel is tolerated (the decision is also on the tracker + in the log);
/// other delivery errors are logged but never fail the gate.
async fn escalate(ctx: &MergeCtx<'_>, severity: NotifySeverity, title: &str, body: &str) {
    let node = match NodeKey::try_new("merge_gate") {
        Ok(n) => n,
        Err(e) => {
            warn!(target: "intake::merge_gate", error = %e, "bad merge-gate notify node key");
            return;
        },
    };
    let rendered = RenderedNotification {
        severity,
        title: title.to_string(),
        body: body.to_string(),
        artifact_paths: vec![],
    };
    let delivery_ctx = NotifyDeliveryContext {
        run_id: ctx.run_id,
        node: &node,
    };
    match ctx
        .notifier
        .deliver(&delivery_ctx, &NotifyChannel::Desktop, &rendered)
        .await
    {
        Ok(()) | Err(NotifyError::ChannelNotConfigured) => {},
        Err(e) => warn!(
            target: "intake::merge_gate",
            error = %e,
            "merge gate escalation delivery failed"
        ),
    }
}

/// Delegate the merge-readiness decision to the provider's
/// [`TaskSource::check_merge_readiness`] implementation.
///
/// Providers without a PR concept (Linear) return the default
/// `Blocked("provider does not implement merge readiness checks")`,
/// which is surfaced as a `merge-blocked` comment on the ticket so L3
/// runs never silently fall through. GitHub queries the PR state and
/// approving reviews; see `surge_intake::github::source`.
///
/// Transport failures (network, auth) are converted to a blocked
/// reason — the gate retries on the next `RunFinished` because the
/// `intake_emit_log` row is only written after both side-effects
/// succeed.
async fn check_merge_readiness(source: &Arc<dyn TaskSource>, task_id: &TaskId) -> MergeReadiness {
    match source.check_merge_readiness(task_id).await {
        Ok(verdict) => verdict,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                task_id = %task_id,
                "check_merge_readiness call failed — surfacing as blocked"
            );
            MergeReadiness::Blocked(format!("merge readiness check errored: {e}"))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_variants_compare() {
        let ready = MergeReadiness::Ready { head_ref: None };
        let blocked = MergeReadiness::Blocked("anything".into());
        assert_ne!(ready, blocked);
        assert_eq!(ready, MergeReadiness::Ready { head_ref: None });
    }

    #[test]
    fn label_constants_are_stable() {
        assert_eq!(labels::MERGE_PROPOSED, "surge:merge-proposed");
        assert_eq!(labels::MERGE_BLOCKED, "surge:merge-blocked");
        assert_eq!(labels::MERGED, "surge:merged");
    }

    #[test]
    fn label_constants_do_not_collide() {
        assert_ne!(labels::MERGE_PROPOSED, labels::MERGE_BLOCKED);
        assert_ne!(labels::MERGED, labels::MERGE_BLOCKED);
        assert_ne!(labels::MERGED, labels::MERGE_PROPOSED);
    }
}
