//! Run state machine — derived purely by folding events.

use crate::content_hash::ContentHash;
use crate::edge::EdgeKind;
use crate::graph::Graph;
use crate::id::SessionId;
use crate::keys::{NodeKey, OutcomeKey};
use crate::node::LedgerEffect;
use crate::roadmap::RoadmapStatus;
use crate::roadmap_patch::{
    ActivePickupPolicy, RoadmapPatchApprovalDecision, RoadmapPatchId, RoadmapPatchStatus,
    RoadmapPatchTarget,
};
use crate::run_event::{BootstrapDecision, BootstrapStage, EventPayload, RunEvent};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Tracks a pending human-input request while the pipeline is paused
/// waiting for operator response.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingHumanInput {
    /// The graph node that issued the request.
    pub node: NodeKey,
    /// Tool-call identifier supplied by the agent; `None` for HumanGate-driven pauses.
    pub call_id: Option<String>,
    /// The prompt shown to the human operator.
    pub prompt: String,
    /// Optional JSON Schema for the expected response structure.
    pub schema: Option<serde_json::Value>,
    /// Sequence number of the `HumanInputRequested` event that created this.
    pub requested_seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    NotStarted,
    Bootstrapping {
        stage: BootstrapStage,
        substate: BootstrapSubstate,
    },
    Pipeline {
        /// `Arc<Graph>` because each fold step shares the current graph by
        /// reference-count. `PipelineMaterialized` establishes revision 0;
        /// `GraphRevisionAccepted` may replace it with an amended graph.
        graph: Arc<Graph>,
        cursor: Cursor,
        memory: RunMemory,
        /// Set when a `HumanInputRequested` event is folded; cleared by
        /// `HumanInputResolved` or `HumanInputTimedOut`.
        pending_human_input: Option<PendingHumanInput>,
    },
    Terminal {
        kind: TerminalReason,
        reason: String,
    },
}

/// What a run needs from the operator right now — the axis the fleet inbox
/// (`surge inbox`) triages on. A pure classification of [`RunState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    /// Blocked on a human decision (a HumanGate, bootstrap approval, or
    /// tool-driven `request_human_input`). This is the "needs me right now"
    /// bucket the inbox surfaces first.
    NeedsInput,
    /// Executing with no human in the loop.
    Working,
    /// Reached a terminal state — no further attention needed.
    Done(TerminalReason),
}

impl RunState {
    /// Classify what this run needs from the operator.
    ///
    /// A run is [`Attention::NeedsInput`] when the fold shows an unresolved
    /// gate: a bootstrap stage awaiting approval, or a Pipeline holding a
    /// `pending_human_input`. Everything else in-flight is
    /// [`Attention::Working`]; a terminal run is [`Attention::Done`].
    #[must_use]
    pub fn attention(&self) -> Attention {
        match self {
            // A just-admitted run that has not folded RunStarted yet — treat
            // as working (it is not blocked on a human).
            Self::NotStarted => Attention::Working,
            Self::Bootstrapping {
                substate: BootstrapSubstate::AwaitingApproval { .. },
                ..
            } => Attention::NeedsInput,
            Self::Bootstrapping { .. } => Attention::Working,
            Self::Pipeline {
                pending_human_input: Some(_),
                ..
            } => Attention::NeedsInput,
            Self::Pipeline { .. } => Attention::Working,
            Self::Terminal { kind, .. } => Attention::Done(*kind),
        }
    }

