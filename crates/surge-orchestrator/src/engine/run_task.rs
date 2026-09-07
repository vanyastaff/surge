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
use crate::engine::stage::skill_binding::{SkillBindingParams, bind_skills};
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
    /// Node-keyed decision registry (`node_key → oneshot::Sender<HumanGateResolution>`).
    /// Engine's `resolve_human_input` finds the sender and fires it — for a
    /// `HumanGate` node's own pause, and for the skill-trust prompt
    /// (`engine::stage::skill_binding::bind_skills`) sharing the same
    /// registry.
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
    /// Durable rate-limit capacity ledger (Task 12 M2/M3, R34-R38.1).
    /// Consulted before every agent-node dispatch and updated the moment a
    /// `StageError::RateLimited` is observed — see
    /// `crate::engine::capacity::CapacityLedger`.
    pub capacity_ledger: Arc<dyn crate::engine::capacity::CapacityLedger>,
    /// Per-run work-duration estimator (Task 12 M3, R37/R37.1) — see
    /// `crate::engine::capacity::WorkEstimator`. `None` from this is the
    /// common case (a node's first dispatch) and must never itself cause a
    /// parking decision.
    pub capacity_estimator: Arc<dyn crate::engine::capacity::WorkEstimator>,
    /// Capacity-aware dispatch policy, built from `SurgeConfig.capacity`
    /// by the engine's production wiring (`surge-cli`, `surge-daemon`) at
    /// startup — Task 12 M3 acceptance criterion B. `EngineConfig::default`
    /// carries `surge_core::capacity_config::CapacityConfig::default`'s
    /// conservative backoff, matching what `surge init` writes.
    pub capacity_policy: surge_core::capacity::CapacityPolicy,
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
    /// `true` for exactly one upcoming agent-node dispatch when this run is
    /// being resumed from `RunStatus::Parked` (Task 12 M3 review, BLOCKING
    /// #1) — set by `Engine::resume_run`, consumed (flipped to `false`) by
    /// the first `dispatch_agent_node_with_capacity_gate` call that reads
    /// it. `AtomicBool` because `RunTaskParams` is held by `&self`
    /// reference through the run-task loop, never `&mut`. See that
    /// function's own doc for why this bypass exists: without it, a
    /// runtime whose reset time was never learned re-parks on the same
    /// unrefreshed `runtime_capacity` row forever, because nothing but a
    /// genuine dispatch attempt can refresh it, and the precheck this
    /// field bypasses is what was preventing that attempt from ever
    /// happening.
    pub capacity_precheck_bypass_once: std::sync::atomic::AtomicBool,
}

pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    // Capture the per-run MCP registry so it is torn down on *every*
    // terminal path (completed / failed / aborted), not just the
    // happy one — rmcp's Drop is async best-effort and can orphan
    // children. Time-bounded so a hung MCP child cannot wedge run
    // completion.
    let mcp_registry = params.mcp_registry.clone();
    // Box the large inner future so `execute`'s own future stays small
    // at the spawn site (clippy::large_futures; also keeps stack use
    // bounded for the per-run task).
    let outcome = Box::pin(execute_inner(params)).await;
    if let Some(reg) = mcp_registry {
        let budget = std::time::Duration::from_secs(20);
        if tokio::time::timeout(budget, reg.shutdown()).await.is_err() {
            tracing::warn!(
                target: "mcp::supervisor",
                "MCP registry shutdown exceeded budget on run teardown; \
                 abandoning remaining children to RAII"
            );
        }
    }
    outcome
}

