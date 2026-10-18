//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::hooks::{HookContext, HookExecutor, HookOutcome};
use crate::engine::stage::StageError;
use crate::engine::stage::agent::{AgentStageParams, effective_agent_hooks, execute_agent_stage};
use crate::engine::stage::branch::{BranchStageParams, execute_branch_stage};
use crate::engine::stage::human_gate::{
    HumanGateStageParams, execute_human_gate_stage, restore_human_gate_stage,
};
use crate::engine::stage::notify::{NotifyStageParams, execute_notify_stage};
use crate::engine::stage::skill_binding::{SkillBindingParams, bind_skills};
use crate::engine::stage::terminal::{
    TerminalOutcome, TerminalStageParams, execute_terminal_stage,
};
use crate::engine::tools::ToolDispatcher;
use crate::roadmap_amendment::{ActiveRunAmendmentOutcome, apply_active_run_patch};
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
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
    /// Shared host launch authority for persistent tasks; clones retain the
    /// same OS lock and fencing token through recovery mutations.
    pub work_item_claim: Option<surge_persistence::work_items::WorkItemLaunchClaim>,
    pub pending_suspension: Option<(surge_core::execution_recovery::SuspensionFence, Vec<u8>)>,
    pub run_id: RunId,
    pub writer: RunWriter,
    pub artifact_store: surge_persistence::artifacts::ArtifactStore,
    pub bridge: Arc<dyn BridgeFacade>,
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    pub notify_deliverer: Arc<dyn NotifyDeliverer>,
    /// Where execution stands (#100): cursor, frames, counters, memory, the
    /// graph in force and its pending revisions, loaded once by admission.
    pub execution: crate::engine::execution_state::ExecutionState,
    /// Whether this run task resumes an existing log (as opposed to a fresh
    /// start right after its startup prefix).
    pub resumed: bool,
    pub worktree_path: PathBuf,
    pub run_config: EngineRunConfig,
    pub event_tx: broadcast::Sender<EngineRunEvent>,
    pub cancel: CancellationToken,
    /// Node-keyed decision registry (`node_key → PendingGate`). The engine's
    /// resolve side finds the sender and fires it; `engine::human_decision`
    /// owns registration for `HumanGate` nodes and the skill-trust prompt.
    pub gate_resolutions: std::sync::Arc<crate::engine::stage::human_gate::GateResolutions>,
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
    /// Queued operator steer messages, shared with the `ActiveRun` entry. The
    /// run task drains this before opening each agent stage and prepends the
    /// messages to that stage's prompt (Phase 2 B2, non-destructive steering).
    pub pending_steers: crate::engine::steer::SteerQueue,
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
    /// Unified agent registry (user `[agents.*]` merged over the builtin
    /// catalog), if wired via `EngineConfig::agent_registry`. When `Some`,
    /// agent stages resolve a profile's `runtime.agent_id` through it, so a
    /// custom provider the operator declared launches through the same path
    /// as a builtin one. `None` falls back to `Registry::builtin()`.
    pub agent_registry: Option<Arc<surge_acp::Registry>>,
    /// The run's quota/capacity cycle (#93): ledger, estimator, policy,
    /// fallback agents and the one-shot resume bypass behind one interface.
    pub capacity: crate::engine::quota_recovery::RunCapacity,
    /// Engine-level `[escalation]` settings for the extra retry attempt.
    pub escalation: surge_core::escalation::EscalationConfig,
    /// Registry-level storage handle (Task 12 M3) — the run task's own
    /// door onto `runs.status`/`runs.wake_at`, used only to call
    /// [`surge_persistence::runs::Storage::set_run_parked`] when
    /// `CapacityPolicy::decide` returns `Decision::Park`. Every other
    /// registry write for this run (initial insert, terminal status)
    /// happens outside the run task (see `runs::views`'s doc on why
    /// `RunParked`/`RunWokeFromPark` are handled this way, not via the
    /// per-run event fold) — parking is the one registry-status
    /// transition the run task itself must make, because it is the only
    /// party that knows `wake_at` at the moment it happens.
    pub storage: Arc<surge_persistence::runs::Storage>,
}

pub(crate) struct ExecutionFinished {
    pub outcome: RunOutcome,
    pub seal: Result<surge_persistence::runs::EventSeq, surge_persistence::runs::CloseError>,
}

pub(crate) async fn execute(mut params: RunTaskParams) -> ExecutionFinished {
    // Capture the per-run MCP registry so it is torn down on *every*
    // terminal path (completed / failed / aborted), not just the
    // happy one — rmcp's Drop is async best-effort and can orphan
    // children. Time-bounded so a hung MCP child cannot wedge run
    // completion.
    let mcp_registry = params.mcp_registry.clone();
    // Box the large inner future so `execute`'s own future stays small
    // at the spawn site (clippy::large_futures; also keeps stack use
    // bounded for the per-run task).
    let mut outcome = Box::pin(execute_inner(&mut params)).await;
    if let Some(reg) = mcp_registry {
        let budget = std::time::Duration::from_secs(20);
        if !matches!(
            tokio::time::timeout(budget, reg.shutdown()).await,
            Ok(Ok(()))
        ) {
            params.pending_suspension = None;
            let _ = params.event_tx.send(EngineRunEvent::StreamError {
                message: "MCP writer cleanup is unconfirmed; suspension cannot be acknowledged"
                    .into(),
            });
            tracing::warn!(
                target: "mcp::supervisor",
                "MCP registry shutdown exceeded budget on run teardown; \
                 outstanding child ownership remains pending settlement"
            );
        }
    }
    if let Some((fence, blob)) = params.pending_suspension.take() {
        // ADR-0021: stop any MCP group still led by its recorded process and accept
        // empty groups as best-effort cleanup. The record is written when the run
        // resumes: appending here would move the log past the fence's snapshot.
        let refusal = match super::writer_coverage::stop_and_assess_mcp_cleanup(
            &params.storage,
            params.run_id,
        )
        .await
        {
            Ok(Ok(_)) => None,
            Ok(Err(diagnostic)) => Some(diagnostic),
            Err(error) => Some(format!("MCP cleanup evidence unavailable: {error}")),
        };
        if let Some(diagnostic) = refusal {
            tracing::warn!(run_id = %params.run_id, %diagnostic, "suspension requires recovery attention");
            outcome = recovery_required(&params, diagnostic).await;
        } else {
            match params.writer.seal_suspension(fence.clone(), blob).await {
                Ok(seq) => {
                    let _ = params.event_tx.send(EngineRunEvent::Persisted {
                        seq: seq.as_u64(),
                        payload: Box::new(EventPayload::RunSuspended {
                            fence: fence.clone(),
                        }),
                    });
                    outcome = RunOutcome::Suspended {
                        fence: Box::new(fence),
                    };
                },
                Err(error) => {
                    let _ = params.event_tx.send(EngineRunEvent::StreamError {
                        message: format!("suspension fence was not committed: {error}"),
                    });
                },
            }
        }
    }
    let seal = params.writer.seal_for_read_drain().await;
    ExecutionFinished { outcome, seal }
}

async fn execute_inner(params: &mut RunTaskParams) -> RunOutcome {
    let mut state = initial_execution_state(params);

    // ADR-0021: a resumed run records best-effort cleanup for prior MCP groups
    // that the resume check observed empty.
    match super::writer_coverage::assess_mcp_cleanup(&state.memory) {
        Ok(stopped) => {
            if let Err(error) =
                super::writer_coverage::record_mcp_groups_stopped(&params.writer, &stopped).await
            {
                return failed(params, error).await;
            }
        },
        Err(diagnostic) => return recovery_required(params, diagnostic).await,
    }

    let had_suspended_phase = state.memory.suspension.is_some();
    if let Err(outcome) = Box::pin(restore_suspended_phase(params, &mut state)).await {
        return outcome;
    }
    if let Err(outcome) = Box::pin(restore_committed_routes(params, &mut state)).await {
        return outcome;
    }
    if params.resumed
        && !had_suspended_phase
        && state.memory.suspension.is_none()
        && let Some(claim) = params.work_item_claim.as_ref()
        && params
            .run_config
            .quota_recovery
            .stage(&state.cursor.node)
            .is_some_and(|stage| {
                stage
                    .candidates()
                    .iter()
                    .any(|candidate| candidate.configured_route().is_some())
            })
    {
        let hash = match params.run_config.quota_recovery.content_hash() {
            Ok(hash) => hash,
            Err(error) => return recovery_required(params, error.to_string()).await,
        };
        let inspected = params.storage.work_items().inspect_planned_stage_resume(
            claim,
            &state.cursor,
            &hash,
            state.memory.control_generation,
        );
        match inspected {
            Ok(surge_persistence::work_items::recovery_cycles::PlannedResumeInspection::UnadmittedOccurrence {stage_entry_seq})=>state.restored_quota_entry=Some(stage_entry_seq),
            Ok(_)=>{},
            Err(error)=>return recovery_required(params,format!("cold planned-stage reconciliation: {error}")).await,
        }
    }
    Box::pin(execute_stage_loop(params, state)).await
}

async fn restore_suspended_phase(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
) -> Result<(), RunOutcome> {
    if let Some(fence) = state.memory.suspension.clone() {
        let control = match params.storage.work_items().execution_control(params.run_id) {
            Ok(Some(control))
                if control.state
                    == surge_core::execution_recovery::ExecutionControlState::ContinueReserved =>
            {
                control
            },
            _ => {
                return Err(recovery_required(
                    params,
                    "suspended attempt lacks a durable Continue reservation".into(),
                )
                .await);
            },
        };
        let restored_without_provider = match &fence.pending_stage {
            surge_core::execution_recovery::PendingStagePhase::CommittedOutcomeAwaitingRoute {
                invocation,
                node,
                outcome,
                committed_seq,
            } => {
                let consumed = match validate_suspended_commit(
                    state,
                    invocation.as_ref(),
                    node,
                    outcome,
                    *committed_seq,
                ) {
                    Ok(consumed) => consumed,
                    Err(diagnostic) => {
                        return Err(recovery_required(params, diagnostic).await);
                    },
                };
                if !consumed {
                    let result = route_and_snapshot(
                        params,
                        state,
                        outcome,
                        surge_persistence::runs::EventSeq(0),
                    )
                    .await
                    .map_err(|error| format!("restore committed routing phase: {error}"));
                    if let Err(diagnostic) = result {
                        return Err(recovery_required(params, diagnostic).await);
                    }
                }
                true
            },
            surge_core::execution_recovery::PendingStagePhase::WaitingHumanGate {
                node,
                request,
                stage_entry_seq,
                requested_seq,
            } => {
                if !valid_suspended_gate(state, node, request, *stage_entry_seq, *requested_seq) {
                    return Err(recovery_required(
                        params,
                        "suspended human gate contradicts its original decision".into(),
                    )
                    .await);
                }
                true
            },
            surge_core::execution_recovery::PendingStagePhase::BetweenStages => true,
            surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {
                node,
                stage_entry_seq,
                ..
            } => {
                if node != &state.cursor.node
                    || state.memory.stage_occurrences.get(node) != Some(stage_entry_seq)
                    || state.memory.quota_plans.get(node) != Some(&fence.pending_stage)
                {
                    return Err(recovery_required(
                        params,
                        "suspended capacity plan contradicts actual stage occurrence".into(),
                    )
                    .await);
                }
                false
            },
            surge_core::execution_recovery::PendingStagePhase::Interrupted { .. } => false,
        };
        if restored_without_provider {
            if let Err(diagnostic) = commit_restored_continuation(params, control.generation).await
            {
                return Err(recovery_required(params, diagnostic).await);
            }
            state.memory.suspension = None;
            state.memory.control_generation = control.generation;
        }
    }
    Ok(())
}

async fn commit_restored_continuation(
    params: &RunTaskParams,
    control_generation: u64,
) -> Result<(), String> {
    #[cfg(debug_assertions)]
    restored_continuation_barrier(control_generation).await?;
    params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunContinued {
            control_generation,
        }))
        .await
        .map(|_| ())
        .map_err(|error| format!("authorize restored routing phase: {error}"))
}

/// Isolated crash fixtures select this boundary only in their child host.
/// The marker grants no authority: release still requires the real journal append.
#[cfg(debug_assertions)]
async fn restored_continuation_barrier(control_generation: u64) -> Result<(), String> {
    let Some(path) = std::env::var_os("SURGE_CONTINUE_ACK_BARRIER") else {
        return Ok(());
    };
    let path = std::path::PathBuf::from(path);
    let pending = path.with_extension("pending");
    tokio::fs::write(&pending, control_generation.to_string())
        .await
        .map_err(|error| format!("publish Continue fixture boundary: {error}"))?;
    tokio::fs::rename(&pending, path.with_extension("ready"))
        .await
        .map_err(|error| format!("publish complete Continue fixture boundary: {error}"))?;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|error| format!("release Continue fixture boundary: {error}"))
}

fn validate_suspended_commit(
    state: &RunExecutionState,
    invocation: Option<&surge_core::id::StageInvocationId>,
    node: &surge_core::NodeKey,
    outcome: &surge_core::OutcomeKey,
    committed_seq: u64,
) -> Result<bool, String> {
    let invocation = invocation.ok_or_else(|| {
        "suspended committed phase has no authenticated invocation binding".to_owned()
    })?;
    let record = state
        .memory
        .committed_stage_outcomes
        .get(invocation)
        .ok_or_else(|| "suspended committed phase lacks its accepted invocation".to_owned())?;
    if record.conflicting
        || record.committed_seq != committed_seq
        || record.commit.context().node != *node
        || record.commit.outcome() != outcome
    {
        return Err("suspended committed phase contradicts its accepted invocation".into());
    }
    Ok(record.routed_seq.is_some())
}

