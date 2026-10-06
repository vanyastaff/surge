//! The fleet inbox: every run classified by what it needs from the operator
//! right now (Needs input / Working / Waiting / Recovery / Unknown / Done).
//!
//! There is no persisted "blocked on human" flag today, so attention is derived
//! authoritatively by folding each run's trusted event log into a
//! `RunState` and classifying it. Registry crash evidence is independent; terminal
//! registry labels alone cannot establish durable completion.
//!
//! **The capacity column is a registry fact, not a run fact (Task 12 M5).**
//! Before this milestone, a scan re-derived "is a rate limit in play" by
//! folding *this run's own* event log for `StageFailed`/`SessionOpened` — a
//! second full `read_events(0..MAX)` alongside the attention fold for every
//! non-terminal run, and a fresh reader + full read for every terminal
//! `Failed`/`Aborted`/`Crashed` run besides. That was two homes for one fact
//! and they disagreed on the first opportunity: run A's own journal knows
//! nothing of the 429 run B took on the *same* runtime, so A's column stayed
//! silent while the scheduler had already parked on that exact exhaustion.
//! Capacity is a fact about a runtime/account, not about any one run's
//! history, so it now comes from exactly one place — the registry-level
//! `runtime_capacity` table (`surge_persistence::runs::capacity`), the same
//! table the scheduler itself parks from — via a canonical-keyed point lookup
//! (`CanonicalRuntimeId::resolve` + `Storage::capacity_status`), never a raw
//! string and never a per-run scan. See [`classify`]'s doc for exactly which
//! `RunStatus` classes this populates the column for, and why the answer is
//! narrower than before.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use surge_core::capacity::{CapacityStatus, WakeBasis};
use surge_core::{Attention, RunId, RunState, TerminalReason};
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::{RunFilter, RunSummary};
use surge_persistence::task_ledger::TaskLedgerIndexFilter;

use crate::engine::capacity::CanonicalRuntimeId;
use crate::operator::error::OperatorError;
use surge_core::run_display::{RunDisplayState, WaitingReason};
use surge_persistence::runs::inspection::RunDatabaseInspection;

/// The inbox group a run belongs to. Serializes as the snake_case label
/// (`needs_input`, `working`, `waiting`, `recovery`, `unknown`, `done`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionGroup {
    /// Blocked on an operator answer.
    NeedsInput,
    /// Making progress.
    Working,
    /// Parked until a wake time (provider rate limit).
    Waiting,
    /// Reached a terminal state.
    Done,
    /// Confirmed daemon loss requiring inspection.
    Recovery,
    /// Unreadable or missing journal evidence.
    Unknown,
}

impl AttentionGroup {
    /// The stable snake_case label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsInput => "needs_input",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Recovery => "recovery",
            Self::Unknown => "unknown",
        }
    }
}

/// Why a settled run is in the Done group. Serializes as the lowercase label
/// (`completed` | `failed` | `aborted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoneReason {
    /// Reached its terminal success node.
    Completed,
    /// Ended in failure.
    Failed,
    /// Aborted by the operator or the engine.
    Aborted,
}

impl DoneReason {
    /// The stable lowercase label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
        }
    }
}

impl From<TerminalReason> for DoneReason {
    fn from(reason: TerminalReason) -> Self {
        match reason {
            TerminalReason::Completed => Self::Completed,
            TerminalReason::Failed => Self::Failed,
            TerminalReason::Aborted => Self::Aborted,
        }
    }
}

/// One classified run for the inbox.
#[derive(Debug, Serialize)]
pub struct InboxEntry {
    /// Shared reason and next-action projection, separate from control decisions.
    pub display: RunDisplayState,
    /// The run. Serializes as its prefixed display form (`run-<ULID>`).
    #[serde(serialize_with = "serialize_display")]
    pub run_id: RunId,
    /// Project (worktree) path the run recorded.
    pub project_path: PathBuf,
    /// `needs_input` | `working` | `waiting` | `done`.
    pub attention: AttentionGroup,
    /// Why the run is done, when it is; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_reason: Option<DoneReason>,
    /// Whether a [`DoneReason::Completed`] run's success is backed by
    /// verifier evidence (spec §10/R30's shared
    /// [`surge_core::evidence::is_evidence_backed`] predicate, read via
    /// [`surge_persistence::task_ledger::TaskLedgerIndexRecord::is_evidence_backed`]
    /// — the same predicate `surge run report` and `surge ledger` apply).
    /// `None` for every other done reason (failed/aborted) and for
    /// every non-"done" attention: the question is only meaningful for a
    /// claimed success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_backed: Option<bool>,
    /// Active node for a working/blocked run, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_node: Option<String>,
    /// Prompt the operator must answer, when blocked with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Rate-limit capacity signal (R34–R36) for the runtime this run is
    /// currently parked on — a registry point-lookup (`runtime_capacity`),
    /// **not** anything folded from this run's own event log; see the
    /// module doc and [`classify`] for which `RunStatus`/`Attention` classes
    /// this is ever populated for. `NeverObserved` is the common case
    /// (R35.1); `Unclassified` (the registry saw *something* for this
    /// runtime it could not decode) is kept distinct from it rather than
    /// folded into the same silence — see `CapacityStatus`'s doc.
    #[serde(skip_serializing_if = "CapacityStatus::is_never_observed")]
    pub capacity: CapacityStatus,
    /// When a parked run (`attention == "waiting"`) is expected to resume on
    /// its own. `None` for every other attention — a run that is not parked
    /// has no wake time to show, not an unknown one. (Task 12 M5.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_at: Option<DateTime<Utc>>,
    /// Why `wake_at` is what it is — an actually-observed provider reset vs.
    /// a configured blind-backoff guess. Carried alongside `wake_at` rather
    /// than folded into a display string, so a machine consumer of
    /// `--format json` learns the same typed fact an operator reading the
    /// text output sees, not just that the run is parked. `None` exactly
    /// when `wake_at` is `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_basis: Option<WakeBasis>,
    /// Run start time, milliseconds since the Unix epoch.
    pub started_at_ms: i64,
}