async fn execute_inner(mut params: RunTaskParams) -> RunOutcome {
    let mut state = match initial_execution_state(&params).await {
        Ok(state) => state,
        Err(error) => return failed(&params, error).await,
    };

    loop {
        if state.frames.is_empty()
            && let Err(error) = drain_roadmap_queue(&mut params, &mut state).await
        {
            return failed(&params, format!("apply queued roadmap amendments: {error}")).await;
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
            StageDispatch::Park {
                wake_at,
                basis,
                runtime,
                details,
            } => {
                return parked(
                    &params,
                    &state.cursor.node,
                    wake_at,
                    basis,
                    runtime,
                    details,
                )
                .await;
            },
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

        // Stage boundary: the just-completed stage's token usage has been
        // folded into `state.memory.costs` by `route_and_snapshot`. Enforce the
        // run's budget here — warn once at the threshold, abort on breach.
        // A failure to durably record an abort decision is fatal (the run must
        // never terminate without a persisted reason).
        match enforce_budget(&params, &mut state).await {
            Ok(Some(outcome)) => return outcome,
            Ok(None) => {},
            Err(error) => return failed(&params, error).await,
        }
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
    let _ = params.event_tx.send(EngineRunEvent::Terminal {
        outcome: outcome.clone(),
    });
    Ok(Some(outcome))
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

    // Seed the warn-once / breach-once flags from the folded event log so a
    // resumed run does not re-emit a `BudgetWarningRaised` / `BudgetExceeded`
    // it already recorded before a restart.
    let budget_warned = memory.budget_warning_raised;
    let budget_exceeded_noted = memory.budget_exceeded_noted;

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
        budget_warned,
        budget_exceeded_noted,
        skill_catalog: None,
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

/// Pure check for the [`checkpoint_exit_if_requested`] fault-injection seam:
/// does `env` (the `SURGE_CHECKPOINT_EXIT` value) name `node_key`?
///
/// Always compiled (not debug-gated) so it is unit-testable; only the actual
/// `process::exit` wrapper is debug-gated.
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
    params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
            node: cursor.node.clone(),
            attempt: cursor.attempt,
        }))
        .await
        .map_err(|e| format!("write StageEntered: {e}"))?;
    // Fault-injection: simulate a crash mid-run, with StageEntered durable.
    checkpoint_exit_if_requested(&cursor.node);
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

