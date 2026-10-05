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
//!
//! Durable `Completed` journals are reconciled at startup, every 30 seconds and
//! after broadcast lag, in bounded keyset pages including terminal tickets.
//! `MergeAttempted` is reserved before the irreversible RPC. Interrupted attempts
//! become `MergeUncertain` and require manual inspection; they are never replayed.

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
    runs: Arc<surge_persistence::runs::Storage>,
    publish_run_report: bool,
) -> JoinHandle<()> {
    tokio::spawn(run(
        rx,
        source_map,
        conn,
        notifier,
        runs,
        publish_run_report,
    ))
}

const RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const COMPLETION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const RECONCILE_PAGE_SIZE: usize = 64;

async fn run(
    mut rx: broadcast::Receiver<GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
    notifier: Arc<dyn NotifyDeliverer>,
    runs: Arc<surge_persistence::runs::Storage>,
    publish_run_report: bool,
) {
    let mut cursor = String::new();
    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first interval tick is immediate: reconcile before receiving events.
    loop {
        let event = tokio::select! {
            _ = interval.tick() => {
                reconcile_page(&source_map, &conn, &notifier, &runs, publish_run_report, &mut cursor).await;
                continue;
            },
            event = rx.recv() => event,
        };
        match event {
            Err(broadcast::error::RecvError::Closed) => {
                info!(target: "intake::merge_gate", "broadcast closed; exiting");
                return;
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(target: "intake::merge_gate", skipped = n, "merge gate lagged; reconciling durable completions");
                reconcile_page(
                    &source_map,
                    &conn,
                    &notifier,
                    &runs,
                    publish_run_report,
                    &mut cursor,
                )
                .await;
            },
            Ok(GlobalDaemonEvent::RunFinished {
                run_id,
                outcome: RunOutcome::Completed { .. },
            }) => {
                complete_bounded(
                    run_id,
                    &source_map,
                    &conn,
                    &notifier,
                    &runs,
                    publish_run_report,
                )
                .await;
            },
            Ok(_) => {},
        }
    }
}

/// Keyset paging includes terminal tickets: the completion consumer may settle
/// them before this consumer sees the broadcast. The cursor survives ticks, so
/// a fixed batch ceiling cannot permanently starve older candidates.
fn reconciliation_candidates(
    conn: &Connection,
    cursor: &str,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = conn.prepare(
        "SELECT task_id, run_id FROM ticket_index AS ticket \
         WHERE run_id IS NOT NULL AND task_id > ?1 AND NOT EXISTS (\
             SELECT 1 FROM intake_emit_log AS emitted \
             WHERE emitted.source_id = ticket.source_id AND emitted.task_id = ticket.task_id \
               AND emitted.run_id = ticket.run_id \
               AND emitted.event_kind IN ('merged','merge_blocked','merge_proposed','merge_uncertain')) \
         ORDER BY task_id LIMIT ?2",
    )?;
    statement
        .query_map(rusqlite::params![cursor, RECONCILE_PAGE_SIZE], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect()
}

async fn reconcile_page(
    sources: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
    notifier: &Arc<dyn NotifyDeliverer>,
    runs: &Arc<surge_persistence::runs::Storage>,
    publish_run_report: bool,
    cursor: &mut String,
) {
    let candidates = {
        let guard = conn.lock().await;
        reconciliation_candidates(&guard, cursor)
    };
    let candidates = match candidates {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(target: "intake::merge_gate", %error, "cannot reconcile durable merge completions");
            return;
        },
    };
    if let Some((task_id, _)) = candidates.last() {
        cursor.clone_from(task_id);
    }
    if candidates.len() < RECONCILE_PAGE_SIZE {
        cursor.clear();
    }
    for (task_id, run_id) in candidates {
        let parsed = match run_id.parse() {
            Ok(parsed) => parsed,
            Err(error) => {
                warn!(target: "intake::merge_gate", %error, %task_id, %run_id, "invalid correlated merge run");
                continue;
            },
        };
        if matches!(
            super::intake_completion::durable_outcome(runs, parsed).await,
            Some(RunOutcome::Completed { .. })
        ) {
            complete_bounded(parsed, sources, conn, notifier, runs, publish_run_report).await;
        }
    }
}

