//! Run Report — the one document a reviewer reads to accept or reject a run,
//! without opening the transcript (R33).
//!
//! [`RunReport::compile`] is a **pure projection of the event log**, not a
//! second source of run state (`.autopilot/competitive-waves/spec.md` §9;
//! [ADR 0017](../../../../docs/adr/0017-run-report-is-a-log-projection.md)).
//! Everything on [`RunReport`] is derived from the [`RunEvent`]s handed to
//! `compile` — nothing is read from a live engine, a materialized SQL view,
//! or `surge.toml`. Two consequences follow directly from that:
//!
//! - A run that never reached a terminal event still compiles: `compile`
//!   never errors, and [`RunReport::completion`] says [`RunCompletion::Incomplete`]
//!   instead of guessing (R27.1).
//! - Nothing here duplicates [`crate::run_state::fold`]. That fold is
//!   authoritative for a *live* run's next action, and by design its
//!   `RunMemory` bookkeeping (costs, outcomes, artifacts, the task ledger) is
//!   **discarded** the moment a `RunCompleted`/`RunFailed`/`RunAborted` event
//!   lands — `apply` replaces the whole `Pipeline` state with a bare
//!   `RunState::Terminal { kind, reason }` (see `run_state.rs`). That is
//!   correct for the engine (a terminal run needs no further bookkeeping) and
//!   wrong for a report, whose most important case is exactly a *completed*
//!   run. `compile` therefore keeps its own running tallies across the whole
//!   event slice instead of reading `fold`'s final state.
//!
//! ## Sections (spec §18)
//!
//! The report has exactly nine content sections, named as fields:
//! [`nodes`](RunReport::nodes), [`outcomes`](RunReport::outcomes),
//! [`verdicts`](RunReport::verdicts), [`evidence`](RunReport::evidence),
//! [`cost`](RunReport::cost), [`skills`](RunReport::skills),
//! [`memory_receipts`](RunReport::memory_receipts),
//! [`steers`](RunReport::steers), [`approvals`](RunReport::approvals).
//!
//! Beyond those nine, [`RunReport::header`] (what the run was asked to do,
//! and when it started/last progressed — R33 needs the identifying facts
//! before any section makes sense), [`RunReport::escalations`] (why a run
//! stopped making progress, when [`RunReport::completion`] alone answers
//! only "did it finish"), and [`RunReport::caveats`] (this report's own
//! coverage gaps, read by every renderer including the JSON form) round out
//! what a reviewer needs — these are structural/identifying facts the plan's
//! nine-section enumeration did not need to call out separately, not a
//! tenth-through-twelfth content section competing with the nine.
//!
//! ## `RunCompletion::Parked` is read from the log, not inferred
//!
//! `RunParked{wake_at, runtime, basis, reason}` and `RunWokeFromPark` are
//! ordinary log events, so `compile` distinguishes "paused on purpose,
//! resuming at a known time" ([`RunCompletion::Parked`]) from "stopped for
//! an unknown reason" ([`RunCompletion::Incomplete`]) — collapsing the two
//! would throw away a fact the log itself proves. This is narrower than
//! `RunStatus`'s registry-table classification (`Crashed` in particular is a
//! pid-liveness fact `Storage::list_runs` assigns, which no event ever
//! records, and stays outside `compile`'s reach) — see [`RunCompletion`]'s
//! own doc for the boundary.
//!
//! ## A named limitation: `memory_receipts` is always empty today
//!
//! [`crate::context_pack::PackReceipt`] — what a context-pack selection kept,
//! dropped, and why — exists and is computed at run-start
//! (`surge-orchestrator::project_context`), but nothing yet appends it to the
//! run event log (that module's own doc says so explicitly: "persisting it
//! into the run event log ... [is a] separate concern owned elsewhere").
//! `compile` reads only the event log, so [`RunReport::memory_receipts`] is
//! the type `.autopilot/competitive-waves/spec.md` §18 requires, wired to the
//! log — it is simply never populated until a future change emits an event
//! carrying a `PackReceipt`. This is not a bug in this module; it is named
//! here so nobody mistakes an empty list for "no memory was used."

mod render;

pub use render::{render_html, render_json, render_markdown};