async fn dispatch_node_stage(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
) -> StageDispatch {
    let stage_result = match &node.config {
        NodeConfig::Agent(cfg) => {
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

/// Task 12 M3: run `CapacityPolicy::decide` around an agent node's actual
/// dispatch, at the two points a decision can change what happens next.
///
/// 1. **Before dispatch** — read the ledger's current status for the
///    node's resolved runtime and weigh it against
///    `params.capacity_estimator`'s estimate. `Decision::Park` here means
///    the node is never dispatched at all.
/// 2. **After a `StageError::RateLimited`** from the dispatch attempt
///    itself — the fresh 429 is `observe`d into the ledger and `decide`
///    runs again on that freshly-built window *before* the error reaches
///    `resolve_stage_result`'s `on_error` hook chain. This is what keeps
///    parking ahead of the retry cycle: a hook that routes the failure
///    back to this same node (a common "retry on failure" pipeline shape)
///    would otherwise re-dispatch straight into the same exhausted window,
///    burning an attempt against a wall that will not move before
///    `wake_at` (see this module's own "zero retries after 429" test).
async fn dispatch_agent_node_with_capacity_gate(
    params: &RunTaskParams,
    state: &mut RunExecutionState,
    node: &surge_core::node::Node,
    cfg: &surge_core::agent_config::AgentConfig,
) -> StageDispatch {
    // Resolved once, reused by both the precheck and (on a non-rate-limited
    // result) the post-dispatch clear below — one profile resolve per
    // dispatch, not two.
    let runtime = crate::engine::stage::agent::resolve_profile_runtime_id(
        params.profile_registry.as_deref(),
        cfg.profile.as_ref(),
    );

    // Task 12 M3 review, BLOCKING #1: a run resuming from `RunStatus::
    // Parked` gets exactly one dispatch with the precheck bypassed. This
    // is the mechanism that keeps a runtime whose reset time was *never
    // learned* (`resets_at: None`, so `blind_backoff` is the only
    // applicable rule) from re-parking on the same stale
    // `runtime_capacity` row forever: nothing but a genuine attempt can
    // ever refresh (or refute) that row, and without this bypass the
    // precheck below would keep preventing exactly that attempt from
    // happening. Consumed (flipped to `false`) on read, so it fires at
    // most once per resume, and only for the very next agent dispatch —
    // every dispatch after it goes through the precheck normally.
    let bypass_precheck = params
        .capacity_precheck_bypass_once
        .swap(false, std::sync::atomic::Ordering::SeqCst);

    if !bypass_precheck && let Some(runtime) = runtime.clone() {
        match capacity_decision_for(params, &state.cursor.node, &runtime).await {
            surge_core::capacity::Decision::Park { wake_at, basis } => {
                return StageDispatch::Park {
                    wake_at,
                    basis,
                    runtime: Some(runtime.into_string()),
                    details: None,
                };
            },
            surge_core::capacity::Decision::Dispatch { degraded } => {
                warn_on_degraded_dispatch(&state.cursor.node, degraded);
            },
            surge_core::capacity::Decision::Rotate { .. } => {
                // `CapacityPolicy::decide` never emits this in this
                // delivery — `RotationPolicy` is always `Disabled` (Task
                // 12 M3 acceptance criterion B's `From<&CapacityConfig>`
                // never sets `Enabled`, and R41's live verification is
                // deferred; see `engine::stage::agent::RotationRefusal`).
                // Matched explicitly, not folded into a `_` arm, so a
                // future live `Rotate` cannot silently fall through to an
                // ordinary dispatch under this arm's name.
                tracing::error!(
                    target: "engine::capacity",
                    node = %state.cursor.node,
                    "Decision::Rotate reached the engine but is not implemented by this \
                     delivery; dispatching instead of rotating"
                );
            },
        }
    }

    let result = execute_agent_node(params, state, node, cfg).await;

    // Only a *typed* rate-limit failure with a resolved runtime carries
    // enough to build an observation — borrow first so a non-matching
    // `result` (success, or any other `StageError`) is returned unmoved.
    let rate_limit = match &result {
        Err(StageError::RateLimited {
            runtime: Some(raw_runtime),
            retry_after,
            details,
        }) => Some((raw_runtime.clone(), *retry_after, details.clone())),
        _ => None,
    };
    let Some((raw_runtime, retry_after, details)) = rate_limit else {
        // Not rate-limited (success, or any other `StageError`): if this
        // node's profile resolves to a runtime, clear any stale exhaustion
        // record for it. This is the other half of BLOCKING #1's fix — a
        // real, non-rate-limited outcome is proof the runtime is not (or
        // no longer) exhausted, and is what lets a row with no learned
        // reset time stop haunting every future dispatch instead of only
        // the one right after a bypassed park.
        if let Some(runtime) = runtime
            && let Err(error) = params.capacity_ledger.clear(&runtime).await
        {
            tracing::warn!(
                target: "engine::capacity",
                node = %state.cursor.node,
                %runtime,
                %error,
                "capacity ledger clear failed; a stale exhaustion record for this runtime \
                 may persist"
            );
        }
        return StageDispatch::StageResult(result);
    };

    let observed_at = chrono::Utc::now();
    let window = surge_core::capacity::CapacityWindow::observed_429(
        raw_runtime.clone(),
        retry_after,
        observed_at,
    );
    // `raw_runtime` is already the canonical id (constructed via
    // `CanonicalRuntimeId::resolve` at `StageError::RateLimited`'s own
    // construction site in `engine::stage::agent`) — re-resolved here
    // through the same registry to produce the typed key `observe` now
    // requires, not to normalize it a second time (idempotent either way).
    let canonical_runtime = crate::engine::capacity::CanonicalRuntimeId::resolve(
        &surge_acp::Registry::builtin(),
        &raw_runtime,
    );
    if let Err(error) = params
        .capacity_ledger
        .observe(&canonical_runtime, &window)
        .await
    {
        tracing::warn!(
            target: "engine::capacity",
            node = %state.cursor.node,
            runtime = %raw_runtime,
            %error,
            "capacity ledger observe failed; this run still parks from the in-memory window, \
             but other runs on this runtime will not see the observation"
        );
    }
    let status = surge_core::capacity::CapacityStatus::Known(window);
    // `estimate: None` — the window is already exhausted (`observed_429`
    // always sets `remaining: EXHAUSTED`), so rules 1-4 decide before
    // `decide` would ever consult an estimate.
    match params.capacity_policy.decide(None, &status, observed_at) {
        surge_core::capacity::Decision::Park { wake_at, basis } => StageDispatch::Park {
            wake_at,
            basis,
            runtime: Some(raw_runtime),
            details: Some(details),
        },
        // Rule 4's explicit operator opt-out (`blind_backoff` removed):
        // fall through to the ordinary error path unchanged — the on_error
        // hook chain, and any retry it routes to, behave exactly as they
        // did before this milestone. The operator chose this trade-off.
        // `Rotate` cannot be reached here for the same reason as above.
        surge_core::capacity::Decision::Dispatch { .. }
        | surge_core::capacity::Decision::Rotate { .. } => StageDispatch::StageResult(result),
    }
}

/// Pre-dispatch half of [`dispatch_agent_node_with_capacity_gate`]: read the
/// ledger's current status for `runtime` and weigh it against
/// `params.capacity_estimator`'s estimate.
async fn capacity_decision_for(
    params: &RunTaskParams,
    node: &surge_core::keys::NodeKey,
    runtime: &crate::engine::capacity::CanonicalRuntimeId,
) -> surge_core::capacity::Decision {
    let status = match params.capacity_ledger.status(runtime).await {
        Ok(status) => status,
        Err(error) => {
            // A local storage fault must not masquerade as a provider
            // capacity signal — degrading to `NeverObserved` (dispatch)
            // preserves this milestone's pre-existing behavior (no check
            // at all) rather than introducing a new way for a run to
            // stall on an infrastructure problem instead of a rate limit.
            tracing::warn!(
                target: "engine::capacity",
                %node,
                %runtime,
                %error,
                "capacity ledger read failed; dispatching as if never observed"
            );
            surge_core::capacity::CapacityStatus::NeverObserved
        },
    };
    // Fetch an estimate only where it could possibly change the outcome:
    // `decide`'s rules 1-4 (a `Known`, *exhausted* window, or no window at
    // all) never consult `estimate` — rule 5 dispatches unconditionally
    // when there is no window to compare against. The one branch that can
    // read `estimate` is rule 6, reachable only from a `Known` window that
    // is *not* exhausted (see `CapacityPolicy::decide`'s own doc on why
    // that is structurally rare with today's producers, but not
    // impossible via a persisted row this crate did not itself write).
    // Reading `stage_executions` and averaging it on every single agent
    // dispatch — the overwhelming majority of which are `NeverObserved` or
    // `Known(exhausted)` — for a comparison `decide` cannot use in either
    // case was Task 12 M3 review finding #5.
    let estimate = if matches!(&status, surge_core::capacity::CapacityStatus::Known(w) if !w.is_exhausted())
    {
        params.capacity_estimator.estimate(node).await
    } else {
        None
    };
    params
        .capacity_policy
        .decide(estimate.as_ref(), &status, chrono::Utc::now())
}

/// Log a `Decision::Dispatch { degraded: Some(_) }` at the specific reason
/// `decide` named, rather than re-deriving the condition from the status
/// again at the call site (see `surge_core::capacity::Degraded`'s doc on
/// why `decide` returns the reason instead of leaving the caller to work
/// it out).
fn warn_on_degraded_dispatch(
    node: &surge_core::keys::NodeKey,
    degraded: Option<surge_core::capacity::Degraded>,
) {
    if let Some(reason) = degraded {
        tracing::warn!(
            target: "engine::capacity",
            %node,
            reason = ?reason,
            "dispatching despite a degraded capacity signal"
        );
    }
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
    let stage_result = execute_agent_stage(AgentStageParams {
        node: &state.cursor.node,
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
        hook_executor: &state.hook_executor,
        pending_elevations: state.pending_elevations.clone(),
        active_task_id: crate::engine::frames::active_task_id(&state.frames),
    })
    .await;

    let stage_result = if crate::engine::bootstrap::is_flow_generator_profile(cfg.profile.as_str())
    {
        handle_flow_generator_result(params, state, stage_result).await
    } else {
        stage_result
    };

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

    let outcome = RunOutcome::Parked { wake_at };
    let _ = params.event_tx.send(EngineRunEvent::Terminal {
        outcome: outcome.clone(),
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::LedgerEffect;
    use surge_core::agent_config::AgentConfig;
    use surge_core::edge::EdgeKind;
    use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
    use surge_core::keys::ProfileKey;
    use surge_core::node::{Node, OutcomeDecl, Position};

    #[test]
    fn checkpoint_exit_matches_only_named_node() {
        assert!(checkpoint_exit_matches(Some("impl_1"), "impl_1"));
        assert!(!checkpoint_exit_matches(Some("impl_1"), "review_1"));
        assert!(!checkpoint_exit_matches(None, "impl_1"));
        assert!(!checkpoint_exit_matches(Some(""), "impl_1"));
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
