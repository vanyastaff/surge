//! `Engine` — the public API. Methods are stubbed in this task and
//! implemented incrementally in Phase 5 (lifecycle), Phase 9 (resolve),
//! Phase 11 (stop).

use crate::engine::config::{EngineConfig, EngineRunConfig};
use crate::engine::error::EngineError;
use crate::engine::event_tap::{RunEventTap, TAP_BUFFER_SIZE};
use crate::engine::handle::RunHandle;
use crate::engine::tools::ToolDispatcher;
use crate::profile_loader::ProfileRegistry;
use crate::roadmap_amendment::ActiveRunAmendmentOutcome;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::roadmap_patch::{RoadmapPatchApplyResult, RoadmapPatchId, RoadmapPatchTarget};
use surge_core::run_event::{EventPayload, VersionedEventPayload};

/// Central orchestration engine. Drives one or more concurrent runs, each
/// executing a frozen [`Graph`] through ACP sessions and persistence writes.
pub struct Engine {
    #[cfg(any(test, feature = "test-support"))]
    pub(super) next_feed_probe: std::sync::Mutex<Option<super::test_support::FeedProbeRequest>>,
    #[cfg(test)]
    completion_poll_gate: std::sync::Mutex<Option<CompletionPollGate>>,
    #[cfg(test)]
    completion_retirement_gate: std::sync::Mutex<Option<CompletionPollGate>>,
    bridge: Arc<dyn BridgeFacade>,
    storage: Arc<surge_persistence::runs::Storage>,
    tool_dispatcher: Arc<dyn ToolDispatcher>,
    notify_deliverer: Arc<dyn surge_notify::NotifyDeliverer>,
    /// Optional MCP registry shared across all runs hosted by this engine.
    /// `None` for the M6 in-process default; `Some` for daemon mode where
    /// the daemon constructed a registry from `RunConfig::mcp_servers`.
    /// Per-stage agent execution wraps the engine dispatcher with
    /// `RoutingToolDispatcher` when this is `Some`.
    mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
    config: Arc<EngineConfig>,
    /// Active runs indexed by `RunId`. Each entry holds the per-run resolution
    /// senders + cancellation token so engine-level methods (resolve, stop)
    /// can route into the right task.
    runs: Arc<tokio::sync::RwLock<HashMap<RunId, ActiveRun>>>,
    /// Engine-level broadcast tap of every run-event appended to any active
    /// run's persisted log. Sized at [`TAP_BUFFER_SIZE`]; slow subscribers
    /// receive [`RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
    /// rather than blocking the writer side. See [`event_tap`] for the
    /// recovery contract.
    event_tap: tokio::sync::broadcast::Sender<RunEventTap>,
}

pub(crate) struct ActiveRun {
    pub cancel: tokio_util::sync::CancellationToken,
    pub gate_resolutions: Arc<
        tokio::sync::Mutex<
            HashMap<surge_core::keys::NodeKey, crate::engine::stage::human_gate::PendingGate>,
        >,
    >,
    pub tool_resolutions:
        Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    pub roadmap_amendments:
        tokio::sync::mpsc::Sender<crate::engine::run_task::RoadmapAmendmentCommand>,
    /// Tracker for in-flight ACP elevation requests. `Engine::resolve_elevation`
    /// looks up entries here and fires their decision oneshot.
    pub pending_elevations: Arc<crate::engine::elevation::PendingElevations>,
    /// Queued operator steer messages. `Engine::submit_steer` pushes here; the
    /// run task drains at the next stage boundary and delivers into the prompt.
    pub pending_steers: crate::engine::steer::SteerQueue,
}

/// The per-run resolution/steer/cancellation state `start_run` and
/// `resume_run` both build identically, then split between `self.runs`
/// (as an [`ActiveRun`], for engine-level resolve/stop methods) and the
/// spawned run task's own [`crate::engine::run_task::RunTaskParams`].
/// Pulled out by [`Engine::register_active_run`] (Task 12 M3) so neither
/// caller has to carry this bookkeeping inline — it was already
/// byte-for-byte duplicated between the two before this extraction.
struct FreshRunRegistration {
    cancel: tokio_util::sync::CancellationToken,
    gate_resolutions: Arc<
        tokio::sync::Mutex<
            HashMap<surge_core::keys::NodeKey, crate::engine::stage::human_gate::PendingGate>,
        >,
    >,
    tool_resolutions:
        Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    roadmap_amendment_rx:
        tokio::sync::mpsc::Receiver<crate::engine::run_task::RoadmapAmendmentCommand>,
    pending_elevations: Arc<crate::engine::elevation::PendingElevations>,
    pending_steers: crate::engine::steer::SteerQueue,
}