    /// The prompt shown to the operator when this run is blocked on input, if
    /// any. `None` unless [`RunState::attention`] is [`Attention::NeedsInput`]
    /// with a captured prompt (bootstrap approvals carry no free-form prompt).
    #[must_use]
    pub fn pending_prompt(&self) -> Option<&str> {
        match self {
            Self::Pipeline {
                pending_human_input: Some(pending),
                ..
            } => Some(pending.prompt.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BootstrapSubstate {
    AgentRunning {
        session: SessionId,
        started_seq: u64,
    },
    AwaitingApproval {
        artifact: ContentHash,
        requested_seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub node: NodeKey,
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalReason {
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunMemory {
    pub artifacts: BTreeMap<String, ArtifactRef>,
    pub artifacts_by_node: BTreeMap<NodeKey, Vec<ArtifactRef>>,
    pub outcomes: BTreeMap<NodeKey, Vec<OutcomeRecord>>,
    pub costs: CostSummary,
    /// Whether a `BudgetWarningRaised` has been folded for this run. Set once
    /// and never cleared, so the engine's stage-boundary budget check warns at
    /// most once per run — and, because it is derived from the event log,
    /// survives a daemon restart / resume (the warning is not re-emitted).
    pub budget_warning_raised: bool,
    /// Whether a `BudgetExceeded` has been folded for this run. Gates the
    /// `WarnOnly` breach record so the hard-limit crossing is surfaced exactly
    /// once (separate from the threshold warning) and is resume-safe.
    pub budget_exceeded_noted: bool,
    /// Per-bootstrap-stage edit-loop counter. Incremented on every
    /// `BootstrapEditRequested` event. Read by the bootstrap HumanGate
    /// handler to enforce `EngineRunConfig.bootstrap.edit_loop_cap`.
    /// Empty for non-bootstrap runs.
    pub bootstrap_edit_counts: BTreeMap<BootstrapStage, u32>,
    /// Per-node visit counter for `EdgeKind::Backtrack` re-entries. The
    /// value is incremented exactly once per `EdgeTraversed { kind: Backtrack }`
    /// event keyed by the *target* node. Forward-edge traversals do not
    /// touch this map. Bootstrap engine code (and any future
    /// backtrack-aware feature) reads it to detect re-entries without
    /// scanning the event log.
    pub node_visits: BTreeMap<NodeKey, u32>,
    /// Per-bootstrap-stage latest edit feedback. Updated on every
    /// `BootstrapEditRequested { stage, feedback }` event — the newest
    /// feedback overwrites the previous entry for that stage. Read by the
    /// `ArtifactSource::EditFeedback` binding resolver in the orchestrator
    /// to expose the most recent operator feedback to the re-entered agent
    /// stage. Empty for non-bootstrap runs.
    pub last_edit_feedback_by_stage: BTreeMap<BootstrapStage, String>,
    /// Latest roadmap patch lifecycle state derived from roadmap amendment
    /// events. This intentionally stores artifact refs only; full patch
    /// bodies stay in the artifact store.
    pub roadmap_patches: BTreeMap<RoadmapPatchId, RoadmapPatchMemory>,
    /// Latest accepted graph revision metadata, if an active amendment
    /// changed the executable graph after `PipelineMaterialized`.
    pub latest_graph_revision: Option<GraphRevisionMemory>,
    /// Task-ledger state derived from `TaskStatusChanged` / `TaskDiscovered` /
    /// `TaskVerified` events. Empty for runs that carry no ledger.
    pub ledger: LedgerState,
}

/// Task-ledger view folded from ledger events. The source of truth is the
/// event log; this is the folded projection the engine and persistence read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LedgerState {
    /// Per-task ledger record, keyed by task id.
    pub tasks: BTreeMap<String, LedgerTask>,
    /// Count of `TaskVerified` events rejected because the reporting node
    /// lacked verification authority in the active graph. Deterministic
    /// (folded from the log); surfaced so callers can flag tampered logs.
    pub rejected_verifications: u64,
}

impl LedgerState {
    /// Record a non-verified status transition (upsert).
    ///
    /// Clears `verified` when the new status is not `Completed`, preventing
    /// an inconsistent state where `verified=true` but the task is not
    /// completed (e.g. due to a reordered event log).
    fn record_status_change(&mut self, task_id: &str, to: RoadmapStatus, node: &NodeKey, seq: u64) {
        let entry = self.tasks.entry(task_id.to_owned()).or_default();
        entry.status = to;
        if to != RoadmapStatus::Completed {
            entry.verified = false;
        }
        entry.last_authority_node = Some(node.clone());
        entry.updated_seq = seq;
    }

    /// Insert a discovered task as pending with a `discovered_from` edge.
    /// First-write-wins: a later duplicate discovery for the same id is a
    /// no-op so replay stays idempotent.
    fn record_discovered(&mut self, task_id: &str, discovered_from: &str, seq: u64) {
        self.tasks
            .entry(task_id.to_owned())
            .or_insert_with(|| LedgerTask {
                status: RoadmapStatus::Pending,
                verified: false,
                discovered_from: Some(discovered_from.to_owned()),
                last_authority_node: None,
                updated_seq: seq,
            });
    }

    /// Record a verification. `authorized` is computed by the caller from the
    /// active graph (the node must declare a `LedgerEffect::Verified` outcome).
    /// An unauthorized verification leaves the task unverified and bumps the
    /// rejection counter — defense in depth against a tampered log.
    fn record_verified(&mut self, task_id: &str, node: &NodeKey, authorized: bool, seq: u64) {
        if !authorized {
            self.rejected_verifications += 1;
            return;
        }
        let entry = self.tasks.entry(task_id.to_owned()).or_default();
        entry.status = RoadmapStatus::Completed;
        entry.verified = true;
        entry.last_authority_node = Some(node.clone());
        entry.updated_seq = seq;
    }
}

/// One task's folded ledger record.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerTask {
    /// Current ledger status.
    pub status: RoadmapStatus,
    /// True only once a verification-authority node confirmed the task.
    pub verified: bool,
    /// Task id this task was discovered from, when discovered mid-run.
    pub discovered_from: Option<String>,
    /// Node that last transitioned this task (audit trail head).
    pub last_authority_node: Option<NodeKey>,
    /// Seq of the last event that touched this task.
    pub updated_seq: u64,
}

impl Default for LedgerTask {
    fn default() -> Self {
        Self {
            status: RoadmapStatus::Pending,
            verified: false,
            discovered_from: None,
            last_authority_node: None,
            updated_seq: 0,
        }
    }
}

/// True when `node` exists in `graph` — at the top level **or inside any
/// subgraph** — and declares an outcome carrying [`LedgerEffect::Verified`],
/// the graph-visible signal that the node has verification authority. Keeps the
/// fold pure (no profile-registry access).
///
/// Subgraphs must be searched: bundled loop flows (e.g. `multi-milestone`) run
/// the sealed verifier inside a task-body subgraph, so a top-level-only lookup
/// would wrongly reject every `TaskVerified` those flows emit.
#[must_use]
pub fn node_has_verification_authority(graph: &Graph, node: &NodeKey) -> bool {
    graph.find_node(node).is_some_and(|found| {
        found
            .declared_outcomes
            .iter()
            .any(|outcome| outcome.ledger_effect == LedgerEffect::Verified)
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoadmapPatchMemory {
    pub target: RoadmapPatchTarget,
    pub status: RoadmapPatchStatus,
    pub patch_artifact: Option<ContentHash>,
    pub patch_path: Option<PathBuf>,
    pub roadmap_artifact: Option<ContentHash>,
    pub roadmap_path: Option<PathBuf>,
    pub flow_artifact: Option<ContentHash>,
    pub flow_path: Option<PathBuf>,
    pub updated_seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphRevisionMemory {
    pub patch_id: RoadmapPatchId,
    pub target: RoadmapPatchTarget,
    pub previous_graph_hash: ContentHash,
    pub graph_hash: ContentHash,
    pub active_pickup: ActivePickupPolicy,
    pub updated_seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactRef {
    pub hash: ContentHash,
    pub path: PathBuf,
    pub name: String,
    pub produced_by: NodeKey,
    pub produced_at_seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeRecord {
    pub outcome: OutcomeKey,
    pub summary: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CostSummary {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_hits: u64,
    pub cost_usd: f64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FoldError {
    #[error("invalid transition: state={from}, event={event}")]
    InvalidTransition {
        from: &'static str,
        event: &'static str,
    },
    #[error("event sequence corrupted: expected seq {expected_seq}, got {got_seq}")]
    CorruptedSequence { expected_seq: u64, got_seq: u64 },
    #[error("event references unknown node: {node}")]
    UnknownNode { node: NodeKey },
}

/// Fold a sequence of events into a final state. Returns FoldError if any
/// transition is invalid or sequence numbers are corrupted.
pub fn fold(events: &[RunEvent]) -> Result<RunState, FoldError> {
    let mut state = RunState::NotStarted;
    for (expected_seq, event) in (1u64..).zip(events.iter()) {
        if event.seq != expected_seq {
            return Err(FoldError::CorruptedSequence {
                expected_seq,
                got_seq: event.seq,
            });
        }
        state = apply(state, event)?;
    }
    Ok(state)
}

/// Apply a single event to the current state. Pure function, no I/O.
pub fn apply(state: RunState, event: &RunEvent) -> Result<RunState, FoldError> {
    match (state, &event.payload) {
        (RunState::NotStarted, EventPayload::RunStarted { .. }) => Ok(RunState::Bootstrapping {
            stage: BootstrapStage::Description,
            substate: BootstrapSubstate::AgentRunning {
                // Replay determinism: fold must not introduce random IDs.
                // The real session id arrives via `BootstrapStageStarted` /
                // `SessionOpened` events; this is a stable placeholder until
                // then.
                session: SessionId::nil(),
                started_seq: event.seq,
            },
        }),
        (
            RunState::Bootstrapping { stage: _, .. },
            EventPayload::BootstrapApprovalDecided {
                stage,
                decision: BootstrapDecision::Approve,
                ..
            },
        ) => Ok(advance_bootstrap_stage(*stage, event.seq)),
        (RunState::Bootstrapping { .. }, EventPayload::PipelineMaterialized { graph, .. }) => {
            // The graph is part of the event payload, so fold can fully
            // reconstruct `Pipeline` state from the event log alone. The
            // first cursor lands on `graph.start` with attempt 1 — actual
            // node execution then drives subsequent `StageEntered` events.
            let start = graph.start.clone();
            Ok(RunState::Pipeline {
                graph: Arc::new(graph.as_ref().clone()),
                cursor: Cursor {
                    node: start,
                    attempt: 1,
                },
                memory: RunMemory::default(),
                pending_human_input: None,
            })
        },
        (state @ RunState::Pipeline { .. }, EventPayload::StageEntered { node, attempt }) => {
            if let RunState::Pipeline {
                graph,
                memory,
                pending_human_input,
                ..
            } = state
            {
                Ok(RunState::Pipeline {
                    graph,
                    cursor: Cursor {
                        node: node.clone(),
                        attempt: *attempt,
                    },
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::ArtifactProduced {
                node,
                artifact,
                path,
                name,
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                let aref = ArtifactRef {
                    hash: *artifact,
                    path: path.clone(),
                    name: name.clone(),
                    produced_by: node.clone(),
                    produced_at_seq: event.seq,
                };
                memory.artifacts.insert(name.clone(), aref.clone());
                memory
                    .artifacts_by_node
                    .entry(node.clone())
                    .or_default()
                    .push(aref);
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::OutcomeReported {
                node,
                outcome,
                summary,
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                memory
                    .outcomes
                    .entry(node.clone())
                    .or_default()
                    .push(OutcomeRecord {
                        outcome: outcome.clone(),
                        summary: summary.clone(),
                        seq: event.seq,
                    });
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::TokensConsumed {
                prompt_tokens,
                output_tokens,
                cache_hits,
                cost_usd,
                ..
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                memory.costs.tokens_in += u64::from(*prompt_tokens);
                memory.costs.tokens_out += u64::from(*output_tokens);
                memory.costs.cache_hits += u64::from(*cache_hits);
                memory.costs.cost_usd += cost_usd.unwrap_or(0.0);
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            RunState::Pipeline { .. } | RunState::Bootstrapping { .. },
            EventPayload::RunCompleted { .. },
        ) => Ok(RunState::Terminal {
            kind: TerminalReason::Completed,
            reason: String::new(),
        }),
        (
            RunState::Pipeline { .. } | RunState::Bootstrapping { .. },
            EventPayload::RunFailed { error },
        ) => Ok(RunState::Terminal {
            kind: TerminalReason::Failed,
            reason: error.clone(),
        }),
        (
            RunState::Pipeline { .. } | RunState::Bootstrapping { .. },
            EventPayload::RunAborted { reason },
        ) => Ok(RunState::Terminal {
            kind: TerminalReason::Aborted,
            reason: reason.clone(),
        }),
        (
            state @ RunState::Pipeline { .. },
            EventPayload::HumanInputRequested {
                node,
                call_id,
                prompt,
                schema,
                ..
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                memory,
                ..
            } = state
            {
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input: Some(PendingHumanInput {
                        node: node.clone(),
                        call_id: call_id.clone(),
                        prompt: prompt.clone(),
                        schema: schema.clone(),
                        requested_seq: event.seq,
                    }),
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::EdgeTraversed {
                kind: EdgeKind::Backtrack,
                to,
                ..
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                *memory.node_visits.entry(to.clone()).or_insert(0) += 1;
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (state @ RunState::Pipeline { .. }, EventPayload::HumanInputResolved { .. }) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                memory,
                ..
            } = state
            {
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input: None,
                })
            } else {
                unreachable!()
            }
        },
        (state @ RunState::Pipeline { .. }, EventPayload::HumanInputTimedOut { .. }) => {
            // Timeout clears the pending field; engine writes a follow-up
            // StageFailed/RunFailed if appropriate. Fold itself stays in
            // Pipeline; the terminal transition is driven by the
            // separately-emitted RunFailed event.
            if let RunState::Pipeline {
                graph,
                cursor,
                memory,
                ..
            } = state
            {
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input: None,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::TaskStatusChanged {
                task_id,
                to,
                authority_node,
                ..
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                memory
                    .ledger
                    .record_status_change(task_id, *to, authority_node, event.seq);
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::TaskDiscovered {
                task_id,
                discovered_from,
                ..
            },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                memory
                    .ledger
                    .record_discovered(task_id, discovered_from, event.seq);
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (state @ RunState::Pipeline { .. }, EventPayload::TaskVerified { task_id, node, .. }) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                // Defense in depth: fold honors the verification only when the
                // reporting node is a verification authority in the active
                // graph (declares a `LedgerEffect::Verified` outcome). The
                // engine (M3) already refuses to emit an unauthorized event;
                // this rejects a tampered log on replay.
                let authorized = node_has_verification_authority(&graph, node);
                memory
                    .ledger
                    .record_verified(task_id, node, authorized, event.seq);
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::RoadmapPatchDrafted { .. }
            | EventPayload::RoadmapPatchApprovalRequested { .. }
            | EventPayload::RoadmapPatchApprovalDecided { .. }
            | EventPayload::RoadmapPatchApplied { .. }
            | EventPayload::RoadmapUpdated { .. },
        ) => {
            if let RunState::Pipeline {
                graph,
                cursor,
                mut memory,
                pending_human_input,
            } = state
            {
                memory.apply_event(event);
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::GraphRevisionAccepted { graph: revised, .. },
        ) => {
            if let RunState::Pipeline {
                cursor,
                mut memory,
                pending_human_input,
                ..
            } = state
            {
                if !revised.nodes.contains_key(&cursor.node) {
                    return Err(FoldError::UnknownNode {
                        node: cursor.node.clone(),
                    });
                }
                memory.apply_event(event);
                Ok(RunState::Pipeline {
                    graph: Arc::new(revised.as_ref().clone()),
                    cursor,
                    memory,
                    pending_human_input,
                })
            } else {
                unreachable!()
            }
        },
        // Many (state, event) pairs are pass-through — events like ToolCalled,
        // EdgeTraversed, SandboxElevation*, ApprovalRequested/Decided,
        // BootstrapStageStarted etc. are recorded for replay but do not drive
        // the M1 state machine. Engine in M5 may extend behavior.
        //
        // Hook-related events are explicit no-ops for replay determinism: the
        // engine appends `HookExecuted` and `OutcomeRejectedByHook` for audit,
        // but `RunMemory.outcomes` is only mutated on `OutcomeReported`. Since
        // a rejecting `on_outcome` hook fires BEFORE the engine appends
        // `OutcomeReported`, no fold-side mutation is needed. These explicit
        // arms prevent a future change from accidentally treating the audit
        // events as state transitions.
        (state, EventPayload::HookExecuted { .. }) => Ok(state),
        (state, EventPayload::OutcomeRejectedByHook { .. }) => Ok(state),
        (state, _) => Ok(state),
    }
}

fn advance_bootstrap_stage(stage: BootstrapStage, seq: u64) -> RunState {
    match stage {
        BootstrapStage::Description => RunState::Bootstrapping {
            stage: BootstrapStage::Roadmap,
            substate: BootstrapSubstate::AgentRunning {
                // Deterministic placeholder — see RunStarted arm in `apply`
                // for context. Real session id flows via separate events.
                session: SessionId::nil(),
                started_seq: seq,
            },
        },
        BootstrapStage::Roadmap => RunState::Bootstrapping {
            stage: BootstrapStage::Flow,
            substate: BootstrapSubstate::AgentRunning {
                // Deterministic placeholder — see RunStarted arm in `apply`
                // for context. Real session id flows via separate events.
                session: SessionId::nil(),
                started_seq: seq,
            },
        },
        BootstrapStage::Flow => RunState::Bootstrapping {
            stage: BootstrapStage::Flow,
            substate: BootstrapSubstate::AwaitingApproval {
                artifact: ContentHash::compute(b"placeholder-flow-toml"),
                requested_seq: seq,
            },
        },
    }
}

impl RunMemory {
    /// Apply an event to the memory accumulator only. Used independently of
    /// the full state machine for "what's the cost so far" queries.
    pub fn apply_event(&mut self, event: &RunEvent) {
        match &event.payload {
            EventPayload::ArtifactProduced {
                node,
                artifact,
                path,
                name,
            } => {
                let aref = ArtifactRef {
                    hash: *artifact,
                    path: path.clone(),
                    name: name.clone(),
                    produced_by: node.clone(),
                    produced_at_seq: event.seq,
                };
                self.artifacts.insert(name.clone(), aref.clone());
                self.artifacts_by_node
                    .entry(node.clone())
                    .or_default()
                    .push(aref);
            },
            EventPayload::OutcomeReported {
                node,
                outcome,
                summary,
            } => {
                self.outcomes
                    .entry(node.clone())
                    .or_default()
                    .push(OutcomeRecord {
                        outcome: outcome.clone(),
                        summary: summary.clone(),
                        seq: event.seq,
                    });
            },
            EventPayload::TokensConsumed {
                prompt_tokens,
                output_tokens,
                cache_hits,
                cost_usd,
                ..
            } => {
                self.costs.tokens_in += u64::from(*prompt_tokens);
                self.costs.tokens_out += u64::from(*output_tokens);
                self.costs.cache_hits += u64::from(*cache_hits);
                self.costs.cost_usd += cost_usd.unwrap_or(0.0);
            },
            EventPayload::BudgetWarningRaised { .. } => {
                self.budget_warning_raised = true;
            },
            EventPayload::BudgetExceeded { .. } => {
                self.budget_exceeded_noted = true;
            },
            EventPayload::BootstrapEditRequested { stage, feedback } => {
                *self.bootstrap_edit_counts.entry(*stage).or_insert(0) += 1;
                self.last_edit_feedback_by_stage
                    .insert(*stage, feedback.clone());
            },
            EventPayload::EdgeTraversed {
                kind: EdgeKind::Backtrack,
                to,
                ..
            } => {
                *self.node_visits.entry(to.clone()).or_insert(0) += 1;
            },
            EventPayload::RoadmapPatchDrafted {
                patch_id,
                target,
                patch_artifact,
                patch_path,
            } => {
                self.roadmap_patches.insert(
                    patch_id.clone(),
                    RoadmapPatchMemory {
                        target: target.clone(),
                        status: RoadmapPatchStatus::Drafted,
                        patch_artifact: Some(*patch_artifact),
                        patch_path: Some(patch_path.clone()),
                        roadmap_artifact: None,
                        roadmap_path: None,
                        flow_artifact: None,
                        flow_path: None,
                        updated_seq: event.seq,
                    },
                );
            },
            EventPayload::RoadmapPatchApprovalRequested {
                patch_id, target, ..
            } => {
                self.upsert_roadmap_patch_status(
                    patch_id,
                    target,
                    RoadmapPatchStatus::PendingApproval,
                    event.seq,
                );
            },
            EventPayload::RoadmapPatchApprovalDecided {
                patch_id, decision, ..
            } => {
                self.update_roadmap_patch_decision(patch_id, *decision, event.seq);
            },
            EventPayload::RoadmapPatchApplied {
                patch_id,
                target,
                amended_roadmap_artifact,
                amended_roadmap_path,
                amended_flow_artifact,
                amended_flow_path,
            } => {
                self.upsert_roadmap_patch_artifacts(
                    patch_id,
                    target,
                    RoadmapPatchArtifactUpdate {
                        roadmap_artifact: *amended_roadmap_artifact,
                        roadmap_path: amended_roadmap_path.clone(),
                        flow_artifact: *amended_flow_artifact,
                        flow_path: amended_flow_path.clone(),
                        seq: event.seq,
                    },
                );
            },
            EventPayload::RoadmapUpdated {
                patch_id,
                target,
                roadmap_artifact,
                roadmap_path,
                flow_artifact,
                flow_path,
                ..
            } => {
                self.upsert_roadmap_patch_artifacts(
                    patch_id,
                    target,
                    RoadmapPatchArtifactUpdate {
                        roadmap_artifact: *roadmap_artifact,
                        roadmap_path: roadmap_path.clone(),
                        flow_artifact: *flow_artifact,
                        flow_path: flow_path.clone(),
                        seq: event.seq,
                    },
                );
            },
            EventPayload::GraphRevisionAccepted {
                patch_id,
                target,
                previous_graph_hash,
                graph_hash,
                active_pickup,
                ..
            } => {
                self.latest_graph_revision = Some(GraphRevisionMemory {
                    patch_id: patch_id.clone(),
                    target: target.clone(),
                    previous_graph_hash: *previous_graph_hash,
                    graph_hash: *graph_hash,
                    active_pickup: *active_pickup,
                    updated_seq: event.seq,
                });
            },
            _ => {},
        }
    }

    fn upsert_roadmap_patch_status(
        &mut self,
        patch_id: &RoadmapPatchId,
        target: &RoadmapPatchTarget,
        status: RoadmapPatchStatus,
        seq: u64,
    ) {
        self.roadmap_patches
            .entry(patch_id.clone())
            .and_modify(|record| {
                record.target = target.clone();
                record.status = status;
                record.updated_seq = seq;
            })
            .or_insert_with(|| RoadmapPatchMemory {
                target: target.clone(),
                status,
                patch_artifact: None,
                patch_path: None,
                roadmap_artifact: None,
                roadmap_path: None,
                flow_artifact: None,
                flow_path: None,
                updated_seq: seq,
            });
    }

    fn update_roadmap_patch_decision(
        &mut self,
        patch_id: &RoadmapPatchId,
        decision: RoadmapPatchApprovalDecision,
        seq: u64,
    ) {
        let status = match decision {
            RoadmapPatchApprovalDecision::Approve => RoadmapPatchStatus::Approved,
            RoadmapPatchApprovalDecision::Edit => RoadmapPatchStatus::Drafted,
            RoadmapPatchApprovalDecision::Reject => RoadmapPatchStatus::Rejected,
        };
        if let Some(record) = self.roadmap_patches.get_mut(patch_id) {
            record.status = status;
            record.updated_seq = seq;
        }
    }

    fn upsert_roadmap_patch_artifacts(
        &mut self,
        patch_id: &RoadmapPatchId,
        target: &RoadmapPatchTarget,
        update: RoadmapPatchArtifactUpdate,
    ) {
        self.roadmap_patches
            .entry(patch_id.clone())
            .and_modify(|record| {
                record.target = target.clone();
                record.status = RoadmapPatchStatus::Applied;
                record.roadmap_artifact = Some(update.roadmap_artifact);
                record.roadmap_path = Some(update.roadmap_path.clone());
                record.flow_artifact = update.flow_artifact;
                record.flow_path = update.flow_path.clone();
                record.updated_seq = update.seq;
            })
            .or_insert_with(|| RoadmapPatchMemory {
                target: target.clone(),
                status: RoadmapPatchStatus::Applied,
                patch_artifact: None,
                patch_path: None,
                roadmap_artifact: Some(update.roadmap_artifact),
                roadmap_path: Some(update.roadmap_path),
                flow_artifact: update.flow_artifact,
                flow_path: update.flow_path,
                updated_seq: update.seq,
            });
    }
}

struct RoadmapPatchArtifactUpdate {
    roadmap_artifact: ContentHash,
    roadmap_path: PathBuf,
    flow_artifact: Option<ContentHash>,
    flow_path: Option<PathBuf>,
    seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::ApprovalPolicy;
    use crate::id::RunId;
    use crate::run_event::RunConfig;
    use crate::sandbox::SandboxMode;
    use chrono::Utc;
    use std::path::PathBuf;

    fn make_event(seq: u64, payload: EventPayload) -> RunEvent {
        RunEvent {
            run_id: RunId::new(),
            seq,
            timestamp: Utc::now(),
            payload,
        }
    }

    fn graph_with_terminal(name: &str, start: &str) -> Graph {
        use crate::graph::{GraphMetadata, SCHEMA_VERSION};
        use std::collections::BTreeMap;

        let start = NodeKey::try_from(start).unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(start.clone(), terminal_node(start.clone()));
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: name.into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    fn terminal_node(key: NodeKey) -> crate::node::Node {
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};

        Node {
            id: key,
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        }
    }

    #[test]
    fn not_started_is_default_initial() {
        let s = RunState::NotStarted;
        assert!(matches!(s, RunState::NotStarted));
    }

    #[test]
    fn empty_event_log_folds_to_not_started() {
        let state = fold(&[]).unwrap();
        assert!(matches!(state, RunState::NotStarted));
    }

    #[test]
    fn run_started_transitions_to_bootstrapping() {
        let events = vec![make_event(
            1,
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: PathBuf::from("/tmp"),
                initial_prompt: "test".into(),
                config: RunConfig {
                    budget: Default::default(),
                    sandbox_default: SandboxMode::WorkspaceWrite,
                    approval_default: ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                },
            },
        )];
        let state = fold(&events).unwrap();
        assert!(matches!(
            state,
            RunState::Bootstrapping {
                stage: BootstrapStage::Description,
                ..
            }
        ));
    }

    #[test]
    fn corrupted_sequence_returns_error() {
        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "test".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                99,
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("end").unwrap(),
                },
            ),
        ];
        let result = fold(&events);
        assert!(matches!(result, Err(FoldError::CorruptedSequence { .. })));
    }

    #[test]
    fn run_failed_transitions_to_terminal() {
        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "test".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::RunFailed {
                    error: "boom".into(),
                },
            ),
        ];
        let state = fold(&events).unwrap();
        assert!(matches!(
            state,
            RunState::Terminal {
                kind: TerminalReason::Failed,
                ..
            }
        ));
    }

    #[test]
    fn run_memory_default_is_empty() {
        let m = RunMemory::default();
        assert!(m.artifacts.is_empty());
        assert_eq!(m.costs.tokens_in, 0);
        assert!(m.bootstrap_edit_counts.is_empty());
        assert!(m.node_visits.is_empty());
        assert!(!m.budget_warning_raised);
    }

    #[test]
    fn budget_warning_raised_folds_into_memory_flag() {
        // The flag is derived from the event log so a resumed run does not
        // re-emit a warning it already recorded (warn-once idempotency).
        let mut m = RunMemory::default();
        assert!(!m.budget_warning_raised);
        m.apply_event(&make_event(
            1,
            EventPayload::BudgetWarningRaised {
                dimension: crate::budget::BudgetDimension::Tokens,
                pct: 80,
                cost_usd: 0.0,
                total_tokens: 800_000,
            },
        ));
        assert!(m.budget_warning_raised);
    }

    #[test]
    fn node_visits_counter_increments_on_backtrack_only() {
        // Forward traversals (`kind: Forward`) must NOT touch node_visits;
        // only Backtrack edges contribute, keyed by the target node.
        use crate::keys::EdgeKey;

        let mut m = RunMemory::default();
        let target = NodeKey::try_from("desc_author").unwrap();
        let other = NodeKey::try_from("plan_author").unwrap();

        // Forward — should be ignored.
        m.apply_event(&make_event(
            1,
            EventPayload::EdgeTraversed {
                edge: EdgeKey::try_from("e_fwd").unwrap(),
                from: NodeKey::try_from("start").unwrap(),
                to: target.clone(),
                kind: EdgeKind::Forward,
            },
        ));
        assert!(m.node_visits.is_empty());

        // First Backtrack into `target` — counter becomes 1.
        m.apply_event(&make_event(
            2,
            EventPayload::EdgeTraversed {
                edge: EdgeKey::try_from("e_bt1").unwrap(),
                from: NodeKey::try_from("gate1").unwrap(),
                to: target.clone(),
                kind: EdgeKind::Backtrack,
            },
        ));
        assert_eq!(m.node_visits[&target], 1);

        // Second Backtrack into the same node — counter becomes 2.
        m.apply_event(&make_event(
            3,
            EventPayload::EdgeTraversed {
                edge: EdgeKey::try_from("e_bt2").unwrap(),
                from: NodeKey::try_from("gate1").unwrap(),
                to: target.clone(),
                kind: EdgeKind::Backtrack,
            },
        ));
        assert_eq!(m.node_visits[&target], 2);

        // Backtrack into a different target — independent counter.
        m.apply_event(&make_event(
            4,
            EventPayload::EdgeTraversed {
                edge: EdgeKey::try_from("e_bt3").unwrap(),
                from: NodeKey::try_from("gate2").unwrap(),
                to: other.clone(),
                kind: EdgeKind::Backtrack,
            },
        ));
        assert_eq!(m.node_visits[&target], 2);
        assert_eq!(m.node_visits[&other], 1);
    }

    #[test]
    fn node_visits_fold_is_deterministic() {
        // Replay-determinism guard: folding the same event sequence twice
        // must produce identical `node_visits` maps. Confirms there is no
        // hidden mutation of prior outcomes when a backtrack lands on a
        // node whose stage already executed.
        use crate::keys::EdgeKey;

        let target = NodeKey::try_from("flow_gen").unwrap();
        let events: Vec<RunEvent> = (1u64..=4)
            .map(|seq| {
                make_event(
                    seq,
                    EventPayload::EdgeTraversed {
                        edge: EdgeKey::try_from(&*format!("e_{seq}")).unwrap(),
                        from: NodeKey::try_from("gate").unwrap(),
                        to: target.clone(),
                        kind: EdgeKind::Backtrack,
                    },
                )
            })
            .collect();

        let mut a = RunMemory::default();
        let mut b = RunMemory::default();
        for e in &events {
            a.apply_event(e);
            b.apply_event(e);
        }
        assert_eq!(a.node_visits, b.node_visits);
        assert_eq!(a.node_visits[&target], 4);
    }

    #[test]
    fn bootstrap_edit_counter_increments_per_stage() {
        let mut m = RunMemory::default();
        for &stage in &[
            BootstrapStage::Description,
            BootstrapStage::Description,
            BootstrapStage::Roadmap,
        ] {
            let evt = make_event(
                1,
                EventPayload::BootstrapEditRequested {
                    stage,
                    feedback: "tighten".into(),
                },
            );
            m.apply_event(&evt);
        }
        assert_eq!(m.bootstrap_edit_counts[&BootstrapStage::Description], 2);
        assert_eq!(m.bootstrap_edit_counts[&BootstrapStage::Roadmap], 1);
        assert!(!m.bootstrap_edit_counts.contains_key(&BootstrapStage::Flow));
    }

    #[test]
    fn bootstrap_edit_counter_is_deterministic() {
        // Folding the same event sequence twice must produce identical
        // bootstrap_edit_counts maps. Replay determinism guard.
        let events: Vec<RunEvent> = (1..=5)
            .map(|seq| {
                make_event(
                    seq,
                    EventPayload::BootstrapEditRequested {
                        stage: if seq % 2 == 0 {
                            BootstrapStage::Description
                        } else {
                            BootstrapStage::Flow
                        },
                        feedback: format!("note-{seq}"),
                    },
                )
            })
            .collect();

        let mut a = RunMemory::default();
        let mut b = RunMemory::default();
        for e in &events {
            a.apply_event(e);
            b.apply_event(e);
        }
        assert_eq!(a.bootstrap_edit_counts, b.bootstrap_edit_counts);
        assert_eq!(a.bootstrap_edit_counts[&BootstrapStage::Description], 2);
        assert_eq!(a.bootstrap_edit_counts[&BootstrapStage::Flow], 3);
    }

    #[test]
    fn pipeline_materialized_transitions_to_pipeline() {
        // Acceptance test for the fold→Pipeline path. The graph is part of
        // the PipelineMaterialized payload, so fold reconstructs Pipeline
        // state from the event log alone (no out-of-band channel needed).
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let end = NodeKey::try_from("end").unwrap();
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
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: end.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "test".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"hash-placeholder"),
                },
            ),
        ];
        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline { cursor, .. } => {
                assert_eq!(cursor.node, end);
                assert_eq!(cursor.attempt, 1);
            },
            other => panic!("expected Pipeline state, got {other:?}"),
        }
    }

    #[test]
    fn roadmap_patch_events_update_run_memory_without_mutating_graph() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let end = NodeKey::try_from("end").unwrap();
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
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "roadmap-patch-memory".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: end.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };
        let patch_id = RoadmapPatchId::new("rpatch-memory").unwrap();
        let target = RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        };
        let roadmap_hash = ContentHash::compute(b"roadmap");

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "test".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"graph"),
                },
            ),
            make_event(
                3,
                EventPayload::RoadmapPatchDrafted {
                    patch_id: patch_id.clone(),
                    target: target.clone(),
                    patch_artifact: ContentHash::compute(b"patch"),
                    patch_path: PathBuf::from("roadmap-patch.toml"),
                },
            ),
            make_event(
                4,
                EventPayload::RoadmapPatchApprovalDecided {
                    patch_id: patch_id.clone(),
                    decision: RoadmapPatchApprovalDecision::Approve,
                    channel_used: crate::approvals::ApprovalChannelKind::Desktop,
                    comment: None,
                    conflict_choice: None,
                },
            ),
            make_event(
                5,
                EventPayload::RoadmapUpdated {
                    patch_id: patch_id.clone(),
                    target,
                    roadmap_artifact: roadmap_hash,
                    roadmap_path: PathBuf::from("roadmap.toml"),
                    flow_artifact: None,
                    flow_path: None,
                    active_pickup: crate::roadmap_patch::ActivePickupPolicy::Allowed,
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline {
                graph,
                cursor,
                memory,
                ..
            } => {
                assert_eq!(cursor.node, end);
                assert_eq!(graph.start, end, "roadmap events must not mutate graph");
                let patch = &memory.roadmap_patches[&patch_id];
                assert_eq!(patch.status, RoadmapPatchStatus::Applied);
                assert_eq!(patch.roadmap_artifact, Some(roadmap_hash));
                assert_eq!(patch.updated_seq, 5);
            },
            other => panic!("expected Pipeline, got {other:?}"),
        }
    }

    #[test]
    fn graph_revision_event_updates_pipeline_graph_and_revision_memory() {
        let base_graph = graph_with_terminal("base", "end");
        let mut amended_graph = graph_with_terminal("amended", "amend_001");
        let old_cursor_node = NodeKey::try_from("end").unwrap();
        amended_graph.nodes.insert(
            old_cursor_node.clone(),
            terminal_node(old_cursor_node.clone()),
        );
        let patch_id = RoadmapPatchId::new("rpatch-graph").unwrap();
        let target = RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        };
        let previous_graph_hash = ContentHash::compute(b"base-flow");
        let graph_hash = ContentHash::compute(b"amended-flow");

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "test".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(base_graph),
                    graph_hash: previous_graph_hash,
                },
            ),
            make_event(
                3,
                EventPayload::GraphRevisionAccepted {
                    patch_id: patch_id.clone(),
                    target: target.clone(),
                    previous_graph_hash,
                    graph: Box::new(amended_graph),
                    graph_hash,
                    active_pickup: ActivePickupPolicy::Allowed,
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline {
                graph,
                cursor,
                memory,
                ..
            } => {
                assert_eq!(graph.start, NodeKey::try_from("amend_001").unwrap());
                assert_eq!(cursor.node, old_cursor_node);
                let revision = memory
                    .latest_graph_revision
                    .expect("graph revision metadata recorded");
                assert_eq!(revision.patch_id, patch_id);
                assert_eq!(revision.target, target);
                assert_eq!(revision.previous_graph_hash, previous_graph_hash);
                assert_eq!(revision.graph_hash, graph_hash);
                assert_eq!(revision.updated_seq, 3);
            },
            other => panic!("expected Pipeline, got {other:?}"),
        }
    }

    #[test]
    fn cursor_clones_cheaply() {
        let c = Cursor {
            node: NodeKey::try_from("n").unwrap(),
            attempt: 1,
        };
        let _c2 = c.clone();
    }

    #[test]
    fn human_input_request_populates_pending_field() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let plan = NodeKey::try_from("plan").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            plan.clone(),
            Node {
                id: plan.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: plan.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "build".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"hash"),
                },
            ),
            make_event(
                3,
                EventPayload::HumanInputRequested {
                    node: plan.clone(),
                    session: None,
                    call_id: Some("c1".into()),
                    prompt: "ok?".into(),
                    schema: None,
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline {
                pending_human_input: Some(p),
                ..
            } => {
                assert_eq!(p.node, plan);
                assert_eq!(p.call_id.as_deref(), Some("c1"));
            },
            other => panic!("expected Pipeline with pending_human_input, got {other:?}"),
        }
    }

    #[test]
    fn backtrack_traversal_then_re_entry_advances_cursor_and_increments_visits() {
        // End-to-end fold-level proof of Task 27 semantics. The event log
        // models a HumanGate edit-loop: an Agent stage runs and reports an
        // outcome (`needs_edit`), the gate emits a `Backtrack` traversal
        // back to that same Agent node, and the engine re-enters the stage
        // (StageEntered, attempt=2). Folding the log must produce a
        // `Pipeline` state whose cursor sits on the Agent node with
        // attempt=2 and whose `RunMemory.node_visits[<agent>]` equals 1
        // (one Backtrack into that node so far). Forward traversals
        // earlier in the log must NOT contribute to the counter.
        use crate::approvals::ApprovalPolicy;
        use crate::content_hash::ContentHash;
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::keys::EdgeKey;
        use crate::node::{Node, NodeConfig, Position};
        use crate::run_event::RunConfig;
        use crate::sandbox::SandboxMode;
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let agent = NodeKey::try_from("desc_author").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            agent.clone(),
            Node {
                id: agent.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "bootstrap-edit-loop".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: agent.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "build".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"hash"),
                },
            ),
            make_event(
                3,
                EventPayload::StageEntered {
                    node: agent.clone(),
                    attempt: 1,
                },
            ),
            // Forward traversal earlier in the run — must NOT bump
            // node_visits.
            make_event(
                4,
                EventPayload::EdgeTraversed {
                    edge: EdgeKey::try_from("e_fwd").unwrap(),
                    from: NodeKey::try_from("start_node").unwrap(),
                    to: agent.clone(),
                    kind: EdgeKind::Forward,
                },
            ),
            // Operator selects "edit" — gate routes back via Backtrack.
            make_event(
                5,
                EventPayload::EdgeTraversed {
                    edge: EdgeKey::try_from("e_back").unwrap(),
                    from: NodeKey::try_from("gate_desc").unwrap(),
                    to: agent.clone(),
                    kind: EdgeKind::Backtrack,
                },
            ),
            // Engine re-enters the Agent stage with a fresh attempt
            // counter — the standard StageEntered fold rule advances the
            // cursor.
            make_event(
                6,
                EventPayload::StageEntered {
                    node: agent.clone(),
                    attempt: 2,
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline { cursor, memory, .. } => {
                assert_eq!(cursor.node, agent);
                assert_eq!(cursor.attempt, 2, "Backtrack must re-enter the stage");
                assert_eq!(
                    memory.node_visits[&agent], 1,
                    "node_visits must increment exactly once per Backtrack",
                );
                assert!(
                    !memory
                        .node_visits
                        .contains_key(&NodeKey::try_from("start_node").unwrap()),
                    "Forward edges must not populate node_visits",
                );
            },
            other => panic!("expected Pipeline, got {other:?}"),
        }
    }