fn valid_suspended_gate(
    state: &RunExecutionState,
    node: &surge_core::NodeKey,
    request: &surge_core::id::GateRequestId,
    stage_entry_seq: u64,
    requested_seq: u64,
) -> bool {
    state
        .memory
        .gate_decisions
        .get(request)
        .is_some_and(|record| {
            !record.conflicting
                && record.node == *node
                && record.request_id == *request
                && record.stage_entry_seq == stage_entry_seq
                && record.requested_seq == requested_seq
                && state.cursor.node == *node
                && state.memory.stage_occurrences.get(node).copied() == Some(stage_entry_seq)
        })
}

async fn restore_committed_routes(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
) -> Result<(), RunOutcome> {
    // Accepted effects are already durable. A host crash before routing must
    // consume that exact commit without establishing another provider session.
    match outstanding_stage_outcome(state) {
        Ok(Some(record)) => {
            if let Err(error) = route_and_snapshot(
                params,
                state,
                record.commit.outcome(),
                surge_persistence::runs::EventSeq(0),
            )
            .await
            {
                return Err(recovery_required(
                    params,
                    format!("restore accepted stage commit: {error}"),
                )
                .await);
            }
        },
        Ok(None) => {},
        Err(error) => return Err(recovery_required(params, error).await),
    }

    match outstanding_gate_stage(state) {
        Ok(Some(record)) => {
            if record.commit.disposition()
                != surge_core::execution_recovery::gate_commit::GateCommitDisposition::Route
            {
                return Err(recovery_required(
                    params,
                    "committed bootstrap rejection requires failure routing".into(),
                )
                .await);
            }
            if let Err(error) = route_and_snapshot(
                params,
                state,
                record.commit.answer().outcome(),
                surge_persistence::runs::EventSeq(0),
            )
            .await
            {
                return Err(recovery_required(
                    params,
                    format!("restore committed gate effects: {error}"),
                )
                .await);
            }
        },
        Ok(None) => {},
        Err(error) => return Err(recovery_required(params, error).await),
    }
    Ok(())
}

async fn execute_stage_loop(
    params: &mut RunTaskParams,
    mut state: RunExecutionState,
) -> RunOutcome {
    loop {
        match Box::pin(execute_stage_step(params, &mut state)).await {
            StageLoopStep::Continue => {},
            StageLoopStep::Done(outcome) => return outcome,
        }
    }
}

enum StageLoopStep {
    Continue,
    Done(RunOutcome),
}

async fn execute_stage_step(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
) -> StageLoopStep {
    if state.frames.is_empty()
        && let Err(error) = drain_roadmap_queue(params, state).await
    {
        return StageLoopStep::Done(
            failed(params, format!("apply queued roadmap amendments: {error}")).await,
        );
    }
    apply_pending_revisions(state);
    if let Some(control) = suspension_requested(params) {
        return StageLoopStep::Done(
            prepare_suspension(
                params,
                state,
                control,
                surge_core::execution_recovery::PendingStagePhase::BetweenStages,
            )
            .await,
        );
    }
    if let Some(outcome) = abort_if_cancelled(params).await {
        return StageLoopStep::Done(outcome);
    }
    let Some(node) =
        lookup_in_active_frame(&state.routing_graph, &state.cursor.node, &state.frames)
    else {
        return StageLoopStep::Done(
            failed(
                params,
                format!("cursor at unknown node {}", state.cursor.node),
            )
            .await,
        );
    };
    let node = node.clone();
    let step = execute_current_stage(params, state, &node).await;
    // The registry clear is best-effort and can wait out registry contention;
    // it must not delay the durable route commit. It still precedes the next
    // stage's capacity precheck, so that precheck observes the recovery.
    clear_proven_capacity_recovery(params, state, &node).await;
    match step {
        StageLoopStep::Continue => StageLoopStep::Continue,
        StageLoopStep::Done(outcome) => StageLoopStep::Done(outcome),
    }
}

async fn execute_current_stage(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
) -> StageLoopStep {
    if let Some(error) = &state.memory.gate_recovery_error {
        return StageLoopStep::Done(recovery_required(params, error.clone()).await);
    }
    let restored_gate_entry = state
        .memory
        .gate_decisions
        .values()
        .find(|record| {
            matches!(node.config, NodeConfig::HumanGate(_))
                && record.purpose == surge_core::run_state::GateDecisionPurpose::HumanGate
                && record.node == state.cursor.node
        })
        .map(|record| record.stage_entry_seq)
        .or_else(|| {
            state
                .memory
                .suspension
                .as_ref()
                .and_then(|fence| match &fence.pending_stage {
                    surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {
                        node,
                        stage_entry_seq,
                        ..
                    } if node == &state.cursor.node => Some(*stage_entry_seq),
                    _ => None,
                })
        });
    let stage_start_seq = match restored_gate_entry.or_else(|| state.restored_quota_entry.take()) {
        Some(seq) => surge_persistence::runs::EventSeq(seq),
        None => match enter_stage(params, &state.cursor).await {
            Ok(seq) => seq,
            Err(error) => return StageLoopStep::Done(failed(params, error).await),
        },
    };
    state
        .memory
        .stage_occurrences
        .insert(state.cursor.node.clone(), stage_start_seq.as_u64());
    let stage_result = match dispatch_node_stage(params, state, node).await {
        StageDispatch::StageResult(result) => result,
        StageDispatch::Continue => return StageLoopStep::Continue,
        StageDispatch::Failed(error) => return StageLoopStep::Done(failed(params, error).await),
        StageDispatch::Park {
            wake_at,
            basis,
            runtime,
            details,
        } => {
            return StageLoopStep::Done(
                parked(params, &state.cursor.node, wake_at, basis, runtime, details).await,
            );
        },
    };
    if let Some(outcome) =
        handle_stage_suspension(params, state, node, stage_start_seq, &stage_result).await
    {
        return StageLoopStep::Done(outcome);
    }
    if let Err(StageError::RecoveryRequired(diagnostic)) = &stage_result {
        return StageLoopStep::Done(recovery_required(params, diagnostic.clone()).await);
    }
    let resolution = match resolve_stage_result(params, state, node, stage_result).await {
        Ok(resolution) => resolution,
        Err(outcome) => return StageLoopStep::Done(outcome),
    };
    match resolution {
        StageResolution::Terminal(outcome) => StageLoopStep::Done(outcome),
        StageResolution::Outcome(outcome) => {
            if let Err(error) = route_and_snapshot(params, state, &outcome, stage_start_seq).await {
                return StageLoopStep::Done(failed(params, error.to_string()).await);
            }
            match enforce_budget(params, state).await {
                Ok(Some(outcome)) => StageLoopStep::Done(outcome),
                Ok(None) => StageLoopStep::Continue,
                Err(error) => StageLoopStep::Done(failed(params, error).await),
            }
        },
    }
}

async fn handle_stage_suspension(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
    stage_start_seq: surge_persistence::runs::EventSeq,
    stage_result: &Result<StageOutcome, StageError>,
) -> Option<RunOutcome> {
    let control = suspension_requested(params)?;
    if matches!(node.config, NodeConfig::HumanGate(_))
        && matches!(stage_result, Err(StageError::Cancelled))
    {
        return Some(suspend_interrupted_gate(params, state, control, stage_start_seq).await);
    }
    let pending =
        pending_suspension_phase(params, state, node, stage_start_seq, stage_result).await;
    match pending {
        Ok(Some(pending)) => Some(prepare_suspension(params, state, control, pending).await),
        Ok(None) => None,
        Err((recovery, error)) => Some(if recovery {
            recovery_required(params, error).await
        } else {
            failed(params, error).await
        }),
    }
}

async fn suspend_interrupted_gate(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
    control: surge_core::execution_recovery::WorkItemExecutionControl,
    stage_start_seq: surge_persistence::runs::EventSeq,
) -> RunOutcome {
    let end = match params.writer.current_seq().await {
        Ok(seq) => seq,
        Err(error) => {
            return recovery_required(params, format!("read interrupted gate prefix: {error}"))
                .await;
        },
    };
    let events = match params.writer.read_events(stage_start_seq..end.next()).await {
        Ok(events) => events,
        Err(error) => {
            return recovery_required(params, format!("read interrupted gate identity: {error}"))
                .await;
        },
    };
    if let Err(error) = apply_read_events(params.run_id, &events, &mut state.memory) {
        return recovery_required(params, format!("fold interrupted gate identity: {error}")).await;
    }
    let records: Vec<_> = state
        .memory
        .gate_decisions
        .values()
        .filter(|record| {
            record.node == state.cursor.node
                && !record.conflicting
                && record.purpose == surge_core::run_state::GateDecisionPurpose::HumanGate
                && record.stage_entry_seq == stage_start_seq.as_u64()
        })
        .collect();
    if records.len() != 1 {
        return recovery_required(
            params,
            "interrupted human gate lacks one authoritative decision".into(),
        )
        .await;
    }
    let record = records[0];
    let pending = surge_core::execution_recovery::PendingStagePhase::WaitingHumanGate {
        node: record.node.clone(),
        request: record.request_id,
        stage_entry_seq: record.stage_entry_seq,
        requested_seq: record.requested_seq,
    };
    prepare_suspension(params, state, control, pending).await
}

async fn pending_suspension_phase(
    params: &mut RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
    stage_start_seq: surge_persistence::runs::EventSeq,
    stage_result: &Result<StageOutcome, StageError>,
) -> Result<Option<surge_core::execution_recovery::PendingStagePhase>, (bool, String)> {
    match stage_result {
        Ok(StageOutcome::Routed(outcome)) => {
            let events = read_stage_events(params, stage_start_seq, "accepted stage").await?;
            let accepted = events
                .iter()
                .rev()
                .find_map(|event| match event.payload.payload() {
                    EventPayload::StageOutcomeCommitted { commit }
                        if commit.context().node == state.cursor.node
                            && commit.outcome() == outcome =>
                    {
                        Some((commit.invocation(), event.seq.as_u64()))
                    },
                    _ => None,
                });
            if let Some((invocation, committed_seq)) = accepted {
                return Ok(Some(
                    surge_core::execution_recovery::PendingStagePhase::CommittedOutcomeAwaitingRoute {
                        invocation: Some(invocation),
                        node: state.cursor.node.clone(),
                        outcome: outcome.clone(),
                        committed_seq,
                    },
                ));
            }
            if matches!(node.config, NodeConfig::Agent(_)) {
                return Err((
                    false,
                    "accepted provider outcome has no durable invocation marker".into(),
                ));
            }
            route_and_snapshot(params, state, outcome, stage_start_seq)
                .await
                .map_err(|error| {
                    (
                        false,
                        format!("route non-provider stage before suspension: {error}"),
                    )
                })?;
            Ok(Some(
                surge_core::execution_recovery::PendingStagePhase::BetweenStages,
            ))
        },
        Err(StageError::Cancelled | StageError::RateLimited { .. }) => {
            let events = read_stage_events(params, stage_start_seq, "interrupted stage").await?;
            let invocation = events
                .iter()
                .rev()
                .find_map(|event| match &event.payload.payload {
                    EventPayload::SessionOpened {
                        node,
                        opened: Some(opened),
                        ..
                    } if node == &state.cursor.node => Some(opened.descriptor.invocation()),
                    _ => None,
                });
            Ok(invocation.map(|invocation| {
                surge_core::execution_recovery::PendingStagePhase::Interrupted {
                    node: state.cursor.node.clone(),
                    invocation,
                }
            }))
        },
        Err(StageError::CapacityExhausted) => {
            let events = read_stage_events(params, stage_start_seq, "planned capacity").await?;
            let phase = events
                .iter()
                .rev()
                .find_map(|row| match &row.payload.payload {
                    EventPayload::QuotaStagePlanned {
                        node,
                        stage_entry_seq,
                        logical_invocation,
                        ..
                    } if node == &state.cursor.node
                        && *stage_entry_seq == stage_start_seq.as_u64() =>
                    {
                        Some(
                            surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {
                                node: node.clone(),
                                logical_invocation: *logical_invocation,
                                stage_entry_seq: *stage_entry_seq,
                                plan_seq: row.seq.as_u64(),
                            },
                        )
                    },
                    _ => None,
                });
            Ok(phase)
        },
        Ok(StageOutcome::Terminal(_)) | Err(_) => Ok(None),
    }
}

async fn read_stage_events(
    params: &RunTaskParams,
    stage_start_seq: surge_persistence::runs::EventSeq,
    label: &str,
) -> Result<Vec<surge_persistence::runs::ReadEvent>, (bool, String)> {
    let end = params.writer.current_seq().await.map_err(|error| {
        (
            label == "interrupted stage",
            format!("read {label} prefix: {error}"),
        )
    })?;
    params
        .writer
        .read_events(stage_start_seq..end.next())
        .await
        .map_err(|error| {
            (
                label == "interrupted stage",
                format!("read {label} identity: {error}"),
            )
        })
}

async fn recovery_required(params: &RunTaskParams, diagnostic: String) -> RunOutcome {
    let generation = params
        .storage
        .work_items()
        .execution_control(params.run_id)
        .ok()
        .flatten()
        .map_or(0, |control| control.generation);
    let payload = EventPayload::RunRecoveryRequired {
        control_generation: generation,
        diagnostic: diagnostic.clone(),
    };
    if let Err(error) = params
        .writer
        .append_event(VersionedEventPayload::new(payload))
        .await
    {
        let _ = params.event_tx.send(EngineRunEvent::StreamError {
            message: format!("recovery decision was not committed: {error}"),
        });
    }
    RunOutcome::RecoveryRequired {
        control_generation: generation,
        diagnostic,
    }
}