impl Engine {
    /// Construct an engine with a default no-op `NotifyDeliverer` (the
    /// default `MultiplexingNotifier` returns `ChannelNotConfigured` for
    /// every channel, matching the M5 "log-only" stub behaviour).
    /// Use `new_with_notifier` for production wiring.
    pub fn new(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        config: EngineConfig,
    ) -> Self {
        let notify_deliverer: Arc<dyn surge_notify::NotifyDeliverer> =
            Arc::new(surge_notify::MultiplexingNotifier::new());
        Self::new_with_notifier(bridge, storage, tool_dispatcher, notify_deliverer, config)
    }

    /// M6 constructor that wires a real notify deliverer (replacing the
    /// no-op default). Production CLI / daemon use this.
    #[must_use]
    pub fn new_with_notifier(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        notify_deliverer: Arc<dyn surge_notify::NotifyDeliverer>,
        config: EngineConfig,
    ) -> Self {
        Self::new_with_mcp(
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            None,
            config,
        )
    }

    /// Construct with an MCP registry. The registry is shared across
    /// all runs hosted by this engine. M7 daemon uses this constructor
    /// to wire user-configured MCP servers; in-process M6-style CLI
    /// stays on `new` / `new_with_notifier` (no MCP).
    #[must_use]
    pub fn new_with_mcp(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        notify_deliverer: Arc<dyn surge_notify::NotifyDeliverer>,
        mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
        config: EngineConfig,
    ) -> Self {
        Self::new_full(
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            None,
            config,
        )
    }

    /// Full constructor: every dependency including the profile registry.
    ///
    /// Use this from production CLI / daemon entry points. Legacy
    /// constructors delegate here, leaving `profile_registry` as the
    /// `EngineConfig` already carries it (so passing `None` for the
    /// argument keeps the legacy mock-only fast path active).
    #[must_use]
    pub fn new_full(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        notify_deliverer: Arc<dyn surge_notify::NotifyDeliverer>,
        mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
        profile_registry: Option<Arc<ProfileRegistry>>,
        mut config: EngineConfig,
    ) -> Self {
        // The argument wins; if a caller already populated `config.profile_registry`
        // and also passed `Some(_)` here, the explicit argument is the
        // authoritative source.
        if profile_registry.is_some() {
            config.profile_registry = profile_registry;
        }
        let bridge = Arc::new(
            crate::recipe_admission::RecipeAdmissionBridge::new(bridge, storage.work_items())
                .with_configured_routes(
                    config.agent_registry.clone(),
                    storage.home().to_path_buf(),
                ),
        );
        let (event_tap, _initial_subscriber) = tokio::sync::broadcast::channel(TAP_BUFFER_SIZE);
        Self {
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            config: Arc::new(config),
            #[cfg(test)]
            completion_poll_gate: std::sync::Mutex::new(None),
            #[cfg(test)]
            completion_retirement_gate: std::sync::Mutex::new(None),
            #[cfg(any(test, feature = "test-support"))]
            next_feed_probe: std::sync::Mutex::new(None),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            event_tap,
        }
    }

    async fn settle_terminal_resume(
        &self,
        writer: surge_persistence::runs::RunWriter,
        outcome: super::handle::RunOutcome,
    ) -> Result<RunHandle, EngineError> {
        let run_id = *writer.run_id();
        let feed = writer.event_feed();
        #[cfg(any(test, feature = "test-support"))]
        let feed = match self.configure_feed_for_test(feed) {
            Ok(feed) => feed,
            Err(error) => return Err(super::run_execution::close_after_error(writer, error).await),
        };
        let settled = super::run_execution::settle_terminal(
            writer,
            &self.storage,
            self.event_tap.clone(),
            outcome,
            feed,
        )
        .await;
        Ok(terminal_resume_handle(run_id, settled))
    }

