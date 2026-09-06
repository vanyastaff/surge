//! Durable, query-answerable trace of `EscalationRequested` events.
//!
//! `.autopilot/competitive-waves/spec.md` §15 / History 45: a `LoopGuard`
//! verdict "must leave a durable, queryable trace" distinguishing a
//! guard-tripped run from any other failure — without parsing the free-form
//! `reason` text apart, or scanning the full event log when an index already
//! answers the question. [`read_escalations`] is the production entry point:
//! it filters on `events.kind = 'EscalationRequested'` in SQL (`idx_events_kind`,
//! `migrations/per_run/0001_initial.sql:8`), not a `read_events` full-range
//! scan folded in Rust. [`fold_escalations`] is a separate, pure reference
//! implementation over an already-read `&[ReadEvent]` slice, kept only for
//! unit-testing the cause/node-attribution logic against synthetic fixtures
//! without a database — mirroring `query::aggregate_status`'s role relative
//! to `query::current_status`, except `current_status` itself still does the
//! full scan `aggregate_status` was built for; this module's production path
//! does not, because here an index exists for the one kind that matters.

use std::collections::HashMap;
use std::sync::Arc;

use surge_core::RunId;
use surge_core::keys::NodeKey;
use surge_core::migrate_payload;
use surge_core::run_event::{EscalationCause, EventPayload};

use crate::runs::error::StorageError;
use crate::runs::reader::{ReadEvent, RunReader};
use crate::runs::registry::RunSummary;
use crate::runs::seq::EventSeq;
use crate::runs::storage::Storage;

/// One `EscalationRequested` event, folded from the run's durable event log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationRecord {
    /// Sequence number of the `EscalationRequested` event.
    pub seq: EventSeq,
    /// Typed origin — see [`EscalationCause`].
    pub cause: EscalationCause,
    /// The node whose agent stage was active when this escalation fired —
    /// the most recent `StageEntered` at or before this event's seq. `None`
    /// if no stage had entered yet (a bootstrap-flow escalation can fire
    /// before any pipeline node exists).
    pub node: Option<NodeKey>,
    /// Free-form operator-readable explanation, unchanged from the event.
    pub reason: String,
    /// Observation timestamp in unix epoch milliseconds.
    pub observed_at_ms: i64,
}

/// Fold `events` into every `EscalationRequested` it contains, attributing
/// each to the most recently entered node at the time it was raised.
///
/// Pure — no I/O — so it is unit-testable against synthetic event slices,
/// the same shape as `query::aggregate_status`.
#[must_use]
pub fn fold_escalations(events: &[ReadEvent]) -> Vec<EscalationRecord> {
    let mut active_node: Option<NodeKey> = None;
    let mut out = Vec::new();
    for ev in events {
        match ev.payload.payload() {
            EventPayload::StageEntered { node, .. } => {
                active_node = Some(node.clone());
            },
            EventPayload::EscalationRequested { reason, cause, .. } => {
                out.push(EscalationRecord {
                    seq: ev.seq,
                    cause: *cause,
                    node: active_node.clone(),
                    reason: reason.clone(),
                    observed_at_ms: ev.timestamp_ms,
                });
            },
            _ => {},
        }
    }
    out
}