fn serialize_display<S: serde::Serializer>(id: &RunId, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(id)
}

/// List runs for `project_path` (or all) and classify each.
///
/// Every run is classified before `done_limit` applies; only the settled Done
/// tail is capped at `done_limit` entries.
///
/// # Errors
/// Returns [`OperatorError`] if the run registry cannot be listed or a
/// non-terminal run's event log cannot be read or folded.
pub async fn collect_entries(
    storage: &Arc<Storage>,
    project_path: Option<PathBuf>,
    done_limit: usize,
) -> Result<Vec<InboxEntry>, OperatorError> {
    // Fetch ALL runs, not a newest-`done_limit` window: a run still blocked on human
    // input can be older than `done_limit` more-recently-started (settled) runs, and
    // it must never fall out of the NEEDS INPUT group — the whole point of the
    // inbox. This is not free — every non-terminal run still costs a full
    // `read_events(0..MAX)` for its attention fold (unrelated to the capacity
    // column; see the module doc) — but that cost is accepted debt for a
    // handful of active runs, not a reason to risk dropping one from NEEDS
    // INPUT by pre-truncating before classification. Terminal runs cost no
    // event read at all, for attention or capacity alike.
    let summaries = storage
        .list_runs(RunFilter {
            status: None,
            project_path,
            limit: None,
        })
        .await
        .map_err(OperatorError::ListRuns)?;
    let mut entries = Vec::with_capacity(summaries.len());
    for summary in &summaries {
        entries.push(classify(storage, summary).await?);
    }
    // Bound only the settled/Done tail: keep every NEEDS INPUT / WORKING entry,
    // cap Done at `done_limit` (Done is a count/`--all` view anyway).
    entries.sort_by_key(|e| e.attention == AttentionGroup::Done);
    let done_start = entries
        .iter()
        .position(|e| e.attention == AttentionGroup::Done)
        .unwrap_or(entries.len());
    entries.truncate(done_start.saturating_add(done_limit));
    Ok(entries)
}

