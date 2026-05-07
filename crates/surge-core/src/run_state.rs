//! Run state machine — derived purely by folding events.

use crate::content_hash::ContentHash;
use crate::edge::EdgeKind;
use crate::graph::Graph;
use crate::id::SessionId;
use crate::keys::{NodeKey, OutcomeKey};
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
        /// `Arc<Graph>` because the graph is frozen post-PipelineMaterialized.
        /// Each fold step shares the same graph; cloning is one atomic increment.
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
            EventPayload::BootstrapEditRequested { stage, .. } => {
                *self.bootstrap_edit_counts.entry(*stage).or_insert(0) += 1;
            },
            EventPayload::EdgeTraversed {
                kind: EdgeKind::Backtrack,
                to,
                ..
            } => {
                *self.node_visits.entry(to.clone()).or_insert(0) += 1;
            },
            _ => {},
        }
    }
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
            RunState::Pipeline {
                cursor, memory, ..
            } => {
                assert_eq!(cursor.node, agent);
                assert_eq!(cursor.attempt, 2, "Backtrack must re-enter the stage");
                assert_eq!(
                    memory.node_visits[&agent], 1,
                    "node_visits must increment exactly once per Backtrack",
                );
                assert!(
                    !memory.node_visits.contains_key(&NodeKey::try_from("start_node").unwrap()),
                    "Forward edges must not populate node_visits",
                );
            },
            other => panic!("expected Pipeline, got {other:?}"),
        }
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