use crate::capacity::WakeBasis;
use crate::content_hash::ContentHash;
use crate::context_pack::PackReceipt;
use crate::evidence::{NodeOutcome, is_evidence_backed};
use crate::graph::Graph;
use crate::id::RunId;
use crate::keys::{NodeKey, OutcomeKey};
use crate::roadmap::RoadmapStatus;
use crate::roadmap_patch::{RoadmapPatchApprovalDecision, RoadmapPatchId};
use crate::run_event::{
    BootstrapDecision, BootstrapStage, ElevationDecision, EscalationCause, EventPayload, RunEvent,
};
use crate::run_state::node_has_verification_authority;
use crate::skill::SkillProvider;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// The compiled report: one document, nine named sections, plus enough
/// header state ([`run_id`](Self::run_id), [`completion`](Self::completion))
/// to make sense of them standalone. Build one with [`RunReport::compile`];
/// render one with [`render_json`], [`render_markdown`], or [`render_html`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    /// The run this report describes.
    pub run_id: RunId,
    /// What the run was asked to do, and when it started/last progressed.
    pub header: RunHeader,
    /// Whether, and how, the run reached a terminal lifecycle event —
    /// R27.1's "explicit run not finished" for a torn log.
    pub completion: RunCompletion,
    /// Whether a [`RunCompletion::Completed`] success is backed by verifier
    /// evidence, or merely declared (spec §10/R30) — `true` iff every task
    /// this run's ledger ever tracked (not only the ones with an actual
    /// verifier verdict — see `compile`'s `ledger_task_ids`) is
    /// [`is_evidence_backed`], and at least one such task exists: one
    /// rejected/unverified/merely-discovered task among several verified
    /// ones still means the run is not a proven success. `None` when
    /// `completion` is not `Completed`: the question "was this proven" is
    /// only meaningful for a claimed success, not for a failure, an abort,
    /// or a run still in flight. `surge inbox` and `surge ledger` answer the
    /// same question through the same predicate, over the same set of
    /// tasks (the registry's `task_ledger_index`) — see
    /// `surge_core::evidence`'s module doc for how the three line up.
    pub evidence_backed: Option<bool>,
    /// Every escalation the run raised, in event order — "why did this run
    /// stop making progress," which [`Self::completion`] alone cannot
    /// answer for an [`RunCompletion::Incomplete`] run.
    pub escalations: Vec<EscalationEntry>,
    /// Every node the run entered, with its attempt count and last observed
    /// status.
    pub nodes: Vec<NodeSummary>,
    /// Every declared outcome a node reported (`OutcomeReported`), in the
    /// order the run reported them.
    pub outcomes: Vec<OutcomeEntry>,
    /// Each task's *current* verifier verdict — R33's evidence that a
    /// terminal success was actually checked, not merely declared. One entry
    /// per `task_id`, not a full history: a later verdict for a task
    /// (including a non-`Completed` `TaskStatusChanged` that sends it back
    /// for re-verification) replaces an earlier one, so this always reflects
    /// the ledger's own current state, the same state `surge inbox`/`surge
    /// ledger` read.
    pub verdicts: Vec<VerifierVerdict>,
    /// Every artifact produced, by a node or by a bootstrap stage.
    pub evidence: Vec<EvidenceEntry>,
    /// Aggregate token/dollar spend across the whole run.
    pub cost: CostTotals,
    /// Every skill pack the run bound onto a node (R14) — reconstructed
    /// entirely from `SkillBound` events, never from `surge.toml` or a
    /// node's declaration (a declaration is a request; `SkillBound` is what
    /// actually happened).
    pub skills: Vec<SkillUsage>,
    /// Context-pack selection receipts. See the module doc's "named
    /// limitation" — always empty until a future event carries this.
    pub memory_receipts: Vec<PackReceipt>,
    /// Operator steer messages delivered mid-run.
    pub steers: Vec<SteerEntry>,
    /// Every approval-shaped request/decision the run raised (skill trust
    /// gate, sandbox elevation, bootstrap stage gates, roadmap-amendment
    /// gates), in the order the log recorded them.
    pub approvals: Vec<ApprovalEntry>,
    /// Structural caveats about this report's own coverage — read by every
    /// renderer (`json`, `md`, `html` alike), so a machine consumer of the
    /// JSON form sees the same "this section can't be trusted as complete"
    /// warnings a human reading the Markdown or HTML form does, rather than
    /// silently getting `"memory_receipts": []` and reading it as "memory
    /// was not used." Always non-empty today: see the module doc's "named
    /// limitation."
    pub caveats: Vec<String>,
}

/// Structural note: no `EventPayload` variant carries a
/// [`crate::context_pack::PackReceipt`] into the log yet, so
/// [`RunReport::memory_receipts`] is always empty. Pushed onto
/// [`RunReport::caveats`] by every [`RunReport::compile`] call, in every
/// format, so this is never mistaken for "no memory was used" — see the
/// module doc's "named limitation."
const MEMORY_RECEIPTS_CAVEAT: &str = "memory_receipts is always empty: no event in this log \
     format carries a context-pack receipt yet (see surge_core::run_report's module doc) — \
     this is a known coverage gap, not evidence memory went unused.";

impl RunReport {
    fn empty(run_id: RunId) -> Self {
        Self {
            run_id,
            header: RunHeader::default(),
            completion: RunCompletion::Incomplete,
            evidence_backed: None,
            escalations: Vec::new(),
            nodes: Vec::new(),
            outcomes: Vec::new(),
            verdicts: Vec::new(),
            evidence: Vec::new(),
            cost: CostTotals::default(),
            skills: Vec::new(),
            memory_receipts: Vec::new(),
            steers: Vec::new(),
            approvals: Vec::new(),
            caveats: vec![MEMORY_RECEIPTS_CAVEAT.to_string()],
        }
    }

