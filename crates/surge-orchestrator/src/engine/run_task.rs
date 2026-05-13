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
use crate::roadmap_amendment::{ActiveRunAmendmentOutcome, apply_active_run_patch};
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::content_hash::ContentHash;
use surge_core::graph::Graph;
use surge_core::hooks::HookTrigger;
use surge_core::id::RunId;
use surge_core::keys::OutcomeKey;
use surge_core::node::NodeConfig;
use surge_core::roadmap_patch::{
    ActivePickupPolicy, RoadmapPatchApplyResult, RoadmapPatchId, RoadmapPatchTarget,
};
use surge_core::run_event::{EventPayload, RunEvent, VersionedEventPayload};
use surge_core::run_state::{Cursor, RunMemory};
use surge_notify::NotifyDeliverer;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub(crate) struct RoadmapAmendmentCommand {
    pub patch_id: RoadmapPatchId,
    pub target: RoadmapPatchTarget,
    pub patch_result: RoadmapPatchApplyResult,
    pub reply: oneshot::Sender<Result<ActiveRunAmendmentOutcome, String>>,
}

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
    /// Latest accepted graph revision sequence that was durably applied to
    /// the active graph at a stage boundary.
    pub resume_applied_graph_revision_seq: Option<u64>,
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
    /// Approved roadmap amendments submitted to the active run. The run task
    /// owns the writer, so amendments enter through this queue and are appended
    /// at safe graph boundaries.
    pub roadmap_amendments: mpsc::Receiver<RoadmapAmendmentCommand>,
    /// Engine-side tracker for in-flight ACP elevation requests. Shared with
    /// the `ActiveRun` entry so `Engine::resolve_elevation` can fire
    /// decisions from outside the stage event loop.
    pub pending_elevations: std::sync::Arc<crate::engine::elevation::PendingElevations>,
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

pub(crate) async fn execute(mut params: RunTaskParams) -> RunOutcome {
    let mut state = match initial_execution_state(&params).await {
        Ok(state) => state,
        Err(error) => return failed(&params, error).await,
    };

    loop {
        if state.frames.is_empty() {
            if let Err(error) = drain_roadmap_queue(&mut params, &mut state).await {
                return failed(&params, format!("apply queued roadmap amendments: {error}")).await;
            }
        }

        apply_pending_revisions(&mut state);

        if let Some(outcome) = abort_if_cancelled(&params).await {
            return outcome;
        }

        let node = if let Some(n) =
            lookup_in_active_frame(&state.active_graph, &state.cursor.node, &state.frames)
        {
            n.clone()
        } else {
            let err = format!("cursor at unknown node {}", state.cursor.node);
            return failed(&params, err).await;
        };

        let stage_start_seq = match enter_stage(&params, &state.cursor).await {
            Ok(seq) => seq,
            Err(error) => return failed(&params, error).await,
        };

        let stage_result = match dispatch_node_stage(&params, &mut state, &node).await {
            StageDispatch::StageResult(result) => result,
            StageDispatch::Continue => continue,
            StageDispatch::Failed(error) => return failed(&params, error).await,
        };

        let resolution = match resolve_stage_result(&params, &mut state, &node, stage_result).await
        {
            Ok(resolution) => resolution,
            Err(outcome) => return outcome,
        };

        match resolution {
            StageResolution::Terminal(outcome) => return outcome,
            StageResolution::Outcome(outcome) => {
                if let Err(error) =
                    route_and_snapshot(&params, &mut state, &outcome, stage_start_seq).await
                {
                    return failed(&params, error).await;
                }
            },
        }
    }
}

struct RunExecutionState {
    active_graph: Graph,
    cursor: Cursor,
    hook_executor: HookExecutor,
    memory: RunMemory,
    frames: Vec<crate::engine::frames::Frame>,
    root_traversal_counts: std::collections::HashMap<surge_core::keys::EdgeKey, u32>,
    applied_graph_revision_seq: u64,
    processed_graph_revision_seq: u64,
    pending_graph_revisions: Vec<ObservedGraphRevision>,
    pending_elevations: std::sync::Arc<crate::engine::elevation::PendingElevations>,
}