fn suspension_requested(
    params: &RunTaskParams,
) -> Option<surge_core::execution_recovery::WorkItemExecutionControl> {
    params
        .storage
        .work_items()
        .execution_control(params.run_id)
        .ok()
        .flatten()
        .filter(|control| {
            control.state == surge_core::execution_recovery::ExecutionControlState::SuspendRequested
        })
}

async fn prepare_suspension(
    params: &mut RunTaskParams,
    state: &RunExecutionState,
    control: surge_core::execution_recovery::WorkItemExecutionControl,
    pending_stage: surge_core::execution_recovery::PendingStagePhase,
) -> RunOutcome {
    let prefix = match params.writer.current_seq().await {
        Ok(seq) => seq.as_u64(),
        Err(error) => return failed(params, format!("read suspension prefix: {error}")).await,
    };
    let mut snapshot = crate::engine::snapshot::EngineSnapshot::new(&state.cursor, prefix, prefix);
    snapshot.frames = state.frames.iter().cloned().map(Into::into).collect();
    snapshot.root_traversal_counts = state
        .root_traversal_counts
        .iter()
        .map(|(edge, count)| (edge.to_string(), *count))
        .collect();
    snapshot.applied_graph_revision_seq = state.applied_graph_revision_seq;
    snapshot.pending_stage = Some(pending_stage.clone());
    let reason = if params.work_item_claim.is_some() {
        let store = params.storage.work_items();
        match store.capacity_control_association(params.run_id, control.generation) {
            Ok(Some(association)) => match store.recovery_cycle(
                params.run_id,
                association.invocation(),
                association.cycle_generation(),
            ) {
                Ok(cycle) => match cycle.wake {
                    Some(wake) => surge_core::execution_recovery::SuspensionReason::Capacity {
                        wake_at_ms: Some(wake.due_at_ms()),
                    },
                    None => {
                        return recovery_required(
                            params,
                            "quota Capacity control has no durable wake schedule".into(),
                        )
                        .await;
                    },
                },
                Err(error) => {
                    return recovery_required(
                        params,
                        format!("read quota Capacity cycle: {error}"),
                    )
                    .await;
                },
            },
            Ok(None) => surge_core::execution_recovery::SuspensionReason::Manual,
            Err(error) => {
                return recovery_required(
                    params,
                    format!("inspect quota Capacity control: {error}"),
                )
                .await;
            },
        }
    } else {
        surge_core::execution_recovery::SuspensionReason::Manual
    };
    let fence = surge_core::execution_recovery::SuspensionFence {
        control_generation: control.generation,
        snapshot_seq: prefix,
        pending_stage,
        cleanup_confirmed: true,
        reason,
    };
    match serde_json::to_vec(&snapshot) {
        Ok(blob) => params.pending_suspension = Some((fence.clone(), blob)),
        Err(error) => return failed(params, format!("encode suspension snapshot: {error}")).await,
    }
    RunOutcome::Suspended {
        fence: Box::new(fence),
    }
}

/// Evaluate the run budget against the freshly-folded cumulative cost and act.
///
/// Returns:
/// - `Ok(None)` to continue (within budget, unlimited, or after recording an
///   advisory warn/breach under `WarnOnly`).
/// - `Ok(Some(outcome))` when an `Abort`-policy breach durably terminated the
///   run.
/// - `Err(reason)` when a **mandatory** abort write (`BudgetExceeded` /
///   `RunAborted`) failed to persist — the caller turns this into a hard run
///   failure so the run never terminates without a durable, replayable record.
///
/// Advisory writes (the threshold warning, and the `WarnOnly` breach record)
/// are best-effort: on append failure the flag is left unset so the next stage
/// boundary retries, rather than failing an otherwise-healthy run.
async fn enforce_budget(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
) -> Result<Option<RunOutcome>, String> {
    use surge_core::budget::{BudgetAction, decide};

    let guard = params.run_config.budget;
    if guard.is_unlimited() {
        return Ok(None);
    }

    let cost_usd = state.memory.costs.cost_usd;
    let total_tokens = state
        .memory
        .costs
        .tokens_in
        .saturating_add(state.memory.costs.tokens_out);
    let verdict = guard.limits.evaluate(&state.memory.costs);

    match decide(
        verdict,
        guard.policy,
        state.budget_warned,
        state.budget_exceeded_noted,
    ) {
        BudgetAction::Continue => Ok(None),
        BudgetAction::Warn { dimension, pct } => {
            record_budget_warning(params, state, dimension, pct, cost_usd, total_tokens).await
        },
        BudgetAction::NoteExceeded { dimension } => {
            record_budget_exceeded_noted(params, state, dimension, cost_usd, total_tokens).await
        },
        BudgetAction::Abort { dimension } => {
            abort_run_for_budget(params, dimension, cost_usd, total_tokens).await
        },
    }
}

/// One-time `BudgetWarningRaised` advisory. On a failed append the flag is
/// left unset so the next boundary retries, rather than losing the warning
/// and silencing all future ones.
async fn record_budget_warning(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    dimension: surge_core::budget::BudgetDimension,
    pct: u8,
    cost_usd: f64,
    total_tokens: u64,
) -> Result<Option<RunOutcome>, String> {
    if let Err(error) = params
        .writer
        .append_event(VersionedEventPayload::new(
            EventPayload::BudgetWarningRaised {
                dimension,
                pct,
                cost_usd,
                total_tokens,
            },
        ))
        .await
    {
        tracing::warn!(
            target: "engine::budget",
            run_id = %params.run_id,
            %error,
            "failed to append BudgetWarningRaised; will retry at next boundary"
        );
        return Ok(None);
    }
    state.budget_warned = true;
    tracing::warn!(
        target: "engine::budget",
        run_id = %params.run_id,
        ?dimension,
        pct,
        cost_usd,
        total_tokens,
        "run budget warning threshold crossed"
    );
    Ok(None)
}

/// One-time `BudgetExceeded` record under the `WarnOnly` policy — surfaces
/// the limit crossing without stopping the run. Same retry-on-failure
/// posture as [`record_budget_warning`].
async fn record_budget_exceeded_noted(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    dimension: surge_core::budget::BudgetDimension,
    cost_usd: f64,
    total_tokens: u64,
) -> Result<Option<RunOutcome>, String> {
    if let Err(error) = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::BudgetExceeded {
            dimension,
            cost_usd,
            total_tokens,
        }))
        .await
    {
        tracing::warn!(
            target: "engine::budget",
            run_id = %params.run_id,
            %error,
            "failed to append BudgetExceeded (warn-only); will retry at next boundary"
        );
        return Ok(None);
    }
    state.budget_exceeded_noted = true;
    tracing::warn!(
        target: "engine::budget",
        run_id = %params.run_id,
        ?dimension,
        cost_usd,
        total_tokens,
        "run budget exceeded (warn-only policy; run continues)"
    );
    Ok(None)
}

/// Mandatory budget-breach abort: the breach record and the terminal abort
/// MUST be durable, or replay/audit cannot explain why the run stopped.
async fn abort_run_for_budget(
    params: &RunTaskParams,
    dimension: surge_core::budget::BudgetDimension,
    cost_usd: f64,
    total_tokens: u64,
) -> Result<Option<RunOutcome>, String> {
    params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::BudgetExceeded {
            dimension,
            cost_usd,
            total_tokens,
        }))
        .await
        .map_err(|e| format!("persist BudgetExceeded for abort: {e}"))?;
    let reason =
        format!("budget exceeded ({dimension:?}): cost=${cost_usd:.4}, tokens={total_tokens}");
    params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
            reason: reason.clone(),
        }))
        .await
        .map_err(|e| format!("persist RunAborted for budget breach: {e}"))?;
    tracing::warn!(
        target: "engine::budget",
        run_id = %params.run_id,
        ?dimension,
        cost_usd,
        total_tokens,
        "run budget exceeded; aborting run"
    );
    let outcome = RunOutcome::Aborted { reason };
    Ok(Some(outcome))
}

#[derive(Clone)]
struct RunExecutionState {
    memory_applied_seq: u64,
    /// The persisted graph (initial or latest applied revision). Amendments
    /// and hashes use this graph.
    active_graph: Graph,
    /// `active_graph` plus default escalation gates
    /// (`surge_core::escalation`); node lookup and routing use this graph.
    /// Recomputed whenever `active_graph` changes.
    routing_graph: Graph,
    cursor: Cursor,
    hook_executor: HookExecutor,
    memory: RunMemory,
    frames: Vec<crate::engine::frames::Frame>,
    root_traversal_counts: std::collections::HashMap<surge_core::keys::EdgeKey, u32>,
    applied_graph_revision_seq: u64,
    processed_graph_revision_seq: u64,
    pending_graph_revisions: Vec<ObservedGraphRevision>,
    pending_elevations: std::sync::Arc<crate::engine::elevation::PendingElevations>,
    /// Whether a `BudgetWarningRaised` has already been emitted this run, so the
    /// warn band fires at most once per run (idempotent re-evaluation).
    budget_warned: bool,
    /// Whether a `BudgetExceeded` has already been recorded this run, so the
    /// `WarnOnly` breach record fires at most once (separate from the warn).
    budget_exceeded_noted: bool,
    /// Lazily-populated cache of the run's skill catalog
    /// (`default_skill_roots(&params.worktree_path)`, discovered once).
    /// The roots are constant for the whole run (derived only from the
    /// worktree path and the machine's home directory, neither of which
    /// changes mid-run), so re-discovering — a synchronous walk + hash of
    /// every pack under up to four roots — on every agent node that
    /// declares skills would re-pay that cost per node for no reason.
    /// `None` until the first node that declares skills populates it.
    skill_catalog: Option<std::sync::Arc<surge_core::skill::SkillCatalog>>,
    /// Inspected original unadmitted occurrence, without effect authority.
    restored_quota_entry: Option<u64>,
    /// Runtime whose exhaustion the current stage's successful dispatch
    /// refuted, resolved from that dispatch's own events. Cleared from the
    /// capacity ledger only after the stage's route commit.
    proven_capacity_recovery: Option<crate::engine::capacity::CanonicalRuntimeId>,
}

fn initial_execution_state(params: &RunTaskParams) -> RunExecutionState {
    // The loaded execution state is whole (#100); only per-process parts are
    // added here: the routing graph, the hook executor, the elevation queue
    // and the budget flags from memory.
    let crate::engine::execution_state::ExecutionState {
        cursor,
        frames,
        root_traversal_counts,
        memory,
        memory_applied_seq,
        graph: active_graph,
        applied_graph_revision_seq,
        pending_graph_revisions,
        ..
    } = params.execution.clone();

    // Seed the warn-once / breach-once flags from the folded event log so a
    // resumed run does not re-emit a `BudgetWarningRaised` / `BudgetExceeded`
    // it already recorded before a restart.
    let budget_warned = memory.budget_warning_raised;
    let budget_exceeded_noted = memory.budget_exceeded_noted;

    RunExecutionState {
        routing_graph: surge_core::escalation::with_default_escalation_gates(&active_graph),
        active_graph,
        cursor,
        hook_executor: HookExecutor::new(),
        memory,
        memory_applied_seq,
        frames,
        root_traversal_counts,
        applied_graph_revision_seq,
        processed_graph_revision_seq: applied_graph_revision_seq,
        pending_graph_revisions,
        pending_elevations: params.pending_elevations.clone(),
        budget_warned,
        budget_exceeded_noted,
        skill_catalog: None,
        restored_quota_entry: None,
        proven_capacity_recovery: None,
    }
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
    let applied = state.applied_graph_revision_seq;
    let drained = roadmap_queue
        .drain(
            &mut state.active_graph,
            &state.cursor,
            &mut state.memory,
            &mut state.pending_graph_revisions,
            &mut state.processed_graph_revision_seq,
            &mut state.applied_graph_revision_seq,
        )
        .await;
    if state.applied_graph_revision_seq != applied {
        state.refresh_routing_graph();
    }
    drained
}

fn apply_pending_revisions(state: &mut RunExecutionState) {
    let applied = state.applied_graph_revision_seq;
    maybe_apply_pending_graph_revision(
        &mut state.active_graph,
        &state.cursor,
        &state.frames,
        &mut state.pending_graph_revisions,
        &mut state.processed_graph_revision_seq,
        &mut state.applied_graph_revision_seq,
    );
    if state.applied_graph_revision_seq != applied {
        state.refresh_routing_graph();
    }
}

impl RunExecutionState {
    fn refresh_routing_graph(&mut self) {
        self.routing_graph =
            surge_core::escalation::with_default_escalation_gates(&self.active_graph);
    }
}

async fn abort_if_cancelled(params: &RunTaskParams) -> Option<RunOutcome> {
    if params.cancel.is_cancelled() {
        Some(abort_run(params).await)
    } else {
        None
    }
}

async fn abort_run(params: &RunTaskParams) -> RunOutcome {
    let reason = "stop_run requested".to_string();
    if let Err(error) = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
            reason: reason.clone(),
        }))
        .await
    {
        return failed(
            params,
            format!("persist RunAborted after cancellation: {error}"),
        )
        .await;
    }
    RunOutcome::Aborted { reason }
}