    /// Compile a report for `run_id` from its full event log.
    ///
    /// Pure — no I/O, no wall-clock, no randomness — and infallible: an
    /// event slice that never reaches a terminal lifecycle event compiles
    /// into a report with [`RunCompletion::Incomplete`] rather than an
    /// error, so a crashed or still-running run is exactly as reportable as
    /// a finished one (R27.1). `run_id` is taken as an explicit parameter
    /// rather than read off the first event, matching
    /// `surge_persistence::runs::query::aggregate_status`'s signature —
    /// an empty event slice (a run whose log has not been written yet)
    /// still produces a well-formed, empty report instead of `None`.
    #[must_use]
    pub fn compile(run_id: RunId, events: &[RunEvent]) -> Self {
        let mut report = Self::empty(run_id);
        // The graph in force at the time of a `TaskVerified` event, needed
        // to check verification authority (see `VerdictResult::Unauthorized`
        // doc). Replaced wholesale on `PipelineMaterialized` /
        // `GraphRevisionAccepted`, exactly the events that establish or
        // amend it for the live engine too.
        let mut active_graph: Option<Graph> = None;
        // Index into `report.nodes` by node key, so a node's second
        // `StageEntered` updates its existing entry instead of appending a
        // duplicate. `report.nodes` itself stays in first-seen (execution)
        // order — a `HashMap` alone would not preserve that.
        let mut node_positions: HashMap<NodeKey, usize> = HashMap::new();
        // Every task id the ledger ever tracked (`TaskDiscovered`,
        // `TaskVerified`, `TaskStatusChanged` alike), mapped to its
        // `{status, verified}` pair — spec §10's "same fact on every
        // surface" requires this to be the SAME *set of tasks*
        // `surge_persistence::task_ledger` mirrors into the registry
        // (`LedgerState.tasks`'s key set), not just the narrower set that
        // happened to receive a verifier verdict: a task that was only ever
        // `TaskDiscovered` (never verified, never rejected) is a row in
        // `task_ledger_index` and must count against this run's proof the
        // same way it counts there — see [`RunReport::evidence_backed`]'s
        // computation below, which reads this map (through the same
        // [`is_evidence_backed`] predicate every other surface uses)
        // instead of `report.verdicts`. Each arm below mirrors the matching
        // `LedgerState::record_*` method line for line (see each arm's own
        // comment) — deliberately narrower than a full `LedgerState` fold
        // (this module's doc explains why `compile` does not share that):
        // only the two fields [`NodeOutcome`] needs, not the whole
        // `LedgerTask`.
        let mut ledger_task_ids: std::collections::BTreeMap<String, (RoadmapStatus, bool)> =
            std::collections::BTreeMap::new();

        for event in events {
            // Positional header timestamps — every event updates these,
            // regardless of kind, so they stay meaningful even when
            // `RunStarted` itself is missing from a torn/partial slice.
            if report.header.first_event_at.is_none() {
                report.header.first_event_at = Some(event.timestamp);
            }
            report.header.last_event_at = Some(event.timestamp);

            match &event.payload {
                EventPayload::RunStarted { initial_prompt, .. } => {
                    report.header.initial_prompt = Some(initial_prompt.clone());
                },
                EventPayload::RunCompleted { terminal_node } => {
                    report.completion = RunCompletion::Completed {
                        terminal_node: terminal_node.clone(),
                    };
                },
                EventPayload::RunFailed { error } => {
                    report.completion = RunCompletion::Failed {
                        error: error.clone(),
                    };
                },
                EventPayload::RunAborted { reason } => {
                    report.completion = RunCompletion::Aborted {
                        reason: reason.clone(),
                    };
                },
                EventPayload::RunParked {
                    wake_at,
                    runtime,
                    basis,
                    reason,
                    ..
                } => {
                    report.completion = RunCompletion::Parked {
                        wake_at: *wake_at,
                        runtime: runtime.clone(),
                        basis: *basis,
                        reason: reason.clone(),
                    };
                },
                EventPayload::RunWokeFromPark {} => {
                    // No longer parked; not yet known to be finished either
                    // — a later terminal event (if any) will overwrite this.
                    report.completion = RunCompletion::Incomplete;
                },
                EventPayload::EscalationRequested {
                    stage,
                    reason,
                    cause,
                } => {
                    report.escalations.push(EscalationEntry {
                        stage: *stage,
                        reason: reason.clone(),
                        cause: *cause,
                    });
                },
                EventPayload::PipelineMaterialized { graph, .. }
                | EventPayload::GraphRevisionAccepted { graph, .. } => {
                    active_graph = Some((**graph).clone());
                },
                EventPayload::StageEntered { node, .. } => {
                    let idx = ensure_node_index(&mut report.nodes, &mut node_positions, node);
                    let entry = &mut report.nodes[idx];
                    entry.attempts += 1;
                    entry.status = NodeStatus::InProgress;
                },
                EventPayload::StageCompleted { node, outcome } => {
                    let idx = ensure_node_index(&mut report.nodes, &mut node_positions, node);
                    let entry = &mut report.nodes[idx];
                    // A torn log can hand `compile` a `StageCompleted` with
                    // no preceding `StageEntered` for this node (e.g. the
                    // read window started mid-attempt) — record at least
                    // one attempt rather than a misleading `attempts: 0`
                    // next to a terminal status.
                    if entry.attempts == 0 {
                        entry.attempts = 1;
                    }
                    entry.status = NodeStatus::Completed {
                        outcome: outcome.clone(),
                    };
                },
                EventPayload::StageFailed {
                    node,
                    reason,
                    retry_available,
                } => {
                    let idx = ensure_node_index(&mut report.nodes, &mut node_positions, node);
                    let entry = &mut report.nodes[idx];
                    if entry.attempts == 0 {
                        entry.attempts = 1;
                    }
                    entry.status = NodeStatus::Failed {
                        reason: reason.clone(),
                        retry_available: *retry_available,
                    };
                },
                EventPayload::OutcomeReported {
                    node,
                    outcome,
                    summary,
                } => {
                    report.outcomes.push(OutcomeEntry {
                        node: node.clone(),
                        outcome: outcome.clone(),
                        summary: summary.clone(),
                        status: OutcomeStatus::Accepted,
                    });
                },
                EventPayload::OutcomeRejectedByHook {
                    node,
                    outcome,
                    hook_id,
                } => {
                    // Find the most recent still-accepted entry for this
                    // exact (node, outcome) pair and flip it, rather than
                    // trusting positional adjacency to `OutcomeReported`.
                    let existing = report.outcomes.iter_mut().rev().find(|entry| {
                        &entry.node == node
                            && &entry.outcome == outcome
                            && entry.status == OutcomeStatus::Accepted
                    });
                    match existing {
                        Some(entry) => {
                            entry.status = OutcomeStatus::RejectedByHook {
                                hook_id: hook_id.clone(),
                            };
                        },
                        None => {
                            // No matching `OutcomeReported` in this slice
                            // (a torn log read starting after it) — record
                            // the rejection anyway rather than dropping it;
                            // an empty `summary` says plainly that the
                            // original report text was not in range.
                            report.outcomes.push(OutcomeEntry {
                                node: node.clone(),
                                outcome: outcome.clone(),
                                summary: String::new(),
                                status: OutcomeStatus::RejectedByHook {
                                    hook_id: hook_id.clone(),
                                },
                            });
                        },
                    }
                },
                EventPayload::TaskDiscovered { task_id, .. } => {
                    // Mirrors `LedgerState::record_discovered` exactly:
                    // `or_insert_with` — first-write-wins, `{Pending, false}`
                    // — does not overwrite a status this task_id already has
                    // (an out-of-order log where the verdict-bearing event
                    // was read first).
                    ledger_task_ids
                        .entry(task_id.clone())
                        .or_insert((RoadmapStatus::Pending, false));
                },
                EventPayload::TaskVerified {
                    task_id,
                    node,
                    evidence,
                } => {
                    let authorized = active_graph
                        .as_ref()
                        .is_some_and(|graph| node_has_verification_authority(graph, node));
                    let result = if authorized {
                        VerdictResult::Verified {
                            evidence: *evidence,
                        }
                    } else {
                        VerdictResult::Unauthorized
                    };
                    // Mirrors `LedgerState::record_verified` exactly: an
                    // *unauthorized* verification does not touch the ledger
                    // task's tracked state at all (only increments a
                    // rejected-verification counter this compiler does not
                    // need) — it is still surfaced in `report.verdicts`
                    // below (R33 needs a reviewer to see it), but it must
                    // not by itself introduce or change this task_id's
                    // evidence-backed bit, exactly as it does not in the
                    // live fold.
                    if authorized {
                        ledger_task_ids.insert(task_id.clone(), (RoadmapStatus::Completed, true));
                    }
                    upsert_verdict(
                        &mut report.verdicts,
                        VerifierVerdict {
                            task_id: task_id.clone(),
                            node: node.clone(),
                            result,
                        },
                    );
                },
                EventPayload::TaskStatusChanged {
                    task_id,
                    to,
                    authority_node,
                    ..
                } => {
                    record_task_status_change(
                        &mut ledger_task_ids,
                        &mut report.verdicts,
                        task_id,
                        *to,
                        authority_node,
                    );
                },
                EventPayload::ArtifactProduced {
                    node,
                    artifact,
                    path,
                    name,
                } => {
                    report.evidence.push(EvidenceEntry {
                        name: name.clone(),
                        hash: *artifact,
                        path: Some(path.clone()),
                        origin: EvidenceOrigin::Node { node: node.clone() },
                    });
                },
                EventPayload::BootstrapArtifactProduced {
                    stage,
                    artifact,
                    name,
                } => {
                    report.evidence.push(EvidenceEntry {
                        name: name.clone(),
                        hash: *artifact,
                        path: None,
                        origin: EvidenceOrigin::Bootstrap { stage: *stage },
                    });
                },
                EventPayload::TokensConsumed {
                    prompt_tokens,
                    output_tokens,
                    cache_hits,
                    cost_usd,
                    ..
                } => {
                    report.cost.prompt_tokens += u64::from(*prompt_tokens);
                    report.cost.output_tokens += u64::from(*output_tokens);
                    report.cost.cache_hits += u64::from(*cache_hits);
                    match cost_usd {
                        Some(usd) => report.cost.cost_usd += usd,
                        None => report.cost.uncosted_token_events += 1,
                    }
                },
                EventPayload::BudgetWarningRaised { .. } => {
                    report.cost.budget_warning_raised = true;
                },
                EventPayload::BudgetExceeded { .. } => {
                    report.cost.budget_exceeded = true;
                },
                EventPayload::SkillBound {
                    node,
                    name,
                    provider,
                    hash,
                    gate_enabled,
                } => {
                    report.skills.push(SkillUsage {
                        node: node.clone(),
                        name: name.clone(),
                        provider: *provider,
                        hash: *hash,
                        gate_enabled: *gate_enabled,
                    });
                },
                EventPayload::SteerDelivered { id, node, message } => {
                    report.steers.push(SteerEntry {
                        id: id.clone(),
                        node: node.clone(),
                        message: message.clone(),
                    });
                },
                EventPayload::HumanInputRequested { node, prompt, .. } => {
                    report.approvals.push(ApprovalEntry::HumanInputRequested {
                        node: node.clone(),
                        prompt: prompt.clone(),
                    });
                },
                EventPayload::HumanInputResolved { node, response, .. } => {
                    report.approvals.push(ApprovalEntry::HumanInputResolved {
                        node: node.clone(),
                        response: response.clone(),
                    });
                },
                EventPayload::HumanInputTimedOut {
                    node,
                    elapsed_seconds,
                    ..
                } => {
                    report.approvals.push(ApprovalEntry::HumanInputTimedOut {
                        node: node.clone(),
                        elapsed_seconds: *elapsed_seconds,
                    });
                },
                EventPayload::SandboxElevationRequested { node, capability } => {
                    report
                        .approvals
                        .push(ApprovalEntry::SandboxElevationRequested {
                            node: node.clone(),
                            capability: capability.clone(),
                        });
                },
                EventPayload::SandboxElevationDecided {
                    node,
                    decision,
                    remember,
                } => {
                    report
                        .approvals
                        .push(ApprovalEntry::SandboxElevationDecided {
                            node: node.clone(),
                            decision: *decision,
                            remember: *remember,
                        });
                },
                EventPayload::SandboxElevationTimedOut {
                    node,
                    capability,
                    elapsed_seconds,
                } => {
                    report
                        .approvals
                        .push(ApprovalEntry::SandboxElevationTimedOut {
                            node: node.clone(),
                            capability: capability.clone(),
                            elapsed_seconds: *elapsed_seconds,
                        });
                },
                EventPayload::BootstrapApprovalRequested { stage, .. } => {
                    report
                        .approvals
                        .push(ApprovalEntry::BootstrapApprovalRequested { stage: *stage });
                },
                EventPayload::BootstrapApprovalDecided {
                    stage, decision, ..
                } => {
                    report
                        .approvals
                        .push(ApprovalEntry::BootstrapApprovalDecided {
                            stage: *stage,
                            decision: *decision,
                        });
                },
                EventPayload::RoadmapPatchApprovalRequested { patch_id, .. } => {
                    report
                        .approvals
                        .push(ApprovalEntry::RoadmapPatchApprovalRequested {
                            patch_id: patch_id.clone(),
                        });
                },
                EventPayload::RoadmapPatchApprovalDecided {
                    patch_id, decision, ..
                } => {
                    report
                        .approvals
                        .push(ApprovalEntry::RoadmapPatchApprovalDecided {
                            patch_id: patch_id.clone(),
                            decision: *decision,
                        });
                },
                // Deliberately not represented in any Run Report section —
                // an exhaustive arm, not a wildcard, so a 58th `EventPayload`
                // variant fails this match at compile time instead of
                // silently vanishing into a catch-all the way the previous
                // `_ => {}` did (26 of 57 variants fell through it
                // unnoticed). Each of these genuinely has no home in the
                // nine sections + header/escalations this report renders:
                // bootstrap-internal telemetry and edit-loop bookkeeping
                // (`BootstrapStageStarted`, `BootstrapEditRequested`,
                // `BootstrapTelemetry`), roadmap-amendment content events
                // that are not themselves approvals (`RoadmapPatchDrafted`,
                // `RoadmapPatchApplied`, `RoadmapUpdated`), stage-internal
                // wiring (`StageInputsResolved`, `SessionOpened`,
                // `SessionClosed`, `EdgeTraversed`), raw tool-call telemetry
                // (`ToolCalled`, `ToolResultReceived`, already summarized by
                // `cost` and `evidence`), loop bookkeeping
                // (`LoopIterationStarted`, `LoopIterationCompleted`,
                // `LoopCompleted`), the dead
                // `ApprovalRequested`/`ApprovalDecided` primitive (see
                // `ApprovalEntry`'s own doc), a version-skew warning
                // (`RuntimeVersionWarning`), hook execution telemetry
                // (`HookExecuted` — only its *rejection* is reported, via
                // `OutcomeRejectedByHook` above), run-forking
                // (`ForkCreated`), and subgraph/notify bookkeeping
                // (`SubgraphEntered`, `SubgraphExited`, `NotifyDelivered`).
                EventPayload::BootstrapStageStarted { .. }
                | EventPayload::BootstrapEditRequested { .. }
                | EventPayload::BootstrapTelemetry { .. }
                | EventPayload::RoadmapPatchDrafted { .. }
                | EventPayload::RoadmapPatchApplied { .. }
                | EventPayload::RoadmapUpdated { .. }
                | EventPayload::StageInputsResolved { .. }
                | EventPayload::SessionOpened { .. }
                | EventPayload::SessionClosed { .. }
                | EventPayload::EdgeTraversed { .. }
                | EventPayload::ToolCalled { .. }
                | EventPayload::ToolResultReceived { .. }
                | EventPayload::LoopIterationStarted { .. }
                | EventPayload::LoopIterationCompleted { .. }
                | EventPayload::LoopCompleted { .. }
                | EventPayload::ApprovalRequested { .. }
                | EventPayload::ApprovalDecided { .. }
                | EventPayload::RuntimeVersionWarning { .. }
                | EventPayload::HookExecuted { .. }
                | EventPayload::ForkCreated { .. }
                | EventPayload::SubgraphEntered { .. }
                | EventPayload::SubgraphExited { .. }
                | EventPayload::NotifyDelivered { .. } => {},
            }
        }

        // Evidence-backing is only a meaningful question for a claimed
        // success (spec §10/R30) — computed last, over `ledger_task_ids`
        // (built through the event loop above), not threaded field-by-field
        // since a task's bit seen before the eventual `RunCompleted` is not
        // yet known to matter.
        //
        // Read over `ledger_task_ids`, NOT `report.verdicts`: the two are
        // different denominators. `verdicts` is scoped to tasks that
        // received an actual verifier verdict event; `ledger_task_ids`
        // additionally includes every task `TaskDiscovered` alone ever
        // introduced. `surge_persistence::task_ledger`'s registry mirror
        // (what `surge inbox`/`surge ledger` read) tracks the *latter* set
        // — a task discovered but never verified is still a
        // `task_ledger_index` row — so computing this over `verdicts` would
        // silently disagree with those two surfaces on any run with a
        // discovered-but-unverified task (spec §10: the same fact on every
        // surface, at the level of *which tasks count*, not only how each
        // one is scored).
        //
        // `all`, not `any`: every task this run's ledger ever tracked must
        // be evidence-backed, not merely one of them. `!is_empty()` is
        // required alongside `all` because `all` on an empty iterator is
        // vacuously `true`, and a `Completed` run with no ledger activity at
        // all (never ran a verifier) is exactly the "success without proof"
        // case this field exists to flag, not a free pass.
        report.evidence_backed = match &report.completion {
            RunCompletion::Completed { .. } => Some(
                !ledger_task_ids.is_empty()
                    && ledger_task_ids.values().all(|(status, verified)| {
                        is_evidence_backed(&NodeOutcome::new(*status, *verified))
                    }),
            ),
            RunCompletion::Failed { .. }
            | RunCompletion::Aborted { .. }
            | RunCompletion::Parked { .. }
            | RunCompletion::Incomplete => None,
        };

        report
    }
}