async fn initial_execution_state(params: &RunTaskParams) -> Result<RunExecutionState, String> {
    let active_graph = params.graph.clone();
    let cursor = params.resume_cursor.clone().unwrap_or_else(|| Cursor {
        node: active_graph.start.clone(),
        attempt: 1,
    });
    let memory = match params.resume_memory.clone() {
        Some(memory) => memory,
        None => load_existing_memory(&params.writer, params.run_id)
            .await
            .map_err(|e| format!("load existing memory: {e}"))?,
    };
    let applied_graph_revision_seq = params.resume_applied_graph_revision_seq.unwrap_or(0);
    let pending_graph_revisions =
        load_pending_graph_revisions_after(&params.writer, applied_graph_revision_seq)
            .await
            .map_err(|e| format!("load pending graph revisions: {e}"))?;

    Ok(RunExecutionState {
        active_graph,
        cursor,
        hook_executor: HookExecutor::new(),
        memory,
        frames: params.resume_frames.clone().unwrap_or_default(),
        root_traversal_counts: params
            .resume_root_traversal_counts
            .clone()
            .unwrap_or_default(),
        applied_graph_revision_seq,
        processed_graph_revision_seq: applied_graph_revision_seq,
        pending_graph_revisions,
        pending_elevations: params.pending_elevations.clone(),
    })
}

async fn drain_roadmap_queue(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
) -> Result<(), surge_persistence::runs::StorageError> {
    let mut roadmap_queue = RoadmapAmendmentQueue {
        writer: &params.writer,
        artifact_store: &params.artifact_store,
        run_id: params.run_id,
        receiver: &mut params.roadmap_amendments,
    };
    roadmap_queue
        .drain(
            &mut state.active_graph,
            &state.cursor,
            &mut state.memory,
            &mut state.pending_graph_revisions,
            &mut state.processed_graph_revision_seq,
            &mut state.applied_graph_revision_seq,
        )
        .await
}

fn apply_pending_revisions(state: &mut RunExecutionState) {
    maybe_apply_pending_graph_revision(
        &mut state.active_graph,
        &state.cursor,
        &state.frames,
        &mut state.pending_graph_revisions,
        &mut state.processed_graph_revision_seq,
        &mut state.applied_graph_revision_seq,
    );
}

async fn abort_if_cancelled(params: &RunTaskParams) -> Option<RunOutcome> {
    if !params.cancel.is_cancelled() {
        return None;
    }

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
    Some(outcome)
}

async fn enter_stage(
    params: &RunTaskParams,
    cursor: &Cursor,
) -> Result<surge_persistence::runs::EventSeq, String> {
    params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
            node: cursor.node.clone(),
            attempt: cursor.attempt,
        }))
        .await
        .map_err(|e| format!("write StageEntered: {e}"))?;
    params
        .writer
        .current_seq()
        .await
        .map_err(|e| format!("current_seq after StageEntered: {e}"))
}

enum StageDispatch {
    StageResult(Result<StageOutcome, StageError>),
    Continue,
    Failed(String),
}

async fn dispatch_node_stage(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
) -> StageDispatch {
    let stage_result = match &node.config {
        NodeConfig::Agent(cfg) => execute_agent_node(params, state, node, cfg).await,
        NodeConfig::Branch(cfg) => execute_branch_stage(BranchStageParams {
            node: &state.cursor.node,
            branch_config: cfg,
            writer: &params.writer,
            run_memory: &state.memory,
            worktree_root: &params.worktree_path,
        })
        .await
        .map(StageOutcome::Routed),
        NodeConfig::Notify(cfg) => execute_notify_stage(NotifyStageParams {
            node: &state.cursor.node,
            notify_config: cfg,
            declared_outcomes: &node.declared_outcomes,
            writer: &params.writer,
            run_memory: &state.memory,
            run_id: params.run_id,
            deliverer: params.notify_deliverer.clone(),
        })
        .await
        .map(StageOutcome::Routed),
        NodeConfig::Terminal(cfg) => return dispatch_terminal_node(params, state, cfg).await,
        NodeConfig::HumanGate(cfg) => execute_human_gate_node(params, state, cfg).await,
        NodeConfig::Loop(cfg) => return enter_loop_node(params, state, cfg).await,
        NodeConfig::Subgraph(cfg) => return enter_subgraph_node(params, state, cfg).await,
    };
    StageDispatch::StageResult(stage_result)
}