    fn spawn_run_execution(
        &self,
        params: super::run_task::RunTaskParams,
        feed: surge_persistence::runs::RunEventFeed,
    ) -> tokio::task::JoinHandle<super::handle::RunOutcome> {
        let run_id = params.run_id;
        let registration = Arc::downgrade(&params.gate_resolutions);
        let project_path = params.worktree_path.clone();
        let runs = self.runs.clone();
        let storage = self.storage.clone();
        let execution =
            super::run_execution::RunExecution::new(params, self.event_tap.clone(), feed);
        #[cfg(test)]
        let retirement_gate = self.completion_retirement_gate.lock().unwrap().take();
        Self::spawn_completion(
            async move {
                let outcome = Box::pin(execution.execute()).await;
                #[cfg(test)]
                if let Some(gate) = retirement_gate {
                    let _ = gate.entered.send(());
                    let _ = gate.release.await;
                }
                {
                    let mut active_runs = runs.write().await;
                    if active_runs.get(&run_id).is_some_and(|active| {
                        registration.ptr_eq(&Arc::downgrade(&active.gate_resolutions))
                    }) {
                        active_runs.remove(&run_id);
                    }
                }
                // Best-effort registry mirror follows actual writer/forwarder settlement.
                mirror_task_ledger(&storage, run_id, &project_path).await;
                outcome
            },
            #[cfg(test)]
            self.completion_poll_gate.lock().unwrap().take(),
        )
    }

    fn spawn_completion(
        future: impl std::future::Future<Output = super::handle::RunOutcome> + Send + 'static,
        #[cfg(test)] gate: Option<CompletionPollGate>,
    ) -> tokio::task::JoinHandle<super::handle::RunOutcome> {
        #[cfg(test)]
        if let Some(gate) = gate {
            return tokio::spawn(async move {
                let _ = gate.entered.send(());
                let _ = gate.release.await;
                future.await
            });
        }
        tokio::spawn(future)
    }

    /// Subscribe to the engine's run-event broadcast tap.
    ///
    /// Receivers see every [`ReadEvent`](surge_persistence::runs::ReadEvent)
    /// appended to any active run's log, wrapped in a [`RunEventTap`] that
    /// carries the originating [`RunId`]. The broadcast buffer is
    /// [`TAP_BUFFER_SIZE`] long; subscribers that fall behind receive
    /// [`RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
    /// and are expected to trigger a reconcile against the persisted log
    /// rather than recover the dropped events.
    #[must_use]
    pub fn subscribe_tap(&self) -> tokio::sync::broadcast::Receiver<RunEventTap> {
        self.event_tap.subscribe()
    }

    /// Borrow the profile registry, if one is wired into this engine.
    /// Used by the agent stage to derive `AgentKind` from the profile's
    /// `runtime.agent_id` (M6+ resolution path that replaces the M5
    /// mock-only fast path).
    #[must_use]
    pub fn profile_registry(&self) -> Option<&Arc<ProfileRegistry>> {
        self.config.profile_registry.as_ref()
    }

    /// Clone the storage handle backing this engine.
    ///
    /// Kept crate-visible for orchestration helpers that need to inspect a
    /// completed run's event log without widening the public `Engine` API.
    #[must_use]
    pub(crate) fn storage(&self) -> Arc<surge_persistence::runs::Storage> {
        self.storage.clone()
    }