/// Get the index into `nodes` for `node`, creating a fresh
/// [`NodeSummary`] (zero attempts, [`NodeStatus::InProgress`]) the first
/// time it is seen — from whichever stage-lifecycle event happens to
/// mention it first, not only `StageEntered` (a torn log's read window can
/// start after a node's `StageEntered` and still contain its
/// `StageCompleted`/`StageFailed`).
fn ensure_node_index(
    nodes: &mut Vec<NodeSummary>,
    positions: &mut HashMap<NodeKey, usize>,
    node: &NodeKey,
) -> usize {
    *positions.entry(node.clone()).or_insert_with(|| {
        nodes.push(NodeSummary {
            node: node.clone(),
            attempts: 0,
            status: NodeStatus::InProgress,
        });
        nodes.len() - 1
    })
}

/// Insert `verdict`, replacing any existing entry for the same `task_id`.
///
/// [`RunReport::verdicts`] holds each task's *current* verifier verdict, not
/// a full history of every verification attempt — a later verdict for a
/// task supersedes an earlier one. This mirrors
/// [`crate::run_state::LedgerState::record_status_change`]'s upsert-by-
/// `task_id` semantics for the same reason: [`RunReport::evidence_backed`]
/// (and the "Verifier verdicts" report section itself) must read a task's
/// current state, not an entry a later event already superseded.
fn upsert_verdict(verdicts: &mut Vec<VerifierVerdict>, verdict: VerifierVerdict) {
    verdicts.retain(|v| v.task_id != verdict.task_id);
    verdicts.push(verdict);
}