/// Classify one run using durable state and independent registry evidence.
/// Registry lifecycle labels cannot substitute for a readable journal. Crashed is
/// not completion: read-only inspection may show a newer terminal event, otherwise
/// recovery requires operator inspection. Missing, corrupt or invalid journals
/// yield an Unknown entry without hiding healthy runs. Capacity is looked up
/// only for the runtime recorded by this run's current parked event.
pub async fn classify(
    storage: &Arc<Storage>,
    summary: &RunSummary,
) -> Result<InboxEntry, OperatorError> {
    let base = |attention: AttentionGroup, done_reason, active_node, prompt, capacity| InboxEntry {
        display: RunDisplayState::Unknown,
        run_id: summary.id,
        project_path: summary.project_path.clone(),
        attention,
        done_reason,
        active_node,
        prompt,
        capacity,
        // Set only by the `Attention::Waiting` arm below, via struct-update
        // syntax — every other attention has no wake time to show.
        wake_at: None,
        wake_basis: None,
        // Set only for a "done"+"completed" entry, via struct-update syntax
        // below — every other
        // attention/reason has no evidence-backing question to answer.
        evidence_backed: None,
        started_at_ms: summary.started_at_ms,
    };

    // Inspect without creating/migrating absent journals. One bad run never hides its peers.
    let state = match storage.inspect_run(summary.id).await {
        Ok(inspection) => match inspection.database {
            RunDatabaseInspection::Present { events } if !events.is_empty() => {
                surge_persistence::runs::query::fold_read_events(summary.id, &events).ok()
            },
            _ => None,
        },
        Err(error) => {
            tracing::warn!(run_id = %summary.id, %error, "inbox journal evidence unavailable");
            None
        },
    };
    let display = RunDisplayState::from_state(state.as_ref(), Some(summary.status));
    let attention_group = match display {
        RunDisplayState::Waiting(WaitingReason::RecoveryRequired) => Some(AttentionGroup::Recovery),
        RunDisplayState::Unknown => Some(AttentionGroup::Unknown),
        _ => None,
    };
    if let Some(group) = attention_group {
        return Ok(InboxEntry {
            display,
            ..base(group, None, None, None, CapacityStatus::NeverObserved)
        });
    }
    let Some(state) = state else {
        return Ok(base(
            AttentionGroup::Unknown,
            None,
            None,
            None,
            CapacityStatus::NeverObserved,
        ));
    };
    let active_node = active_node(&state);
    let attention = state.attention();
    // Capacity is a registry point-lookup keyed on *this run's own* parked
    // runtime, not a fold over its journal — see the module doc. Every
    // attention other than `Waiting` reads `NeverObserved`; see `classify`'s
    // own doc for why that is the correct class, not merely the cheap one.
    let capacity = match &attention {
        Attention::Waiting {
            runtime: Some(raw_runtime),
            ..
        } => runtime_capacity_status(storage, raw_runtime).await,
        Attention::Waiting { runtime: None, .. }
        | Attention::NeedsInput
        | Attention::Working
        | Attention::Done(_) => CapacityStatus::NeverObserved,
    };
    let mut entry = match attention {
        Attention::NeedsInput => base(
            AttentionGroup::NeedsInput,
            None,
            active_node,
            state.pending_prompt().map(ToOwned::to_owned),
            capacity,
        ),
        Attention::Working => base(AttentionGroup::Working, None, active_node, None, capacity),
        // Task 12 M5: the wake time and its basis (observed provider reset
        // vs. a policy-backoff guess) ride along on the entry itself, typed,
        // rather than only being visible as the "waiting" label — see
        // `InboxEntry::wake_at`/`wake_basis`.
        Attention::Waiting { until, basis, .. } => InboxEntry {
            wake_at: Some(until),
            wake_basis: Some(basis),
            ..base(AttentionGroup::Waiting, None, active_node, None, capacity)
        },
        // A run whose registry status has not yet caught up to a
        // `RunCompleted`/`RunFailed`/`RunAborted` its own log already
        // recorded (see the module doc's `Attention::Done` handling above).
        // `evidence_backed` still needs the registry's task-ledger index,
        // not the just-folded `state`: the fold's `RunMemory` (including its
        // `LedgerState`) is discarded the moment a terminal event lands
        // (`run_state::fold`'s own doc), so this queries the same registry
        // index, for the one reason (`Completed`) where proof is meaningful.
        // A stale registry lifecycle label cannot substitute for this fold.
        Attention::Done(reason) => {
            let evidence_backed = match reason {
                TerminalReason::Completed => {
                    evidence_backed_for_completed_run(storage, summary.id).await
                },
                TerminalReason::Failed | TerminalReason::Aborted => None,
            };
            InboxEntry {
                evidence_backed,
                ..base(
                    AttentionGroup::Done,
                    Some(reason.into()),
                    None,
                    None,
                    capacity,
                )
            }
        },
    };
    entry.display = display;
    Ok(entry)
}

/// Whether a `Completed` run's terminal success is backed by verifier
/// evidence (spec §10/R30) — a cheap, indexed registry query (the same
/// `task_ledger_index` `surge ledger` reads), not a full event-log read.
/// Classification has already established completion from a trusted journal.
/// Verification remains a separate indexed fact: the terminal core state no
/// longer retains its pipeline ledger, so success proof comes from the shared
/// registry index instead of another event-log scan.
///
/// Zero ledger rows for this run reads `Some(false)`, not `None`: a flow
/// that never ran a verifier at all is exactly the "success without proof"
/// case R30 exists to flag, not an unknown. Degrades to `None` only on a
/// storage error — "we could not find out" is a different fact from "we
/// asked and found nothing," the same distinction `runtime_capacity_status`
/// draws for the capacity column.
///
/// `all`, not `any`, across every ledger row this run recorded: one verified
/// task among several rejected ones is not a proven run. `!is_empty()` is
/// required alongside `all` because `all` on an empty iterator is vacuously
/// `true`, which would contradict the zero-rows case above.
async fn evidence_backed_for_completed_run(storage: &Arc<Storage>, run_id: RunId) -> Option<bool> {
    let mut records = storage
        .task_ledger_store()
        .list(&TaskLedgerIndexFilter {
            status: None,
            project_path: None,
            run_id: Some(run_id),
            discovered_only: false,
            limit: None,
        })
        .inspect_err(|err| {
            tracing::debug!(
                run_id = %run_id,
                error = %err,
                "task-ledger index read failed; evidence-backed status unknown"
            );
        })
        .ok()?;
    super::verification::enrich_records(storage, &mut records);
    if records.iter().any(|record| {
        record.freshness == surge_core::verification_evidence::ProofFreshness::Unknown
    }) {
        return None;
    }
    Some(
        !records.is_empty()
            && records
                .iter()
                .all(surge_persistence::task_ledger::TaskLedgerIndexRecord::is_evidence_backed),
    )
}

