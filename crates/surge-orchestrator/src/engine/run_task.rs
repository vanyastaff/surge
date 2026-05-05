//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::stage::StageError;
use crate::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use crate::engine::stage::branch::{BranchStageParams, execute_branch_stage};
use crate::engine::stage::human_gate::{HumanGateStageParams, execute_human_gate_stage};
use crate::engine::stage::notify::{NotifyStageParams, execute_notify_stage};
use crate::engine::stage::terminal::{
    TerminalOutcome, TerminalStageParams, execute_terminal_stage,
};
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
use surge_notify::NotifyDeliverer;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunTaskParams {
    pub run_id: RunId,
    pub writer: RunWriter,
    pub bridge: Arc<dyn BridgeFacade>,
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    pub notify_deliverer: Arc<dyn NotifyDeliverer>,
    pub graph: Graph,
    pub worktree_path: PathBuf,
    pub run_config: EngineRunConfig,
    pub event_tx: broadcast::Sender<EngineRunEvent>,
    pub cancel: CancellationToken,
    /// Resume from an existing cursor; if None, start at graph.start.
    pub resume_cursor: Option<Cursor>,
    /// Resume from existing memory; if None, start fresh.
    pub resume_memory: Option<RunMemory>,
    /// Resume from an existing frame stack; if None, start with an empty stack.
    pub resume_frames: Option<Vec<crate::engine::frames::Frame>>,
    /// Resume from existing root traversal counts; if None, start fresh.
    pub resume_root_traversal_counts:
        Option<std::collections::HashMap<surge_core::keys::EdgeKey, u32>>,
    /// Map of `node_key → oneshot::Sender<HumanGateResolution>`.
    /// Engine's `resolve_human_input` finds the sender and fires it.
    pub gate_resolutions: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<
                surge_core::keys::NodeKey,
                tokio::sync::oneshot::Sender<crate::engine::stage::human_gate::HumanGateResolution>,
            >,
        >,
    >,
    /// Map of `call_id → oneshot::Sender<serde_json::Value>`.
    /// Engine's `resolve_human_input` finds the sender and fires it for
    /// tool-driven `request_human_input` calls from agent stages.
    pub tool_resolutions: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>,
        >,
    >,
    /// Optional MCP registry. When `Some`, agent stages wrap the
    /// engine dispatcher with `RoutingToolDispatcher` to expose
    /// configured MCP tools alongside engine built-ins.
    pub mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
    /// Run-level MCP server registry (mirror of
    /// `RunConfig::mcp_servers`). Per-stage `ToolOverride::mcp_add`
    /// references entries by name; agent stages use this to look
    /// up timeouts and `allowed_tools` filters.
    pub mcp_servers: Vec<surge_core::mcp_config::McpServerRef>,
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    let mut cursor = params.resume_cursor.clone().unwrap_or_else(|| Cursor {
        node: params.graph.start.clone(),
        attempt: 1,
    });
    let mut memory = params.resume_memory.clone().unwrap_or_default();
    let mut frames: Vec<crate::engine::frames::Frame> =
        params.resume_frames.clone().unwrap_or_default();
    let mut root_traversal_counts: std::collections::HashMap<surge_core::keys::EdgeKey, u32> =
        params
            .resume_root_traversal_counts
            .clone()
            .unwrap_or_default();

    loop {
        if params.cancel.is_cancelled() {
            let reason = "stop_run requested".to_string();
            let _ = params
                .writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: reason.clone(),
                }))
                .await;
            let outcome = RunOutcome::Aborted { reason };
            let _ = params
                .event_tx
                .send(EngineRunEvent::Terminal(outcome.clone()));
            return outcome;
        }

        let node = if let Some(n) = lookup_in_active_frame(&params.graph, &cursor.node, &frames) {
            n.clone()
        } else {
            let err = format!("cursor at unknown node {}", cursor.node);
            return failed(&params, err).await;
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
                    declared_outcomes: &node.declared_outcomes,
                    bridge: &params.bridge,
                    writer: &params.writer,
                    worktree_path: &params.worktree_path,
                    tool_dispatcher: &params.tool_dispatcher,
                    run_memory: &memory,
                    run_id: params.run_id,
                    tool_resolutions: &params.tool_resolutions,
                    human_input_timeout: params.run_config.human_input_timeout,
                    mcp_registry: params.mcp_registry.clone(),
                    mcp_servers: params.mcp_servers.clone(),
                })
                .await;
                r.map(StageOutcome::Routed)
            },
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
                declared_outcomes: &node.declared_outcomes,
                writer: &params.writer,
                run_memory: &memory,
                run_id: params.run_id,
                deliverer: params.notify_deliverer.clone(),
            })
            .await
            .map(StageOutcome::Routed),
            NodeConfig::Terminal(cfg) => {
                use crate::engine::frames::TerminalSignal;
                match crate::engine::frames::on_terminal_decision(&frames, &cursor) {
                    TerminalSignal::OuterComplete => {
                        let r = execute_terminal_stage(TerminalStageParams {
                            node: &cursor.node,
                            terminal_config: cfg,
                            writer: &params.writer,
                        })
                        .await;
                        r.map(StageOutcome::Terminal)
                    },
                    TerminalSignal::LoopIterDone => {
                        // The most recent OutcomeReported event drives the iteration's outcome.
                        let just_completed = memory
                            .outcomes
                            .get(&cursor.node)
                            .and_then(|recs| recs.last())
                            .map_or_else(
                                || {
                                    surge_core::keys::OutcomeKey::try_from("completed")
                                        .expect("'completed' is valid OutcomeKey")
                                },
                                |r| r.outcome.clone(),
                            );

                        if let Err(e) = crate::engine::stage::loop_stage::on_loop_iteration_done(
                            &just_completed,
                            &params.graph,
                            &mut frames,
                            &mut cursor,
                            &params.writer,
                        )
                        .await
                        {
                            return failed(&params, format!("loop iter done: {e}")).await;
                        }
                        continue;
                    },
                    TerminalSignal::SubgraphDone => {
                        // Look up the outer SubgraphConfig::outputs by walking back to the
                        // outer node referenced by the top frame.
                        let outputs = match frames.last() {
                            Some(crate::engine::frames::Frame::Subgraph(sf)) => {
                                match params.graph.nodes.get(&sf.outer_node).map(|n| &n.config) {
                                    Some(surge_core::node::NodeConfig::Subgraph(cfg)) => {
                                        cfg.outputs.clone()
                                    },
                                    _ => {
                                        return failed(
                                            &params,
                                            format!(
                                                "outer subgraph node {} missing or wrong kind",
                                                sf.outer_node
                                            ),
                                        )
                                        .await;
                                    },
                                }
                            },
                            _ => {
                                return failed(
                                    &params,
                                    "SubgraphDone signal but no Subgraph frame on top".into(),
                                )
                                .await;
                            },
                        };

                        if let Err(e) = crate::engine::stage::subgraph_stage::on_subgraph_done(
                            &outputs,
                            &memory,
                            &mut frames,
                            &mut cursor,
                            &params.writer,
                        )
                        .await
                        {
                            return failed(&params, format!("subgraph done: {e}")).await;
                        }
                        continue;
                    },
                }
            },
            NodeConfig::HumanGate(cfg) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                params
                    .gate_resolutions
                    .lock()
                    .await
                    .insert(cursor.node.clone(), tx);
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
            },
            NodeConfig::Loop(cfg) => {
                // Compute return_to (outer-graph node to advance to when loop completes).
                let completed_outcome = match surge_core::keys::OutcomeKey::try_from("completed") {
                    Ok(o) => o,
                    Err(e) => return failed(&params, format!("'completed' outcome: {e}")).await,
                };
                let return_to = match crate::engine::routing::edge_target_after_outcome_or_default(
                    &params.graph,
                    &cursor.node,
                    &completed_outcome,
                ) {
                    Ok(n) => n,
                    Err(e) => return failed(&params, format!("loop return_to: {e}")).await,
                };

                let effect = match crate::engine::stage::loop_stage::execute_loop_entry(
                    crate::engine::stage::loop_stage::LoopStageParams {
                        node: &cursor.node,
                        loop_config: cfg,
                        graph: &params.graph,
                        run_memory: &memory,
                        writer: &params.writer,
                        frames: &mut frames,
                        return_to,
                    },
                )
                .await
                {
                    Ok(e) => e,
                    Err(e) => return failed(&params, format!("loop entry: {e}")).await,
                };

                match effect {
                    crate::engine::stage::loop_stage::LoopEntryEffect::Skipped(outcome) => {
                        Ok(StageOutcome::Routed(outcome))
                    },
                    crate::engine::stage::loop_stage::LoopEntryEffect::Entered(body_start) => {
                        cursor.node = body_start;
                        cursor.attempt = 1;
                        continue; // Skip the routing block below — we're in a fresh frame's body.
                    },
                }
            },
            NodeConfig::Subgraph(cfg) => {
                let completed_outcome = match surge_core::keys::OutcomeKey::try_from("completed") {
                    Ok(o) => o,
                    Err(e) => return failed(&params, format!("'completed' outcome: {e}")).await,
                };
                let return_to = match crate::engine::routing::edge_target_after_outcome_or_default(
                    &params.graph,
                    &cursor.node,
                    &completed_outcome,
                ) {
                    Ok(n) => n,
                    Err(e) => return failed(&params, format!("subgraph return_to: {e}")).await,
                };

                let effect = match crate::engine::stage::subgraph_stage::execute_subgraph_entry(
                    crate::engine::stage::subgraph_stage::SubgraphStageParams {
                        node: &cursor.node,
                        subgraph_config: cfg,
                        graph: &params.graph,
                        run_memory: &memory,
                        writer: &params.writer,
                        frames: &mut frames,
                        return_to,
                    },
                )
                .await
                {
                    Ok(e) => e,
                    Err(e) => return failed(&params, format!("subgraph entry: {e}")).await,
                };

                cursor.node = effect.inner_start;
                cursor.attempt = 1;
                continue; // Skip routing block — we're now in the inner subgraph's body.
            },
        };

        let outcome: OutcomeKey = match stage_result {
            Ok(StageOutcome::Routed(k)) => k,
            Ok(StageOutcome::Terminal(TerminalOutcome::Completed { node: n })) => {
                let outcome = RunOutcome::Completed { terminal: n };
                let _ = params
                    .event_tx
                    .send(EngineRunEvent::Terminal(outcome.clone()));
                return outcome;
            },
            Ok(StageOutcome::Terminal(TerminalOutcome::Failed { error })) => {
                let outcome = RunOutcome::Failed { error };
                let _ = params
                    .event_tx
                    .send(EngineRunEvent::Terminal(outcome.clone()));
                return outcome;
            },
            Ok(StageOutcome::Terminal(TerminalOutcome::Aborted { reason })) => {
                let outcome = RunOutcome::Aborted { reason };
                let _ = params
                    .event_tx
                    .send(EngineRunEvent::Terminal(outcome.clone()));
                return outcome;
            },
            Err(e) => {
                return failed(&params, format!("stage error at {}: {e}", cursor.node)).await;
            },
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
        let next = match crate::engine::routing::next_node_after_with_counters(
            &params.graph,
            &cursor.node,
            &outcome,
            &mut frames,
            &mut root_traversal_counts,
        ) {
            Ok(n) => n,
            Err(crate::engine::routing::RoutingError::ExceededTraversal {
                edge,
                action,
                count: _,
                max: _,
            }) => {
                use surge_core::edge::ExceededAction;
                match action {
                    ExceededAction::Escalate => {
                        // Synthesise a max_traversals_exceeded outcome and re-route.
                        let synthetic =
                            match surge_core::keys::OutcomeKey::try_from("max_traversals_exceeded")
                            {
                                Ok(o) => o,
                                Err(e) => {
                                    return failed(&params, format!("synthetic outcome: {e}"))
                                        .await;
                                },
                            };
                        match crate::engine::routing::next_node_after_with_counters(
                            &params.graph,
                            &cursor.node,
                            &synthetic,
                            &mut frames,
                            &mut root_traversal_counts,
                        ) {
                            Ok(n) => n,
                            Err(_) => {
                                return failed(
                                    &params,
                                    format!(
                                        "max_traversals exceeded on edge {edge} and no escalate route declared"
                                    ),
                                )
                                .await;
                            },
                        }
                    },
                    ExceededAction::Fail => {
                        return failed(
                            &params,
                            format!("max_traversals exceeded on edge {edge} (action: Fail)"),
                        )
                        .await;
                    },
                }
            },
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

        // Snapshot at stage boundary (per spec §2.6, §12).
        let next_cursor = Cursor {
            node: next.clone(),
            attempt: 1,
        };
        let current_seq = match params.writer.current_seq().await {
            Ok(s) => s,
            Err(e) => return failed(&params, format!("current_seq: {e}")).await,
        };
        let snapshot = crate::engine::snapshot::EngineSnapshot::new(
            &next_cursor,
            current_seq.as_u64(),
            current_seq.as_u64(),
        );
        let blob = match serde_json::to_vec(&snapshot) {
            Ok(b) => b,
            Err(e) => return failed(&params, format!("snapshot serialize: {e}")).await,
        };
        if let Err(e) = params.writer.write_graph_snapshot(current_seq, blob).await {
            return failed(&params, format!("write_graph_snapshot: {e}")).await;
        }

        cursor = next_cursor;
    }
}

enum StageOutcome {
    Routed(OutcomeKey),
    Terminal(TerminalOutcome),
}

fn lookup_in_active_frame<'a>(
    graph: &'a surge_core::graph::Graph,
    node_key: &surge_core::keys::NodeKey,
    frames: &[crate::engine::frames::Frame],
) -> Option<&'a surge_core::node::Node> {
    use crate::engine::frames::Frame;
    match frames.last() {
        None => graph.nodes.get(node_key),
        Some(Frame::Loop(lf)) => graph
            .subgraphs
            .get(&lf.config.body)
            .and_then(|sg| sg.nodes.get(node_key)),
        Some(Frame::Subgraph(sf)) => graph
            .subgraphs
            .get(&sf.inner_subgraph)
            .and_then(|sg| sg.nodes.get(node_key)),
    }
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