/// Fold one `TaskStatusChanged` event into `ledger_task_ids` and `verdicts`.
/// Mirrors [`crate::run_state::LedgerState::record_status_change`] exactly
/// for `ledger_task_ids`: upsert the entry, always set `status = to`, and
/// clear `verified` unless `to == Completed`.
fn record_task_status_change(
    ledger_task_ids: &mut std::collections::BTreeMap<String, (RoadmapStatus, bool)>,
    verdicts: &mut Vec<VerifierVerdict>,
    task_id: &str,
    to: RoadmapStatus,
    authority_node: &NodeKey,
) {
    let entry = ledger_task_ids
        .entry(task_id.to_owned())
        .or_insert((RoadmapStatus::Pending, false));
    entry.0 = to;
    if to != RoadmapStatus::Completed {
        entry.1 = false;
    }
    match to {
        RoadmapStatus::FailedVerification => {
            upsert_verdict(
                verdicts,
                VerifierVerdict {
                    task_id: task_id.to_owned(),
                    node: authority_node.clone(),
                    result: VerdictResult::Rejected,
                },
            );
        },
        // A `Completed` transition alone carries no verdict of its own —
        // only an actual `TaskVerified` event does. Any verdict already
        // recorded for this task stands.
        RoadmapStatus::Completed => {},
        RoadmapStatus::Pending
        | RoadmapStatus::Running
        | RoadmapStatus::Paused
        | RoadmapStatus::ReadyForVerification
        | RoadmapStatus::Failed
        | RoadmapStatus::Skipped => {
            // A task sent back here after an earlier `TaskVerified` must be
            // re-verified before it counts again, but it must still count
            // in the denominator (`ledger_task_ids` above) — a task that
            // *fails outright after being verified* must drag the aggregate
            // down, not silently shrink out of it. Production emits exactly
            // this sequence (`TaskVerified` →
            // `TaskStatusChanged{to: ReadyForVerification}`,
            // `stage::agent::emit_ledger_event`) when a re-verification
            // cycle starts.
            //
            // Only replace an *existing* verdict entry — a task_id that
            // never had one (e.g. one only ever `TaskDiscovered`) gets no
            // spurious `Superseded` row in `verdicts`; it is still tracked
            // for `evidence_backed` via `ledger_task_ids`, just not
            // displayed in the "Verifier verdicts" section.
            if verdicts.iter().any(|v| v.task_id == task_id) {
                upsert_verdict(
                    verdicts,
                    VerifierVerdict {
                        task_id: task_id.to_owned(),
                        node: authority_node.clone(),
                        result: VerdictResult::Superseded,
                    },
                );
            }
        },
    }
}