async fn complete_bounded(
    run_id: RunId,
    sources: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
    notifier: &Arc<dyn NotifyDeliverer>,
    runs: &Arc<surge_persistence::runs::Storage>,
    publish_run_report: bool,
) {
    if tokio::time::timeout(
        COMPLETION_TIMEOUT,
        handle_completion(run_id, sources, conn, notifier, runs, publish_run_report),
    )
    .await
    .is_err()
    {
        warn!(target: "intake::merge_gate", %run_id, "merge completion timed out; any published merge attempt requires manual inspection on recovery");
    }
}

async fn handle_completion(
    run_id: RunId,
    source_map: &Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: &Arc<Mutex<Connection>>,
    notifier: &Arc<dyn NotifyDeliverer>,
    runs: &Arc<surge_persistence::runs::Storage>,
    publish_run_report: bool,
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
    let task_id = match TaskId::try_new(row.task_id.clone()) {
        Ok(task_id) => task_id,
        Err(error) => {
            warn!(target: "intake::merge_gate", %error, task_id = %row.task_id, "invalid merge task identity");
            return;
        },
    };
    let ctx = MergeCtx {
        source: &source,
        task_id: &task_id,
        source_id: &row.source_id,
        task_id_str: &row.task_id,
        run_id,
        run_id_str,
        conn,
        notifier,
        runs,
        publish_run_report,
    };
    if already_emitted(conn, &row.source_id, &row.task_id, run_id_str).await
        || recover_interrupted_merge(&ctx).await
    {
        return;
    }
    let Some(policy) = fetch_policy_for_completion(&row, &source, &task_id).await else {
        return;
    };
    if !is_l3_with_merge(&policy) {
        info!(target: "intake::merge_gate", task_id = %row.task_id, %run_id, tier = policy.tier_code(), "non-L3 merge gate no-op");
        return;
    }
    let readiness = check_merge_readiness(&source, &task_id).await;
    apply_merge_decision(&ctx, readiness).await;
}

/// Classification precedes tracker reads and current policy: a revoked label
/// cannot erase an already-published request's unknown external outcome.
async fn recover_interrupted_merge(ctx: &MergeCtx<'_>) -> bool {
    let attempted = {
        let guard = ctx.conn.lock().await;
        has(
            &guard,
            EmitKey {
                source_id: ctx.source_id,
                task_id: ctx.task_id_str,
                event_kind: EmitEventKind::MergeAttempted,
                run_id: ctx.run_id_str,
            },
        )
    };
    match attempted {
        Ok(true) => {
            classify_uncertain_merge(ctx).await;
            true
        },
        Ok(false) => false,
        Err(error) => {
            warn!(target: "intake::merge_gate", %error, run_id = %ctx.run_id, "cannot inspect merge attempt receipt; refusing merge");
            true
        },
    }
}

/// Neither an interrupted request nor a returned transport/protocol error proves
/// the provider did not apply a published merge. Persist classification before
/// best-effort delivery, so a failed notification cannot authorize a retry.
async fn classify_uncertain_merge(ctx: &MergeCtx<'_>) {
    if reserve_marker(ctx, EmitEventKind::MergeUncertain).await {
        let reason = "Surge L3 auto-merge: published merge request has unknown external outcome; manual PR inspection required. Merge will not be retried.";
        warn!(target: "intake::merge_gate", run_id = %ctx.run_id, task_id = %ctx.task_id_str, "merge outcome uncertain; manual PR inspection required");
        record_blocked(ctx, reason).await;
    }
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
    /// The run event-log store, needed only to compile the optional Run
    /// Report attachment on a successful merge (spec §10/R31) — every other
    /// field above pre-dates that and never touches it.
    runs: &'a Arc<surge_persistence::runs::Storage>,
    /// `surge.toml`'s `merge_gate.publish_run_report` — off by default.
    /// Consent to L3 auto-merge is consent to *merge*, not to publish the
    /// run's transcript-derived report to the tracker; this flag is the
    /// separate, explicit opt-in for that.
    publish_run_report: bool,
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
    task_id: &TaskId,
) -> Option<AutomationPolicy> {
    match source.fetch_task(task_id).await {
        Ok(details) => Some(resolve_policy(&details.labels)),
        Err(error) => {
            warn!(target: "intake::merge_gate", %error, task_id = %row.task_id, "fetch_task failed; skipping merge gate");
            None
        },
    }
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
        EmitEventKind::MergeUncertain,
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
                return true;
            },
        }
    }
    false
}

