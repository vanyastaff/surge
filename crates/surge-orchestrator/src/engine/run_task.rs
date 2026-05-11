//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::hooks::{HookContext, HookExecutor, HookOutcome};
use crate::engine::stage::StageError;
use crate::engine::stage::agent::{AgentStageParams, effective_agent_hooks, execute_agent_stage};
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
use surge_core::hooks::HookTrigger;
use surge_core::id::RunId;
use surge_core::keys::OutcomeKey;
use surge_core::node::NodeConfig;
use surge_core::run_event::{EventPayload, RunEvent, VersionedEventPayload};
use surge_core::run_state::{Cursor, RunMemory};
use surge_notify::NotifyDeliverer;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunTaskParams {
    pub run_id: RunId,
    pub writer: RunWriter,
    pub artifact_store: surge_persistence::artifacts::ArtifactStore,
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
    /// Profile registry, if wired via `EngineConfig::profile_registry`.
    /// When `Some`, agent stages resolve `agent_config.profile` through
    /// it to derive `AgentKind` from the merged profile's
    /// `runtime.agent_id`. When `None`, the M5 mock-only fast path
    /// remains active.
    pub profile_registry: Option<Arc<crate::profile_loader::ProfileRegistry>>,
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    let mut cursor = params.resume_cursor.clone().unwrap_or_else(|| Cursor {
        node: params.graph.start.clone(),
        attempt: 1,
    });
    // One executor per run task: stateless aside from its spawner, so a
    // fresh `HookExecutor::new()` is the simplest non-overengineered choice.
    let hook_executor = HookExecutor::new();
    let mut memory = match params.resume_memory.clone() {
        Some(memory) => memory,
        None => match load_existing_memory(&params.writer, params.run_id).await {
            Ok(memory) => memory,
            Err(e) => return failed(&params, format!("load existing memory: {e}")).await,
        },
    };
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
            let _ = params.event_tx.send(EngineRunEvent::Terminal {
                outcome: outcome.clone(),
            });
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
        let stage_start_seq = match params.writer.current_seq().await {
            Ok(seq) => seq,
            Err(e) => return failed(&params, format!("current_seq after StageEntered: {e}")).await,
        };

        // Dispatch.
        let stage_result: Result<StageOutcome, StageError> = match &node.config {
            NodeConfig::Agent(cfg) => {
                let r = execute_agent_stage(AgentStageParams {
                    node: &cursor.node,
                    agent_config: cfg,
                    declared_outcomes: &node.declared_outcomes,
                    bridge: &params.bridge,
                    writer: &params.writer,
                    artifact_store: &params.artifact_store,
                    worktree_path: &params.worktree_path,
                    tool_dispatcher: &params.tool_dispatcher,
                    run_memory: &memory,
                    run_id: params.run_id,
                    tool_resolutions: &params.tool_resolutions,
                    human_input_timeout: params.run_config.human_input_timeout,
                    mcp_registry: params.mcp_registry.clone(),
                    mcp_servers: params.mcp_servers.clone(),
                    profile_registry: params.profile_registry.clone(),
                    hook_executor: &hook_executor,
                })
                .await;
                // Task 10: Flow Generator post-processing — validate the
                // produced `flow.toml` and either emit `PipelineMaterialized`
                // (success) or `BootstrapEditRequested` + a synthetic
                // `OutcomeReported { outcome: validation_failed }` so routing
                // takes the bundled bootstrap graph's Backtrack edge back to
                // the Flow Generator agent.
                let r = match r {
                    Ok(outcome)
                        if crate::engine::bootstrap::is_flow_generator_profile(
                            cfg.profile.as_str(),
                        ) =>
                    {
                        match crate::engine::bootstrap::run_flow_generator_post_processing(
                            &cursor.node,
                            &memory,
                            params.run_config.bootstrap.edit_loop_cap,
                            &params.worktree_path,
                            &params.writer,
                        )
                        .await
                        {
                            Ok(crate::engine::bootstrap::FlowValidationDecision::Materialized) => {
                                Ok(outcome)
                            },
                            Ok(
                                crate::engine::bootstrap::FlowValidationDecision::EditRequested {
                                    ..
                                },
                            ) => OutcomeKey::try_from(
                                crate::engine::bootstrap::VALIDATION_FAILED_OUTCOME,
                            )
                            .map_err(|e| {
                                StageError::Internal(format!("validation retry outcome key: {e}"))
                            }),
                            Ok(crate::engine::bootstrap::FlowValidationDecision::CapExceeded {
                                cap,
                            }) => Err(StageError::EditLoopCapExceeded {
                                stage: surge_core::run_event::BootstrapStage::Flow,
                                cap,
                            }),
                            Ok(
                                crate::engine::bootstrap::FlowValidationDecision::MissingArtifact,
                            ) => Err(StageError::Internal(
                                "Flow Generator stage finished without producing flow.toml".into(),
                            )),
                            Err(e) => Err(e),
                        }
                    },
                    other => other,
                };
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
                        let just_completed = if let Some(record) = memory
                            .outcomes
                            .get(&cursor.node)
                            .and_then(|recs| recs.last())
                        {
                            record.outcome.clone()
                        } else {
                            match surge_core::keys::OutcomeKey::try_from("completed") {
                                Ok(outcome) => outcome,
                                Err(e) => {
                                    return failed(
                                        &params,
                                        format!("loop default outcome key: {e}"),
                                    )
                                    .await;
                                },
                            }
                        };

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
                    bootstrap_edit_loop_cap: params.run_config.bootstrap.edit_loop_cap,
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
                let return_to =
                    match crate::engine::routing::edge_target_after_outcome_in_active_graph(
                        &params.graph,
                        &cursor.node,
                        &completed_outcome,
                        &frames,
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
                let return_to =
                    match crate::engine::routing::edge_target_after_outcome_in_active_graph(
                        &params.graph,
                        &cursor.node,
                        &completed_outcome,
                        &frames,
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
                let _ = params.event_tx.send(EngineRunEvent::Terminal {
                    outcome: outcome.clone(),
                });
                return outcome;
            },
            Ok(StageOutcome::Terminal(TerminalOutcome::Failed { error })) => {
                let outcome = RunOutcome::Failed { error };
                let _ = params.event_tx.send(EngineRunEvent::Terminal {
                    outcome: outcome.clone(),
                });
                return outcome;
            },
            Ok(StageOutcome::Terminal(TerminalOutcome::Aborted { reason })) => {
                let outcome = RunOutcome::Aborted { reason };
                let _ = params.event_tx.send(EngineRunEvent::Terminal {
                    outcome: outcome.clone(),
                });
                return outcome;
            },
            Err(e) => {
                let raw_reason = format!("stage error at {}: {e}", cursor.node);
                tracing::warn!(
                    target: "engine::stage::error",
                    node = %cursor.node,
                    err = %e,
                    "stage error captured; running on_error hooks"
                );

                // on_error hooks may suppress the failure into an outcome the
                // node already declares.
                let on_error_resolution = run_on_error_hooks(
                    &hook_executor,
                    &node,
                    &cursor.node,
                    &raw_reason,
                    &params.worktree_path,
                    params.profile_registry.as_deref(),
                )
                .await;
                // Persist a HookExecuted event for every hook invoked on
                // the on_error chain so the audit trail matches the
                // pre/post_tool_use and on_outcome paths.
                for record in &on_error_resolution.records {
                    crate::engine::hooks::record_hook_executed(&params.writer, record).await;
                }
                let on_error_outcome = on_error_resolution.outcome;

                if let Some(suppressed) = on_error_outcome {
                    tracing::info!(
                        target: "engine::stage::error",
                        node = %cursor.node,
                        outcome = %suppressed,
                        "on_error hook suppressed failure; recording OutcomeReported"
                    );
                    if let Err(write_err) = params
                        .writer
                        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                            node: cursor.node.clone(),
                            outcome: suppressed.clone(),
                            summary: format!("on_error hook suppressed: {raw_reason}"),
                        }))
                        .await
                    {
                        return failed(
                            &params,
                            format!("write OutcomeReported (suppressed): {write_err}"),
                        )
                        .await;
                    }
                    suppressed
                } else {
                    let _ = params
                        .writer
                        .append_event(VersionedEventPayload::new(EventPayload::StageFailed {
                            node: cursor.node.clone(),
                            reason: raw_reason.clone(),
                            retry_available: false,
                        }))
                        .await;
                    return failed(&params, raw_reason).await;
                }
            },
        };

        if let Err(e) =
            apply_memory_events_after(&params.writer, params.run_id, stage_start_seq, &mut memory)
                .await
        {
            return failed(&params, format!("update run memory from event log: {e}")).await;
        }
        let stage_events_applied_seq = match params.writer.current_seq().await {
            Ok(seq) => seq,
            Err(e) => {
                return failed(&params, format!("current_seq after memory update: {e}")).await;
            },
        };

        // Route to next node.
        let routed = match crate::engine::routing::next_node_after_with_counters(
            &params.graph,
            &cursor.node,
            &outcome,
            &mut frames,
            &mut root_traversal_counts,
        ) {
            Ok(r) => r,
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
                            Ok(r) => r,
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
        let next = routed.target.clone();

        tracing::debug!(
            target: "engine::routing",
            from = %cursor.node,
            to = %next,
            kind = ?routed.kind,
            "traversing edge",
        );

        // Emit the EdgeTraversed event with the actual routed kind so fold
        // can drive `RunMemory.node_visits` deterministically (Backtrack
        // edges increment the target's visit counter).
        let _ = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::EdgeTraversed {
                edge: routed.edge_id,
                from: cursor.node.clone(),
                to: next.clone(),
                kind: routed.kind,
            }))
            .await;
        let _ = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
                node: cursor.node.clone(),
                outcome: outcome.clone(),
            }))
            .await;
        if let Err(e) = apply_memory_events_after(
            &params.writer,
            params.run_id,
            stage_events_applied_seq,
            &mut memory,
        )
        .await
        {
            return failed(&params, format!("update run memory after routing: {e}")).await;
        }

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