/// Whether, and how, the run reached a terminal lifecycle event — or is
/// deliberately paused rather than stalled.
///
/// R27.1: a torn event log (crashed daemon, still running) has none of
/// `RunCompleted`/`RunFailed`/`RunAborted`/`RunParked` — [`Self::Incomplete`]
/// is the explicit, honest answer rather than a guess. This is deliberately
/// **not** the richer `RunStatus` registry enum. Most of that enum's
/// variants genuinely are registry-table facts the event log alone cannot
/// answer — `RunStatus::Crashed` in particular is assigned by
/// `Storage::list_runs` probing a daemon pid, and no event ever records it.
/// **`Parked` is the one exception**, and is modeled here as its own
/// variant rather than folded into `Incomplete`: `RunParked{wake_at, ..}`
/// and `RunWokeFromPark` are ordinary log events, so a compiler that reads
/// only the log can and does distinguish "paused on purpose, resuming at a
/// known time" from "stopped for an unknown reason" — collapsing the two
/// into one `Incomplete` bucket would throw away a fact the log proves,
/// which is exactly the class of mistake `compile`'s whole contract exists
/// to avoid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunCompletion {
    /// `RunCompleted` was observed.
    Completed {
        /// The terminal node the run reached.
        terminal_node: NodeKey,
    },
    /// `RunFailed` was observed.
    Failed {
        /// The recorded failure reason.
        error: String,
    },
    /// `RunAborted` was observed.
    Aborted {
        /// The recorded abort reason.
        reason: String,
    },
    /// The most recent lifecycle signal is `RunParked`: the run is paused
    /// waiting for a provider rate-limit window, not stalled or crashed.
    /// Cleared back to [`Self::Incomplete`] by a following `RunWokeFromPark`
    /// (the run is then simply "not finished yet" again, same as any other
    /// in-flight run, until a real terminal event lands).
    Parked {
        /// When the run is expected to resume on its own.
        wake_at: DateTime<Utc>,
        /// Canonical agent-runtime registry id the parked capacity window
        /// belongs to, when known.
        runtime: Option<String>,
        /// Whether `wake_at` is an actually-observed provider reset or a
        /// configured blind-backoff guess.
        basis: WakeBasis,
        /// Free-form, human-readable explanation, straight from the event.
        reason: String,
    },
    /// No terminal lifecycle event was found in the events given to
    /// `compile`, and the run is not currently parked either.
    Incomplete,
}

impl RunCompletion {
    /// True once a terminal lifecycle event was observed. [`Self::Parked`]
    /// is deliberately **not** terminal, mirroring
    /// `RunStatus::Parked::is_terminal`'s own exclusion — a parked run
    /// resumes on its own and needs no further operator action to become
    /// reportable again.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Incomplete | Self::Parked { .. })
    }
}