    /// Build a fresh run's resolution/steer/cancellation state, register it
    /// in `self.runs` as an [`ActiveRun`], and hand back the pieces the
    /// caller's `RunTaskParams` needs. See [`FreshRunRegistration`]'s doc.
    async fn register_active_run(&self, run_id: RunId) -> FreshRunRegistration {
        let cancel = tokio_util::sync::CancellationToken::new();
        let gate_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let tool_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let (roadmap_amendment_tx, roadmap_amendment_rx) = tokio::sync::mpsc::channel(16);
        let pending_elevations = crate::engine::elevation::PendingElevations::new();
        let pending_steers = crate::engine::steer::new_queue();
        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
            roadmap_amendments: roadmap_amendment_tx,
            pending_elevations: pending_elevations.clone(),
            pending_steers: pending_steers.clone(),
        };
        self.runs.write().await.insert(run_id, active);
        FreshRunRegistration {
            cancel,
            gate_resolutions,
            tool_resolutions,
            roadmap_amendment_rx,
            pending_elevations,
            pending_steers,
        }
    }

    /// Start a new run.
    pub async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        self.admit_and_launch(super::admission::AdmissionKind::Fresh {
            run_id,
            graph: Box::new(graph),
            worktree: worktree_path,
            config: Box::new(run_config),
        })
        .await
    }

    /// Whether host configured-source freezing requires an exclusive launch-workspace preparation.
    /// This inspects graph/config metadata only; it never reads credentials or route keys.
    #[must_use]
    pub fn requires_start_preparation(&self, graph: &Graph) -> bool {
        matches!(
            self.config.capacity.rotation,
            surge_core::capacity::RotationPolicy::Candidate { .. }
        ) && graph
            .nodes
            .values()
            .chain(graph.subgraphs.values().flat_map(|g| g.nodes.values()))
            .any(|node| matches!(node.config, surge_core::node::NodeConfig::Agent(_)))
    }

    /// Freeze validated host configuration before creating a durable task
    /// reservation: the task-claimed front half of run admission (#97). A
    /// task start trusts this frozen config and does not re-validate it.
    pub async fn freeze_work_item_config(
        &self,
        graph: &Graph,
        config: EngineRunConfig,
        working_dir: &Path,
    ) -> Result<EngineRunConfig, EngineError> {
        self.admission()
            .freeze_task_config(graph, config, working_dir)
            .await
            .map_err(EngineError::from)
    }

    /// The validated handoff of a completed bootstrap run, against this
    /// engine's profile registry and agent catalog (#98). Read-only.
    ///
    /// # Errors
    /// A typed [`crate::bootstrap::handoff::HandoffRejection`].
    pub async fn prepare_bootstrap_handoff(
        &self,
        parent: RunId,
    ) -> Result<
        crate::bootstrap::handoff::BootstrapHandoff,
        crate::bootstrap::handoff::HandoffRejection,
    > {
        let context = crate::bootstrap::handoff::HandoffContext {
            profiles: self.config.profile_registry.as_deref(),
            agents: self.config.agent_registry.as_deref(),
        };
        crate::bootstrap::handoff::prepare(&self.storage, parent, &context).await
    }

    /// Launch exactly the host's durable reservation, never a caller-owned binding.
    pub async fn start_work_item(
        &self,
        claim: &surge_persistence::work_items::WorkItemLaunchClaim,
    ) -> Result<RunHandle, EngineError> {
        self.admit_and_launch(super::admission::AdmissionKind::TaskStart(claim))
            .await
    }

    /// The run admission over this engine (#97).
    fn admission(&self) -> super::admission::Admission<'_> {
        super::admission::Admission {
            storage: &self.storage,
            config: &self.config,
            mcp_registry: self.mcp_registry.as_ref(),
            active_runs: &self.runs,
        }
    }

    /// Admit a run, then register and spawn it. `Engine` keeps only the
    /// active-run registry and the spawning; everything else is admission's.
    async fn admit_and_launch(
        &self,
        kind: super::admission::AdmissionKind<'_>,
    ) -> Result<RunHandle, EngineError> {
        // Boxed: admission's state machine is large, and every start/resume
        // caller would otherwise carry it inline.
        let admitted = match Box::pin(self.admission().admit(kind)).await? {
            super::admission::Admitted::Terminal { writer, outcome } => {
                return self.settle_terminal_resume(writer, outcome).await;
            },
            super::admission::Admitted::Run(admitted) => *admitted,
        };
        let run_id = admitted.run_id;
        let feed = admitted.writer.event_feed();
        #[cfg(any(test, feature = "test-support"))]
        let feed = match self.configure_feed_for_test(feed) {
            Ok(feed) => feed,
            Err(error) => {
                return Err(super::run_execution::close_after_error(admitted.writer, error).await);
            },
        };
        let (event_tx, event_rx) = tokio::sync::broadcast::channel(256);
        let registration = self.register_active_run(run_id).await;
        let params = self.run_task_params(admitted, registration, event_tx);
        let join = self.spawn_run_execution(params, feed);
        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    /// The one constructor of a run task's parameters: from an admitted run
    /// plus its registration. A new run input is wired here once, for fresh,
    /// replayed and task-claimed runs alike.
    fn run_task_params(
        &self,
        admitted: super::admission::AdmittedRun,
        registration: FreshRunRegistration,
        event_tx: tokio::sync::broadcast::Sender<crate::engine::handle::EngineRunEvent>,
    ) -> super::run_task::RunTaskParams {
        let super::admission::AdmittedRun {
            run_id,
            execution,
            worktree_path,
            run_config,
            writer,
            artifact_store,
            claim,
            resume_evidence,
            effect_fence,
            mcp_registry,
            mcp_servers,
            agent_registry,
            capacity_ledger,
            capacity_estimator,
        } = admitted;
        let capacity = crate::engine::quota_recovery::RunCapacity::new(
            capacity_ledger,
            capacity_estimator,
            self.config.capacity.clone(),
            self.config.fallback_agents.clone(),
            agent_registry.clone(),
            resume_evidence.as_ref(),
        );
        super::run_task::RunTaskParams {
            work_item_claim: claim,
            pending_suspension: None,
            run_id,
            writer,
            artifact_store,
            bridge: super::owned_effects::bridge(self.bridge.clone(), effect_fence),
            tool_dispatcher: self.tool_dispatcher.clone(),
            notify_deliverer: self.notify_deliverer.clone(),
            execution,
            resumed: resume_evidence.is_some(),
            worktree_path,
            run_config,
            event_tx,
            cancel: registration.cancel,
            gate_resolutions: registration.gate_resolutions,
            tool_resolutions: registration.tool_resolutions,
            roadmap_amendments: registration.roadmap_amendment_rx,
            pending_elevations: registration.pending_elevations,
            pending_steers: registration.pending_steers,
            mcp_registry,
            mcp_servers,
            profile_registry: self.config.profile_registry.clone(),
            agent_registry,
            capacity,
            escalation: self.config.escalation.clone(),
            storage: self.storage.clone(),
        }
    }

    /// Resume an existing run from its latest snapshot + event tail.
    ///
    /// Opens the persisted event log, replays snapshots and events to
    /// reconstruct the last known cursor and memory, then resumes execution
    /// from that point. Returns immediately if the run is already active in
    /// this process.
    pub async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        self.admit_and_launch(super::admission::AdmissionKind::Replayed {
            run_id,
            worktree: worktree_path,
        })
        .await
    }

    /// Validate the exact durable startup association before any host reconciliation.
    pub async fn validate_work_item_startup(
        &self,
        claim: &surge_persistence::work_items::WorkItemLaunchClaim,
    ) -> Result<(), EngineError> {
        super::work_items::validate_resume(&self.storage, claim)
            .await
            .map(|_| ())
    }

    /// Recover an initialized task run under host ownership. This is not a task Stop control.
    pub async fn resume_work_item(
        &self,
        claim: &surge_persistence::work_items::WorkItemLaunchClaim,
    ) -> Result<RunHandle, EngineError> {
        self.admit_and_launch(super::admission::AdmissionKind::TaskResume(claim))
            .await
    }

    /// Submit an approved roadmap amendment to an active run.
    pub async fn submit_roadmap_amendment(
        &self,
        run_id: RunId,
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        patch_result: RoadmapPatchApplyResult,
    ) -> Result<ActiveRunAmendmentOutcome, EngineError> {
        if let RoadmapPatchTarget::RunRoadmap {
            run_id: target_run_id,
            ..
        } = target
            && target_run_id != run_id
        {
            return Err(EngineError::Internal(format!(
                "roadmap amendment target run {target_run_id} does not match active run {run_id}"
            )));
        }

        let sender = {
            let runs = self.runs.read().await;
            runs.get(&run_id)
                .map(|active| active.roadmap_amendments.clone())
                .ok_or(EngineError::RunNotFound(run_id))?
        };
        let (reply, result) = tokio::sync::oneshot::channel();
        sender
            .send(crate::engine::run_task::RoadmapAmendmentCommand {
                patch_id,
                target,
                patch_result,
                reply,
            })
            .await
            .map_err(|_| EngineError::Internal("roadmap amendment receiver dropped".into()))?;
        result
            .await
            .map_err(|_| EngineError::Internal("roadmap amendment reply dropped".into()))?
            .map_err(|error| EngineError::Internal(format!("active roadmap amendment: {error}")))
    }

    /// Fire the operator's decision into a pending ACP elevation request.
    ///
    /// Looks up `(run_id, session_id, request_id)` in the run's
    /// [`crate::engine::elevation::PendingElevations`] tracker and routes the
    /// decision through the parked oneshot. The agent stage observes it,
    /// appends `SandboxElevationDecided`, and calls
    /// `AcpBridge::reply_to_permission` to release the agent.
    ///
    /// # Errors
    ///
    /// - [`EngineError::RunNotFound`] when no run with `run_id` is active.
    /// - [`EngineError::Internal`] when the elevation is not pending or the
    ///   stage's receiver was already dropped.
    pub async fn resolve_elevation(
        &self,
        run_id: RunId,
        session_id: surge_core::SessionId,
        request_id: String,
        decision: crate::engine::elevation::EngineElevationDecision,
    ) -> Result<(), EngineError> {
        let runs = self.runs.read().await;
        let active = runs.get(&run_id).ok_or(EngineError::RunNotFound(run_id))?;
        active
            .pending_elevations
            .resolve(session_id, &request_id, decision)
            .await
            .map(|_| ())
            .map_err(|e| EngineError::Internal(format!("resolve elevation: {e}")))
    }

    /// Provide answer to a paused run waiting on human input.
    pub async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        let runs = self.runs.read().await;
        let active = runs.get(&run_id).ok_or(EngineError::RunNotFound(run_id))?;

        if let Some(call_id_str) = call_id {
            // Tool-driven resolution.
            let mut tools = active.tool_resolutions.lock().await;
            let tx = tools.remove(&call_id_str).ok_or_else(|| {
                EngineError::Internal(format!("no pending tool call '{call_id_str}'"))
            })?;
            tx.send(response)
                .map_err(|_| EngineError::Internal("tool resolution receiver dropped".into()))?;
            Ok(())
        } else {
            Err(EngineError::MissingGateRequestIdentity)
        }
    }

    /// Resolve a captured event identity without looking up a newer pending request.
    pub async fn resolve_requested_input(
        &self,
        run_id: RunId,
        node: surge_core::keys::NodeKey,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        match call_id {
            Some(id) => {
                if let Some(gate) = surge_core::id::GateRequestId::from_event_call_id(&id) {
                    self.resolve_gate_input(run_id, node, gate, response).await
                } else {
                    self.resolve_human_input(run_id, Some(id), response).await
                }
            },
            None => Err(EngineError::MissingGateRequestIdentity),
        }
    }

    /// Observe an exact live gate waiter without submitting or mutating an answer.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn gate_waiter_is_ready_for_test(
        &self,
        run_id: RunId,
        node: &surge_core::NodeKey,
        request: surge_core::id::GateRequestId,
    ) -> bool {
        let runs = self.runs.read().await;
        let Some(active) = runs.get(&run_id) else {
            return false;
        };
        active
            .gate_resolutions
            .lock()
            .await
            .get(node)
            .is_some_and(|pending| pending.request_id == request)
    }

    /// Consume only the exact registered gate; never fall back to run-only input.
    pub async fn resolve_gate_input(
        &self,
        run_id: RunId,
        node: surge_core::keys::NodeKey,
        request_id: surge_core::id::GateRequestId,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        if let Some(accepted) = self
            .storage
            .inspect_gate_answer(run_id, node.clone(), request_id)
            .await
            .map_err(|error| EngineError::Storage(error.to_string()))?
        {
            return if accepted == response {
                Ok(())
            } else {
                Err(EngineError::StaleGateRequest)
            };
        }
        let runs = self.runs.read().await;
        let Some(active) = runs.get(&run_id) else {
            drop(runs);
            return super::gate_answers::resolve_suspended(
                &self.storage,
                run_id,
                node,
                request_id,
                response,
            )
            .await;
        };
        let outcome = response
            .get("outcome")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                EngineError::Internal("HumanGate resolution missing 'outcome' field".into())
            })?;
        let outcome = surge_core::keys::OutcomeKey::try_from(outcome)
            .map_err(|error| EngineError::Internal(format!("invalid outcome: {error}")))?;
        let mut gates = active.gate_resolutions.lock().await;
        if !gates
            .get(&node)
            .is_some_and(|pending| pending.request_id == request_id)
        {
            return Err(EngineError::StaleGateRequest);
        }
        let pending = gates.get(&node).ok_or(EngineError::StaleGateRequest)?;
        surge_core::human_gate_config::validate_gate_response(
            &response,
            &pending.allowed_outcomes,
            pending.allow_freetext,
        )
        .map_err(|error| EngineError::Internal(error.to_string()))?;
        if active.cancel.is_cancelled()
            || (!pending.allow_freetext && !pending.allowed_outcomes.contains(&outcome))
        {
            return Err(EngineError::StaleGateRequest);
        }
        // Retain the waiter if durable acceptance is rejected. This capability
        // uses the same writer as the stage and cannot open a second writer.
        let committed_seq = pending
            .recorder
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputResolved {
                    node: node.clone(),
                    call_id: Some(request_id.to_string()),
                    response: response.clone(),
                },
            ))
            .await
            .map_err(|error| EngineError::Storage(error.to_string()))?
            .as_u64();
        let pending = gates.remove(&node).ok_or(EngineError::StaleGateRequest)?;
        pending
            .sender
            .send(crate::engine::stage::human_gate::HumanGateResolution {
                committed_seq: Some(committed_seq),
                outcome,
                response,
            })
            .map_err(|_| EngineError::StaleGateRequest)
    }

    /// Queue an operator steer message for a live run. Non-destructive: the
    /// message is delivered (prepended to the prompt) at the next agent stage
    /// boundary — ACP v1 has no mid-turn injection channel. Returns the queued
    /// steer's id.
    pub async fn submit_steer(
        &self,
        run_id: RunId,
        message: String,
    ) -> Result<String, EngineError> {
        // Validate here, not only in the CLI: the SubmitSteer IPC reaches this
        // directly. Reject empty/whitespace and oversize messages, and cap the
        // queue so a run parked at a non-agent node can't accumulate steers
        // unbounded in daemon memory.
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return Err(EngineError::Internal("steer message is empty".into()));
        }
        if message.len() > crate::engine::steer::MAX_STEER_LEN {
            return Err(EngineError::Internal(format!(
                "steer message is {} bytes; max is {}",
                message.len(),
                crate::engine::steer::MAX_STEER_LEN
            )));
        }
        let runs = self.runs.read().await;
        let active = runs.get(&run_id).ok_or(EngineError::RunNotFound(run_id))?;
        let mut queue = active.pending_steers.lock().await;
        if queue.pending.len() >= crate::engine::steer::MAX_QUEUED_STEERS {
            return Err(EngineError::Internal(format!(
                "steer queue for run {run_id} is full ({} pending); \
                 deliver or cancel some before adding more",
                queue.pending.len()
            )));
        }
        let id = crate::engine::steer::new_steer_id();
        queue.pending.push(crate::engine::steer::QueuedSteer {
            id: id.clone(),
            message,
        });
        Ok(id)
    }

    /// List the steer messages currently queued (not yet delivered) for a run.
    pub async fn list_steers(
        &self,
        run_id: RunId,
    ) -> Result<Vec<crate::engine::steer::QueuedSteer>, EngineError> {
        let runs = self.runs.read().await;
        let active = runs.get(&run_id).ok_or(EngineError::RunNotFound(run_id))?;
        Ok(active.pending_steers.lock().await.pending.clone())
    }

    /// Cancel a steer by id. Removes it from the pending queue if present, and
    /// tombstones the id so that a steer already drained into an in-flight stage
    /// is not resurrected by the run task's re-queue-on-error path. Returns
    /// `true` if it was still pending; `false` means it was already delivered or
    /// in-flight (the tombstone still prevents any re-delivery).
    pub async fn cancel_steer(&self, run_id: RunId, steer_id: &str) -> Result<bool, EngineError> {
        let runs = self.runs.read().await;
        let active = runs.get(&run_id).ok_or(EngineError::RunNotFound(run_id))?;
        let mut queue = active.pending_steers.lock().await;
        let before = queue.pending.len();
        queue.pending.retain(|steer| steer.id != steer_id);
        let removed = queue.pending.len() != before;
        queue.cancelled.insert(steer_id.to_owned());
        Ok(removed)
    }

    /// Snapshot the in-process active-run map as a `Vec<RunSummary>`.
    /// Used by `LocalEngineFacade::list_runs`. M7 simplification: the
    /// engine doesn't track per-run `started_at`, so we return
    /// `chrono::Utc::now()` for every entry. The daemon facade has a
    /// richer view; M8+ may add real per-run start timestamps to
    /// `ActiveRun`.
    pub async fn snapshot_active_runs(&self) -> Vec<crate::engine::handle::RunSummary> {
        use crate::engine::handle::{RunStatus, RunSummary};
        let runs = self.runs.read().await;
        let now = chrono::Utc::now();
        runs.keys()
            .map(|id| RunSummary {
                run_id: *id,
                status: RunStatus::Active,
                started_at: now,
                last_event_seq: None,
            })
            .collect()
    }

    /// Interrupt an owned attempt for its already durable suspension request.
    /// Completion confirms the fence; returning here is not an acknowledgement.
    pub async fn suspend_work_item(
        &self,
        run_id: RunId,
        generation: u64,
    ) -> Result<(), EngineError> {
        let control = self
            .storage
            .work_items()
            .execution_control(run_id)
            .map_err(|error| EngineError::Internal(error.to_string()))?
            .ok_or_else(|| EngineError::Internal("durable suspension request missing".into()))?;
        if control.generation != generation
            || control.state
                != surge_core::execution_recovery::ExecutionControlState::SuspendRequested
        {
            return Err(EngineError::Internal(
                "suspension control is obsolete".into(),
            ));
        }
        let runs = self.runs.read().await;
        let active = runs.get(&run_id).ok_or(EngineError::RunNotFound(run_id))?;
        active.cancel.cancel();
        Ok(())
    }

    /// Cancel an in-flight run. Signals the cancellation token so the run task
    /// will emit `RunAborted` and exit. Returns [`EngineError::RunNotFound`] if
    /// no run with `run_id` is currently active.
    pub async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError> {
        let cancel = {
            let runs = self.runs.read().await;
            runs.get(&run_id).map(|a| a.cancel.clone())
        };

        match cancel {
            Some(cancel) => {
                tracing::info!(run_id = %run_id, reason = %reason, "stop_run requested");
                cancel.cancel();
                // Wait briefly for the task to wind down — best-effort.
                // The task itself will emit RunAborted and exit.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Ok(())
            },
            None => Err(EngineError::RunNotFound(run_id)),
        }
    }
}