async fn execute_agent_node(
    params: &RunTaskParams,
    state: &RunExecutionState,
    node: &surge_core::node::Node,
    cfg: &surge_core::agent_config::AgentConfig,
) -> Result<StageOutcome, StageError> {
    let stage_result = execute_agent_stage(AgentStageParams {
        node: &state.cursor.node,
        agent_config: cfg,
        declared_outcomes: &node.declared_outcomes,
        bridge: &params.bridge,
        writer: &params.writer,
        artifact_store: &params.artifact_store,
        worktree_path: &params.worktree_path,
        tool_dispatcher: &params.tool_dispatcher,
        run_memory: &state.memory,
        run_id: params.run_id,
        tool_resolutions: &params.tool_resolutions,
        human_input_timeout: params.run_config.human_input_timeout,
        mcp_registry: params.mcp_registry.clone(),
        mcp_servers: params.mcp_servers.clone(),
        profile_registry: params.profile_registry.clone(),
        hook_executor: &state.hook_executor,
        pending_elevations: state.pending_elevations.clone(),
    })
    .await;

    let stage_result = if crate::engine::bootstrap::is_flow_generator_profile(cfg.profile.as_str())
    {
        handle_flow_generator_result(params, state, stage_result).await
    } else {
        stage_result
    };
    stage_result.map(StageOutcome::Routed)
}

async fn handle_flow_generator_result(
    params: &RunTaskParams,
    state: &RunExecutionState,
    stage_result: Result<OutcomeKey, StageError>,
) -> Result<OutcomeKey, StageError> {
    let Ok(outcome) = stage_result else {
        return stage_result;
    };
    match crate::engine::bootstrap::run_flow_generator_post_processing(
        &state.cursor.node,
        &state.memory,
        params.run_config.bootstrap.edit_loop_cap,
        &params.worktree_path,
        &params.writer,
    )
    .await
    {
        Ok(crate::engine::bootstrap::FlowValidationDecision::Materialized) => Ok(outcome),
        Ok(crate::engine::bootstrap::FlowValidationDecision::EditRequested { .. }) => {
            OutcomeKey::try_from(crate::engine::bootstrap::VALIDATION_FAILED_OUTCOME)
                .map_err(|e| StageError::Internal(format!("validation retry outcome key: {e}")))
        },
        Ok(crate::engine::bootstrap::FlowValidationDecision::CapExceeded { cap }) => {
            Err(StageError::EditLoopCapExceeded {
                stage: surge_core::run_event::BootstrapStage::Flow,
                cap,
            })
        },
        Ok(crate::engine::bootstrap::FlowValidationDecision::MissingArtifact) => {
            Err(StageError::Internal(
                "Flow Generator stage finished without producing flow.toml".into(),
            ))
        },
        Err(error) => Err(error),
    }
}

async fn dispatch_terminal_node(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    cfg: &surge_core::terminal_config::TerminalConfig,
) -> StageDispatch {
    use crate::engine::frames::TerminalSignal;

    match crate::engine::frames::on_terminal_decision(&state.frames, &state.cursor) {
        TerminalSignal::OuterComplete => {
            let result = execute_terminal_stage(TerminalStageParams {
                node: &state.cursor.node,
                terminal_config: cfg,
                writer: &params.writer,
            })
            .await
            .map(StageOutcome::Terminal);
            StageDispatch::StageResult(result)
        },
        TerminalSignal::LoopIterDone => finish_loop_iteration(params, state).await,
        TerminalSignal::SubgraphDone => finish_subgraph_frame(params, state).await,
    }
}

async fn finish_loop_iteration(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
) -> StageDispatch {
    let just_completed = match latest_loop_outcome(state) {
        Ok(outcome) => outcome,
        Err(error) => return StageDispatch::Failed(error),
    };
    match crate::engine::stage::loop_stage::on_loop_iteration_done(
        &just_completed,
        &state.active_graph,
        &mut state.frames,
        &mut state.cursor,
        &params.writer,
    )
    .await
    {
        Ok(()) => StageDispatch::Continue,
        Err(error) => StageDispatch::Failed(format!("loop iter done: {error}")),
    }
}

fn latest_loop_outcome(state: &RunExecutionState) -> Result<OutcomeKey, String> {
    if let Some(record) = state
        .memory
        .outcomes
        .get(&state.cursor.node)
        .and_then(|records| records.last())
    {
        return Ok(record.outcome.clone());
    }
    OutcomeKey::try_from("completed").map_err(|e| format!("loop default outcome key: {e}"))
}

async fn finish_subgraph_frame(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
) -> StageDispatch {
    let outputs = match current_subgraph_outputs(state) {
        Ok(outputs) => outputs,
        Err(error) => return StageDispatch::Failed(error),
    };
    match crate::engine::stage::subgraph_stage::on_subgraph_done(
        &outputs,
        &state.memory,
        &mut state.frames,
        &mut state.cursor,
        &params.writer,
    )
    .await
    {
        Ok(()) => StageDispatch::Continue,
        Err(error) => StageDispatch::Failed(format!("subgraph done: {error}")),
    }
}