/// What the run was asked to do, and when it started and last made progress
/// — the identifying facts a reviewer needs before reading any section
/// (R33: accept/reject from the report alone means knowing what was even
/// being attempted).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunHeader {
    /// `RunStarted.initial_prompt`. `None` when no `RunStarted` event is in
    /// the slice `compile` was given (a genuinely torn log missing even its
    /// own start, or a caller that only read part of the log).
    pub initial_prompt: Option<String>,
    /// Timestamp of the first event in the slice `compile` was given —
    /// positional, not tied to `RunStarted` specifically, so this is still
    /// meaningful even when `RunStarted` itself is missing.
    pub first_event_at: Option<DateTime<Utc>>,
    /// Timestamp of the last event in the slice `compile` was given.
    pub last_event_at: Option<DateTime<Utc>>,
}

/// One escalation the run raised (`EscalationRequested`) — surfaced
/// separately from [`RunReport::completion`] because an escalation does not
/// always stop the run (some are warn-and-continue), and because
/// [`RunCompletion::Incomplete`] on its own answers "did it finish" but not
/// "why did it stop making progress," which is the first thing a reviewer
/// asks about an incomplete run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EscalationEntry {
    /// Bootstrap stage the escalation originated from, when applicable.
    pub stage: Option<BootstrapStage>,
    /// Free-form operator-readable explanation.
    pub reason: String,
    /// Typed origin — see [`EscalationCause`].
    pub cause: EscalationCause,
}

/// One node's execution summary: how many attempts, and the last observed
/// status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSummary {
    /// The node.
    pub node: NodeKey,
    /// Count of `StageEntered` events observed for this node.
    pub attempts: u32,
    /// Status as of the last stage-lifecycle event observed for this node.
    pub status: NodeStatus,
}

/// A node's status as of the last stage-lifecycle event `compile` observed
/// for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeStatus {
    /// `StageCompleted` was the last event observed for this node's current
    /// attempt.
    Completed {
        /// The reported outcome.
        outcome: OutcomeKey,
    },
    /// `StageFailed` was the last event observed for this node's current
    /// attempt.
    Failed {
        /// The recorded failure reason.
        reason: String,
        /// Whether the engine recorded a retry as available.
        retry_available: bool,
    },
    /// `StageEntered` was observed with no matching `StageCompleted` /
    /// `StageFailed` before the log given to `compile` ended.
    InProgress,
}

/// One declared node outcome (`OutcomeReported`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeEntry {
    /// The node that reported the outcome.
    pub node: NodeKey,
    /// The reported outcome key.
    pub outcome: OutcomeKey,
    /// The agent-provided summary text.
    pub summary: String,
    /// Whether a hook later rejected this exact outcome.
    pub status: OutcomeStatus,
}

/// Whether an [`OutcomeEntry`] stood, or was rejected by a hook
/// (`OutcomeRejectedByHook`) after the agent reported it.
///
/// This exists because a reviewer reading `outcomes` alone would otherwise
/// see a declared outcome and have no way to know the hook pipeline threw
/// it out — the same class of gap [`VerdictResult::Unauthorized`] closes
/// for verifier verdicts. Half of that story without the other half would
/// make `outcomes` actively misleading: a rejected outcome rendered exactly
/// like an accepted one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome_status", rename_all = "snake_case")]
pub enum OutcomeStatus {
    /// No `OutcomeRejectedByHook` was recorded against this outcome.
    Accepted,
    /// A hook rejected this outcome after it was reported.
    RejectedByHook {
        /// The hook that rejected it.
        hook_id: String,
    },
}

/// One verifier verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifierVerdict {
    /// Ledger task id the verdict concerns.
    pub task_id: String,
    /// The node that authored the verdict.
    pub node: NodeKey,
    /// The verdict itself.
    pub result: VerdictResult,
}

/// The result a [`VerifierVerdict`] carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum VerdictResult {
    /// A verification-authority node confirmed the task.
    Verified {
        /// Content hash of the verification-report artifact.
        evidence: ContentHash,
    },
    /// The task's status transitioned to `RoadmapStatus::FailedVerification`.
    Rejected,
    /// A `TaskVerified` event was recorded for this node, but the graph
    /// active at that point did not grant the node verification authority
    /// (the same trust boundary `crate::run_state::LedgerState::record_verified`
    /// enforces, via the same [`node_has_verification_authority`] predicate).
    /// Surfaced rather than silently dropped: R33 asks a reviewer to
    /// accept/reject from this report alone, and a log entry that *claims*
    /// verification without the authority to grant it is exactly the kind
    /// of fact that decision needs to see, not one this report should hide
    /// behind an empty `verdicts` list.
    Unauthorized,
    /// This task previously had a verdict (`Verified`/`Rejected`/
    /// `Unauthorized`), but a later `TaskStatusChanged` moved it to some
    /// other non-`Completed` status (back to `Pending`/`Running`/`Paused`/
    /// `ReadyForVerification`, or an outright `Failed`) — the earlier
    /// verdict no longer holds and must be re-earned. Recorded explicitly
    /// rather than removing the task's entry from [`RunReport::verdicts`]:
    /// dropping it would shrink [`is_evidence_backed`]'s denominator, and a
    /// task that failed *after* being verified would then silently stop
    /// counting against the run instead of correctly failing it. Mirrors
    /// `run_state::LedgerState::record_status_change`, which *clears*
    /// `verified` on such a transition rather than deleting the task's
    /// ledger row.
    Superseded,
}

/// One evidence artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEntry {
    /// Logical artifact name.
    pub name: String,
    /// Content hash addressing the artifact.
    pub hash: ContentHash,
    /// Path on disk relative to the run's artifact store. `None` for a
    /// `BootstrapArtifactProduced` artifact, whose event carries no path.
    pub path: Option<PathBuf>,
    /// What produced the artifact.
    pub origin: EvidenceOrigin,
}