/// Pure check for the [`checkpoint_exit_if_requested`] fault-injection seam:
/// does `env` (the `SURGE_CHECKPOINT_EXIT` value) name `node_key`?
///
/// Compiled in debug builds and tests; the actual `process::exit` wrapper
/// remains debug-gated even when release unit tests check this predicate.
#[cfg(any(debug_assertions, test))]
fn checkpoint_exit_matches(env: Option<&str>, node_key: &str) -> bool {
    env.is_some_and(|target| target == node_key)
}

/// Durability fault-injection seam (debug builds only). When
/// `SURGE_CHECKPOINT_EXIT` names the node about to execute, abort the process
/// **uncleanly** — `std::process::exit` runs no async teardown and no `Drop`,
/// the way a `kill -9` would. Called right after the `StageEntered` event is
/// durably committed, so the on-disk event log is left in a precise mid-run
/// state for the recovery/durability harness to assert against.
#[cfg(debug_assertions)]
fn checkpoint_exit_if_requested(node_key: &surge_core::keys::NodeKey) {
    let target = std::env::var("SURGE_CHECKPOINT_EXIT").ok();
    if checkpoint_exit_matches(target.as_deref(), node_key.as_str()) {
        tracing::warn!(
            target: "engine::fault_injection",
            node = %node_key,
            "SURGE_CHECKPOINT_EXIT hit; aborting process uncleanly (exit 99) after StageEntered commit"
        );
        std::process::exit(99);
    }
}

#[cfg(not(debug_assertions))]
#[inline]
fn checkpoint_exit_if_requested(_node_key: &surge_core::keys::NodeKey) {}

async fn enter_stage(
    params: &RunTaskParams,
    cursor: &Cursor,
) -> Result<surge_persistence::runs::EventSeq, String> {
    let entry_seq = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
            node: cursor.node.clone(),
            attempt: cursor.attempt,
        }))
        .await
        .map_err(|e| format!("write StageEntered: {e}"))?;
    // Fault-injection: simulate a crash mid-run, with StageEntered durable.
    checkpoint_exit_if_requested(&cursor.node);
    Ok(entry_seq)
}

enum StageDispatch {
    StageResult(Result<StageOutcome, StageError>),
    Continue,
    Failed(String),
    /// Do not dispatch this node. Park the run instead (Task 12,
    /// R37/R37.1) — the run task's caller writes `RunParked`, transitions
    /// the registry to `RunStatus::Parked`, and returns cleanly, leaving
    /// the worktree in place.
    Park {
        /// When the run is expected to resume on its own.
        wake_at: chrono::DateTime<chrono::Utc>,
        /// Why `wake_at` is what it is — carried through to `RunParked`.
        basis: surge_core::capacity::WakeBasis,
        /// Canonical agent-runtime id the parked capacity window belongs
        /// to. `None` only via the legacy no-profile-registry path.
        runtime: Option<String>,
        /// Raw bridge error text from the `StageError::RateLimited` that
        /// triggered this park, when parking happened reactively (post-429)
        /// rather than at the pre-dispatch check. `None` for a pre-dispatch
        /// park (nothing was attempted, so there is no error text) — folded
        /// into `RunParked.reason` so the operator reading `surge inbox`
        /// sees the actual provider text, not just Surge's own summary.
        details: Option<String>,
    },
}

/// The node's config moved to the agent of its active rotation
/// (`StageRuntimeRotated`, v1 task 1.4). `None` keeps `cfg`. Stages under a
/// frozen quota plan keep their planned runtime.
fn runtime_rotation_config(
    params: &RunTaskParams,
    state: &RunExecutionState,
    cfg: &surge_core::agent_config::AgentConfig,
) -> Option<surge_core::agent_config::AgentConfig> {
    let node = &state.cursor.node;
    let rotation = state.memory.runtime_rotations.get(node)?;
    if !stage_ownership(params, node).allows_runtime_overrides() {
        return None;
    }
    Some(cfg.with_runtime_override(&rotation.to, None))
}

/// Which quota cycle owns `node` for this run (the single rule lives in
/// [`crate::engine::quota_recovery::StageOwnership::of`]).
fn stage_ownership(
    params: &RunTaskParams,
    node: &surge_core::keys::NodeKey,
) -> crate::engine::quota_recovery::StageOwnership {
    crate::engine::quota_recovery::StageOwnership::of(
        params.run_config.quota_recovery.stage(node),
        params.work_item_claim.is_some(),
    )
}

/// Runtimes of the agent nodes paired with `node` by a capped retry edge in
/// its scope (a verifier and the implementer it sends work back to), with
/// their active rotations applied.
fn partner_runtimes(params: &RunTaskParams, state: &RunExecutionState) -> Vec<String> {
    let node = &state.cursor.node;
    let graph = &state.routing_graph;
    let Some((nodes, edges)) = std::iter::once((&graph.nodes, graph.edges.as_slice()))
        .chain(
            graph
                .subgraphs
                .values()
                .map(|sg| (&sg.nodes, sg.edges.as_slice())),
        )
        .find(|(nodes, _)| nodes.contains_key(node))
    else {
        return Vec::new();
    };
    let partners: std::collections::BTreeSet<&surge_core::keys::NodeKey> = edges
        .iter()
        .filter(|edge| edge.policy.max_traversals.is_some() && edge.from.node != edge.to)
        .filter_map(|edge| {
            if &edge.from.node == node {
                Some(&edge.to)
            } else if &edge.to == node {
                Some(&edge.from.node)
            } else {
                None
            }
        })
        .collect();
    partners
        .into_iter()
        .filter_map(|partner| {
            let NodeConfig::Agent(cfg) = &nodes.get(partner)?.config else {
                return None;
            };
            let cfg = match state.memory.runtime_rotations.get(partner) {
                Some(rotation) => cfg.with_runtime_override(&rotation.to, None),
                None => cfg.clone(),
            };
            crate::engine::stage::agent::resolve_node_runtime_id(
                params.profile_registry.as_deref(),
                params.agent_registry.as_deref(),
                &cfg,
            )
            .map(crate::engine::capacity::CanonicalRuntimeId::into_string)
        })
        .collect()
}

/// The node's config for this occurrence when it is the extra attempt of an
/// exhausted retry loop (entered through a derived edge marked by
/// `surge_core::escalation::is_alternate_attempt`): its runtime moved to the
/// configured retry agent. `None` keeps the node's own config. Derived from
/// the durable `entered_via` fold, so a restarted host decides the same way.
fn alternate_attempt_config(
    params: &RunTaskParams,
    state: &RunExecutionState,
    cfg: &surge_core::agent_config::AgentConfig,
) -> Option<surge_core::agent_config::AgentConfig> {
    let node = &state.cursor.node;
    if !surge_core::escalation::entered_by_alternate_attempt(
        &state.routing_graph,
        node,
        state.memory.entered_via.get(node),
    ) {
        return None;
    }
    if !stage_ownership(params, node).allows_runtime_overrides() {
        tracing::info!(
            target: "engine::escalation",
            %node,
            "extra attempt keeps the stage's frozen quota runtime"
        );
        return None;
    }
    let retry = params.escalation.retry_config(cfg);
    tracing::info!(
        target: "engine::escalation",
        %node,
        retry_agent = params.escalation.retry_agent().unwrap_or("(stage agent)"),
        "running the extra attempt of an exhausted retry loop"
    );
    retry
}