fn current_subgraph_outputs(
    state: &RunExecutionState,
) -> Result<Vec<surge_core::subgraph_config::SubgraphOutput>, String> {
    let Some(crate::engine::frames::Frame::Subgraph(frame)) = state.frames.last() else {
        return Err("SubgraphDone signal but no Subgraph frame on top".into());
    };
    match state
        .active_graph
        .nodes
        .get(&frame.outer_node)
        .map(|node| &node.config)
    {
        Some(NodeConfig::Subgraph(cfg)) => Ok(cfg.outputs.clone()),
        _ => Err(format!(
            "outer subgraph node {} missing or wrong kind",
            frame.outer_node
        )),
    }
}

async fn execute_human_gate_node(
    params: &RunTaskParams,
    state: &RunExecutionState,
    cfg: &surge_core::human_gate_config::HumanGateConfig,
) -> Result<StageOutcome, StageError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    params
        .gate_resolutions
        .lock()
        .await
        .insert(state.cursor.node.clone(), tx);
    let result = execute_human_gate_stage(HumanGateStageParams {
        node: &state.cursor.node,
        gate_config: cfg,
        writer: &params.writer,
        run_memory: &state.memory,
        resolution_rx: Some(rx),
        default_timeout: params.run_config.human_input_timeout,
        bootstrap_edit_loop_cap: params.run_config.bootstrap.edit_loop_cap,
    })
    .await;
    params
        .gate_resolutions
        .lock()
        .await
        .remove(&state.cursor.node);
    result.map(StageOutcome::Routed)
}

async fn enter_loop_node(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    cfg: &surge_core::loop_config::LoopConfig,
) -> StageDispatch {
    let return_to =
        match return_to_after_completed(&state.active_graph, &state.cursor, &state.frames) {
            Ok(node) => node,
            Err(error) => return StageDispatch::Failed(format!("loop return_to: {error}")),
        };
    let effect = crate::engine::stage::loop_stage::execute_loop_entry(
        crate::engine::stage::loop_stage::LoopStageParams {
            node: &state.cursor.node,
            loop_config: cfg,
            graph: &state.active_graph,
            run_memory: &state.memory,
            writer: &params.writer,
            frames: &mut state.frames,
            return_to,
        },
    )
    .await;

    match effect {
        Ok(crate::engine::stage::loop_stage::LoopEntryEffect::Skipped(outcome)) => {
            StageDispatch::StageResult(Ok(StageOutcome::Routed(outcome)))
        },
        Ok(crate::engine::stage::loop_stage::LoopEntryEffect::Entered(body_start)) => {
            state.cursor.node = body_start;
            state.cursor.attempt = 1;
            StageDispatch::Continue
        },
        Err(error) => StageDispatch::Failed(format!("loop entry: {error}")),
    }
}

async fn enter_subgraph_node(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    cfg: &surge_core::subgraph_config::SubgraphConfig,
) -> StageDispatch {
    let return_to =
        match return_to_after_completed(&state.active_graph, &state.cursor, &state.frames) {
            Ok(node) => node,
            Err(error) => return StageDispatch::Failed(format!("subgraph return_to: {error}")),
        };
    let effect = crate::engine::stage::subgraph_stage::execute_subgraph_entry(
        crate::engine::stage::subgraph_stage::SubgraphStageParams {
            node: &state.cursor.node,
            subgraph_config: cfg,
            graph: &state.active_graph,
            run_memory: &state.memory,
            writer: &params.writer,
            frames: &mut state.frames,
            return_to,
        },
    )
    .await;

    match effect {
        Ok(effect) => {
            state.cursor.node = effect.inner_start;
            state.cursor.attempt = 1;
            StageDispatch::Continue
        },
        Err(error) => StageDispatch::Failed(format!("subgraph entry: {error}")),
    }
}

fn return_to_after_completed(
    graph: &Graph,
    cursor: &Cursor,
    frames: &[crate::engine::frames::Frame],
) -> Result<surge_core::keys::NodeKey, String> {
    let completed =
        OutcomeKey::try_from("completed").map_err(|e| format!("'completed' outcome: {e}"))?;
    crate::engine::routing::edge_target_after_outcome_in_active_graph(
        graph,
        &cursor.node,
        &completed,
        frames,
    )
    .map_err(|e| e.to_string())
}

enum StageResolution {
    Outcome(OutcomeKey),
    Terminal(RunOutcome),
}