/// What produced an [`EvidenceEntry`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceOrigin {
    /// Produced by a pipeline node (`ArtifactProduced`).
    Node {
        /// The producing node.
        node: NodeKey,
    },
    /// Produced by a bootstrap stage (`BootstrapArtifactProduced`).
    Bootstrap {
        /// The producing bootstrap stage.
        stage: BootstrapStage,
    },
}

/// Aggregate token/dollar spend across the whole run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostTotals {
    /// Total prompt (input) tokens.
    pub prompt_tokens: u64,
    /// Total output tokens.
    pub output_tokens: u64,
    /// Total cache-hit tokens credited.
    pub cache_hits: u64,
    /// Total cost in USD, summed only across `TokensConsumed` events that
    /// carried a recorded price (`cost_usd: Some(_)`). **Not the true total
    /// spend when [`Self::uncosted_token_events`] is nonzero** — a `None`
    /// price is silently treated as `0.0` in the sum (the only sane fallback
    /// for an arithmetic total), so this figure understates spend whenever
    /// the agent runtime didn't report a price for some tokens. Always read
    /// the two fields together.
    pub cost_usd: f64,
    /// Count of `TokensConsumed` events with `cost_usd: None` — spend that
    /// happened but was never priced. A nonzero value here means
    /// [`Self::cost_usd`] is a floor, not the true total: render callers
    /// must say "at least $X (+N events without a recorded price)", never
    /// bare "$X", whenever this is nonzero.
    pub uncosted_token_events: u32,
    /// Whether a `BudgetWarningRaised` was observed anywhere in the log.
    pub budget_warning_raised: bool,
    /// Whether a `BudgetExceeded` was observed anywhere in the log.
    pub budget_exceeded: bool,
}

/// One skill pack bound onto a node (R14) — a direct projection of a
/// `SkillBound` event; every field here is that event's own field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillUsage {
    /// The node the skill bound onto.
    pub node: NodeKey,
    /// The skill's declared name.
    pub name: String,
    /// Which root the bound pack was found under.
    pub provider: SkillProvider,
    /// Content hash of the pack as resolved at bind time.
    pub hash: ContentHash,
    /// Whether the operator-approval trust gate was active for this bind.
    pub gate_enabled: bool,
}

/// One operator steer message delivered mid-run (`SteerDelivered`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteerEntry {
    /// Queue id of the delivered steer.
    pub id: String,
    /// The node whose prompt the steer was prepended to.
    pub node: NodeKey,
    /// The operator's steer message.
    pub message: String,
}

/// One approval-shaped request or decision. Each variant is a direct
/// projection of one event — request and decision are listed as separate,
/// chronological entries rather than paired, since pairing a request to its
/// eventual decision would require guessing correlation the log does not
/// make explicit for every one of these event families (e.g. `HumanInput*`
/// carries an optional `call_id` that is `None` for `HumanGate`-driven
/// pauses). A torn run's still-pending request simply has no matching
/// decision entry after it, which is itself the R27.1-shaped fact a
/// reviewer needs to see.
///
/// Deliberately excludes `EventPayload::ApprovalRequested`/`ApprovalDecided`:
/// no production code path emits that pair today (`docs/adr/0015-skill-binding-trust-via-content-hash.md`
/// documents it as a dead primitive with zero renderer) — including it here
/// would list an approval flow that never actually asked anyone anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalEntry {
    /// `HumanInputRequested` — a `HumanGate`, the skill trust gate, or a
    /// tool-driven `request_human_input` call asked the operator something.
    HumanInputRequested {
        /// The node that issued the request.
        node: NodeKey,
        /// The prompt shown to the operator.
        prompt: String,
    },
    /// `HumanInputResolved` — the operator answered.
    HumanInputResolved {
        /// The node the answer resolves.
        node: NodeKey,
        /// The operator's response.
        response: serde_json::Value,
    },
    /// `HumanInputTimedOut` — nobody answered before the configured timeout.
    HumanInputTimedOut {
        /// The node whose request timed out.
        node: NodeKey,
        /// How long the engine waited before timing out.
        elapsed_seconds: u32,
    },
    /// `SandboxElevationRequested` — a node asked for a capability beyond
    /// its configured sandbox.
    SandboxElevationRequested {
        /// The requesting node.
        node: NodeKey,
        /// The requested capability.
        capability: String,
    },
    /// `SandboxElevationDecided` — the operator allowed or denied it.
    SandboxElevationDecided {
        /// The node the decision applies to.
        node: NodeKey,
        /// The decision.
        decision: ElevationDecision,
        /// Whether the decision was remembered for the rest of the run.
        remember: bool,
    },
    /// `SandboxElevationTimedOut` — nobody answered; the engine implicitly denied.
    SandboxElevationTimedOut {
        /// The node whose request timed out.
        node: NodeKey,
        /// The requested capability.
        capability: String,
        /// How long the engine waited before timing out.
        elapsed_seconds: u32,
    },
    /// `BootstrapApprovalRequested` — a bootstrap stage (description,
    /// roadmap, flow) asked for operator approval.
    BootstrapApprovalRequested {
        /// The bootstrap stage.
        stage: BootstrapStage,
    },
    /// `BootstrapApprovalDecided` — the operator's decision.
    BootstrapApprovalDecided {
        /// The bootstrap stage.
        stage: BootstrapStage,
        /// The decision.
        decision: BootstrapDecision,
    },
    /// `RoadmapPatchApprovalRequested` — a roadmap-amendment patch asked for
    /// operator approval.
    RoadmapPatchApprovalRequested {
        /// The patch id.
        patch_id: RoadmapPatchId,
    },
    /// `RoadmapPatchApprovalDecided` — the operator's decision.
    RoadmapPatchApprovalDecided {
        /// The patch id.
        patch_id: RoadmapPatchId,
        /// The decision.
        decision: RoadmapPatchApprovalDecision,
    },
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