fn terminal_resume_handle(run_id: RunId, outcome: crate::engine::RunOutcome) -> RunHandle {
    let (event_tx, events) = tokio::sync::broadcast::channel(1);
    let event = if matches!(outcome, crate::engine::RunOutcome::SettlementFailed { .. }) {
        crate::engine::handle::EngineRunEvent::StreamError {
            message: "replayed run settlement could not be confirmed".into(),
        }
    } else {
        crate::engine::handle::EngineRunEvent::Terminal {
            outcome: outcome.clone(),
        }
    };
    let _ = event_tx.send(event);
    RunHandle {
        run_id,
        events,
        completion: tokio::spawn(async move { outcome }),
    }
}

// Suppress unused-field warning for config until Phase 6+ uses it.
#[allow(dead_code)]
fn _engine_config_used(e: &Engine) {
    let _ = &e.config;
}

/// Best-effort mirror of a run's task-ledger into the cross-run registry
/// index. Logs and swallows errors — a mirror failure never affects the run
/// outcome (the per-run event log remains the source of truth).
async fn mirror_task_ledger(
    storage: &Arc<surge_persistence::runs::Storage>,
    run_id: RunId,
    project_path: &std::path::Path,
) {
    if let Err(error) = storage.sync_task_ledger_index(run_id, project_path).await {
        tracing::warn!(
            target: "engine::ledger",
            run_id = %run_id,
            err = %error,
            "task-ledger index mirror failed; surge ready may be stale for this run"
        );
    }
}