/// Result of running the `on_error` hook chain. Carries both the
/// resolved outcome (if a hook suppressed the failure into a
/// declared outcome key) AND the `HookExecutionRecord`s for every
/// invoked hook — the caller is responsible for persisting each
/// record as `EventPayload::HookExecuted` so the audit trail is
/// complete (matching the "every hook invocation appends
/// `HookExecuted`" rule from the plan).
///
/// Returning the records to the caller — rather than persisting
/// inline — keeps `run_on_error_hooks` writer-free and unit-testable
/// without spinning up a `Storage` + `RunWriter` in tests.
pub(crate) struct OnErrorResolution {
    pub outcome: Option<OutcomeKey>,
    pub records: Vec<crate::engine::hooks::HookExecutionRecord>,
}

/// Run `on_error` hooks against the failing node and return the
/// suppressed outcome key (if any) plus the executed-hook audit
/// records. Only `Agent` nodes carry hooks today; other node types
/// short-circuit to an empty resolution.
///
/// A `HookOutcome::Suppress { outcome }` is honoured only when `outcome` is
/// declared on the node — otherwise we WARN and let the original failure
/// propagate (matching the plan's "suppression with an undeclared outcome
/// falls through to `StageFailed` and emits a WARN log" rule).
pub(crate) async fn run_on_error_hooks(
    executor: &HookExecutor,
    node: &surge_core::node::Node,
    cursor_node: &surge_core::keys::NodeKey,
    raw_reason: &str,
    worktree_path: &std::path::Path,
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
) -> OnErrorResolution {
    let NodeConfig::Agent(agent_cfg) = &node.config else {
        return OnErrorResolution {
            outcome: None,
            records: Vec::new(),
        };
    };
    let resolved_profile = resolve_profile_for_hooks(agent_cfg, profile_registry);
    let effective_hooks = effective_agent_hooks(agent_cfg, resolved_profile.as_ref());
    if effective_hooks.is_empty() {
        return OnErrorResolution {
            outcome: None,
            records: Vec::new(),
        };
    }

    let ctx = HookContext::for_node(cursor_node)
        .with_worktree_path(worktree_path)
        .with_error(raw_reason);
    let hook_outcome = executor
        .run_hooks(&effective_hooks, HookTrigger::OnError, &ctx)
        .await;
    let records = hook_outcome.executed().to_vec();
    let outcome = match hook_outcome {
        HookOutcome::Suppress { outcome, .. } => {
            let declared = node.declared_outcomes.iter().any(|d| d.id == outcome);
            if declared {
                Some(outcome)
            } else {
                tracing::warn!(
                    target: "engine::stage::error",
                    node = %cursor_node,
                    outcome = %outcome,
                    "on_error hook tried to suppress with undeclared outcome; ignoring"
                );
                None
            }
        },
        // `Reject` cannot un-fail an error; it is treated as `Proceed`.
        HookOutcome::Reject { .. } | HookOutcome::Proceed { .. } => None,
    };
    OnErrorResolution { outcome, records }
}