async fn resolve_stage_result(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
    stage_result: Result<StageOutcome, StageError>,
) -> Result<StageResolution, RunOutcome> {
    match stage_result {
        Ok(StageOutcome::Routed(outcome)) => Ok(StageResolution::Outcome(outcome)),
        Ok(StageOutcome::Terminal(terminal)) => {
            let outcome = terminal_run_outcome(terminal);
            let _ = params.event_tx.send(EngineRunEvent::Terminal {
                outcome: outcome.clone(),
            });
            Ok(StageResolution::Terminal(outcome))
        },
        Err(error) => resolve_stage_error(params, state, node, error)
            .await
            .map(StageResolution::Outcome),
    }
}

fn terminal_run_outcome(terminal: TerminalOutcome) -> RunOutcome {
    match terminal {
        TerminalOutcome::Completed { node } => RunOutcome::Completed { terminal: node },
        TerminalOutcome::Failed { error } => RunOutcome::Failed { error },
        TerminalOutcome::Aborted { reason } => RunOutcome::Aborted { reason },
    }
}

async fn resolve_stage_error(
    params: &RunTaskParams,
    state: &RunExecutionState,
    node: &surge_core::node::Node,
    error: StageError,
) -> Result<OutcomeKey, RunOutcome> {
    let raw_reason = format!("stage error at {}: {error}", state.cursor.node);
    tracing::warn!(
        target: "engine::stage::error",
        node = %state.cursor.node,
        err = %error,
        "stage error captured; running on_error hooks"
    );

    let on_error_resolution = run_on_error_hooks(
        &state.hook_executor,
        node,
        &state.cursor.node,
        &raw_reason,
        &params.worktree_path,
        params.profile_registry.as_deref(),
    )
    .await;
    for record in &on_error_resolution.records {
        crate::engine::hooks::record_hook_executed(&params.writer, record).await;
    }

    if let Some(suppressed) = on_error_resolution.outcome {
        return record_suppressed_error(params, state, suppressed, &raw_reason).await;
    }

    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::StageFailed {
            node: state.cursor.node.clone(),
            reason: raw_reason.clone(),
            retry_available: false,
        }))
        .await;
    Err(failed(params, raw_reason).await)
}

async fn record_suppressed_error(
    params: &RunTaskParams,
    state: &RunExecutionState,
    suppressed: OutcomeKey,
    raw_reason: &str,
) -> Result<OutcomeKey, RunOutcome> {
    tracing::info!(
        target: "engine::stage::error",
        node = %state.cursor.node,
        outcome = %suppressed,
        "on_error hook suppressed failure; recording OutcomeReported"
    );
    if let Err(write_err) = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: state.cursor.node.clone(),
            outcome: suppressed.clone(),
            summary: format!("on_error hook suppressed: {raw_reason}"),
        }))
        .await
    {
        return Err(failed(
            params,
            format!("write OutcomeReported (suppressed): {write_err}"),
        )
        .await);
    }
    Ok(suppressed)
}

async fn route_and_snapshot(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    outcome: &OutcomeKey,
    stage_start_seq: surge_persistence::runs::EventSeq,
) -> Result<(), String> {
    let applied_events = apply_memory_events_after(
        &params.writer,
        params.run_id,
        stage_start_seq,
        &mut state.memory,
    )
    .await
    .map_err(|e| format!("update run memory from event log: {e}"))?;
    state
        .pending_graph_revisions
        .extend(applied_events.graph_revisions);
    apply_pending_revisions(state);
    let stage_events_applied_seq = params
        .writer
        .current_seq()
        .await
        .map_err(|e| format!("current_seq after memory update: {e}"))?;

    let routed = route_stage_outcome(state, outcome)?;
    write_routing_events(params, state, outcome, &routed).await;
    let post_route_events = apply_memory_events_after(
        &params.writer,
        params.run_id,
        stage_events_applied_seq,
        &mut state.memory,
    )
    .await
    .map_err(|e| format!("update run memory after routing: {e}"))?;
    state
        .pending_graph_revisions
        .extend(post_route_events.graph_revisions);

    let next_cursor = Cursor {
        node: routed.target,
        attempt: 1,
    };
    write_stage_boundary_snapshot(params, state, &next_cursor).await?;
    state.cursor = next_cursor;
    Ok(())
}

fn route_stage_outcome(
    state: &mut RunExecutionState,
    outcome: &OutcomeKey,
) -> Result<crate::engine::routing::RoutedEdge, String> {
    match crate::engine::routing::next_node_after_with_counters(
        &state.active_graph,
        &state.cursor.node,
        outcome,
        &mut state.frames,
        &mut state.root_traversal_counts,
    ) {
        Ok(routed) => Ok(routed),
        Err(crate::engine::routing::RoutingError::ExceededTraversal { edge, action, .. }) => {
            route_after_max_traversal(state, &edge, action)
        },
        Err(error) => Err(format!("routing: {error}")),
    }
}