/// Point-read the registry's current capacity status for the runtime a
/// parked run recorded in its own `RunParked.runtime` (Task 12 M5) — the
/// registry-fact half of the module doc's split. `raw_runtime` has always
/// been canonical since `RunParked`'s introduction (every write site builds
/// it via `CanonicalRuntimeId::resolve` — see `engine::run_task`), unlike
/// the legacy `SessionOpened.agent_id` the old per-run scan keyed off,
/// which predates that write-site fix and can still carry a raw, un-
/// normalized value on a run started before it landed (ADR-0016, M1). This
/// function does not inherit that risk — it never reads `agent_id` at
/// all — but re-resolves through [`CanonicalRuntimeId::resolve`] anyway
/// rather than trusting the string, the same discipline the write side
/// (`CapacityLedger::observe`) already applies: the read half of a typed
/// contract should not depend on every writer, past or future, having
/// gotten it right.
///
/// A read failure (registry unreachable) degrades to `NeverObserved` —
/// best-effort, same as the scan this replaces: one run's optional capacity
/// line must never block the rest of the inbox listing.
async fn runtime_capacity_status(storage: &Arc<Storage>, raw_runtime: &str) -> CapacityStatus {
    let canonical = CanonicalRuntimeId::resolve(&surge_acp::Registry::builtin(), raw_runtime);
    storage
        .capacity_status(canonical.as_str())
        .await
        .unwrap_or_else(|err| {
            tracing::debug!(
                runtime = %canonical,
                error = %err,
                "capacity status read failed; showing none"
            );
            CapacityStatus::NeverObserved
        })
}