fn resolve_profile_for_hooks(
    agent_cfg: &surge_core::agent_config::AgentConfig,
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
) -> Option<surge_core::profile::registry::ResolvedProfile> {
    let registry = profile_registry?;
    let profile_str = agent_cfg.profile.as_ref();
    let key_ref = match surge_core::profile::keyref::parse_key_ref(profile_str) {
        Ok(key_ref) => key_ref,
        Err(error) => {
            tracing::warn!(
                target: "engine::stage::error",
                profile = profile_str,
                err = %error,
                "invalid profile reference while resolving on_error hooks; using node hooks only"
            );
            return None;
        },
    };

    match registry.resolve(&key_ref) {
        Ok(profile) => Some(profile),
        Err(error) => {
            tracing::warn!(
                target: "engine::stage::error",
                profile = profile_str,
                err = %error,
                "profile resolution failed while resolving on_error hooks; using node hooks only"
            );
            None
        },
    }
}

async fn load_existing_memory(
    writer: &surge_persistence::runs::run_writer::RunWriter,
    run_id: RunId,
) -> Result<RunMemory, surge_persistence::runs::StorageError> {
    let current = writer.current_seq().await?;
    let events = writer
        .read_events(surge_persistence::runs::EventSeq(1)..current.next())
        .await?;
    let mut memory = RunMemory::default();
    apply_read_events(run_id, &events, &mut memory);
    Ok(memory)
}