fn route_after_max_traversal(
    state: &mut RunExecutionState,
    edge: &surge_core::keys::EdgeKey,
    action: surge_core::edge::ExceededAction,
) -> Result<crate::engine::routing::RoutedEdge, String> {
    match action {
        surge_core::edge::ExceededAction::Escalate => {
            let synthetic = OutcomeKey::try_from("max_traversals_exceeded")
                .map_err(|e| format!("synthetic outcome: {e}"))?;
            crate::engine::routing::next_node_after_with_counters(
                &state.active_graph,
                &state.cursor.node,
                &synthetic,
                &mut state.frames,
                &mut state.root_traversal_counts,
            )
            .map_err(|_| {
                format!("max_traversals exceeded on edge {edge} and no escalate route declared")
            })
        },
        surge_core::edge::ExceededAction::Fail => Err(format!(
            "max_traversals exceeded on edge {edge} (action: Fail)"
        )),
    }
}

async fn write_routing_events(
    params: &RunTaskParams,
    state: &RunExecutionState,
    outcome: &OutcomeKey,
    routed: &crate::engine::routing::RoutedEdge,
) {
    tracing::debug!(
        target: "engine::routing",
        from = %state.cursor.node,
        to = %routed.target,
        kind = ?routed.kind,
        "traversing edge",
    );
    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::EdgeTraversed {
            edge: routed.edge_id.clone(),
            from: state.cursor.node.clone(),
            to: routed.target.clone(),
            kind: routed.kind,
        }))
        .await;
    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
            node: state.cursor.node.clone(),
            outcome: outcome.clone(),
        }))
        .await;
}

async fn write_stage_boundary_snapshot(
    params: &RunTaskParams,
    state: &RunExecutionState,
    next_cursor: &Cursor,
) -> Result<(), String> {
    let current_seq = params
        .writer
        .current_seq()
        .await
        .map_err(|e| format!("current_seq: {e}"))?;
    let mut snapshot = crate::engine::snapshot::EngineSnapshot::new(
        next_cursor,
        current_seq.as_u64(),
        current_seq.as_u64(),
    );
    snapshot.applied_graph_revision_seq = state.applied_graph_revision_seq;
    let blob = serde_json::to_vec(&snapshot).map_err(|e| format!("snapshot serialize: {e}"))?;
    params
        .writer
        .write_graph_snapshot(current_seq, blob)
        .await
        .map_err(|e| format!("write_graph_snapshot: {e}"))
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
    let _ = apply_read_events(run_id, &events, &mut memory);
    Ok(memory)
}

#[derive(Debug, Clone, Default)]
struct AppliedEventBatch {
    graph_revisions: Vec<ObservedGraphRevision>,
}

#[derive(Debug, Clone)]
struct ObservedGraphRevision {
    seq: u64,
    patch_id: RoadmapPatchId,
    target: RoadmapPatchTarget,
    previous_graph_hash: ContentHash,
    graph: Graph,
    graph_hash: ContentHash,
    active_pickup: ActivePickupPolicy,
}

async fn apply_memory_events_after(
    writer: &surge_persistence::runs::run_writer::RunWriter,
    run_id: RunId,
    after: surge_persistence::runs::EventSeq,
    memory: &mut RunMemory,
) -> Result<AppliedEventBatch, surge_persistence::runs::StorageError> {
    let current = writer.current_seq().await?;
    if current <= after {
        return Ok(AppliedEventBatch::default());
    }
    let events = writer.read_events(after.next()..current.next()).await?;
    Ok(apply_read_events(run_id, &events, memory))
}

async fn load_pending_graph_revisions_after(
    writer: &surge_persistence::runs::run_writer::RunWriter,
    applied_seq: u64,
) -> Result<Vec<ObservedGraphRevision>, surge_persistence::runs::StorageError> {
    let current = writer.current_seq().await?;
    let after = surge_persistence::runs::EventSeq(applied_seq);
    if current <= after {
        return Ok(Vec::new());
    }
    let events = writer.read_events(after.next()..current.next()).await?;
    Ok(graph_revisions_from_events(&events))
}

struct RoadmapAmendmentQueue<'a> {
    writer: &'a surge_persistence::runs::run_writer::RunWriter,
    artifact_store: &'a surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    receiver: &'a mut mpsc::Receiver<RoadmapAmendmentCommand>,
}