    /// Build a graph whose start is a terminal node and which also contains a
    /// `verify` node declaring a `LedgerEffect::Verified` outcome (the
    /// graph-visible verification-authority signal) plus a plain `impl` node
    /// with no ledger effect.
    fn ledger_graph() -> Graph {
        use crate::edge::EdgeKind;
        use crate::graph::{GraphMetadata, SCHEMA_VERSION};
        use crate::keys::OutcomeKey;
        use crate::node::{LedgerEffect, Node, NodeConfig, OutcomeDecl, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let start = NodeKey::try_from("end").unwrap();
        let verify = NodeKey::try_from("verify_1").unwrap();
        let implement = NodeKey::try_from("impl_1").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(start.clone(), terminal_node(start.clone()));
        nodes.insert(
            verify.clone(),
            Node {
                id: verify.clone(),
                position: Position::default(),
                declared_outcomes: vec![OutcomeDecl {
                    id: OutcomeKey::try_from("passed").unwrap(),
                    description: "verified".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: LedgerEffect::Verified,
                }],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        nodes.insert(
            implement.clone(),
            Node {
                id: implement.clone(),
                position: Position::default(),
                declared_outcomes: vec![OutcomeDecl {
                    id: OutcomeKey::try_from("ready_for_verification").unwrap(),
                    description: "impl done".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: LedgerEffect::ReadyForVerification,
                }],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "ledger".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    fn ledger_run_prefix() -> Vec<RunEvent> {
        vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "build".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(ledger_graph()),
                    graph_hash: ContentHash::compute(b"ledger-graph"),
                },
            ),
        ]
    }

    #[test]
    fn node_authority_is_graph_visible_via_verified_outcome() {
        let graph = ledger_graph();
        assert!(node_has_verification_authority(
            &graph,
            &NodeKey::try_from("verify_1").unwrap()
        ));
        assert!(!node_has_verification_authority(
            &graph,
            &NodeKey::try_from("impl_1").unwrap()
        ));
        assert!(!node_has_verification_authority(
            &graph,
            &NodeKey::try_from("missing").unwrap()
        ));
    }

    #[test]
    fn subgraph_verify_node_has_authority() {
        use crate::edge::EdgeKind;
        use crate::graph::Subgraph;
        use crate::keys::{OutcomeKey, SubgraphKey};
        use crate::node::{LedgerEffect, Node, NodeConfig, OutcomeDecl, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        // Bundled loop flows run the sealed verifier inside a task-body
        // subgraph; its TaskVerified must be honored (regression: top-level-only
        // lookup rejected every subgraph verifier).
        let mut graph = ledger_graph();
        let verify_in_task = NodeKey::try_from("verify_in_task").unwrap();
        let mut sg_nodes = BTreeMap::new();
        sg_nodes.insert(
            verify_in_task.clone(),
            Node {
                id: verify_in_task.clone(),
                position: Position::default(),
                declared_outcomes: vec![OutcomeDecl {
                    id: OutcomeKey::try_from("passed").unwrap(),
                    description: "verified".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: LedgerEffect::Verified,
                }],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        graph.subgraphs.insert(
            SubgraphKey::try_from("task_body").unwrap(),
            Subgraph {
                start: verify_in_task.clone(),
                nodes: sg_nodes,
                edges: vec![],
            },
        );

        assert!(
            node_has_verification_authority(&graph, &verify_in_task),
            "a verify node inside a subgraph must carry authority"
        );
    }

    #[test]
    fn task_status_change_and_discovery_fold_into_ledger() {
        let verify = NodeKey::try_from("verify_1").unwrap();
        let mut events = ledger_run_prefix();
        events.push(make_event(
            3,
            EventPayload::TaskStatusChanged {
                task_id: "m1-t1".into(),
                from: RoadmapStatus::Pending,
                to: RoadmapStatus::ReadyForVerification,
                authority_node: NodeKey::try_from("impl_1").unwrap(),
            },
        ));
        events.push(make_event(
            4,
            EventPayload::TaskDiscovered {
                task_id: "m1-t2".into(),
                discovered_from: "m1-t1".into(),
                title: "Handle empty input".into(),
            },
        ));
        events.push(make_event(
            5,
            EventPayload::TaskVerified {
                task_id: "m1-t1".into(),
                node: verify.clone(),
                evidence: ContentHash::compute(b"report"),
            },
        ));

        let RunState::Pipeline { memory, .. } = fold(&events).unwrap() else {
            panic!("expected Pipeline");
        };
        let t1 = &memory.ledger.tasks["m1-t1"];
        assert_eq!(t1.status, RoadmapStatus::Completed);
        assert!(t1.verified);
        assert_eq!(t1.last_authority_node.as_ref(), Some(&verify));
        assert_eq!(t1.updated_seq, 5);

        let t2 = &memory.ledger.tasks["m1-t2"];
        assert_eq!(t2.status, RoadmapStatus::Pending);
        assert!(!t2.verified);
        assert_eq!(t2.discovered_from.as_deref(), Some("m1-t1"));
        assert_eq!(memory.ledger.rejected_verifications, 0);
    }

    #[test]
    fn unauthorized_task_verified_is_ignored_and_counted() {
        // `impl_1` has a ReadyForVerification outcome but NOT Verified, so it
        // is not a verification authority. A TaskVerified naming it must not
        // flip the task to verified — it is rejected and counted.
        let mut events = ledger_run_prefix();
        events.push(make_event(
            3,
            EventPayload::TaskStatusChanged {
                task_id: "m1-t1".into(),
                from: RoadmapStatus::Pending,
                to: RoadmapStatus::ReadyForVerification,
                authority_node: NodeKey::try_from("impl_1").unwrap(),
            },
        ));
        events.push(make_event(
            4,
            EventPayload::TaskVerified {
                task_id: "m1-t1".into(),
                node: NodeKey::try_from("impl_1").unwrap(),
                evidence: ContentHash::compute(b"forged"),
            },
        ));

        let RunState::Pipeline { memory, .. } = fold(&events).unwrap() else {
            panic!("expected Pipeline");
        };
        let t1 = &memory.ledger.tasks["m1-t1"];
        assert_eq!(t1.status, RoadmapStatus::ReadyForVerification);
        assert!(!t1.verified, "unauthorized verification must not stick");
        assert_eq!(memory.ledger.rejected_verifications, 1);
    }

    #[test]
    fn ledger_fold_is_deterministic() {
        let verify = NodeKey::try_from("verify_1").unwrap();
        let mut events = ledger_run_prefix();
        events.push(make_event(
            3,
            EventPayload::TaskDiscovered {
                task_id: "m1-t1".into(),
                discovered_from: "seed".into(),
                title: "t1".into(),
            },
        ));
        events.push(make_event(
            4,
            EventPayload::TaskVerified {
                task_id: "m1-t1".into(),
                node: verify,
                evidence: ContentHash::compute(b"report"),
            },
        ));
        // Duplicate discovery must be a no-op (first-write-wins).
        events.push(make_event(
            5,
            EventPayload::TaskDiscovered {
                task_id: "m1-t1".into(),
                discovered_from: "other".into(),
                title: "dup".into(),
            },
        ));

        let RunState::Pipeline { memory: a, .. } = fold(&events).unwrap() else {
            panic!("expected Pipeline");
        };
        let RunState::Pipeline { memory: b, .. } = fold(&events).unwrap() else {
            panic!("expected Pipeline");
        };
        assert_eq!(a.ledger, b.ledger);
        assert_eq!(
            a.ledger.tasks["m1-t1"].discovered_from.as_deref(),
            Some("seed")
        );
        assert!(a.ledger.tasks["m1-t1"].verified);
    }

    #[test]
    fn status_change_after_verified_clears_verified_flag() {
        // If a TaskStatusChanged arrives after a TaskVerified (reordered log
        // or engine bug), the task must not retain verified=true with a
        // non-Completed status.
        let mut events = ledger_run_prefix();
        events.push(make_event(
            3,
            EventPayload::TaskVerified {
                task_id: "m1-t1".into(),
                node: NodeKey::try_from("verify_1").unwrap(),
                evidence: ContentHash::compute(b"report"),
            },
        ));
        events.push(make_event(
            4,
            EventPayload::TaskStatusChanged {
                task_id: "m1-t1".into(),
                from: RoadmapStatus::Completed,
                to: RoadmapStatus::ReadyForVerification,
                authority_node: NodeKey::try_from("impl_1").unwrap(),
            },
        ));

        let RunState::Pipeline { memory, .. } = fold(&events).unwrap() else {
            panic!("expected Pipeline");
        };
        let t1 = &memory.ledger.tasks["m1-t1"];
        assert_eq!(t1.status, RoadmapStatus::ReadyForVerification);
        assert!(
            !t1.verified,
            "verified must be cleared after non-Completed status change"
        );
    }

    #[test]
    fn attention_classifies_pipeline_working_and_needs_input() {
        let mut events = ledger_run_prefix();
        // Fold with only the run prefix (RunStarted + PipelineMaterialized) →
        // Pipeline, no pending input → Working.
        let working = fold(&events).unwrap();
        assert_eq!(working.attention(), Attention::Working);
        assert_eq!(working.pending_prompt(), None);

        // A HumanInputRequested puts it into NeedsInput with the prompt.
        events.push(make_event(
            3,
            EventPayload::HumanInputRequested {
                node: NodeKey::try_from("verify_1").unwrap(),
                session: None,
                call_id: Some("c1".into()),
                prompt: "Approve the risky migration?".into(),
                schema: None,
            },
        ));
        let blocked = fold(&events).unwrap();
        assert_eq!(blocked.attention(), Attention::NeedsInput);
        assert_eq!(
            blocked.pending_prompt(),
            Some("Approve the risky migration?")
        );

        // Resolving it returns to Working.
        events.push(make_event(
            4,
            EventPayload::HumanInputResolved {
                node: NodeKey::try_from("verify_1").unwrap(),
                call_id: Some("c1".into()),
                response: serde_json::json!({"decision": "approve"}),
            },
        ));
        assert_eq!(fold(&events).unwrap().attention(), Attention::Working);
    }

    #[test]
    fn attention_classifies_terminal_as_done() {
        assert_eq!(
            RunState::Terminal {
                kind: TerminalReason::Completed,
                reason: String::new(),
            }
            .attention(),
            Attention::Done(TerminalReason::Completed)
        );
        assert_eq!(
            RunState::NotStarted.attention(),
            Attention::Working,
            "a not-yet-folded run is working, not blocked"
        );
    }

    #[test]
    fn attention_classifies_bootstrap_awaiting_approval_as_needs_input() {
        let awaiting = RunState::Bootstrapping {
            stage: BootstrapStage::Flow,
            substate: BootstrapSubstate::AwaitingApproval {
                artifact: ContentHash::compute(b"flow"),
                requested_seq: 5,
            },
        };
        assert_eq!(awaiting.attention(), Attention::NeedsInput);

        let running = RunState::Bootstrapping {
            stage: BootstrapStage::Description,
            substate: BootstrapSubstate::AgentRunning {
                session: SessionId::nil(),
                started_seq: 1,
            },
        };
        assert_eq!(running.attention(), Attention::Working);
    }

    #[test]
    fn human_input_resolution_clears_pending_field() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let plan = NodeKey::try_from("plan").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            plan.clone(),
            Node {
                id: plan.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: plan.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "build".into(),
                    config: RunConfig {
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"hash"),
                },
            ),
            make_event(
                3,
                EventPayload::HumanInputRequested {
                    node: plan.clone(),
                    session: None,
                    call_id: Some("c1".into()),
                    prompt: "ok?".into(),
                    schema: None,
                },
            ),
            make_event(
                4,
                EventPayload::HumanInputResolved {
                    node: plan.clone(),
                    call_id: Some("c1".into()),
                    response: serde_json::json!({"decision": "approve"}),
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline {
                pending_human_input: None,
                ..
            } => {},
            other => panic!("expected Pipeline with cleared pending_human_input, got {other:?}"),
        }
    }
}
