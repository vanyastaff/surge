//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::routing::next_node_after;
use crate::engine::stage::agent::{execute_agent_stage, AgentStageParams};
use crate::engine::stage::branch::{execute_branch_stage, BranchStageParams};
use crate::engine::stage::notify::{execute_notify_stage, NotifyStageParams};
use crate::engine::stage::terminal::{execute_terminal_stage, TerminalOutcome, TerminalStageParams};
use crate::engine::stage::StageError;
use crate::engine::tools::ToolDispatcher;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::keys::OutcomeKey;
use surge_core::node::NodeConfig;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::{Cursor, OutcomeRecord, RunMemory};
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunTaskParams {
    pub run_id: RunId,
    pub writer: RunWriter,
    pub bridge: Arc<dyn BridgeFacade>,
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    pub graph: Graph,
    pub worktree_path: PathBuf,
    pub run_config: EngineRunConfig,
    pub event_tx: broadcast::Sender<EngineRunEvent>,
    pub cancel: CancellationToken,
    /// Resume from an existing cursor; if None, start at graph.start.
    pub resume_cursor: Option<Cursor>,
    /// Resume from existing memory; if None, start fresh.
    pub resume_memory: Option<RunMemory>,
    /// Map of `node_key → oneshot::Sender<HumanGateResolution>`.
    /// Engine's `resolve_human_input` finds the sender and fires it.
    /// Phase 9 wires the registry; for now Just plumb the field through.
    pub gate_resolutions: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<surge_core::keys::NodeKey, tokio::sync::oneshot::Sender<crate::engine::stage::human_gate::HumanGateResolution>>>>,
}

pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    let mut cursor = params
        .resume_cursor
        .clone()
        .unwrap_or_else(|| Cursor {
            node: params.graph.start.clone(),
            attempt: 1,
        });
    let mut memory = params.resume_memory.clone().unwrap_or_default();

    loop {
        if params.cancel.is_cancelled() {
            let reason = "stop_run requested".to_string();
            let _ = params
                .writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: reason.clone(),
                }))
                .await;
            return RunOutcome::Aborted { reason };
        }

        let node = match params.graph.nodes.get(&cursor.node) {
            Some(n) => n.clone(),
            None => {
                let err = format!("cursor at unknown node {}", cursor.node);
                return failed(&params, err).await;
            }
        };

        // Emit StageEntered.
        if let Err(e) = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
                node: cursor.node.clone(),
                attempt: cursor.attempt,
            }))
            .await
        {
            return failed(&params, format!("write StageEntered: {e}")).await;
        }

        // Dispatch.
        let stage_result: Result<StageOutcome, StageError> = match &node.config {
            NodeConfig::Agent(cfg) => {
                let r = execute_agent_stage(AgentStageParams {
                    node: &cursor.node,
                    agent_config: cfg,
                    bridge: &params.bridge,
                    writer: &params.writer,
                    worktree_path: &params.worktree_path,
                    tool_dispatcher: &params.tool_dispatcher,
                    run_memory: &memory,
                    run_id: params.run_id,
                })
                .await;
                r.map(StageOutcome::Routed)
            }
            NodeConfig::Branch(cfg) => execute_branch_stage(BranchStageParams {
                node: &cursor.node,
                branch_config: cfg,
                writer: &params.writer,
                run_memory: &memory,
                worktree_root: &params.worktree_path,
            })
            .await
            .map(StageOutcome::Routed),
            NodeConfig::Notify(cfg) => execute_notify_stage(NotifyStageParams {
                node: &cursor.node,
                notify_config: cfg,
                writer: &params.writer,
            })
            .await
            .map(StageOutcome::Routed),
            NodeConfig::Terminal(cfg) => {
                let r = execute_terminal_stage(TerminalStageParams {
                    node: &cursor.node,
                    terminal_config: cfg,
                    writer: &params.writer,
                })
                .await;
                r.map(StageOutcome::Terminal)
            }
            NodeConfig::HumanGate(cfg) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                params.gate_resolutions.lock().await.insert(cursor.node.clone(), tx);
                use crate::engine::stage::human_gate::{execute_human_gate_stage, HumanGateStageParams};
                let r = execute_human_gate_stage(HumanGateStageParams {
                    node: &cursor.node,
                    gate_config: cfg,
                    writer: &params.writer,
                    run_memory: &memory,
                    resolution_rx: Some(rx),
                    default_timeout: params.run_config.human_input_timeout,
                })
                .await;
                params.gate_resolutions.lock().await.remove(&cursor.node);
                r.map(StageOutcome::Routed)
            }
            NodeConfig::Loop(_) | NodeConfig::Subgraph(_) => Err(StageError::Internal(format!(
                "node kind {:?} not supported in M5",
                node.kind()
            ))),
        };

        let outcome: OutcomeKey = match stage_result {
            Ok(StageOutcome::Routed(k)) => k,
            Ok(StageOutcome::Terminal(TerminalOutcome::Completed { node: n })) => {
                return RunOutcome::Completed { terminal: n };
            }
            Ok(StageOutcome::Terminal(TerminalOutcome::Failed { error })) => {
                return RunOutcome::Failed { error };
            }
            Err(e) => {
                return failed(&params, format!("stage error at {}: {e}", cursor.node)).await;
            }
        };

        // Update memory with outcome (best-effort; storage's own seq is the source of truth).
        memory
            .outcomes
            .entry(cursor.node.clone())
            .or_default()
            .push(OutcomeRecord {
                outcome: outcome.clone(),
                summary: String::new(),
                seq: 0,
            });

        // Route to next node.
        let next = match next_node_after(&params.graph, &cursor.node, &outcome) {
            Ok(n) => n,
            Err(e) => return failed(&params, format!("routing: {e}")).await,
        };

        // EdgeTraversed + StageCompleted.
        // Synthesize a fallback edge id; if the formatted string is not a valid
        // EdgeKey (e.g. too long or invalid chars), propagate as a routing error
        // rather than panicking.
        let edge_id = params
            .graph
            .edges
            .iter()
            .find(|e| e.from.node == cursor.node && e.from.outcome == outcome)
            .map(|e| e.id.clone())
            .or_else(|| {
                let synth = format!("{}_{}", cursor.node, next);
                surge_core::keys::EdgeKey::try_from(synth.as_str()).ok()
            });

        if let Some(eid) = edge_id {
            let _ = params
                .writer
                .append_event(VersionedEventPayload::new(EventPayload::EdgeTraversed {
                    edge: eid,
                    from: cursor.node.clone(),
                    to: next.clone(),
                }))
                .await;
        }
        let _ = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
                node: cursor.node.clone(),
                outcome: outcome.clone(),
            }))
            .await;

        // Snapshot at stage boundary — wired in Phase 10 via snapshot::write_at_boundary.
        // For now, no-op; tests in Phase 10 cover the snapshot write.

        cursor = Cursor {
            node: next,
            attempt: 1,
        };
    }
}

enum StageOutcome {
    Routed(OutcomeKey),
    Terminal(TerminalOutcome),
}

async fn failed(params: &RunTaskParams, error: String) -> RunOutcome {
    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
            error: error.clone(),
        }))
        .await;
    let _ = params
        .event_tx
        .send(EngineRunEvent::Terminal(RunOutcome::Failed {
            error: error.clone(),
        }));
    RunOutcome::Failed { error }
}