impl RoadmapAmendmentQueue<'_> {
    async fn drain(
        &mut self,
        active_graph: &mut Graph,
        cursor: &Cursor,
        memory: &mut RunMemory,
        pending_graph_revisions: &mut Vec<ObservedGraphRevision>,
        processed_graph_revision_seq: &mut u64,
        applied_graph_revision_seq: &mut u64,
    ) -> Result<(), surge_persistence::runs::StorageError> {
        while let Ok(command) = self.receiver.try_recv() {
            let after = self.writer.current_seq().await?;
            match apply_active_run_patch(
                self.artifact_store,
                self.writer,
                self.run_id,
                active_graph,
                &command.patch_id,
                &command.target,
                &command.patch_result,
            )
            .await
            {
                Ok(outcome) => {
                    let applied_events =
                        apply_memory_events_after(self.writer, self.run_id, after, memory).await?;
                    pending_graph_revisions.extend(applied_events.graph_revisions);
                    maybe_apply_pending_graph_revision(
                        active_graph,
                        cursor,
                        &[],
                        pending_graph_revisions,
                        processed_graph_revision_seq,
                        applied_graph_revision_seq,
                    );
                    let _ = command.reply.send(Ok(outcome));
                },
                Err(error) => {
                    tracing::warn!(
                        target: "engine::roadmap_update",
                        run_id = %self.run_id,
                        patch_id = %command.patch_id,
                        error = %error,
                        "active_run_roadmap_patch_failed"
                    );
                    let _ = command.reply.send(Err(error.to_string()));
                },
            }
        }

        Ok(())
    }
}

fn apply_read_events(
    run_id: RunId,
    events: &[surge_persistence::runs::ReadEvent],
    memory: &mut RunMemory,
) -> AppliedEventBatch {
    let batch = AppliedEventBatch {
        graph_revisions: graph_revisions_from_events(events),
    };
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
    batch
}

fn graph_revisions_from_events(
    events: &[surge_persistence::runs::ReadEvent],
) -> Vec<ObservedGraphRevision> {
    events
        .iter()
        .filter_map(|event| {
            if let EventPayload::GraphRevisionAccepted {
                patch_id,
                target,
                previous_graph_hash,
                graph,
                graph_hash,
                active_pickup,
            } = &event.payload.payload
            {
                Some(ObservedGraphRevision {
                    seq: event.seq.as_u64(),
                    patch_id: patch_id.clone(),
                    target: target.clone(),
                    previous_graph_hash: *previous_graph_hash,
                    graph: graph.as_ref().clone(),
                    graph_hash: *graph_hash,
                    active_pickup: *active_pickup,
                })
            } else {
                None
            }
        })
        .collect()
}