fn active_node(state: &RunState) -> Option<String> {
    match state {
        RunState::Pipeline { cursor, .. } => Some(cursor.node.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crashed_run_remains_actionable_when_done_history_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let id = RunId::new();
        let writer = storage.create_run(id, dir.path(), None).await.unwrap();
        append(&writer, vec![run_started()]).await;
        writer.flush().await.unwrap();
        storage
            .set_run_status(&id, RunStatus::Crashed, None)
            .await
            .unwrap();
        let entries = collect_entries(&storage, None, 0).await.unwrap();
        assert_eq!(
            entries.len(),
            1,
            "recovery is actionable, not capped done history"
        );
        assert_eq!(entries[0].attention.as_str(), "recovery");
        assert!(entries[0].done_reason.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unreadable_peer_is_unknown_and_durable_terminal_overrides_crash() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let broken = RunId::new();
        let writer = storage.create_run(broken, dir.path(), None).await.unwrap();
        writer.close().await.unwrap();
        let db = storage
            .home()
            .join("runs")
            .join(broken.to_string())
            .join("events.sqlite");
        std::fs::write(&db, b"corrupt journal").unwrap();
        let terminal = RunId::new();
        let writer = storage
            .create_run(terminal, dir.path(), None)
            .await
            .unwrap();
        append(
            &writer,
            vec![
                run_started(),
                EventPayload::RunAborted {
                    reason: "stopped".into(),
                },
            ],
        )
        .await;
        writer.flush().await.unwrap();
        storage
            .set_run_status(&terminal, RunStatus::Crashed, None)
            .await
            .unwrap();
        let entries = collect_entries(&storage, None, 100).await.unwrap();
        let unknown = entries.iter().find(|entry| entry.run_id == broken).unwrap();
        assert_eq!(unknown.attention, AttentionGroup::Unknown);
        assert_eq!(unknown.display.label(), "Run state is unconfirmed");
        let done = entries
            .iter()
            .find(|entry| entry.run_id == terminal)
            .unwrap();
        assert_eq!(done.attention, AttentionGroup::Done);
        assert_eq!(done.done_reason, Some(DoneReason::Aborted));
        assert_eq!(std::fs::read(db).unwrap(), b"corrupt journal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registry_terminal_without_journal_is_unconfirmed_not_done() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        for status in [RunStatus::Completed, RunStatus::Failed, RunStatus::Aborted] {
            let id = RunId::new();
            let writer = storage.create_run(id, dir.path(), None).await.unwrap();
            writer.flush().await.unwrap();
            writer.close().await.unwrap();
            storage.set_run_status(&id, status, Some(1)).await.unwrap();
            std::fs::remove_file(
                storage
                    .home()
                    .join("runs")
                    .join(id.to_string())
                    .join("events.sqlite"),
            )
            .unwrap();
            let summary = storage.inspect_run(id).await.unwrap().registry.unwrap();
            let entry = classify(&storage, &summary).await.unwrap();
            assert_eq!(entry.display, RunDisplayState::Unknown);
            assert_eq!(entry.attention, AttentionGroup::Unknown);
            assert!(entry.done_reason.is_none());
            let path = storage
                .home()
                .join("runs")
                .join(id.to_string())
                .join("events.sqlite");
            std::fs::write(&path, b"unreadable terminal journal").unwrap();
            let entry = classify(&storage, &summary).await.unwrap();
            assert_eq!(entry.display, RunDisplayState::Unknown);
            assert_eq!(std::fs::read(path).unwrap(), b"unreadable terminal journal");
        }
        assert_eq!(collect_entries(&storage, None, 0).await.unwrap().len(), 3);
    }
    use std::collections::BTreeMap;
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::content_hash::ContentHash;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::NodeKey;
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
    use surge_core::sandbox::SandboxMode;
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_core::{RunId, RunStatus};

    fn minimal_graph() -> Graph {
        let end = NodeKey::try_from("plan").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            end.clone(),
            Node {
                id: end.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "inbox-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: end,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    fn run_started() -> EventPayload {
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/proj"),
            initial_prompt: "x".into(),
            config: RunConfig {
                bootstrap_edit_loop_cap: None,
                budget: Default::default(),
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        }
    }

    fn pipeline_materialized() -> EventPayload {
        EventPayload::PipelineMaterialized {
            graph: Box::new(minimal_graph()),
            graph_hash: ContentHash::compute(b"g"),
        }
    }

    async fn append(writer: &surge_persistence::runs::RunWriter, payloads: Vec<EventPayload>) {
        for p in payloads {
            writer
                .append_event(VersionedEventPayload::new(p))
                .await
                .unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_groups_runs_by_attention() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        // Blocked run: RunStarted → PipelineMaterialized → HumanInputRequested.
        let blocked = RunId::new();
        let w = storage.create_run(blocked, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::HumanInputRequested {
                    node: NodeKey::try_from("plan").unwrap(),
                    session: None,
                    call_id: Some("c1".into()),
                    prompt: "Approve the plan?".into(),
                    schema: None,
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        // Working run: RunStarted → PipelineMaterialized (no gate).
        let working = RunId::new();
        let w2 = storage.create_run(working, &project, None).await.unwrap();
        append(&w2, vec![run_started(), pipeline_materialized()]).await;
        w2.flush().await.unwrap();

        // Done run: durable completion is also mirrored in the registry.
        let done = RunId::new();
        let w3 = storage.create_run(done, &project, None).await.unwrap();
        append(
            &w3,
            vec![
                run_started(),
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("plan").unwrap(),
                },
            ],
        )
        .await;
        w3.flush().await.unwrap();
        storage
            .set_run_status(&done, RunStatus::Completed, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        assert_eq!(entries.len(), 3);

        let find = |id: RunId| {
            entries
                .iter()
                .find(|e| e.run_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let b = find(blocked);
        assert_eq!(b.attention, AttentionGroup::NeedsInput);
        assert_eq!(b.prompt.as_deref(), Some("Approve the plan?"));
        assert_eq!(b.active_node.as_deref(), Some("plan"));

        assert_eq!(find(working).attention, AttentionGroup::Working);

        let d = find(done);
        assert_eq!(d.attention, AttentionGroup::Done);
        assert_eq!(d.done_reason, Some(DoneReason::Completed));
        // Spec §10/R30: a completed run that never ran a verifier reads
        // `Some(false)`, never `None` — "success without proof" is a known
        // fact, not an unanswered question.
        assert_eq!(d.evidence_backed, Some(false));
    }

    /// Spec §10/R30's three-surface requirement, the `surge inbox` third:
    /// two completed runs in the same project, one whose task ledger has an
    /// authorized `TaskVerified` and one with no ledger activity at all,
    /// must come back with different `evidence_backed` values — the same
    /// [`surge_core::evidence::is_evidence_backed`] predicate `surge run
    /// report` and `surge ledger` apply, read here through
    /// [`surge_persistence::task_ledger::TaskLedgerIndexRecord::is_evidence_backed`]
    /// over the registry's task-ledger index rather than a second, inbox-local
    /// copy of the rule.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn legacy_unbound_completion_and_registry_flags_do_not_prove_success() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        // Verified: a `TaskVerified` event lands in the per-run ledger view,
        // then gets mirrored into the registry index exactly the way the
        // engine does at run completion (`engine::mirror_task_ledger`).
        let verified_run = RunId::new();
        let wv = storage
            .create_run(verified_run, &project, None)
            .await
            .unwrap();
        append(
            &wv,
            vec![
                run_started(),
                EventPayload::TaskVerified {
                    task_id: "t1".into(),
                    node: NodeKey::try_from("verify_1").unwrap(),
                    evidence: ContentHash::compute(b"verification-report"),

                    report: None,
                },
            ],
        )
        .await;
        append(
            &wv,
            vec![EventPayload::RunCompleted {
                terminal_node: NodeKey::try_from("plan").unwrap(),
            }],
        )
        .await;
        wv.flush().await.unwrap();
        storage
            .sync_task_ledger_index(verified_run, &project)
            .await
            .unwrap();
        storage
            .set_run_status(&verified_run, RunStatus::Completed, Some(2))
            .await
            .unwrap();

        // Unverified: completes successfully but never touches the ledger.
        let unverified_run = RunId::new();
        let wu = storage
            .create_run(unverified_run, &project, None)
            .await
            .unwrap();
        append(&wu, vec![run_started()]).await;
        append(
            &wu,
            vec![EventPayload::RunCompleted {
                terminal_node: NodeKey::try_from("plan").unwrap(),
            }],
        )
        .await;
        wu.flush().await.unwrap();
        storage
            .sync_task_ledger_index(unverified_run, &project)
            .await
            .unwrap();
        storage
            .set_run_status(&unverified_run, RunStatus::Completed, Some(1))
            .await
            .unwrap();

        // Boundary case, constructed directly against the registry store
        // (bypassing normal event replay, the way `task_ledger::tests` and
        // `ledger::tests` pin the same boundary at their own layers): a row
        // whose `verified` flag disagrees with its `status`. No production
        // writer can build this combination through the event log today
        // (`views.rs`'s `TaskStatusChanged`/`TaskVerified` handling always
        // sets them together), but the predicate this entry reads through
        // must not take that on faith either — this is the case a `status
        // == Completed && verified` → `||` mutation would silently pass
        // through as evidence-backed.
        let boundary_run = RunId::new();
        let wb = storage
            .create_run(boundary_run, &project, None)
            .await
            .unwrap();
        append(
            &wb,
            vec![
                run_started(),
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("plan").unwrap(),
                },
            ],
        )
        .await;
        wb.flush().await.unwrap();
        storage
            .task_ledger_store()
            .upsert(&surge_persistence::task_ledger::TaskLedgerIndexUpsert {
                run_id: boundary_run,
                task_id: "t1".into(),
                project_path: project.clone(),
                status: surge_core::RoadmapStatus::ReadyForVerification,
                verified: true,
                discovered_from: None,
                last_authority_node: None,
                updated_seq: 1,
                observed_at_ms: 0,
                accepted_by_human: false,
                requirement_revised: false,
            })
            .unwrap();
        storage
            .set_run_status(&boundary_run, RunStatus::Completed, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let find = |id: RunId| {
            entries
                .iter()
                .find(|e| e.run_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        assert_ne!(find(verified_run).evidence_backed, Some(true));
        assert_eq!(find(unverified_run).evidence_backed, Some(false));
        assert_eq!(find(boundary_run).evidence_backed, Some(false));
    }

    /// The aggregation the review round asked to pin: a run with one
    /// verified task and several rejected ones must not read as a proven
    /// success. `evidence_backed_for_completed_run` requires `!is_empty() &&
    /// all(..)` over every ledger row this run recorded — mutating that back
    /// to `.any(..)` turns this run's mixed-outcome fixture green when it
    /// should read `Some(false)`. Every fixture above this one carries 0 or 1
    /// ledger row, so none of them can catch that mutation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn evidence_backed_requires_every_task_verified_not_just_one() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let mixed_run = RunId::new();
        let wm = storage.create_run(mixed_run, &project, None).await.unwrap();
        append(
            &wm,
            vec![
                run_started(),
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("plan").unwrap(),
                },
            ],
        )
        .await;
        wm.flush().await.unwrap();
        storage
            .task_ledger_store()
            .upsert(&surge_persistence::task_ledger::TaskLedgerIndexUpsert {
                run_id: mixed_run,
                task_id: "t1".into(),
                project_path: project.clone(),
                status: surge_core::RoadmapStatus::Completed,
                verified: true,
                discovered_from: None,
                last_authority_node: None,
                updated_seq: 1,
                observed_at_ms: 0,
                accepted_by_human: false,
                requirement_revised: false,
            })
            .unwrap();
        for (i, task_id) in ["t2", "t3", "t4", "t5"].into_iter().enumerate() {
            storage
                .task_ledger_store()
                .upsert(&surge_persistence::task_ledger::TaskLedgerIndexUpsert {
                    run_id: mixed_run,
                    task_id: task_id.into(),
                    project_path: project.clone(),
                    status: surge_core::RoadmapStatus::FailedVerification,
                    verified: false,
                    discovered_from: None,
                    last_authority_node: None,
                    updated_seq: 2 + i as u64,
                    observed_at_ms: 0,
                    accepted_by_human: false,
                    requirement_revised: false,
                })
                .unwrap();
        }
        storage
            .set_run_status(&mixed_run, RunStatus::Completed, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == mixed_run)
            .unwrap_or_else(|| panic!("missing {mixed_run}"));
        assert_eq!(entry.evidence_backed, Some(false));
    }

    /// `classify`'s own doc names a race window: a run whose event log
    /// already recorded `RunCompleted` but whose registry `RunStatus`
    /// hasn't caught up yet (still non-terminal) takes the *fold* path
    /// (`Attention::Done`), not the terminal fast path
    /// `legacy_unbound_completion_and_registry_flags_do_not_prove_success`
    /// above exercises — a second, independent place `evidence_backed` gets
    /// computed. `cargo mutants` found this path uncovered: replacing
    /// `evidence_backed.flatten()`'s result with `None` in that arm survived
    /// every test until this one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn evidence_backed_is_computed_on_the_fold_path_too_when_registry_status_lags() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run_id = RunId::new();
        let writer = storage.create_run(run_id, &project, None).await.unwrap();
        // Registry status is left at `create_run`'s default (`Bootstrapping`,
        // non-terminal) — deliberately never calling `set_run_status` — so
        // `classify` takes the fold path below, not the terminal fast path.
        append(
            &writer,
            vec![
                run_started(),
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("end").unwrap(),
                },
            ],
        )
        .await;
        writer.flush().await.unwrap();
        storage
            .sync_task_ledger_index(run_id, &project)
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run_id)
            .unwrap_or_else(|| panic!("missing {run_id}"));
        assert_eq!(entry.attention, AttentionGroup::Done);
        assert_eq!(
            entry.evidence_backed,
            Some(false),
            "no verifier ran, so this must read Some(false), not None"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_surfaces_waiting_group_for_a_parked_run() {
        // Task 12 M1 (non-blocking review item): a parked run must land in
        // its own "waiting" group, not silently vanish from every printed
        // bucket (`needs_input`/`working`/`done` all filter by exact string
        // match — a fourth label that matches none of them would otherwise
        // only be visible via `--json`).
        use surge_core::capacity::WakeBasis;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let parked = RunId::new();
        let w = storage.create_run(parked, &project, None).await.unwrap();
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: Some("claude-acp".into()),
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::ObservedReset,
                    reason: "provider rate limit exhausted".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == parked)
            .expect("parked run present");
        assert_eq!(entry.attention, AttentionGroup::Waiting);
        // Task 12 M5: the wake time and its basis must ride along on the
        // entry itself — an operator (or a `--json` consumer) must be able
        // to answer "when does this come back, and why" without re-deriving
        // it from the run's event log.
        assert_eq!(entry.wake_at, Some(wake_at));
        assert_eq!(entry.wake_basis, Some(WakeBasis::ObservedReset));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_json_output_carries_wake_at_and_basis_for_a_parked_run() {
        // Task 12 M5 acceptance: a machine consumer of `--format json` must
        // not have to fall back to the text output (or re-scan the run's
        // event log) to learn a run is parked and when/why it wakes — the
        // same typed fact the printed WAITING group shows must be present
        // in the serialized entry.
        use surge_core::capacity::WakeBasis;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let parked = RunId::new();
        let w = storage.create_run(parked, &project, None).await.unwrap();
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: Some("claude-acp".into()),
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::PolicyBackoff,
                    reason: "blind backoff, no observed reset time".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let json = serde_json::to_value(&entries).unwrap();
        let entry = json
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["run_id"] == parked.to_string())
            .expect("parked run present in JSON output");

        assert_eq!(entry["attention"], "waiting");
        assert_eq!(entry["wake_at"], serde_json::json!(wake_at));
        assert_eq!(entry["wake_basis"], serde_json::json!("PolicyBackoff"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_attention_leaves_waiting_once_the_run_wakes() {
        // Task 12 M4: the other half of the sibling test above —
        // `RunWokeFromPark` must clear `RunState::Pipeline.parked` in the
        // fold, or a resumed run keeps showing "waiting" in `surge inbox`
        // while it is actually executing again.
        use surge_core::capacity::WakeBasis;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: Some("claude-acp".into()),
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::ObservedReset,
                    reason: "provider rate limit exhausted".into(),
                },
                EventPayload::RunWokeFromPark {},
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run)
            .expect("run present");
        assert_eq!(
            entry.attention,
            AttentionGroup::Working,
            "RunWokeFromPark must clear the parked state — a resumed run must not still show \
             as \"waiting\" in the inbox"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_waiting_run_capacity_reads_from_the_registry_not_the_journal() {
        // Task 12 M5: the positive case for the whole redesign. This run's
        // *own* journal never carries a `StageFailed`/window of any kind —
        // only `RunParked` (a run fact: which runtime, and until when).
        // `entry.capacity` can therefore only become `Known` by reading the
        // registry's `runtime_capacity` table, proving the column's source
        // moved rather than merely asserting it did.
        use surge_core::capacity::{CapacityWindow, WakeBasis};

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let observed_at = chrono::Utc::now();
        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(45)),
                observed_at,
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        let wake_at = observed_at + chrono::Duration::seconds(45);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: Some("claude-acp".into()),
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::ObservedReset,
                    reason: "provider rate limit exhausted".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run)
            .expect("run present");
        assert_eq!(entry.attention, AttentionGroup::Waiting);
        let window = entry.capacity.window().expect(
            "the registry's observation must surface even though this run's own \
                     journal carries no StageFailed at all",
        );
        assert_eq!(window.runtime(), "claude-acp");
        assert!(window.is_exhausted());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_two_runs_parked_on_the_same_runtime_converge_through_the_registry() {
        // The exact divergence this milestone closes (module doc): before
        // it, run A's own journal knew nothing of a 429 run B observed on
        // the *same* runtime, so their capacity columns disagreed. Now both
        // read the one registry row, so they agree — proven here with run A
        // parked under a raw, unnormalized alias ("claude") and run B under
        // the canonical spelling ("claude-acp"), so this also proves
        // `CanonicalRuntimeId::resolve` normalizes at read time rather than
        // trusting the journal's own string.
        use surge_core::capacity::{CapacityWindow, WakeBasis};

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let observed_at = chrono::Utc::now();
        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(60)),
                observed_at,
            ))
            .await
            .unwrap();

        let wake_at = observed_at + chrono::Duration::seconds(60);
        let park_event = |runtime: &str| EventPayload::RunParked {
            wake_at,
            runtime: Some(runtime.into()),
            worktree: dir.path().to_path_buf(),
            basis: WakeBasis::ObservedReset,
            reason: "provider rate limit exhausted".into(),
        };

        let run_a = RunId::new();
        let wa = storage.create_run(run_a, &project, None).await.unwrap();
        append(
            &wa,
            vec![run_started(), pipeline_materialized(), park_event("claude")],
        )
        .await;
        wa.flush().await.unwrap();

        let run_b = RunId::new();
        let wb = storage.create_run(run_b, &project, None).await.unwrap();
        append(
            &wb,
            vec![
                run_started(),
                pipeline_materialized(),
                park_event("claude-acp"),
            ],
        )
        .await;
        wb.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let find = |id: RunId| {
            entries
                .iter()
                .find(|e| e.run_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let window_a = find(run_a).capacity.window().expect(
            "run A must see the registry's observation despite its own RunParked \
                     recording the unnormalized alias \"claude\"",
        );
        let window_b = find(run_b)
            .capacity
            .window()
            .expect("run B must see the same observation");
        assert_eq!(
            window_a, window_b,
            "two runs parked on the same runtime must read the identical registry fact, not \
             two independently-derived ones"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_waiting_run_with_no_recorded_runtime_shows_never_observed() {
        // Task 12 plan point 10: absence of a runtime is absence of a fact,
        // not a fabricated key — a run parked via the legacy
        // no-profile-registry path (`RunParked.runtime: None`) has nothing
        // to look up, even though the registry has a real row for some
        // *other* runtime.
        use surge_core::capacity::{CapacityWindow, WakeBasis};

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                None,
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        let wake_at = chrono::Utc::now() + chrono::Duration::minutes(5);
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::RunParked {
                    wake_at,
                    runtime: None,
                    worktree: dir.path().to_path_buf(),
                    basis: WakeBasis::PolicyBackoff,
                    reason: "blind backoff, no observed reset time".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run)
            .expect("run present");
        assert_eq!(entry.attention, AttentionGroup::Waiting);
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_terminal_failed_run_never_consults_the_registry() {
        // Task 12 M5, deliberate narrowing (see `classify`'s own doc): a
        // terminal run gets no capacity registry lookup for this column,
        // even when a real, matching registry row
        // exists — a dead run has no cheap, correct way to be attributed to
        // a specific runtime, so it must not *appear* to inherit one. This
        // is the mutation-sensitive half of the redesign: without the
        // terminal branch's unconditional `NeverObserved`, this test would
        // go red the moment a future change tried reading the registry for
        // a terminal run too.
        use surge_core::capacity::CapacityWindow;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(30)),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(
            &w,
            vec![
                run_started(),
                pipeline_materialized(),
                EventPayload::StageFailed {
                    node: NodeKey::try_from("plan").unwrap(),
                    reason: "429 Too Many Requests; Retry-After: 30".into(),
                    retry_available: false,
                },
                EventPayload::RunFailed {
                    error: "rate limited".into(),
                },
            ],
        )
        .await;
        w.flush().await.unwrap();
        // Durable failure, like other durable terminal outcomes, has no
        // current parked runtime to correlate with a capacity registry row.
        storage
            .set_run_status(&run, RunStatus::Failed, Some(1))
            .await
            .unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run)
            .expect("run present");
        assert_eq!(entry.attention, AttentionGroup::Done);
        assert_eq!(
            entry.capacity,
            CapacityStatus::NeverObserved,
            "a terminal run must never surface a registry row, even one that matches its own \
             last-known runtime"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_working_run_never_consults_the_registry() {
        // The non-terminal counterpart to the test above: a run that is
        // `Working` (not currently parked) is, by the dispatch gate's own
        // guarantee (`capacity_decision_for` runs before every agent
        // stage), not blocked by capacity right now — so it reads
        // `NeverObserved` even with a matching registry row in play. Only
        // `Attention::Waiting` ever triggers the lookup; see `classify`'s
        // doc for the full `RunStatus` accounting this covers the
        // `Bootstrapping`/`Running` classes for.
        use surge_core::capacity::CapacityWindow;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        storage
            .observe_capacity(&CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(30)),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(&w, vec![run_started(), pipeline_materialized()]).await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run)
            .expect("run present");
        assert_eq!(entry.attention, AttentionGroup::Working);
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inbox_capacity_is_never_observed_when_no_failure_occurred() {
        // The primary case (R35.1): an ordinary run carries no rate-limit
        // signal, and this must read as absent, never a guess.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let storage = Storage::open(dir.path()).await.unwrap();

        let run = RunId::new();
        let w = storage.create_run(run, &project, None).await.unwrap();
        append(&w, vec![run_started(), pipeline_materialized()]).await;
        w.flush().await.unwrap();

        let entries = collect_entries(&storage, Some(project.clone()), 100)
            .await
            .unwrap();
        let entry = entries
            .iter()
            .find(|e| e.run_id == run)
            .expect("run present");
        assert_eq!(entry.capacity, CapacityStatus::NeverObserved);
    }
}