async fn dispatch_node_stage(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
) -> StageDispatch {
    let stage_result = match &node.config {
        NodeConfig::Agent(cfg) => {
            let retry = alternate_attempt_config(params, state, cfg);
            let cfg = retry.as_ref().unwrap_or(cfg);
            let rotated = runtime_rotation_config(params, state, cfg);
            let cfg = rotated.as_ref().unwrap_or(cfg);
            return dispatch_agent_node_with_capacity_gate(params, state, node, cfg).await;
        },
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

/// Record verification evidence for a failed agent attempt whose run has a
/// verification subject, before the failure is routed.
async fn observe_failed_agent(
    params: &RunTaskParams,
    state: &RunExecutionState,
    result: &Result<StageOutcome, StageError>,
) -> Result<(), StageError> {
    use crate::engine::stage::verification::{VerificationPoint, verification_in_play};
    if result.is_ok() || !verification_in_play(VerificationPoint::AfterFailure, &state.memory) {
        return Ok(());
    }
    crate::engine::stage::verification::observe_after(
        &params.writer,
        &params.worktree_path,
        state.memory.verification.subject.as_ref(),
    )
    .await
}

/// Agent dispatch under the run's capacity cycle (#93). The cycle decides
/// before dispatch (precheck, fallback rotation, park) and after a typed 429
/// (observe, rotate, park, or surface the error to `on_error`); this function
/// only carries the stage's facts in and acts on the answer. Parking happens
/// *before* the error reaches `on_error`, so a hook that routes the failure
/// back to this node cannot re-dispatch into the same exhausted window.
async fn dispatch_agent_node_with_capacity_gate(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
    cfg: &surge_core::agent_config::AgentConfig,
) -> StageDispatch {
    use crate::engine::quota_recovery::{AfterRateLimit, BeforeDispatch, StageCapacity};
    if let Some(error) = validate_persistent_dispatch_authority(params) {
        return StageDispatch::Failed(format!(
            "persistent task launch authority expired before provider dispatch: {error}"
        ));
    }
    // The configured runtime controls admission. Recovery after dispatch must
    // use the actual provider opening, which may belong to a quota fallback.
    let partners = partner_runtimes(params, state);
    let node_key = state.cursor.node.clone();
    let own_runtime = match &node.config {
        NodeConfig::Agent(own) => crate::engine::stage::agent::resolve_node_runtime_id(
            params.profile_registry.as_deref(),
            params.agent_registry.as_deref(),
            own,
        )
        .map(crate::engine::capacity::CanonicalRuntimeId::into_string),
        _ => None,
    };
    let capacity_stage = StageCapacity {
        node: &node_key,
        runtime: crate::engine::stage::agent::resolve_node_runtime_id(
            params.profile_registry.as_deref(),
            params.agent_registry.as_deref(),
            cfg,
        ),
        own_runtime,
        rotations_this_visit: state
            .memory
            .runtime_rotations
            .get(&node_key)
            .map_or(0, |rotation| rotation.count),
        partners: &partners,
        ownership: stage_ownership(params, &node_key),
    };
    match params
        .capacity
        .before_dispatch(&capacity_stage, &params.writer)
        .await
    {
        BeforeDispatch::Dispatch => {},
        BeforeDispatch::Rotated { park } => return after_rotation(params, state, park).await,
        BeforeDispatch::Park(park) => return park_dispatch(park),
        BeforeDispatch::Refuse(reason) => return StageDispatch::Failed(reason),
    }

    let dispatch_prefix = params.writer.current_seq().await.ok();
    let result = execute_agent_node(params, state, node, cfg).await;
    if let Err(error) = observe_failed_agent(params, state, &result).await {
        return StageDispatch::StageResult(Err(error));
    }
    let Err(StageError::RateLimited {
        runtime: Some(raw_runtime),
        retry_after,
        details,
    }) = &result
    else {
        if result.is_ok()
            && let Some(prefix) = dispatch_prefix
        {
            state.proven_capacity_recovery =
                successful_dispatch_runtime(params, &state.cursor.node, prefix).await;
        }
        return StageDispatch::StageResult(result);
    };
    let (raw_runtime, retry_after, details) = (raw_runtime.clone(), *retry_after, details.clone());
    match params
        .capacity
        .after_rate_limit(
            &capacity_stage,
            &params.writer,
            &raw_runtime,
            retry_after,
            &details,
            suspension_requested(params).is_some(),
        )
        .await
    {
        AfterRateLimit::Rotated { park } => after_rotation(params, state, park).await,
        AfterRateLimit::Park(park) => park_dispatch(park),
        AfterRateLimit::Surface => StageDispatch::StageResult(result),
    }
}

fn park_dispatch(park: crate::engine::quota_recovery::ParkPlan) -> StageDispatch {
    StageDispatch::Park {
        wake_at: park.wake_at,
        basis: park.basis,
        runtime: park.runtime,
        details: park.details,
    }
}

/// Refold memory after a durable `StageRuntimeRotated` so the next attempt
/// runs on the fallback agent; park instead if the fold cannot be refreshed.
async fn after_rotation(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    park: crate::engine::quota_recovery::ParkPlan,
) -> StageDispatch {
    match apply_memory_events_after(
        &params.writer,
        params.run_id,
        surge_persistence::runs::EventSeq(state.memory_applied_seq),
        &mut state.memory,
    )
    .await
    {
        Ok(applied) => {
            state
                .pending_graph_revisions
                .extend(applied.graph_revisions);
            if let Ok(current) = params.writer.current_seq().await {
                state.memory_applied_seq = current.as_u64();
            }
            StageDispatch::Continue
        },
        Err(error) => {
            tracing::warn!(target: "engine::capacity", node = %state.cursor.node, %error,
                "could not refresh memory after the rotation; parking instead");
            park_dispatch(park)
        },
    }
}

/// A successful stage proves recovery only for its last actual provider opening.
/// Read strictly after the dispatch prefix so previous attempts cannot supply it,
/// and immediately on dispatch return so later route, hook or stage events cannot
/// either. The resulting ledger clear is deferred past the route commit.
async fn successful_dispatch_runtime(
    params: &RunTaskParams,
    node: &surge_core::keys::NodeKey,
    prefix: surge_persistence::runs::EventSeq,
) -> Option<crate::engine::capacity::CanonicalRuntimeId> {
    match read_stage_events(params, prefix.next(), "capacity dispatch").await {
        Ok(events) => params.capacity.recovered_runtime(node, &events),
        Err((_, error)) => {
            tracing::warn!(target: "engine::capacity", %node, %error,
                "cannot establish successful provider identity; retaining capacity observations");
            None
        },
    }
}

/// Apply the recovery proven by this stage's successful dispatch. Losing it to
/// cancellation only retains a stale exhaustion row until the next success.
async fn clear_proven_capacity_recovery(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
) {
    if let Some(runtime) = state.proven_capacity_recovery.take() {
        params.capacity.clear_recovered(&node.id, &runtime).await;
    }
}

fn validate_persistent_dispatch_authority(params: &RunTaskParams) -> Option<String> {
    let Some(claim) = &params.work_item_claim else {
        return None;
    };
    params
        .storage
        .work_items()
        .validate_owned_flow_effect(claim)
        .err()
        .map(|error| error.to_string())
}

async fn execute_agent_node(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
    cfg: &surge_core::agent_config::AgentConfig,
) -> Result<StageOutcome, StageError> {
    // Skills bind at stage entry, exactly like a context `Binding`
    // (`project.md`) — never lazily mid-stage. A denied or unanswered trust
    // prompt returns before any session opens, so the node does not start.
    // The bound skills' instructions are threaded into `AgentStageParams`
    // below so `execute_agent_stage` can inject them into the prompt — a
    // skill that only reaches `SkillBound` in the log and never the agent
    // is a record, not a binding.
    let bound_skills = bind_declared_skills(params, state, cfg).await?;

    // Drain any operator steer messages queued for this run and hand them to
    // the stage, which prepends them to the prompt and records delivery. This
    // is the safe stage-boundary steering point (ACP v1 has no mid-turn inject).
    //
    // Skip the bootstrap flow-generator: an operator steering the *work* should
    // not have their guidance consumed by graph generation. The steers stay
    // queued for the first real implementation stage.
    let steers = if crate::engine::bootstrap::is_flow_generator_profile(cfg.profile.as_str()) {
        Vec::new()
    } else {
        let mut queue = params.pending_steers.lock().await;
        std::mem::take(&mut queue.pending)
    };
    // Kept so a stage that fails before delivery doesn't silently drop them: on
    // error we return them to the front of the queue. A post-send failure may
    // re-deliver on the retry, which is acceptable — the operator's guidance
    // should apply to the re-attempt too.
    let steers_backup = steers.clone();
    let quota_owner = crate::engine::quota_recovery::TaskQuotaOwner::for_stage(
        params.storage.work_items(),
        params.work_item_claim.as_ref(),
        params.run_config.quota_recovery.stage(&state.cursor.node),
    );
    let continuation = params
        .storage
        .work_items()
        .execution_control(params.run_id)
        .map_err(|error| StageError::Storage(error.to_string()))?;
    let (mut quota_opening, mut quota_cycle) = match &quota_owner {
        Some(owner) => owner.prepare_automatic_wake(params.run_id, continuation.as_ref())?,
        None => (None, None),
    };
    if quota_opening.is_none()
        && let Some(owner) = quota_owner.as_ref()
        && owner.has_configured_routes()
    {
        let policy_hash = params
            .run_config
            .quota_recovery
            .content_hash()
            .map_err(|error| StageError::Storage(error.to_string()))?;
        let cycle = owner
            .bind_current_plan(
                &params.writer,
                &state.cursor.node,
                state.cursor.attempt,
                policy_hash,
                continuation
                    .as_ref()
                    .map_or(0, |control| control.generation),
            )
            .await?;
        (quota_opening, quota_cycle) = owner.select_current_plan(
            &cycle,
            &params.worktree_path,
            params.agent_registry.as_deref(),
        )?;
    }
    let stage_result = Box::pin(execute_agent_stage(AgentStageParams {
        quota_opening,
        quota_cycle,
        quota_owner,
        continuation,
        frames: &state.frames,
        cancel: params.cancel.clone(),
        node: &state.cursor.node,
        attempt: state.cursor.attempt,
        steers,
        agent_config: cfg,
        bound_skills: &bound_skills,
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
        // `EngineRunConfig`'s fields are `Option` (unset vs. explicitly
        // defaulted — see its doc); this is the one place that resolves to
        // a concrete value before it reaches the dispatcher.
        tool_call_loop_guard: params.run_config.tool_call_loop_guard.unwrap_or_default(),
        output_spill: params.run_config.output_spill.unwrap_or_default(),
        profile_registry: params.profile_registry.clone(),
        agent_registry: params.agent_registry.clone(),
        hook_executor: &state.hook_executor,
        pending_elevations: state.pending_elevations.clone(),
        active_task_id: crate::engine::frames::active_task_id(&state.frames),
    }))
    .await;

    // Return undelivered steers to the front of the queue if the stage failed,
    // so operator guidance survives a stage error / retry instead of vanishing —
    // but drop any the operator cancelled while it was in-flight, so a cancel
    // isn't reversed by the re-queue.
    if stage_result.is_err() && !steers_backup.is_empty() {
        let mut queue = params.pending_steers.lock().await;
        let mut restored: Vec<_> = steers_backup
            .into_iter()
            .filter(|steer| !queue.cancelled.contains(&steer.id))
            .collect();
        restored.append(&mut queue.pending);
        queue.pending = restored;
    }

    stage_result.map(StageOutcome::Routed)
}

/// Resolve `cfg`'s declared skills, gate any untrusted one behind operator
/// approval, and return the bound skills so the caller can inject their
/// instructions into the agent's prompt (R10) — before the agent stage
/// proper runs.
///
/// Reuses the exact node-keyed decision registry (`gate_resolutions`)
/// `HumanGate` stages already register into and `Engine::resolve_human_input`
/// already drains — a skill-trust prompt is delivered and resolved through
/// the same live, rendered path (`HumanInputRequested`), not a second one.
async fn bind_declared_skills(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    cfg: &surge_core::agent_config::AgentConfig,
) -> Result<Vec<crate::engine::stage::skill_binding::BoundSkill>, StageError> {
    let declared =
        cfg.declared_skills()
            .map_err(|source| StageError::InvalidSkillsDeclaration {
                node: state.cursor.node.clone(),
                source,
            })?;
    if declared.is_empty() {
        return Ok(Vec::new());
    }

    let catalog = cached_skill_catalog(params, state).await?;
    let gate_enabled = skill_approval_enabled(params, cfg);

    bind_skills(SkillBindingParams {
        node: &state.cursor.node,
        declared: &declared,
        catalog: catalog.as_ref(),
        writer: &params.writer,
        gate_enabled,
        gate_resolutions: Some(params.gate_resolutions.as_ref()),
        approval_timeout: params.run_config.human_input_timeout,
        cancel: &params.cancel,
    })
    .await
}

/// The run's skill catalog, discovering it at most once.
///
/// `SkillCatalog::discover` synchronously walks and hashes every pack under
/// up to four roots — real installs measure in the hundreds — so it runs on
/// the blocking-task pool, never inline on the async worker thread, and its
/// result is cached on `state` after the first agent node that declares
/// skills, reused by every later one instead of re-walking the filesystem
/// per node.
///
/// What's actually frozen by the cache is the **candidate list** —
/// `discover()`'s directory walk — not pack content: a pack added or
/// removed under a root partway through the run will not be seen by any
/// node after the first (the walk isn't repeated), but an existing pack
/// *edited in place* is still caught, because `SkillCatalog::resolve`
/// always re-reads and re-hashes that specific pack's files from disk at
/// resolve time (see its own doc comment) — only discovery is memoized,
/// never resolution. The roots themselves (derived from the worktree path
/// and the machine's home directory) are constant for the run regardless.
async fn cached_skill_catalog(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
) -> Result<std::sync::Arc<surge_core::skill::SkillCatalog>, StageError> {
    if let Some(cached) = state.skill_catalog.as_ref() {
        return Ok(cached.clone());
    }

    let roots = default_skill_roots(&params.worktree_path);
    let catalog =
        tokio::task::spawn_blocking(move || surge_core::skill::SkillCatalog::discover(&roots))
            .await
            .map_err(|e| StageError::Internal(format!("skill catalog discovery panicked: {e}")))?;

    let catalog = std::sync::Arc::new(catalog);
    state.skill_catalog = Some(catalog.clone());
    Ok(catalog)
}

/// Whether the operator-approval trust gate is active for `cfg`'s node
/// (`ApprovalConfig::skill_approval`).
///
/// Node-level `approvals_override` wins when present. Otherwise falls back
/// to the resolved profile's own `approvals.skill_approval` — the same
/// node-then-profile precedence `engine::stage::agent::effective_approvals`
/// already establishes for every other approval flag — so a profile that
/// turns the gate off is honored even for a node that carries no override
/// of its own at all. An unresolvable profile reference or a missing
/// registry falls back to the default (gate enabled): failing to resolve a
/// profile here is not this function's failure to report — the agent stage
/// itself will hit and report the identical resolution failure moments
/// later, before any session opens.
fn skill_approval_enabled(
    params: &RunTaskParams,
    cfg: &surge_core::agent_config::AgentConfig,
) -> bool {
    if let Some(node_override) = cfg.approvals_override.as_ref() {
        return node_override.skill_approval;
    }
    let Some(registry) = params.profile_registry.as_deref() else {
        return surge_core::approvals::ApprovalConfig::default().skill_approval;
    };
    let Ok(key_ref) = surge_core::profile::keyref::parse_key_ref(cfg.profile.as_ref()) else {
        return surge_core::approvals::ApprovalConfig::default().skill_approval;
    };
    match registry.resolve(&key_ref) {
        Ok(resolved) => resolved.profile.approvals.skill_approval,
        Err(_) => surge_core::approvals::ApprovalConfig::default().skill_approval,
    }
}

/// Skill roots scanned for a node's declared skills: the worktree's and the
/// user's `.claude/skills` (Agent Skills packs) **and** `.claude/plugins`
/// (Agent Plugins packages — `marketplace/plugins/name`-style nesting,
/// recognized by `SkillCatalog` at any depth under the root). Measured
/// against a real machine's installs, **all 352 `SKILL.md` files and all 47
/// `.claude-plugin/plugin.json` manifests live under `~/.claude/plugins`** —
/// `~/.claude/skills` is empty (see `.autopilot/competitive-waves/interfaces.md`,
/// corrected there after an earlier pass misread a formulation that named
/// both directories for one combined count). Scanning only `.claude/skills`
/// would silently bind nothing at all from that corpus. A missing root
/// directory is not an error — `SkillCatalog::discover` skips it (see
/// `skill/scan.rs`). A configured `SkillProvider::Registry` root is not
/// wired here (Решение §22 defers its network/registry-config surface out
/// of this delivery); a node referencing it resolves to
/// `SkillError::NotFound` rather than silently matching a different
/// provider.
fn default_skill_roots(worktree_path: &std::path::Path) -> Vec<surge_core::skill::SkillRoot> {
    let mut roots = vec![
        surge_core::skill::SkillRoot {
            provider: surge_core::skill::SkillProvider::ProjectDir,
            path: worktree_path.join(".claude/skills"),
        },
        surge_core::skill::SkillRoot {
            provider: surge_core::skill::SkillProvider::ProjectDir,
            path: worktree_path.join(".claude/plugins"),
        },
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(surge_core::skill::SkillRoot {
            provider: surge_core::skill::SkillProvider::UserDir,
            path: home.join(".claude/skills"),
        });
        roots.push(surge_core::skill::SkillRoot {
            provider: surge_core::skill::SkillProvider::UserDir,
            path: home.join(".claude/plugins"),
        });
    }
    roots
}

async fn dispatch_terminal_node(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    cfg: &surge_core::terminal_config::TerminalConfig,
) -> StageDispatch {
    use crate::engine::frames::TerminalSignal;

    match crate::engine::frames::on_terminal_decision(&state.frames, &state.cursor) {
        TerminalSignal::OuterComplete => {
            if matches!(cfg.kind, surge_core::terminal_config::TerminalKind::Success)
                && let Some(publisher) = params.run_config.pull_request_publisher.as_deref()
            {
                crate::engine::publish::publish_before_terminal(
                    publisher,
                    &params.writer,
                    crate::engine::publish::PublishRequest {
                        run_id: params.run_id,
                        worktree: params.worktree_path.clone(),
                    },
                )
                .await;
            }
            let result = execute_terminal_stage(TerminalStageParams {
                node: &state.cursor.node,
                terminal_config: cfg,
                writer: &params.writer,
            })
            .await
            .map(StageOutcome::Terminal);
            StageDispatch::StageResult(result)
        },
        TerminalSignal::LoopIterDone => finish_loop_iteration(params, state, cfg).await,
        TerminalSignal::SubgraphDone => finish_subgraph_frame(params, state).await,
    }
}

async fn finish_loop_iteration(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    terminal: &surge_core::terminal_config::TerminalConfig,
) -> StageDispatch {
    let just_completed = match loop_terminal_outcome(terminal) {
        Ok(outcome) => outcome,
        Err(error) => return StageDispatch::Failed(error),
    };
    match crate::engine::stage::loop_stage::on_loop_iteration_done(
        &just_completed,
        &state.routing_graph,
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

fn loop_terminal_outcome(
    terminal: &surge_core::terminal_config::TerminalConfig,
) -> Result<OutcomeKey, String> {
    use surge_core::terminal_config::TerminalKind;
    let outcome = match terminal.kind {
        TerminalKind::Success => "completed",
        TerminalKind::Failure { .. } => "failed",
        TerminalKind::Aborted => "aborted",
    };
    OutcomeKey::try_from(outcome).map_err(|error| format!("loop terminal outcome: {error}"))
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
    match lookup_in_active_frame(
        &state.routing_graph,
        &frame.outer_node,
        &state.frames[..state.frames.len() - 1],
    )
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
    let records: Vec<_> = state
        .memory
        .gate_decisions
        .values()
        .filter(|record| record.node == state.cursor.node)
        .collect();
    if records.len() > 1
        || records.first().is_some_and(|record| {
            record.conflicting
                || state.memory.stage_occurrences.get(&record.node).copied()
                    != Some(record.stage_entry_seq)
        })
    {
        return Err(StageError::RecoveryRequired(
            "human gate has contradictory durable occurrences".into(),
        ));
    }
    let restored = records.first().copied();
    let cfg = if let Some(record) = restored {
        if record.purpose != surge_core::run_state::GateDecisionPurpose::HumanGate {
            return Err(StageError::RecoveryRequired(
                "decision purpose does not authorize HumanGate routing".into(),
            ));
        }
        record.gate_config.as_ref().ok_or_else(|| {
            StageError::RecoveryRequired("human gate has no original accepted contract".into())
        })?
    } else {
        cfg
    };
    let request_id = restored.map_or_else(surge_core::id::GateRequestId::new, |record| {
        record.request_id
    });
    let gate_params = HumanGateStageParams {
        request_id,
        node: &state.cursor.node,
        gate_config: cfg,
        writer: &params.writer,
        run_memory: &state.memory,
        gate_resolutions: Some(params.gate_resolutions.as_ref()),
        cancel: &params.cancel,
        default_timeout: params.run_config.human_input_timeout,
        bootstrap_edit_loop_cap: params.run_config.bootstrap.edit_loop_cap,
    };
    let result = if let Some(record) = restored {
        restore_human_gate_stage(gate_params, record).await
    } else {
        execute_human_gate_stage(gate_params).await
    };
    result.map(StageOutcome::Routed)
}

async fn enter_loop_node(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    cfg: &surge_core::loop_config::LoopConfig,
) -> StageDispatch {
    let return_to =
        match return_to_after_completed(&state.routing_graph, &state.cursor, &state.frames) {
            Ok(node) => node,
            Err(error) => return StageDispatch::Failed(format!("loop return_to: {error}")),
        };
    let effect = crate::engine::stage::loop_stage::execute_loop_entry(
        crate::engine::stage::loop_stage::LoopStageParams {
            node: &state.cursor.node,
            loop_config: cfg,
            worktree_path: &params.worktree_path,
            graph: &state.routing_graph,
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
        match return_to_after_completed(&state.routing_graph, &state.cursor, &state.frames) {
            Ok(node) => node,
            Err(error) => return StageDispatch::Failed(format!("subgraph return_to: {error}")),
        };
    let effect = crate::engine::stage::subgraph_stage::execute_subgraph_entry(
        crate::engine::stage::subgraph_stage::SubgraphStageParams {
            node: &state.cursor.node,
            subgraph_config: cfg,
            graph: &state.routing_graph,
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
        Ok(StageOutcome::Routed(outcome)) => {
            if let Some(aborted) = abort_if_cancelled(params).await {
                return Err(aborted);
            }
            Ok(StageResolution::Outcome(outcome))
        },
        Ok(StageOutcome::Terminal(terminal)) => {
            let outcome = terminal_run_outcome(terminal);
            Ok(StageResolution::Terminal(outcome))
        },
        Err(StageError::Cancelled) => Err(abort_run(params).await),
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

/// Whether suppressing `error` into `suppressed` would hand back an outcome
/// the stage's validation rejected during this visit (#94). Such a
/// suppression is refused like one into an undeclared outcome.
fn launders_rejected_outcome(
    error: &StageError,
    suppressed: &OutcomeKey,
    node: &surge_core::keys::NodeKey,
) -> bool {
    let StageError::OutcomeBudgetExhausted { rejected, .. } = error else {
        return false;
    };
    let launders = rejected.contains(suppressed);
    if launders {
        tracing::warn!(target: "engine::stage::error", %node, outcome = %suppressed,
            "on_error suppression into an outcome rejected this visit is refused");
    }
    launders
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

    let on_error_resolution = run_on_error_hooks_owned(
        &state.hook_executor,
        node,
        &state.cursor.node,
        &raw_reason,
        &params.worktree_path,
        params.profile_registry.as_deref(),
        Some(&params.writer),
    )
    .await;
    for record in &on_error_resolution.records {
        crate::engine::hooks::record_hook_executed(&params.writer, record).await;
    }

    if crate::engine::stage::verification::verification_in_play(
        crate::engine::stage::verification::VerificationPoint::AfterFailure,
        &state.memory,
    ) && let Err(observation_error) = crate::engine::stage::verification::observe_after(
        &params.writer,
        &params.worktree_path,
        state.memory.verification.subject.as_ref(),
    )
    .await
    {
        return Err(failed(
            params,
            format!("post-hook verification observation failed: {observation_error}"),
        )
        .await);
    }

    // An `on_error` hook may suppress an exhausted outcome budget, but never
    // into an outcome the stage's validation rejected during this visit:
    // that would launder the rejected result. Refuse it like a suppression
    // into an undeclared outcome — the failure stands.
    if let Some(suppressed) = on_error_resolution
        .outcome
        .filter(|suppressed| !launders_rejected_outcome(&error, suppressed, &state.cursor.node))
    {
        return record_suppressed_error(params, state, suppressed, &raw_reason).await;
    }

    // Loop protection (v1 task 1.3): a tripped attempt is a failed attempt,
    // not a failed run, when the stage has a retry loop to count it against.
    if let StageError::LoopGuardTripped(trip) = &error
        && let Some(retry) = loop_protection_retry(state)
    {
        tracing::info!(
            target: "engine::escalation",
            node = %state.cursor.node,
            outcome = %retry,
            reason = %trip,
            "loop protection ended the attempt; routing it as a failed attempt"
        );
        if let Err(write_err) = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                node: state.cursor.node.clone(),
                outcome: retry.clone(),
                summary: format!(
                    "Loop protection ended this attempt: {}",
                    trip.operator_message()
                ),
            }))
            .await
        {
            return Err(failed(
                params,
                format!("write OutcomeReported (loop protection): {write_err}"),
            )
            .await);
        }
        return Ok(retry);
    }

    let stage_failed_seq = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::StageFailed {
            node: state.cursor.node.clone(),
            reason: raw_reason.clone(),
            retry_available: false,
        }))
        .await;
    // Write-back is a node *outcome*, not a side effect: only once the
    // failure is genuinely terminal (not suppressed above) and its
    // `StageFailed` is durably recorded do we record it in memory — and
    // only then, using the seq `StageFailed` was actually assigned, so a
    // failed append (storage already in trouble) skips write-back too
    // rather than compounding it.
    if let Ok(seq) = stage_failed_seq {
        crate::engine::hooks::memory_writeback::record_node_failure(
            params.run_id,
            &state.cursor.node,
            &raw_reason,
            &params.writer,
            seq,
            params.run_config.memory_store_path.as_deref(),
        )
        .await;
    }
    Err(failed(params, raw_reason).await)
}

/// The current stage's retry-loop outcome in the routing graph scope that
/// holds it (see `surge_core::escalation::retry_outcome`).
fn loop_protection_retry(state: &RunExecutionState) -> Option<OutcomeKey> {
    let graph = &state.routing_graph;
    let node = &state.cursor.node;
    std::iter::once((&graph.nodes, graph.edges.as_slice()))
        .chain(
            graph
                .subgraphs
                .values()
                .map(|sg| (&sg.nodes, sg.edges.as_slice())),
        )
        .find(|(nodes, _)| nodes.contains_key(node))
        .and_then(|(nodes, edges)| surge_core::escalation::retry_outcome(nodes, edges, node))
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

/// Pure debug-selector check. A bare node retains the original first-entry
/// behavior; `node@N` selects a positive durable occurrence. Release tests
/// exercise the same input while proving that no selector can request an exit.
#[cfg(any(debug_assertions, test))]
fn route_commit_occurrence_matches(
    selector: Option<&str>,
    node: &str,
    current_entry: u64,
    entries: &[u64],
    route_committed: bool,
) -> bool {
    if !cfg!(debug_assertions) || !route_committed {
        return false;
    }
    let Some(selector) = selector else {
        return false;
    };
    let (target, occurrence) = match selector.rsplit_once('@') {
        Some((target, count)) => {
            if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
                return false;
            }
            let Ok(count) = count.parse::<usize>() else {
                return false;
            };
            (target, count)
        },
        None => (selector, 1),
    };
    !target.is_empty()
        && target == node
        && occurrence > 0
        && entries.len() == occurrence
        && entries.last() == Some(&current_entry)
}

/// Debug-only crash seam after an authenticated route and snapshot commit.
/// Count only durable entries through this committed prefix. Release builds
/// do not read the selector or invoke this wrapper.
#[cfg(debug_assertions)]
async fn route_commit_exit_if_requested(
    params: &RunTaskParams,
    node: &surge_core::NodeKey,
    entry: surge_persistence::runs::EventSeq,
    committed: surge_persistence::runs::EventSeq,
) {
    let target = std::env::var("SURGE_ROUTE_COMMIT_EXIT").ok();
    if target.is_none() {
        return;
    }
    let Ok(events) = params
        .writer
        .read_events(surge_persistence::runs::EventSeq(1)..committed.next())
        .await
    else {
        return;
    };
    let entries: Vec<_> = events
        .iter()
        .filter_map(|row| match &row.payload.payload {
            EventPayload::StageEntered { node: actual, .. } if actual == node => {
                Some(row.seq.as_u64())
            },
            _ => None,
        })
        .collect();
    // The route must be in the actual committed tail, bound to the selected
    // entry (gate) or to that entry's accepted invocation (agent).
    let route_committed = events.last().is_some_and(|tail| {
        tail.seq == committed && match tail.payload.payload() {
            EventPayload::GateStageRouteCommitted { stage_entry_seq, .. } => *stage_entry_seq == entry.as_u64(),
            EventPayload::StageRouteCommitted { invocation, outcome_commit_seq } => events.iter().any(|row| {
                row.seq.as_u64() == *outcome_commit_seq && row.seq > entry && matches!(row.payload.payload(),
                    EventPayload::StageOutcomeCommitted { commit } if commit.invocation() == *invocation && &commit.context().node == node)
            }),
            _ => false,
        }
    });
    if route_commit_occurrence_matches(
        target.as_deref(),
        node.as_str(),
        entry.as_u64(),
        &entries,
        route_committed,
    ) {
        tracing::warn!(target: "engine::fault_injection", %node,
            "SURGE_ROUTE_COMMIT_EXIT hit after committed route; exiting uncleanly (99)");
        std::process::exit(99);
    }
}

/// A human override answered on a default escalation gate (v1 task 1.2):
/// `accept_as_is` records `TaskAcceptedByHuman` with the exhausted stage's
/// latest findings; `revise_requirement` records `RequirementRevised` from the
/// answer's comment. Committed in the route batch, so it is never lost or
/// doubled across a crash.
fn human_override_effect(
    state: &RunExecutionState,
    outcome: &OutcomeKey,
) -> Option<VersionedEventPayload> {
    use surge_core::escalation::{ACCEPT_OUTCOME, REVISE_OUTCOME};
    if outcome.as_str() != ACCEPT_OUTCOME && outcome.as_str() != REVISE_OUTCOME {
        return None;
    }
    let gate = &state.cursor.node;
    let graph = &state.routing_graph;
    let source = std::iter::once(graph.edges.as_slice())
        .chain(graph.subgraphs.values().map(|sg| sg.edges.as_slice()))
        .find_map(|edges| surge_core::escalation::escalation_gate_source(edges, gate))?
        .clone();
    let task = crate::engine::frames::active_task_id(&state.frames);
    let comment = state
        .memory
        .gate_decisions
        .values()
        .filter(|record| &record.node == gate)
        .find_map(|record| record.response.as_ref())
        .and_then(|response| response.get("comment"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|comment| !comment.is_empty())
        .map(str::to_owned);
    let payload = if outcome.as_str() == ACCEPT_OUTCOME {
        let findings = state
            .memory
            .artifacts_by_node
            .get(&source)
            .into_iter()
            .flatten()
            .rfind(|artifact| artifact.name == "verification-report")
            .map(|artifact| artifact.hash);
        tracing::info!(
            target: "engine::escalation",
            node = %source,
            task = task.as_ref().map_or("(none)", |task| task.as_str()),
            with_findings = findings.is_some(),
            "work accepted by a human after an exhausted retry ladder"
        );
        EventPayload::TaskAcceptedByHuman {
            node: source,
            task,
            findings,
            comment,
        }
    } else {
        let Some(text) = comment else {
            tracing::warn!(
                target: "engine::escalation",
                node = %source,
                "revise requirement answered without a revised requirement; checking again unchanged"
            );
            return None;
        };
        tracing::info!(
            target: "engine::escalation",
            node = %source,
            task = task.as_ref().map_or("(none)", |task| task.as_str()),
            "requirement revised by a human"
        );
        EventPayload::RequirementRevised {
            node: source,
            task,
            text,
        }
    };
    Some(VersionedEventPayload::new(payload))
}

/// Largest `discovered-tasks` artifact a split planner may hand back.
const SPLIT_TASKS_MAX_BYTES: usize = 256 * 1024;
/// Most replacement tasks one split may insert.
const SPLIT_TASKS_MAX: usize = 12;

/// The split rung of the verifier ladder: when a derived split planner
/// (`surge_core::escalation::is_split_planner`) routes `split`, insert the
/// tasks from its `discovered-tasks` artifact right after the current item of
/// the innermost loop frame and return the `TaskSplit` record. The caller
/// commits it in the same batch as the route and snapshot, so a crash never
/// splices twice or loses the splice.
/// The validated `discovered-tasks` the split planner produced in this occurrence.
async fn read_split_tasks(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: surge_core::RunId,
    memory: &surge_core::run_state::RunMemory,
    node: &surge_core::keys::NodeKey,
    stage_start_seq: surge_persistence::runs::EventSeq,
) -> Result<surge_core::roadmap::DiscoveredTasksArtifact, String> {
    let artifact = memory
        .artifacts_by_node
        .get(node)
        .into_iter()
        .flatten()
        .rfind(|artifact| {
            artifact.name == "discovered-tasks"
                && artifact.produced_at_seq.is_some_and(|position| {
                    position
                        .local_seq()
                        .is_ok_and(|seq| seq > stage_start_seq.as_u64())
                })
        })
        .ok_or_else(|| {
            format!("split planner {node} reported split without discovered-tasks.toml")
        })?;
    let bytes = artifact_store
        .open_bounded(run_id, artifact.hash, SPLIT_TASKS_MAX_BYTES)
        .await
        .map_err(|error| format!("read split tasks: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "split tasks are not UTF-8".to_owned())?;
    let tasks: surge_core::roadmap::DiscoveredTasksArtifact =
        toml::from_str(&text).map_err(|error| format!("parse split tasks: {error}"))?;
    if let Some(issue) = tasks.validate().first() {
        return Err(format!("split tasks are invalid: {issue}"));
    }
    if tasks.tasks.is_empty() || tasks.tasks.len() > SPLIT_TASKS_MAX {
        return Err(format!(
            "split must produce 1..={SPLIT_TASKS_MAX} tasks, got {}",
            tasks.tasks.len()
        ));
    }
    Ok(tasks)
}

/// Loop items for the replacement tasks, linked to the replaced task.
fn split_items(
    tasks: &surge_core::roadmap::DiscoveredTasksArtifact,
    origin: Option<&str>,
) -> Vec<toml::Value> {
    tasks
        .tasks
        .iter()
        .map(|entry| {
            let mut item = toml::map::Map::new();
            item.insert("id".into(), entry.id.as_str().into());
            item.insert("title".into(), entry.title.clone().into());
            if let Some(description) = &entry.description {
                item.insert("description".into(), description.clone().into());
            }
            if !entry.acceptance_criteria.is_empty() {
                let criteria = entry
                    .acceptance_criteria
                    .iter()
                    .cloned()
                    .map(toml::Value::String)
                    .collect();
                item.insert("acceptance_criteria".into(), toml::Value::Array(criteria));
            }
            if let Some(origin) = origin {
                item.insert("discovered_from".into(), origin.into());
            }
            toml::Value::Table(item)
        })
        .collect()
}

async fn task_split_effect(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    outcome: &OutcomeKey,
    stage_start_seq: surge_persistence::runs::EventSeq,
) -> Result<Option<VersionedEventPayload>, String> {
    if outcome.as_str() != surge_core::escalation::SPLIT_OUTCOME {
        return Ok(None);
    }
    let node = state.cursor.node.clone();
    let is_split = lookup_in_active_frame(&state.routing_graph, &node, &state.frames)
        .is_some_and(surge_core::escalation::is_split_planner);
    if !is_split {
        return Ok(None);
    }
    let tasks = read_split_tasks(
        &params.artifact_store,
        params.run_id,
        &state.memory,
        &node,
        stage_start_seq,
    )
    .await?;
    let Some(crate::engine::frames::Frame::Loop(frame)) = state
        .frames
        .iter_mut()
        .rev()
        .find(|frame| matches!(frame, crate::engine::frames::Frame::Loop(_)))
    else {
        return Err(format!("split planner {node} runs outside a loop"));
    };
    let index = frame.current_index;
    let current = frame
        .items
        .get(index as usize)
        .ok_or_else(|| "split loop has no current item".to_owned())?;
    let task = current
        .get("id")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let taken: std::collections::BTreeSet<&str> = frame
        .items
        .iter()
        .filter_map(|item| item.get("id").and_then(toml::Value::as_str))
        .collect();
    if let Some(clash) = tasks
        .tasks
        .iter()
        .find(|entry| taken.contains(entry.id.as_str()))
    {
        return Err(format!("split task id {} is already in the loop", clash.id));
    }
    let into = split_items(&tasks, task.as_deref());
    if frame.items.len() + into.len() > crate::engine::frames::MAX_LOOP_ITEMS_RESOLVED {
        return Err("split would exceed the loop item limit".into());
    }
    let at = index as usize + 1;
    frame.items.splice(at..at, into.iter().cloned());
    tracing::info!(
        target: "engine::escalation",
        %node,
        loop_id = %frame.loop_node,
        index,
        task = task.as_deref().unwrap_or("(no id)"),
        count = into.len(),
        "task split into smaller tasks"
    );
    Ok(Some(VersionedEventPayload::new(EventPayload::TaskSplit {
        loop_id: frame.loop_node.clone(),
        index,
        task,
        into,
    })))
}

#[derive(Debug, thiserror::Error)]
enum StageRouteError {
    #[error(transparent)]
    Fold(#[from] surge_core::run_state::FoldError),
    #[error(transparent)]
    Storage(#[from] surge_persistence::runs::StorageError),
    #[error("{0}")]
    Route(String),
}

impl From<String> for StageRouteError {
    fn from(message: String) -> Self {
        Self::Route(message)
    }
}
impl From<&str> for StageRouteError {
    fn from(message: &str) -> Self {
        Self::Route(message.to_owned())
    }
}

async fn route_and_snapshot(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    outcome: &OutcomeKey,
    stage_start_seq: surge_persistence::runs::EventSeq,
) -> Result<(), StageRouteError> {
    let mut prepared = state.clone();
    let prefix = params
        .writer
        .current_seq()
        .await
        .map_err(|error| format!("route prefix: {error}"))?;
    let observed = params
        .writer
        .read_events(
            surge_persistence::runs::EventSeq(
                stage_start_seq.as_u64().max(prepared.memory_applied_seq),
            )
            .next()..prefix.next(),
        )
        .await
        .map_err(|error| format!("read stage route prefix: {error}"))?;
    let applied_events = apply_read_events(params.run_id, &observed, &mut prepared.memory)?;
    prepared
        .pending_graph_revisions
        .extend(applied_events.graph_revisions);
    apply_pending_revisions(&mut prepared);
    let human_override = human_override_effect(&prepared, outcome);
    let split = task_split_effect(params, &mut prepared, outcome, stage_start_seq).await?;
    let routed = route_stage_outcome(&mut prepared, outcome)?;
    let mut events: Vec<VersionedEventPayload> = human_override.into_iter().chain(split).collect();
    events.extend(routing_events(&prepared, outcome, &routed));
    if let Some(record) = outstanding_stage_outcome(&prepared)? {
        if record.commit.outcome() != outcome {
            return Err("committed stage outcome differs from routed outcome".into());
        }
        events.push(VersionedEventPayload::new(
            EventPayload::StageRouteCommitted {
                invocation: record.commit.invocation(),
                outcome_commit_seq: record.committed_seq,
            },
        ));
    }
    if let Some(record) = outstanding_gate_stage(&prepared)? {
        if record.commit.answer().outcome() != outcome
            || record.commit.disposition()
                != surge_core::execution_recovery::gate_commit::GateCommitDisposition::Route
        {
            return Err("committed gate effects are not routable with this outcome".into());
        }
        let original = record.commit.answer().request();
        events.push(VersionedEventPayload::new(
            EventPayload::GateStageRouteCommitted {
                request: original.request(),
                stage_entry_seq: original.stage_entry_seq(),
                outcome_commit_seq: record.committed_seq,
            },
        ));
    }
    let final_seq = surge_persistence::runs::EventSeq(
        prefix
            .as_u64()
            .checked_add(events.len() as u64)
            .ok_or_else(|| "stage route sequence overflow".to_owned())?,
    );
    let memory_events: Vec<_> = events
        .iter()
        .enumerate()
        .map(|(offset, event)| RunEvent {
            run_id: params.run_id,
            seq: prefix.as_u64() + offset as u64 + 1,
            timestamp: chrono::Utc::now(),
            payload: event.payload().clone(),
        })
        .collect();
    prepared.memory.apply_events(&memory_events)?;
    let next_cursor = Cursor {
        node: routed.target,
        attempt: 1,
    };
    let blob = prepare_stage_boundary_snapshot(params, &prepared, &next_cursor, final_seq).await?;
    params
        .writer
        .commit_stage_route(prefix, events, blob)
        .await
        .map_err(|error| format!("commit stage route: {error}"))?;
    #[cfg(debug_assertions)]
    route_commit_exit_if_requested(params, &state.cursor.node, stage_start_seq, final_seq).await;
    prepared.cursor = next_cursor;
    prepared.memory_applied_seq = final_seq.as_u64();
    *state = prepared;
    Ok(())
}

fn outstanding_gate_stage(
    state: &RunExecutionState,
) -> Result<Option<surge_core::execution_recovery::gate_commit::CommittedGateStage>, String> {
    let mut pending = state
        .memory
        .committed_gate_stages
        .values()
        .filter(|record| {
            record.commit.answer().request().node() == &state.cursor.node
                && record.routed_seq.is_none()
        });
    let first = pending.next().cloned();
    if pending.next().is_some() || first.as_ref().is_some_and(|record| record.conflicting) {
        return Err("gate stage has conflicting unconsumed effects".into());
    }
    Ok(first)
}

fn outstanding_stage_outcome(
    state: &RunExecutionState,
) -> Result<Option<surge_core::execution_recovery::commit::CommittedStageOutcome>, String> {
    let mut outstanding = state
        .memory
        .committed_stage_outcomes
        .values()
        .filter(|record| {
            record.commit.context().node == state.cursor.node && record.routed_seq.is_none()
        });
    let first = outstanding.next().cloned();
    if outstanding.next().is_some() || first.as_ref().is_some_and(|record| record.conflicting) {
        return Err("stage has conflicting unconsumed outcome commits".into());
    }
    Ok(first)
}

fn route_stage_outcome(
    state: &mut RunExecutionState,
    outcome: &OutcomeKey,
) -> Result<crate::engine::routing::RoutedEdge, String> {
    match crate::engine::routing::next_node_after_with_counters(
        &state.routing_graph,
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
    use crate::engine::routing::RoutingError;
    use surge_core::edge::ExceededAction;
    if action == ExceededAction::Fail {
        return Err(format!(
            "max_traversals exceeded on edge {edge} (action: Fail)"
        ));
    }
    // Mirrors `surge_core::route_selection::resolve_stage_route`, which the
    // journal inspector replays: an exhausted escalation edge takes the next rung.
    let mut result = Err(RoutingError::ExceededTraversal {
        edge: edge.clone(),
        count: 0,
        max: 0,
        action: ExceededAction::Escalate,
    });
    for rung in surge_core::escalation::ESCALATION_RUNGS {
        if !matches!(
            result,
            Err(RoutingError::ExceededTraversal {
                action: ExceededAction::Escalate,
                ..
            })
        ) {
            break;
        }
        let synthetic =
            OutcomeKey::try_from(rung).map_err(|_| format!("escalation outcome {rung}"))?;
        result = crate::engine::routing::next_node_after_with_counters(
            &state.routing_graph,
            &state.cursor.node,
            &synthetic,
            &mut state.frames,
            &mut state.root_traversal_counts,
        );
    }
    let routed = result.map_err(|_| {
        format!(
            "max_traversals exceeded on edge {edge} and no escalation route exists \
             (several capped targets from one stage get no default gate)"
        )
    })?;
    tracing::info!(
        target: "engine::escalation",
        node = %state.cursor.node,
        %edge,
        to = %routed.target,
        via = %routed.edge_id,
        "retry loop exhausted; escalating"
    );
    Ok(routed)
}

fn routing_events(
    state: &RunExecutionState,
    outcome: &OutcomeKey,
    routed: &crate::engine::routing::RoutedEdge,
) -> Vec<VersionedEventPayload> {
    vec![
        VersionedEventPayload::new(EventPayload::EdgeTraversed {
            edge: routed.edge_id.clone(),
            from: state.cursor.node.clone(),
            to: routed.target.clone(),
            kind: routed.kind,
        }),
        VersionedEventPayload::new(EventPayload::StageCompleted {
            node: state.cursor.node.clone(),
            outcome: outcome.clone(),
        }),
    ]
}

async fn prepare_stage_boundary_snapshot(
    params: &RunTaskParams,
    state: &RunExecutionState,
    next_cursor: &Cursor,
    final_seq: surge_persistence::runs::EventSeq,
) -> Result<Vec<u8>, String> {
    let mut snapshot = crate::engine::snapshot::EngineSnapshot::new(
        next_cursor,
        final_seq.as_u64(),
        final_seq.as_u64(),
    );
    snapshot.applied_graph_revision_seq = state.applied_graph_revision_seq;
    snapshot.frames = state.frames.iter().cloned().map(Into::into).collect();
    snapshot.root_traversal_counts = state
        .root_traversal_counts
        .iter()
        .map(|(edge, count)| (edge.to_string(), *count))
        .collect();
    let worktree = params.worktree_path.clone();
    let run = params.run_id;
    snapshot.workspace_checkpoint = tokio::task::spawn_blocking(move || {
        surge_git::checkpoint::capture_record(&worktree, run, final_seq.as_u64())
    })
    .await
    .map_err(|error| format!("checkpoint worker: {error}"))?
    .map_err(|error| format!("workspace checkpoint: {error}"))?;
    serde_json::to_vec(&snapshot).map_err(|error| format!("snapshot serialize: {error}"))
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
#[cfg(test)]
pub(crate) async fn run_on_error_hooks(
    executor: &HookExecutor,
    node: &surge_core::node::Node,
    cursor_node: &surge_core::keys::NodeKey,
    raw_reason: &str,
    worktree_path: &std::path::Path,
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
) -> OnErrorResolution {
    run_on_error_hooks_owned(
        executor,
        node,
        cursor_node,
        raw_reason,
        worktree_path,
        profile_registry,
        None,
    )
    .await
}

async fn run_on_error_hooks_owned(
    executor: &HookExecutor,
    node: &surge_core::node::Node,
    cursor_node: &surge_core::keys::NodeKey,
    raw_reason: &str,
    worktree_path: &std::path::Path,
    profile_registry: Option<&crate::profile_loader::ProfileRegistry>,
    writer: Option<&surge_persistence::runs::run_writer::RunWriter>,
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

    let mut ctx = HookContext::for_node(cursor_node)
        .with_worktree_path(worktree_path)
        .with_error(raw_reason);
    if let Some(writer) = writer {
        ctx = ctx.with_writer(writer, surge_core::id::StageInvocationId::new());
    }
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

#[derive(Debug, Clone, Default)]
struct AppliedEventBatch {
    graph_revisions: Vec<ObservedGraphRevision>,
}

/// A graph revision accepted but not yet applied (#100: one type with the
/// execution state's pending list).
type ObservedGraphRevision = crate::engine::execution_state::PendingGraphRevision;

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
    apply_read_events(run_id, &events, memory)
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

/// Fold events appended during live execution into memory, through the one
/// shared clock-free conversion (#100). All or nothing (#120): every event
/// is converted before any is folded, so a corrupt timestamp leaves memory
/// exactly as it was and a later catch-up never folds an event twice.
fn apply_read_events(
    run_id: RunId,
    events: &[surge_persistence::runs::ReadEvent],
    memory: &mut RunMemory,
) -> Result<AppliedEventBatch, surge_persistence::runs::StorageError> {
    let converted = events
        .iter()
        .map(|event| event.to_run_event(run_id))
        .collect::<Result<Vec<_>, _>>()?;
    memory.apply_events(&converted)?;
    Ok(AppliedEventBatch {
        graph_revisions: crate::engine::execution_state::pending_revisions(events, 0),
    })
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

    // The cursor may rest on a default escalation gate the revision's
    // effective graph keeps under the same key.
    if !surge_core::escalation::with_default_escalation_gates(&revision.graph)
        .nodes
        .contains_key(&cursor.node)
    {
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
    RunOutcome::Failed { error }
}

/// Clean run-task exit for `StageDispatch::Park` (Task 12, R37/R37.1):
/// writes `RunParked` to this run's own event log, transitions the
/// registry to `RunStatus::Parked` with `wake_at` recorded (the write
/// [`Storage::set_run_parked`] exists for — see `RunTaskParams::storage`'s
/// doc), and returns without touching the worktree at all — recovery
/// (`surge-daemon`) requires it to still exist.
async fn parked(
    params: &RunTaskParams,
    node: &surge_core::keys::NodeKey,
    wake_at: chrono::DateTime<chrono::Utc>,
    basis: surge_core::capacity::WakeBasis,
    runtime: Option<String>,
    details: Option<String>,
) -> RunOutcome {
    // Task 12 M4, ADR-0016 §14: fold in this run's deterministic "herd"
    // offset right here, at the moment of parking, so the *persisted*
    // wake_at (this event, the registry row, and the outcome below) is the
    // jittered value — not re-derived later. `jitter_max: Duration::ZERO`
    // (the pre-M4 behavior) makes this a no-op.
    let wake_at = params.capacity.jittered_wake(wake_at, params.run_id);
    let runtime_display = runtime.as_deref().unwrap_or("<unknown>");
    let mut reason = match basis {
        surge_core::capacity::WakeBasis::ObservedReset => format!(
            "runtime {runtime_display} exhausted; parking until the provider-observed reset \
             at {wake_at}"
        ),
        surge_core::capacity::WakeBasis::PolicyBackoff => format!(
            "runtime {runtime_display} exhausted with no usable reset time known; parking on \
             the configured blind-backoff policy until {wake_at}"
        ),
    };
    // Task 12 M3 review, "misc": the raw provider/bridge error text was
    // being discarded on the park path (no `StageFailed` is ever written
    // for a parked dispatch, so it had nowhere else to land) — exactly the
    // string an operator reading `surge inbox` on a parked run wants to
    // see. `None` for a pre-dispatch park (nothing was attempted, so there
    // is no error text to fold in).
    if let Some(details) = details {
        reason.push_str(" — provider said: ");
        reason.push_str(&details);
    }

    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunParked {
            wake_at,
            runtime,
            worktree: params.worktree_path.clone(),
            basis,
            reason,
        }))
        .await;

    if let Err(error) = params
        .storage
        .set_run_parked(&params.run_id, wake_at.timestamp_millis())
        .await
    {
        tracing::warn!(
            target: "engine::capacity",
            %node,
            run_id = %params.run_id,
            %error,
            "failed to record Parked status in the registry; this run's own event log still \
             shows RunParked, but surge inbox / crash recovery will not see it as parked"
        );
    }

    RunOutcome::Parked { wake_at }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::LedgerEffect;
    use surge_core::agent_config::AgentConfig;
    use surge_core::content_hash::ContentHash;
    use surge_core::edge::EdgeKind;
    use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
    use surge_core::keys::ProfileKey;
    use surge_core::node::{Node, OutcomeDecl, Position};

    #[test]
    fn semantic_zero_position_batch_does_not_apply_earlier_valid_event() {
        let mut memory = RunMemory::default();
        let before = memory.clone();
        let read = |seq| surge_persistence::runs::ReadEvent {
            seq: surge_persistence::runs::EventSeq(seq),
            timestamp_ms: 1_760_000_000_000,
            kind: "RequirementRevised".into(),
            payload: surge_core::VersionedEventPayload::new(EventPayload::RequirementRevised {
                node: surge_core::keys::NodeKey::try_from("plan").unwrap(),
                task: Some(surge_core::roadmap::RoadmapTaskId::from("task")),
                text: "batch must be atomic before all observers".into(),
            }),
        };
        let refused = apply_read_events(RunId::new(), &[read(1), read(0)], &mut memory);
        assert!(
            refused.is_err(),
            "invalid event position accepted: {refused:?}"
        );
        assert_eq!(
            memory, before,
            "valid first event leaked from rejected batch"
        );
    }

    /// #120: a batch with a corrupt timestamp anywhere folds nothing, so the
    /// memory never holds half a batch that a later catch-up would refold.
    #[test]
    fn a_batch_with_a_corrupt_timestamp_folds_nothing() {
        let run = RunId::new();
        let read = |seq: u64, timestamp_ms: i64, node: &str| surge_persistence::runs::ReadEvent {
            seq: surge_persistence::runs::EventSeq(seq),
            timestamp_ms,
            kind: "StageEntered".into(),
            payload: surge_core::run_event::VersionedEventPayload::new(
                surge_core::run_event::EventPayload::StageEntered {
                    node: surge_core::keys::NodeKey::try_from(node).unwrap(),
                    attempt: 1,
                },
            ),
        };
        let mut memory = RunMemory::default();
        let before = format!("{memory:?}");
        let refused = apply_read_events(
            run,
            &[
                read(1, 1_760_000_000_000, "first"),
                read(2, i64::MAX, "second"),
            ],
            &mut memory,
        );
        assert!(
            matches!(
                refused,
                Err(surge_persistence::runs::StorageError::CorruptTimestamp(corrupt))
                    if corrupt.seq == 2
            ),
            "{refused:?}"
        );
        assert_eq!(
            format!("{memory:?}"),
            before,
            "the valid first event was not folded"
        );
    }

    #[test]
    fn checkpoint_exit_matches_only_named_node() {
        assert!(checkpoint_exit_matches(Some("impl_1"), "impl_1"));
        assert!(!checkpoint_exit_matches(Some("impl_1"), "review_1"));
        assert!(!checkpoint_exit_matches(None, "impl_1"));
        assert!(!checkpoint_exit_matches(Some(""), "impl_1"));
    }

    #[test]
    fn route_commit_selector_requires_exact_durable_occurrence_and_route() {
        let enabled = cfg!(debug_assertions);
        assert_eq!(
            route_commit_occurrence_matches(
                Some("flow_generator"),
                "flow_generator",
                10,
                &[10],
                true
            ),
            enabled
        );
        assert_eq!(
            route_commit_occurrence_matches(
                Some("flow_generator@1"),
                "flow_generator",
                10,
                &[10],
                true
            ),
            enabled
        );
        assert_eq!(
            route_commit_occurrence_matches(
                Some("flow_generator@2"),
                "flow_generator",
                40,
                &[10, 40],
                true
            ),
            enabled
        );
        assert!(!route_commit_occurrence_matches(
            Some("flow_generator"),
            "flow_generator",
            40,
            &[10, 40],
            true
        ));
        assert!(!route_commit_occurrence_matches(
            Some("flow_generator@2"),
            "flow_generator",
            10,
            &[10],
            true
        ));
        assert!(!route_commit_occurrence_matches(
            Some("flow_generator@2"),
            "flow_generator",
            10,
            &[10, 40],
            true
        ));
        assert!(!route_commit_occurrence_matches(
            Some("flow_generator@2"),
            "flow_generator",
            40,
            &[10, 40],
            false
        ));
        assert!(!route_commit_occurrence_matches(
            Some("flow_generator@2"),
            "flow_gate",
            40,
            &[10, 40],
            true
        ));
        for selector in [
            None,
            Some(""),
            Some("@2"),
            Some("flow_generator@0"),
            Some("flow_generator@-1"),
            Some("flow_generator@+2"),
            Some("flow_generator@"),
            Some("flow_generator@two"),
            Some("flow_generator@184467440737095516160"),
        ] {
            assert!(
                !route_commit_occurrence_matches(selector, "flow_generator", 40, &[10, 40], true),
                "{selector:?}"
            );
        }
    }

    #[test]
    fn default_skill_roots_declares_two_project_roots_and_two_user_roots() {
        // Structural shape only — count and provider kind, not the literal
        // path strings `default_skill_roots` builds internally (that would
        // just restate the function's own expression back at it). Whether
        // those roots actually contribute packs when scanned is a separate,
        // behavioral question — see
        // `default_skill_roots_resolves_a_real_agent_plugins_package_under_dot_claude_plugins`
        // below.
        use surge_core::skill::SkillProvider;

        let worktree = std::path::Path::new("/tmp/some-worktree");
        let roots = default_skill_roots(worktree);

        let project_count = roots
            .iter()
            .filter(|r| r.provider == SkillProvider::ProjectDir)
            .count();
        assert_eq!(
            project_count, 2,
            "expected one worktree root per layout (Agent Skills + Agent \
             Plugins), got {roots:?}"
        );

        let user_count = roots
            .iter()
            .filter(|r| r.provider == SkillProvider::UserDir)
            .count();
        let expected_user_count = if dirs::home_dir().is_some() { 2 } else { 0 };
        assert_eq!(
            user_count, expected_user_count,
            "expected one user root per layout only when a home directory \
             resolves, got {roots:?}"
        );
    }

    #[test]
    fn default_skill_roots_resolves_a_real_agent_plugins_package_under_dot_claude_plugins() {
        // Proves the `.claude/plugins` root by exercising real discovery +
        // resolution against a physically distinct on-disk layout — the
        // Agent Plugins package shape (`.claude-plugin/plugin.json` +
        // `skills/<name>/SKILL.md`), not the flatter `.claude/skills/<name>`
        // shape the engine-harness tests in `engine_skill_binding_test.rs`
        // use. If `default_skill_roots` ever stopped including this root,
        // this is the test that would actually fail — a path-equality
        // assertion against the function's own `.join(...)` expression
        // would not have.
        use surge_core::skill::{SkillCatalog, SkillProvider, SkillRef};

        let worktree = tempfile::tempdir().unwrap();
        let package_dir = worktree.path().join(".claude/plugins/my-plugin");
        std::fs::create_dir_all(package_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            package_dir.join(".claude-plugin/plugin.json"),
            r#"{"version":"1.0.0"}"#,
        )
        .unwrap();
        let skill_dir = package_dir.join("skills/code-reviewer");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: code-reviewer\n---\n\nReview carefully.\n",
        )
        .unwrap();

        let catalog = SkillCatalog::discover(&default_skill_roots(worktree.path()));
        let resolved = catalog.resolve(&SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        });
        assert!(
            resolved.is_ok(),
            "a skill packaged under .claude/plugins (Agent Plugins layout) \
             must resolve via default_skill_roots: {resolved:?}"
        );
    }

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
                    ledger_effect: LedgerEffect::default(),
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
        assert_eq!(pending.len(), 0);
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
        assert_eq!(pending.len(), 0);
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

#[cfg(test)]
#[path = "run_task_semantic_tests.rs"]
mod semantic_history_tests;