/// Read every `EscalationRequested` event this run has raised, filtered in
/// SQL (`WHERE kind = 'EscalationRequested'`, backed by `idx_events_kind`) —
/// never a full-log scan. Node attribution reuses the same "most recently
/// entered node" rule as [`fold_escalations`], expressed as a correlated
/// subquery against `stage_executions.started_seq` (that view is itself
/// already maintained in the append transaction, so no per-escalation event
/// replay is needed to answer it).
///
/// # Errors
/// Returns [`StorageError`] if the underlying SQL read fails, or if a row
/// tagged `kind = 'EscalationRequested'` fails to decode as that variant
/// (a data-integrity bug, not a normal outcome — the tag and the payload
/// are written together, in the same `INSERT`, by `runs::writer`).
pub async fn read_escalations(reader: &RunReader) -> Result<Vec<EscalationRecord>, StorageError> {
    let pool = reader.pool.clone();
    tokio::task::spawn_blocking(move || -> Result<Vec<EscalationRecord>, StorageError> {
        let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT e.seq, e.timestamp, e.payload, e.schema_version,
                    (SELECT se.node_id FROM stage_executions se
                     WHERE se.started_seq <= e.seq
                     ORDER BY se.started_seq DESC LIMIT 1) AS node_id
             FROM events e
             WHERE e.kind = 'EscalationRequested'
             ORDER BY e.seq",
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(2)?;
            Ok((
                EventSeq(row.get::<_, i64>(0)? as u64),
                row.get::<_, i64>(1)?,
                blob,
                row.get::<_, i64>(3)? as u32,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;

        let mut out = Vec::new();
        for r in rows {
            let (seq, timestamp_ms, blob, schema_version, node_id) = r?;
            let payload = migrate_payload(schema_version, &blob)
                .map_err(|e| StorageError::MigrationFailed(format!("seq={}: {e}", seq.0)))?;
            let EventPayload::EscalationRequested { reason, cause, .. } = payload else {
                return Err(StorageError::MigrationFailed(format!(
                    "seq={}: row tagged kind='EscalationRequested' decoded as a different \
                     variant — writer/kind-column mismatch",
                    seq.0
                )));
            };
            let node = node_id
                .map(|s| NodeKey::try_from(s.as_str()))
                .transpose()
                .map_err(|e| {
                    StorageError::MigrationFailed(format!(
                        "seq={}: stage_executions.node_id {e}",
                        seq.0
                    ))
                })?;
            out.push(EscalationRecord {
                seq,
                cause,
                node,
                reason,
                observed_at_ms: timestamp_ms,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| StorageError::Pool(e.to_string()))?
}

/// True if `cause` originates from `surge_orchestrator::guard::LoopGuard`
/// (a repeated tool call or a node past its wall-clock budget) — the class
/// a run being "stopped by the guard" means, as opposed to MCP
/// restart-exhaustion or a bootstrap/roadmap-amendment edit-loop cap.
#[must_use]
pub fn is_loop_guard_cause(cause: EscalationCause) -> bool {
    matches!(
        cause,
        EscalationCause::LoopGuardRepeatedToolCall | EscalationCause::LoopGuardNodeDeadline
    )
}

/// One run [`list_loop_guard_stopped_runs`] could not scan — its per-run
/// event log could not be opened or read. The scan continues past it
/// rather than failing the whole cross-run query: mirrors
/// `surge-cli::commands::inbox::scan_run_capacity`'s rule, "a run whose
/// reader has already been pruned must never block the rest of the
/// listing" — a stale-registry-row-with-no-per-run-directory case that
/// really occurs (`OpenError::RunNotFound`), not a hypothetical one.
#[derive(Debug, Clone)]
pub struct SkippedRun {
    /// The run whose log could not be read.
    pub run_id: RunId,
    /// Human-readable reason, from the underlying `OpenError`/`StorageError`.
    pub reason: String,
}

/// Every run in `runs` whose durable event log carries at least one
/// loop-guard escalation, mapped to the first such [`EscalationCause`]
/// found — the answer to "which runs did the loop guard stop", asked in
/// the plural. `surge_persistence::memory::audit::run_audit` is the one
/// caller today: it needs to correlate memory claims against more than one
/// run at a time, which [`read_escalations`] (about exactly one run) can't
/// answer on its own.
///
/// Never fails outright: a run whose reader can't be opened or whose log
/// can't be read is logged (`tracing::debug!`) and returned in the second
/// element instead of aborting the scan (see [`SkippedRun`]) — the caller
/// (`run_audit`) surfaces each one as a `caveats` entry rather than losing
/// the whole report to one unreadable run.
///
/// # Cost — read this before calling it from a hot path
/// [`read_escalations`] answers the singular question — "did THIS run get
/// stopped by the guard" — with one indexed SQL query (`idx_events_kind`)
/// against that run's own per-run SQLite file; see this module's doc
/// comment. The plural question asked here has no equivalent single-query
/// answer: `crate::runs::registry`'s `runs` table holds only coarse
/// metadata (id, status, timestamps) and carries no escalation index, so
/// there is no registry row to filter on. This function instead opens a
/// [`RunReader`] for **every** run in `runs` and re-runs
/// [`read_escalations`] against each one — O(len(runs)) SQLite opens and
/// indexed queries, not one query, and each open additionally builds an
/// r2d2 connection pool sized `StorageConfig::reader_pool_size` (default
/// 4) plus its background reaper thread pool — `Storage::open_run_reader`'s
/// existing construction, shared by every caller (`surge-cli::commands::
/// inbox` included), not something narrowing `runs` removes. `run_audit`
/// bounds `len(runs)` to the runs at least one claim actually names
/// (`run_id_from_source`), not the whole registry — the main lever
/// available here without changing `open_run_reader` itself.
///
/// Closing the multi-query cost for real would mean denormalizing an
/// escalation flag into the registry `runs` table, written from wherever
/// `EscalationRequested` is appended
/// (`surge-orchestrator::engine::stage::agent`) — through `surge-daemon`,
/// the only process that currently writes registry status (every
/// `registry::update_status` caller lives in `surge-daemon`/`surge-cli`;
/// the per-run writer itself never touches the registry pool). That is a
/// schema change plus a new cross-crate write path, not something this
/// crate can close on its own for this caller. Recorded here, and in
/// `.autopilot/competitive-waves/interfaces.md`, as the accepted cost of
/// answering this question today rather than solved silently.
///
/// Deliberately does not filter `runs` by status before scanning: a loop
/// guard trip that races a dead daemon can leave its `EscalationRequested`
/// durably in the per-run log before `RunFailed` (and the registry status
/// update that follows it) ever gets appended, so the run reads `Crashed`
/// in the registry, not `Failed` — the same class of defect this repo's
/// `RunStatus`-filtering lesson names (`interfaces.md`, "Факт о
/// `RunStatus`..."). The caller decides which runs go into `runs`; this
/// function never narrows that set further.
pub async fn list_loop_guard_stopped_runs(
    storage: &Arc<Storage>,
    runs: &[RunSummary],
) -> (HashMap<RunId, EscalationCause>, Vec<SkippedRun>) {
    let mut stopped = HashMap::new();
    let mut skipped = Vec::new();
    for run in runs {
        let reader = match storage.open_run_reader(run.id).await {
            Ok(reader) => reader,
            Err(e) => {
                tracing::debug!(
                    run_id = %run.id,
                    error = %e,
                    "skipping run for loop-guard scan: could not open its per-run event log"
                );
                skipped.push(SkippedRun {
                    run_id: run.id,
                    reason: e.to_string(),
                });
                continue;
            },
        };
        let escalations = match read_escalations(&reader).await {
            Ok(escalations) => escalations,
            Err(e) => {
                tracing::debug!(
                    run_id = %run.id,
                    error = %e,
                    "skipping run for loop-guard scan: could not read its escalations"
                );
                skipped.push(SkippedRun {
                    run_id: run.id,
                    reason: e.to_string(),
                });
                continue;
            },
        };
        if let Some(found) = escalations.iter().find(|e| is_loop_guard_cause(e.cause)) {
            stopped.insert(run.id, found.cause);
        }
    }
    (stopped, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SessionId;
    use surge_core::keys::NodeKey;
    use surge_core::migrations::MAX_SUPPORTED_VERSION;
    use surge_core::run_event::VersionedEventPayload;

    fn event(seq: u64, timestamp_ms: i64, payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms,
            kind: "fixture".to_owned(),
            payload: VersionedEventPayload {
                schema_version: MAX_SUPPORTED_VERSION,
                payload,
            },
        }
    }

    #[test]
    fn attributes_the_escalation_to_the_most_recently_entered_node() {
        let node = NodeKey::try_from("implement").unwrap();
        let events = vec![
            event(
                1,
                1_000,
                EventPayload::StageEntered {
                    node: node.clone(),
                    attempt: 1,
                },
            ),
            event(
                2,
                2_000,
                EventPayload::SessionOpened {
                    node: node.clone(),
                    session: SessionId::new(),
                    agent: "claude-opus-4-7".into(),
                    agent_id: None,
                },
            ),
            event(
                3,
                3_000,
                EventPayload::EscalationRequested {
                    stage: None,
                    reason: "node loop guard: node has run for 3600s, past its 3600s \
                              wall-clock budget; escalating"
                        .into(),
                    cause: EscalationCause::LoopGuardNodeDeadline,
                },
            ),
        ];

        let found = fold_escalations(&events);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].node.as_ref(), Some(&node));
        assert_eq!(found[0].cause, EscalationCause::LoopGuardNodeDeadline);
        assert!(is_loop_guard_cause(found[0].cause));
    }

    #[test]
    fn does_not_misclassify_mcp_or_bootstrap_causes_as_loop_guard() {
        let events = vec![
            event(
                1,
                1_000,
                EventPayload::EscalationRequested {
                    stage: None,
                    reason: "MCP server 'flaky' restart policy exhausted".into(),
                    cause: EscalationCause::McpRestartsExhausted,
                },
            ),
            event(
                2,
                2_000,
                EventPayload::EscalationRequested {
                    stage: Some(surge_core::run_event::BootstrapStage::Flow),
                    reason: "edit-loop cap exceeded".into(),
                    cause: EscalationCause::BootstrapEditLoopExhausted,
                },
            ),
        ];

        let found = fold_escalations(&events);
        assert_eq!(found.len(), 2);
        assert!(!is_loop_guard_cause(found[0].cause));
        assert!(!is_loop_guard_cause(found[1].cause));
    }

    #[test]
    fn no_escalation_yields_empty() {
        assert!(fold_escalations(&[]).is_empty());
    }

    /// Proves the SQL query itself (kind filter + `stage_executions`
    /// correlated subquery), not just the pure fold — against a real
    /// on-disk per-run database, the same one `read_escalations` reads in
    /// production. `fold_escalations`'s tests above exercise the same
    /// node-attribution rule on a synthetic slice; this one exercises the
    /// hand-written SQL that replaced the full-log scan.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_escalations_uses_the_kind_index_and_attributes_the_node() {
        use crate::runs::storage::Storage;
        use surge_core::run_event::VersionedEventPayload;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let node = NodeKey::try_from("implement").unwrap();
        writer
            .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            }))
            .await
            .unwrap();
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::EscalationRequested {
                    stage: None,
                    reason: "node loop guard: node has run for 0s, past its 0s wall-clock \
                              budget; escalating"
                        .into(),
                    cause: EscalationCause::LoopGuardNodeDeadline,
                },
            ))
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let found = read_escalations(&reader).await.unwrap();

        assert_eq!(found.len(), 1, "expected exactly one escalation row");
        assert_eq!(found[0].cause, EscalationCause::LoopGuardNodeDeadline);
        assert_eq!(found[0].node.as_ref(), Some(&node));
        assert!(is_loop_guard_cause(found[0].cause));
    }

    /// The cross-run entry point: across three real runs (one plain, one
    /// guard-tripped, one escalated for an unrelated bootstrap reason),
    /// only the guard-tripped one comes back — proving both that the scan
    /// finds the run it should and that it does not misclassify the
    /// bootstrap-cause run the way a text-matching approach would.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_loop_guard_stopped_runs_finds_only_the_guard_tripped_run() {
        use crate::runs::registry::RunFilter;
        use crate::runs::storage::Storage;
        use surge_core::run_event::VersionedEventPayload;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let plain_run = surge_core::RunId::new();
        let plain_writer = storage
            .create_run(plain_run, dir.path(), None)
            .await
            .unwrap();
        drop(plain_writer);

        let guard_tripped_run = surge_core::RunId::new();
        let guard_writer = storage
            .create_run(guard_tripped_run, dir.path(), None)
            .await
            .unwrap();
        guard_writer
            .append_event(VersionedEventPayload::new(
                EventPayload::EscalationRequested {
                    stage: None,
                    reason: "node loop guard: tool 'shell_exec' called 4 times in a row \
                              (threshold 3); escalating instead of dispatching it again"
                        .into(),
                    cause: EscalationCause::LoopGuardRepeatedToolCall,
                },
            ))
            .await
            .unwrap();
        guard_writer.flush().await.unwrap();
        drop(guard_writer);

        let bootstrap_escalated_run = surge_core::RunId::new();
        let bootstrap_writer = storage
            .create_run(bootstrap_escalated_run, dir.path(), None)
            .await
            .unwrap();
        bootstrap_writer
            .append_event(VersionedEventPayload::new(
                EventPayload::EscalationRequested {
                    stage: Some(surge_core::run_event::BootstrapStage::Flow),
                    reason: "edit-loop cap exceeded".into(),
                    cause: EscalationCause::BootstrapEditLoopExhausted,
                },
            ))
            .await
            .unwrap();
        bootstrap_writer.flush().await.unwrap();
        drop(bootstrap_writer);

        let runs = storage.list_runs(RunFilter::default()).await.unwrap();
        assert_eq!(runs.len(), 3, "all three seeded runs must be listed");

        let (stopped, skipped) = list_loop_guard_stopped_runs(&storage, &runs).await;

        assert!(
            skipped.is_empty(),
            "every seeded run has a readable log: {skipped:?}"
        );
        assert_eq!(
            stopped.len(),
            1,
            "only the guard-tripped run may be reported: {stopped:?}"
        );
        assert_eq!(
            stopped.get(&guard_tripped_run),
            Some(&EscalationCause::LoopGuardRepeatedToolCall)
        );
        assert!(!stopped.contains_key(&plain_run));
        assert!(!stopped.contains_key(&bootstrap_escalated_run));
    }

    /// Blocker regression: a registry row with no per-run directory behind
    /// it (`OpenError::RunNotFound` — the exact shape a pruned or
    /// never-fully-created run produces) must not fail the scan for every
    /// other run. Measured before this fix: the whole function returned
    /// `Err`, and `run_audit` printed no report at all for zero claims.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_loop_guard_stopped_runs_skips_an_unreadable_run_instead_of_failing_the_scan() {
        use crate::runs::registry::{self, RunFilter, RunSummary};
        use surge_core::RunStatus;
        use surge_core::run_event::VersionedEventPayload;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let good_run = surge_core::RunId::new();
        let writer = storage
            .create_run(good_run, dir.path(), None)
            .await
            .unwrap();
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::EscalationRequested {
                    stage: None,
                    reason: "node loop guard: node has run for 0s, past its 0s wall-clock \
                              budget; escalating"
                        .into(),
                    cause: EscalationCause::LoopGuardNodeDeadline,
                },
            ))
            .await
            .unwrap();
        writer.flush().await.unwrap();
        drop(writer);

        // A registry row with no per-run directory at all — `insert_run`
        // directly, bypassing `create_run`'s directory setup.
        let orphaned_run = surge_core::RunId::new();
        registry::insert_run(
            &storage.registry_pool,
            &RunSummary {
                id: orphaned_run,
                project_path: dir.path().to_path_buf(),
                pipeline_template: None,
                status: RunStatus::Failed,
                started_at_ms: 1,
                ended_at_ms: Some(2),
                daemon_pid: None,
                wake_at_ms: None,
            },
        )
        .unwrap();

        let runs = storage.list_runs(RunFilter::default()).await.unwrap();
        assert_eq!(runs.len(), 2);

        let (stopped, skipped) = list_loop_guard_stopped_runs(&storage, &runs).await;

        assert_eq!(
            stopped.get(&good_run),
            Some(&EscalationCause::LoopGuardNodeDeadline),
            "the orphaned run must not block scanning the good one: {stopped:?}"
        );
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].run_id, orphaned_run);
    }
}