/// Only the unique durable reservation winner may publish a merge or recovery
/// notification. Persistence errors fail closed before any external action.
async fn reserve_marker(ctx: &MergeCtx<'_>, kind: EmitEventKind) -> bool {
    let result = {
        let guard = ctx.conn.lock().await;
        record(
            &guard,
            EmitKey {
                source_id: ctx.source_id,
                task_id: ctx.task_id_str,
                event_kind: kind,
                run_id: ctx.run_id_str,
            },
        )
    };
    match result {
        Ok(inserted) => inserted,
        Err(error) => {
            error!(target: "intake::merge_gate", %error, run_id = %ctx.run_id_str, kind = %kind.as_str(), "cannot persist merge reservation; refusing external action");
            false
        },
    }
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
    if !reserve_marker(ctx, EmitEventKind::MergeAttempted).await {
        return;
    }
    match ctx.source.merge_pr(ctx.task_id, expected_head).await {
        Ok(MergeOutcome::Merged | MergeOutcome::AlreadyMerged) => {
            // The merge is irreversible. Record the terminal `Merged` dedup
            // row FIRST: even if the follow-up comment/label fail, a
            // re-fired completion then no-ops instead of re-attempting the
            // merge (which would 405 against an already-merged PR).
            record_dedup(ctx, EmitEventKind::Merged).await;

            let prefix = "Surge L3 auto-merge: PR merged ✓ (checks green + review approved).";
            let mut body = prefix.to_string();
            if ctx.publish_run_report {
                // Reserve the prefix and its separator too, so the size
                // check inside `run_report_attachment` bounds the *whole*
                // posted comment, not just the attachment on its own — see
                // `MAX_ATTACHMENT_BODY_LEN`'s own doc comment.
                let reserved_len = prefix.len() + "\n\n".len();
                if let Some(attachment) =
                    run_report_attachment(ctx.runs, ctx.run_id, ctx.source.provider(), reserved_len)
                        .await
                {
                    body.push_str("\n\n");
                    body.push_str(&attachment);
                }
            }
            let body = body.as_str();
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
        Err(_) => classify_uncertain_merge(ctx).await,
    }
}

/// A defensive ceiling, well under GitHub's actual issue/PR comment body
/// limit (65,536 *characters*, per GitHub's own support docs — the REST API
/// reference this constant used to cite does not itself state a body-size
/// limit, so it is not linked here as if it did), so a pathologically large
/// Run Report (many nodes/verdicts/skills across a long-running run) cannot
/// turn a merge success into a failed `post_comment` call. The attachment is
/// dropped past this size rather than silently mangled by a partial
/// truncation mid-Markdown.
///
/// Deliberately conservative in two ways at once: the number itself sits
/// well below GitHub's 65,536-character limit, *and* this counts UTF-8
/// **bytes** (`str::len()`), not characters — one character can be several
/// bytes, so a byte count is always `>=` the character count for the same
/// text. Both slack margins exist for the same reason: this is a
/// best-effort guard against a pathological case, not a precise
/// reproduction of GitHub's limit, and erring conservative costs nothing
/// (the attachment is a value-add, never a precondition for the merge
/// succeeding).
///
/// Bounds the whole posted comment, not the attachment in isolation —
/// [`attempt_merge`] passes its own success-line prefix length in as
/// `reserved_len` so this checks what `post_comment` actually receives.
const MAX_ATTACHMENT_BODY_LEN: usize = 60_000;

/// Compile `run_id`'s Run Report and render it as an attachment for the
/// merge gate's success comment (spec §10/R31: "the report can be attached
/// to the PR, optionally, through the existing L3 merge gate" — this is the
/// whole mechanism: no new tracker integration, just more text in the
/// [`TaskSource::post_comment`] call the gate already makes on every merge).
/// Only called when `MergeCtx::publish_run_report` is set — the caller
/// decides *whether* to publish; this decides *what*, once that is settled.
///
/// - `provider` (`TaskSource::provider()`) picks the wrapper: every provider
///   except `"linear"` gets the `<details>` block, since GitHub (this
///   mechanism's primary target) renders inline HTML inside a Markdown
///   comment and a `<details>` block actually collapses there. Linear is the
///   one *named* exception — it does not render raw HTML tags at all, so
///   wrapping there would leave the literal `<details>`/`<summary>` text
///   sitting in the comment instead of collapsing anything. Excluding the
///   one known non-renderer rather than allowlisting the one known renderer
///   means a test double or a future provider defaults to the
///   (already-escaped, so still safe) collapsible form instead of silently
///   degrading.
///
///   **This branch is not reachable through Linear today**, and that is
///   worth saying plainly rather than leaving as an implied "live" choice:
///   `TaskSource::check_merge_readiness`'s trait default — the one Linear's
///   `LinearSource` inherits, never overriding — always returns
///   `Blocked("provider does not implement merge readiness checks")` (see
///   `docs/tracker-automation.md`'s "GitHub readiness check" section, which
///   already documents this), so [`attempt_merge`]'s success arm, and this
///   function with it, never executes for an L3 Linear ticket at all. The
///   branch exists for a hypothetical future PR-capable, non-HTML-rendering
///   provider — it is forward-compatibility, not a decision this mechanism
///   currently exercises.
/// - `reserved_len` is the byte length of whatever [`attempt_merge`]
///   prepends before this attachment in the final posted body (its
///   success-line prefix plus the `"\n\n"` separator) — included in the
///   [`MAX_ATTACHMENT_BODY_LEN`] check so the check bounds the comment that
///   is actually posted, not the attachment considered on its own.
/// - The rendered Markdown is scrubbed via [`scrub_for_publication`] before
///   it is wrapped: this is the one surface a run's own event log — an
///   agent's `initial_prompt`, failure reasons, operator steer/response
///   text, artifact paths — leaves the machine for a third party to read
///   (spec §10's threat model), and none of that text was ever scanned for
///   credentials or shortened of the local username before this attachment
///   existed.
///
/// Best-effort: a read/compile failure returns `None` rather than blocking
/// the merge — the report is a value-add on top of an already-decided
/// outcome, never a precondition for it. A summary line leads with
/// `surge_core::evidence::is_evidence_backed`'s own verdict (spec §10/R30),
/// so a reviewer scanning the PR sees "unverified success" before expanding
/// anything, the same signal Run Report/`surge inbox`/`surge ledger` show.
async fn run_report_attachment(
    runs: &Arc<surge_persistence::runs::Storage>,
    run_id: RunId,
    provider: &str,
    reserved_len: usize,
) -> Option<String> {
    let reader = runs
        .open_run_reader(run_id)
        .await
        .inspect_err(|err| {
            tracing::debug!(
                target: "intake::merge_gate",
                run_id = %run_id,
                error = %err,
                "could not open run reader for the report attachment; omitting it"
            );
        })
        .ok()?;
    let events = reader
        .read_run_events()
        .await
        .inspect_err(|err| {
            tracing::debug!(
                target: "intake::merge_gate",
                run_id = %run_id,
                error = %err,
                "could not read events for the report attachment; omitting it"
            );
        })
        .ok()?;
    let report = surge_core::run_report::RunReport::compile(run_id, &events);
    let summary = match report.evidence_backed {
        Some(true) => "Surge Run Report (verified)",
        Some(false) => {
            "⚠ Surge Run Report — UNVERIFIED SUCCESS (no authorized verifier confirmed this run)"
        },
        None => "Surge Run Report",
    };
    let markdown = scrub_for_publication(&surge_core::run_report::render_markdown(&report));
    let attachment = if provider == "linear" {
        format!("**{summary}**\n\n{markdown}")
    } else {
        format!("<details>\n<summary>{summary}</summary>\n\n{markdown}\n</details>")
    };
    if attachment.len() + reserved_len > MAX_ATTACHMENT_BODY_LEN {
        tracing::debug!(
            target: "intake::merge_gate",
            run_id = %run_id,
            len = attachment.len(),
            "run report attachment exceeds the comment size ceiling; omitting it"
        );
        return None;
    }
    Some(attachment)
}

/// Scrub a rendered Run Report before it leaves the machine as a tracker
/// comment (spec §10/R31 gates *whether* to publish it at all via
/// `MergeCtx::publish_run_report`; this governs *what* goes out once that is
/// on). Two passes, in order:
///
/// 1. [`surge_acp::secrets::redact_secrets`] — the same credential-pattern
///    scan an agent's own file reads go through, applied here to the whole
///    rendered report, which is the first time any of this run's free text
///    (`initial_prompt`, escalation/failure reasons,
///    `HumanInputRequested`/`HumanInputResolved` text — including the
///    operator's own answers, steer messages, sandbox elevation capability
///    strings) is scanned at all. **This is best-effort against known
///    credential *shapes*** — a dozen or so recognized patterns (API keys,
///    DSNs with embedded credentials, JWTs, …); see that function's own
///    pattern table. It does not understand *meaning*: free text that isn't
///    shaped like one of those patterns is posted verbatim, credential or
///    not. `surge-daemon` already depends on `surge-acp`; `surge-core`
///    (where [`surge_core::run_report::render_markdown`] lives) does not and
///    must not gain that dependency just for this, so the scan happens
///    here, on the rendered text, not inside the renderer.
/// 2. Absolute-path shortening ([`apply_home_shortening`]): every artifact
///    path this report renders (`artifact.path.display()`) is an absolute
///    path inside the run's worktree, anchored at the surge home directory
///    ([`resolve_surge_home`]), which typically embeds the machine's OS
///    username — shortened to `~/...` for no reason a tracker comment needs
///    the rest.
fn scrub_for_publication(markdown: &str) -> String {
    let (redacted, _was_redacted) = surge_acp::secrets::redact_secrets(markdown);
    apply_home_shortening(&redacted, resolve_surge_home().as_deref())
}

/// Apply [`shorten_home_prefix`] when `home` is a plausible path — guards
/// against `None`, empty, or literally the filesystem root `/` (an operator
/// with `HOME=/`, or `SURGE_HOME=/`, is unusual but not impossible):
/// `str::replace` with `"/"` as the pattern would turn every path separator
/// in the *entire* document into `~`, shredding the report instead of
/// shortening one path in it. Split out from [`scrub_for_publication`] so a
/// test can exercise the guard without mutating `HOME`/`SURGE_HOME`.
fn apply_home_shortening(text: &str, home: Option<&str>) -> String {
    match home {
        Some(home) if home.len() > 1 => shorten_home_prefix(text, home),
        _ => text.to_string(),
    }
}

/// Resolve the surge home directory the same way `surge-daemon::main`'s
/// `surge_runs_dir` and `surge-cli`'s `surge_home_dir` do: `SURGE_HOME` (set
/// and non-empty) relocates it; otherwise `$HOME/.surge`.
///
/// Worktree and artifact paths this report renders are anchored here, not
/// necessarily under bare `$HOME` — a scrubber that only strips `$HOME`
/// finds nothing to strip once an operator points `SURGE_HOME` somewhere
/// else entirely (e.g. `/srv/surge`), and a path under *that* root can still
/// embed a username the same way a path under `$HOME/.surge` does.
fn resolve_surge_home() -> Option<String> {
    if let Ok(custom) = std::env::var("SURGE_HOME")
        && !custom.is_empty()
    {
        return Some(custom);
    }
    dirs::home_dir().map(|h| h.join(".surge").to_string_lossy().into_owned())
}

/// Replace every occurrence of `home` in `text` with `~`. Split out from
/// [`scrub_for_publication`] so a test can exercise the replacement without
/// mutating the process-global `HOME`/`SURGE_HOME` environment variables —
/// this crate's tests do not do that (a stale value would leak into every
/// other test in the same process).
fn shorten_home_prefix(text: &str, home: &str) -> String {
    text.replace(home, "~")
}

/// Post a `merge-blocked` comment + label, escalate to the operator, and
/// record the dedup row. Used for both readiness-blocked and merge-failed
/// paths so a stalled L3 run is always visible.
async fn record_blocked(ctx: &MergeCtx<'_>, body: &str) {
    // Escalate first: "never a silent stall" must hold even if the tracker
    // comment fails. Uncertain attempts already carry a terminal classification;
    // ordinary blocked decisions remain eligible until their receipt is durable.
    escalate(ctx, NotifySeverity::Warn, "L3 auto-merge blocked", body).await;

    let comment_ok = match ctx.source.post_comment(ctx.task_id, body).await {
        Ok(()) => true,
        Err(e) => {
            warn!(
                target: "intake::merge_gate",
                error = %e,
                task_id = %ctx.task_id_str,
                "merge gate comment post failed; recovery eligibility depends on durable receipts"
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
                "merge gate set_label failed; recovery eligibility depends on durable receipts"
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
            "merge gate emission incomplete; preserving existing receipts without recording successful delivery"
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
/// require manual inspection. The pre-dispatch `MergeAttempted` receipt prevents
/// replay even when the success receipt cannot be written; recovery durably
/// classifies that unknown external outcome as `MergeUncertain`.
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
                     durable attempt receipt prevents retry; recovery requires manual PR inspection."
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
/// reason — the gate retries during durable reconciliation because the
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
    fn reconciliation_pages_include_terminal_tickets_and_exclude_receipts() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ticket_index(task_id TEXT PRIMARY KEY, source_id TEXT, run_id TEXT, state TEXT);").unwrap();
        conn.execute_batch(include_str!(
            "../../surge-persistence/src/runs/migrations/registry/0013_intake_emit_log.sql"
        ))
        .unwrap();
        for index in 0..65 {
            conn.execute(
                "INSERT INTO ticket_index VALUES (?1,'mock:test','run-fixture','Completed')",
                [format!("mock:test#{index:03}")],
            )
            .unwrap();
        }
        let first = reconciliation_candidates(&conn, "").unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(first[0].0, "mock:test#000");
        assert_eq!(first[63].0, "mock:test#063");
        let next = reconciliation_candidates(&conn, "mock:test#063").unwrap();
        assert_eq!(next, [("mock:test#064".into(), "run-fixture".into())]);
        record(
            &conn,
            EmitKey {
                source_id: "mock:test",
                task_id: "mock:test#064",
                event_kind: EmitEventKind::Merged,
                run_id: "run-fixture",
            },
        )
        .unwrap();
        assert!(
            reconciliation_candidates(&conn, "mock:test#063")
                .unwrap()
                .is_empty()
        );
    }

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

    /// Spec §10's threat model: nothing on the merge-gate's Run Report
    /// attachment path scanned event-log text for credentials before
    /// `scrub_for_publication` existed. `redact_secrets` itself is already
    /// covered by `surge_acp::secrets`'s own tests — this pins that
    /// `scrub_for_publication` actually calls it, not just that the pattern
    /// scanner works in isolation.
    #[test]
    fn scrub_for_publication_redacts_a_credential_pattern() {
        let input = "operator pasted: sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa into a steer";
        let out = scrub_for_publication(input);
        assert!(
            out.contains("[REDACTED:anthropic-key]"),
            "credential pattern must be redacted: {out}"
        );
        assert!(!out.contains("sk-ant-aaaa"));
    }

    /// `shorten_home_prefix` is the part of `scrub_for_publication` that
    /// removes the machine's OS username from an absolute artifact path —
    /// tested directly with an explicit `home` argument rather than
    /// mutating the real `HOME` environment variable (unsound to do from a
    /// test in this crate — see the function's own doc comment).
    #[test]
    fn shorten_home_prefix_replaces_every_occurrence() {
        let text = "- **spec.md** (/home/vanyastaff/.surge/worktrees/r1/spec.md) — node n\n\
                     - **plan.md** (/home/vanyastaff/.surge/worktrees/r1/plan.md) — node n";
        let out = shorten_home_prefix(text, "/home/vanyastaff");
        assert!(
            !out.contains("/home/vanyastaff"),
            "leaked the username: {out}"
        );
        assert_eq!(
            out,
            "- **spec.md** (~/.surge/worktrees/r1/spec.md) — node n\n\
             - **plan.md** (~/.surge/worktrees/r1/plan.md) — node n"
        );
    }

    /// Spec §10/R31 review: a pathological `home` value (`HOME=/` or
    /// `SURGE_HOME=/`) must not shorten anything — `str::replace(text, "/",
    /// "~")` would corrupt every path separator in the document, not just
    /// the machine's home prefix.
    #[test]
    fn apply_home_shortening_refuses_the_filesystem_root() {
        let text = "artifact at /srv/surge/worktrees/r1/out.md, path /var/log/x";
        let out = apply_home_shortening(text, Some("/"));
        assert_eq!(
            out, text,
            "a root `home` must leave the document untouched, not replace \
             every slash"
        );
    }

    /// `None` (neither `SURGE_HOME` nor `$HOME` resolved) and an empty
    /// string are the other two values the guard must refuse.
    #[test]
    fn apply_home_shortening_refuses_none_and_empty() {
        let text = "artifact at /srv/surge/out.md";
        assert_eq!(apply_home_shortening(text, None), text);
        assert_eq!(apply_home_shortening(text, Some("")), text);
    }

    /// The positive case: a plausible `SURGE_HOME` (deliberately unrelated
    /// to any `$HOME`-shaped path, proving the scrubber does not hardcode
    /// `$HOME`) is applied.
    #[test]
    fn apply_home_shortening_applies_a_custom_surge_home() {
        let text = "artifact at /srv/surge/worktrees/r1/out.md";
        let out = apply_home_shortening(text, Some("/srv/surge"));
        assert_eq!(out, "artifact at ~/worktrees/r1/out.md");
    }

    /// `cargo mutants` found this boundary uncovered: every existing test's
    /// Run Report is tiny, so `attachment.len() + reserved_len >
    /// MAX_ATTACHMENT_BODY_LEN` and `attachment.len() + reserved_len ==
    /// MAX_ATTACHMENT_BODY_LEN` (and `+` mutated to `-`) were all
    /// indistinguishable from the real check — none of them ever came close
    /// to the cap. `reserved_len` alone, deliberately chosen right at the
    /// boundary against a report too small to reach the cap on its own,
    /// exercises the addition and the comparison without needing a
    /// genuinely 60000-byte report.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_report_attachment_omits_when_reserved_len_pushes_it_over_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let storage = surge_persistence::runs::Storage::open(dir.path())
            .await
            .unwrap();
        let run_id = RunId::new();
        let project = dir.path().join("proj");
        let writer = storage.create_run(run_id, &project, None).await.unwrap();
        writer
            .append_event(surge_core::run_event::VersionedEventPayload::new(
                surge_core::run_event::EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_new("end").unwrap(),
                },
            ))
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let unreserved = run_report_attachment(&storage, run_id, "github_issues", 0)
            .await
            .expect("a small report with no reservation must fit under the cap");
        assert!(unreserved.len() < MAX_ATTACHMENT_BODY_LEN);

        // Reserve exactly enough that the total lands one byte over the cap.
        let reserved_len = MAX_ATTACHMENT_BODY_LEN - unreserved.len() + 1;
        let omitted = run_report_attachment(&storage, run_id, "github_issues", reserved_len).await;
        assert!(
            omitted.is_none(),
            "a reserved_len that pushes the total one byte over the cap must omit the attachment"
        );

        // One byte under the cap must still fit — pins the check as a
        // strict `>`, not `>=`.
        let reserved_len_fits = MAX_ATTACHMENT_BODY_LEN - unreserved.len();
        let fits =
            run_report_attachment(&storage, run_id, "github_issues", reserved_len_fits).await;
        assert!(
            fits.is_some(),
            "landing exactly at the cap must still be posted, not omitted"
        );
    }
}