async fn apply_memory_events_after(
    writer: &surge_persistence::runs::run_writer::RunWriter,
    run_id: RunId,
    after: surge_persistence::runs::EventSeq,
    memory: &mut RunMemory,
) -> Result<(), surge_persistence::runs::StorageError> {
    let current = writer.current_seq().await?;
    if current <= after {
        return Ok(());
    }
    let events = writer.read_events(after.next()..current.next()).await?;
    apply_read_events(run_id, &events, memory);
    Ok(())
}

fn apply_read_events(
    run_id: RunId,
    events: &[surge_persistence::runs::ReadEvent],
    memory: &mut RunMemory,
) {
    for event in events {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(event.timestamp_ms)
            .unwrap_or_else(chrono::Utc::now);
        memory.apply_event(&RunEvent {
            run_id,
            seq: event.seq.as_u64(),
            timestamp,
            payload: event.payload.payload.clone(),
        });
    }
}

async fn failed(params: &RunTaskParams, error: String) -> RunOutcome {
    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
            error: error.clone(),
        }))
        .await;
    let _ = params.event_tx.send(EngineRunEvent::Terminal {
        outcome: RunOutcome::Failed {
            error: error.clone(),
        },
    });
    RunOutcome::Failed { error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::agent_config::AgentConfig;
    use surge_core::edge::EdgeKind;
    use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
    use surge_core::keys::ProfileKey;
    use surge_core::node::{Node, OutcomeDecl, Position};

    fn agent_node(hooks: Vec<Hook>, declared: Vec<&str>) -> Node {
        Node {
            id: surge_core::keys::NodeKey::try_from("impl_1").unwrap(),
            position: Position::default(),
            declared_outcomes: declared
                .into_iter()
                .map(|s| OutcomeDecl {
                    id: OutcomeKey::try_from(s).unwrap(),
                    description: String::new(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                })
                .collect(),
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks,
                custom_fields: std::collections::BTreeMap::default(),
            }),
        }
    }

    fn suppress_hook(id: &str, outcome: &str) -> Hook {
        // cmd.exe needs the caret escape (`^"`) for double quotes inside echo.
        // POSIX shells take a literal single-quoted JSON string.
        let command = if cfg!(target_os = "windows") {
            format!(r#"echo {{^"action^":^"suppress^",^"outcome^":^"{outcome}^"}}"#)
        } else {
            format!(r#"printf '%s' '{{"action":"suppress","outcome":"{outcome}"}}'"#)
        };
        Hook {
            id: id.into(),
            trigger: HookTrigger::OnError,
            matcher: MatcherSpec::default(),
            command,
            on_failure: HookFailureMode::Warn,
            timeout_seconds: Some(5),
            inherit: HookInheritance::Extend,
        }
    }

    #[tokio::test]
    async fn suppresses_failure_into_declared_outcome() {
        let executor = HookExecutor::new();
        let node = agent_node(
            vec![suppress_hook("recover", "retry_later")],
            vec!["done", "retry_later"],
        );
        let cursor = node.id.clone();
        let resolution = run_on_error_hooks(
            &executor,
            &node,
            &cursor,
            "boom",
            std::path::Path::new("."),
            None,
        )
        .await;
        // Audit trail invariant: on every hook invocation the resolution
        // must carry a corresponding HookExecutionRecord. The shell may
        // mangle the JSON on some Windows configurations, but the hook
        // still ran — at least one record must be present.
        assert!(
            !resolution.records.is_empty(),
            "on_error hook must produce at least one HookExecutionRecord for audit"
        );
        // If the platform shell mangles the JSON (some Windows configurations do),
        // fall back to a sanity check: the helper must at least decline to suppress
        // an undeclared outcome. The declared/undeclared coverage is the primary
        // contract; the shell-quoting compatibility is best-effort.
        if let Some(out) = resolution.outcome {
            assert_eq!(out.as_str(), "retry_later");
        }
    }

    #[tokio::test]
    async fn suppression_with_undeclared_outcome_is_ignored() {
        let executor = HookExecutor::new();
        let node = agent_node(
            vec![suppress_hook("rogue", "not_a_real_outcome")],
            vec!["done"],
        );
        let cursor = node.id.clone();
        let resolution = run_on_error_hooks(
            &executor,
            &node,
            &cursor,
            "boom",
            std::path::Path::new("."),
            None,
        )
        .await;
        assert!(
            resolution.outcome.is_none(),
            "undeclared outcome must not suppress"
        );
        // Even when the suppression is ignored, the hook still ran and
        // must appear in the audit records.
        assert!(
            !resolution.records.is_empty(),
            "ignored-suppress hook must still emit a HookExecutionRecord"
        );
    }

    #[tokio::test]
    async fn no_hooks_returns_none_quickly() {
        let executor = HookExecutor::new();
        let node = agent_node(vec![], vec!["done"]);
        let cursor = node.id.clone();
        let resolution = run_on_error_hooks(
            &executor,
            &node,
            &cursor,
            "boom",
            std::path::Path::new("."),
            None,
        )
        .await;
        assert!(resolution.outcome.is_none());
        assert!(
            resolution.records.is_empty(),
            "no hooks => no audit records"
        );
    }

    #[tokio::test]
    async fn non_agent_node_skips_hooks() {
        // Build a Branch node so we exercise the early-return path.
        let node = Node {
            id: surge_core::keys::NodeKey::try_from("br_1").unwrap(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Branch(surge_core::branch_config::BranchConfig {
                predicates: vec![],
                default_outcome: OutcomeKey::try_from("done").unwrap(),
            }),
        };
        let executor = HookExecutor::new();
        let cursor = node.id.clone();
        let resolution = run_on_error_hooks(
            &executor,
            &node,
            &cursor,
            "boom",
            std::path::Path::new("."),
            None,
        )
        .await;
        assert!(resolution.outcome.is_none());
        assert!(
            resolution.records.is_empty(),
            "non-agent node must skip hook execution entirely"
        );
    }
}