#[cfg(test)]
mod exhausted_reason_tests {
    use crate::engine::capacity::exhausted_reason;
    use surge_core::capacity::{CapacityStatus, CapacityWindow};

    #[test]
    fn only_a_window_that_has_not_reset_blocks_planning() {
        let now = chrono::Utc::now();
        let hour = std::time::Duration::from_secs(3600);
        assert_eq!(exhausted_reason(&CapacityStatus::NeverObserved, now), None);

        let pending =
            CapacityStatus::Known(CapacityWindow::observed_429("codex-acp", Some(hour), now));
        assert!(exhausted_reason(&pending, now).is_some_and(|w| w.contains("resets")));

        let earlier = now - chrono::Duration::hours(2);
        let lapsed = CapacityStatus::Known(CapacityWindow::observed_429(
            "codex-acp",
            Some(hour),
            earlier,
        ));
        assert_eq!(exhausted_reason(&lapsed, now), None, "reset already passed");

        let unknown = CapacityStatus::Known(CapacityWindow::observed_429("codex-acp", None, now));
        assert!(
            exhausted_reason(&unknown, now).is_some(),
            "no reset hint: still exhausted"
        );
    }
}

#[cfg(test)]
struct CompletionPollGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
#[path = "engine/settlement_tests.rs"]
mod settlement_tests;