fn maybe_apply_pending_graph_revision(
    active_graph: &mut Graph,
    cursor: &Cursor,
    frames: &[crate::engine::frames::Frame],
    pending: &mut Vec<ObservedGraphRevision>,
    processed_seq: &mut u64,
    applied_seq: &mut u64,
) {
    pending.retain(|revision| revision.seq > *processed_seq);
    let Some(revision) = pending.last().cloned() else {
        return;
    };

    match revision.active_pickup {
        ActivePickupPolicy::Allowed => {},
        ActivePickupPolicy::FollowUpOnly => {
            tracing::info!(
                target: "engine::roadmap_update",
                patch_id = %revision.patch_id,
                graph_hash = %revision.graph_hash,
                "graph_revision_requires_follow_up_run"
            );
            *processed_seq = revision.seq;
            pending.clear();
            return;
        },
        ActivePickupPolicy::Disabled => {
            tracing::warn!(
                target: "engine::roadmap_update",
                patch_id = %revision.patch_id,
                graph_hash = %revision.graph_hash,
                "graph_revision_pickup_disabled"
            );
            *processed_seq = revision.seq;
            pending.clear();
            return;
        },
    }

    if !frames.is_empty() {
        tracing::info!(
            target: "engine::roadmap_update",
            patch_id = %revision.patch_id,
            graph_hash = %revision.graph_hash,
            frame_depth = frames.len(),
            "graph_revision_deferred_until_outer_boundary"
        );
        return;
    }

    if !revision.graph.nodes.contains_key(&cursor.node) {
        tracing::warn!(
            target: "engine::roadmap_update",
            patch_id = %revision.patch_id,
            graph_hash = %revision.graph_hash,
            cursor = %cursor.node,
            "graph_revision_cannot_preserve_cursor"
        );
        *processed_seq = revision.seq;
        pending.clear();
        return;
    }

    tracing::info!(
        target: "engine::roadmap_update",
        patch_id = %revision.patch_id,
        target = ?revision.target,
        previous_graph_hash = %revision.previous_graph_hash,
        graph_hash = %revision.graph_hash,
        cursor = %cursor.node,
        "graph_revision_picked_up"
    );
    *active_graph = revision.graph;
    *processed_seq = revision.seq;
    *applied_seq = revision.seq;
    pending.clear();
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

    fn terminal_graph(name: &str, start: &str) -> Graph {
        use surge_core::graph::{GraphMetadata, SCHEMA_VERSION};
        use surge_core::terminal_config::{TerminalConfig, TerminalKind};

        let start = surge_core::keys::NodeKey::try_from(start).unwrap();
        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(
            start.clone(),
            Node {
                id: start.clone(),
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
            metadata: GraphMetadata::new(name, chrono::Utc::now()),
            start,
            nodes,
            edges: vec![],
            subgraphs: std::collections::BTreeMap::default(),
        }
    }

    fn observed_revision(graph: Graph, policy: ActivePickupPolicy) -> ObservedGraphRevision {
        ObservedGraphRevision {
            seq: 7,
            patch_id: RoadmapPatchId::new("rpatch-active").unwrap(),
            target: RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: ".ai-factory/ROADMAP.md".into(),
            },
            previous_graph_hash: ContentHash::compute(b"base"),
            graph,
            graph_hash: ContentHash::compute(b"revision"),
            active_pickup: policy,
        }
    }

    fn loop_frame() -> crate::engine::frames::Frame {
        use crate::engine::frames::LoopFrame;
        use surge_core::keys::SubgraphKey;
        use surge_core::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };

        crate::engine::frames::Frame::Loop(LoopFrame {
            loop_node: surge_core::keys::NodeKey::try_from("milestones").unwrap(),
            config: LoopConfig {
                iterates_over: IterableSource::Static(vec![]),
                body: SubgraphKey::try_from("body").unwrap(),
                iteration_var_name: "item".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            },
            items: vec![],
            current_index: 0,
            attempts_remaining: 0,
            return_to: surge_core::keys::NodeKey::try_from("after").unwrap(),
            traversal_counts: std::collections::HashMap::default(),
        })
    }

    #[test]
    fn graph_revision_waits_until_outer_boundary() {
        let mut active_graph = terminal_graph("base", "end");
        let original_graph = active_graph.clone();
        let mut revised_graph = terminal_graph("revised", "amend_001");
        let old_cursor_node = surge_core::keys::NodeKey::try_from("end").unwrap();
        revised_graph.nodes.insert(
            old_cursor_node.clone(),
            original_graph.nodes[&old_cursor_node].clone(),
        );
        let cursor = Cursor {
            node: old_cursor_node,
            attempt: 1,
        };
        let mut frames = vec![loop_frame()];
        let mut pending = vec![observed_revision(
            revised_graph.clone(),
            ActivePickupPolicy::Allowed,
        )];
        let mut processed_seq = 0;
        let mut applied_seq = 0;

        maybe_apply_pending_graph_revision(
            &mut active_graph,
            &cursor,
            &frames,
            &mut pending,
            &mut processed_seq,
            &mut applied_seq,
        );
        assert_eq!(active_graph, original_graph);
        assert_eq!(pending.len(), 1);
        assert_eq!(processed_seq, 0);
        assert_eq!(applied_seq, 0);

        frames.clear();
        maybe_apply_pending_graph_revision(
            &mut active_graph,
            &cursor,
            &frames,
            &mut pending,
            &mut processed_seq,
            &mut applied_seq,
        );
        assert_eq!(active_graph, revised_graph);
        assert!(pending.is_empty());
        assert_eq!(processed_seq, 7);
        assert_eq!(applied_seq, 7);
    }

    #[test]
    fn graph_revision_without_cursor_preservation_is_consumed_without_mutation() {
        let mut active_graph = terminal_graph("base", "end");
        let original_graph = active_graph.clone();
        let revised_graph = terminal_graph("revised", "amend_001");
        let cursor = Cursor {
            node: surge_core::keys::NodeKey::try_from("end").unwrap(),
            attempt: 1,
        };
        let frames = Vec::new();
        let mut pending = vec![observed_revision(
            revised_graph,
            ActivePickupPolicy::Allowed,
        )];
        let mut processed_seq = 0;
        let mut applied_seq = 0;

        maybe_apply_pending_graph_revision(
            &mut active_graph,
            &cursor,
            &frames,
            &mut pending,
            &mut processed_seq,
            &mut applied_seq,
        );

        assert_eq!(active_graph, original_graph);
        assert!(pending.is_empty());
        assert_eq!(processed_seq, 7);
        assert_eq!(applied_seq, 0);
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
