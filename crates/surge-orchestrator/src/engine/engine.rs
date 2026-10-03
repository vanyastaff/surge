//! `Engine` — the public API. Methods are stubbed in this task and
//! implemented incrementally in Phase 5 (lifecycle), Phase 9 (resolve),
//! Phase 11 (stop).

use crate::engine::config::{EngineConfig, EngineRunConfig, RunSeedArtifact};
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
use surge_core::mcp_config::McpServerRef;
use surge_core::roadmap_patch::{RoadmapPatchApplyResult, RoadmapPatchId, RoadmapPatchTarget};
use surge_core::run_event::{EventPayload, VersionedEventPayload};

/// Central orchestration engine. Drives one or more concurrent runs, each
/// executing a frozen [`Graph`] through ACP sessions and persistence writes.
pub struct Engine {
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
        let (event_tap, _initial_subscriber) = tokio::sync::broadcast::channel(TAP_BUFFER_SIZE);
        Self {
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            config: Arc::new(config),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            event_tap,
        }
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
        let (event_tap, _initial_subscriber) = tokio::sync::broadcast::channel(TAP_BUFFER_SIZE);
        Self {
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            config: Arc::new(config),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            event_tap,
        }
    }

    /// Forward every event appended to `writer` onto the engine's broadcast
    /// tap until the underlying subscription terminates (run closed or
    /// subscriber stream errored).
    ///
    /// The forwarder uses `RunWriter::subscribe_events` — the same SQL-backed
    /// polling stream consumed by the run handle's `events_stream` — so the
    /// tap reflects the persisted log, not transient in-memory state. The
    /// task self-terminates when the stream ends; there is no explicit
    /// cancellation path because dropping the underlying storage handle
    /// closes the subscription.
    fn spawn_event_forwarder(&self, run_id: RunId, writer: &surge_persistence::runs::RunWriter) {
        let stream = writer.subscribe_events();
        let tap_sender = self.event_tap.clone();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = std::pin::pin!(stream);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(event) => {
                        let _ = tap_sender.send(RunEventTap { run_id, event });
                    },
                    Err(err) => {
                        tracing::debug!(
                            target: "engine::tap",
                            %run_id,
                            error = ?err,
                            "run event forwarder ended",
                        );
                        break;
                    },
                }
            }
        });
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

    /// Build the two Task 12 M3 capacity ports a `RunTaskParams` needs,
    /// shared verbatim between `start_run` and `resume_run`.
    ///
    /// The ledger is engine-wide (shared across every run this `Engine`
    /// hosts, keyed by canonical runtime — an exhausted runtime is a fact
    /// about the runtime, not about any one run). The estimator is
    /// per-run (reads only `writer`'s own `stage_executions`) — built from
    /// a cloned `RunReader` (cheap; see `RunWriter::reader`'s doc) so it
    /// keeps working independently of `writer` itself, which the caller
    /// moves into `RunTaskParams` right after this call.
    fn capacity_ports_for(
        &self,
        writer: &surge_persistence::runs::run_writer::RunWriter,
    ) -> (
        Arc<dyn crate::engine::capacity::CapacityLedger>,
        Arc<dyn crate::engine::capacity::WorkEstimator>,
    ) {
        let ledger: Arc<dyn crate::engine::capacity::CapacityLedger> = Arc::new(
            crate::engine::capacity::PersistentCapacityLedger::new(self.storage.clone()),
        );
        let history: Arc<dyn crate::engine::capacity::ArchetypeHistory> = Arc::new(
            crate::engine::capacity::PersistentArchetypeHistory::new(self.storage.clone()),
        );
        let estimator: Arc<dyn crate::engine::capacity::WorkEstimator> = Arc::new(
            crate::engine::capacity::RunHistoryWorkEstimator::new(writer.reader())
                .with_history(history),
        );
        (ledger, estimator)
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
        self.start_run_owned(run_id, graph, worktree_path, run_config, None)
            .await
    }

    /// Freeze validated host configuration before creating a durable task reservation.
    pub async fn freeze_work_item_config(
        &self,
        graph: &Graph,
        mut config: EngineRunConfig,
    ) -> Result<EngineRunConfig, EngineError> {
        match self.config.profile_registry.as_ref() {
            Some(registry) => {
                crate::engine::validate::validate_for_m6_with_resolver(graph, registry.as_ref())?;
                crate::engine::validate::validate_profile_inputs(graph, registry)?;
            },
            None => crate::engine::validate::validate_for_m6(graph)?,
        }
        crate::engine::validate::validate_quota_policy(
            graph,
            &config.quota_recovery,
            self.config.profile_registry.as_deref(),
        )?;
        self.seed_profile_catalog(&mut config).await?;
        crate::engine::validate::validate_loop_seeds(graph, &config.seed_artifacts)?;
        Ok(config)
    }

    /// Launch exactly the host's durable reservation, never a caller-owned binding.
    pub async fn start_work_item(
        &self,
        claim: &surge_persistence::work_items::WorkItemLaunchClaim,
    ) -> Result<RunHandle, EngineError> {
        let (attempt, workspace, _, config) = super::work_items::pinned(&self.storage, claim)?;
        self.start_run_owned(
            attempt.run,
            *attempt.graph,
            workspace.path,
            config,
            Some(claim),
        )
        .await
    }

    async fn start_run_owned(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
        claim: Option<&surge_persistence::work_items::WorkItemLaunchClaim>,
    ) -> Result<RunHandle, EngineError> {
        use crate::engine::run_task::{RunTaskParams, execute};
        use tokio::sync::broadcast;
        let (writer, artifact_store, run_config) = self
            .prepare_run_start(run_id, &graph, &worktree_path, run_config, claim)
            .await?;

        let per_run_mcp_registry =
            self.resolve_run_mcp_registry(&run_config.mcp_servers, &worktree_path, &writer);
        let mcp_servers_clone = run_config.mcp_servers.clone();

        let (event_tx, event_rx) = broadcast::channel(256);
        let registration = self.register_active_run(run_id).await;

        // Captured before `worktree_path` moves into params — used by the
        // post-completion task-ledger mirror. Matches the RunStarted event's
        // `project_path` (which is the worktree path today).
        let project_path_for_ledger = worktree_path.clone();
        let (capacity_ledger, capacity_estimator) = self.capacity_ports_for(&writer);
        let run_agent_registry = self.agent_registry_for(&run_config);
        let params = RunTaskParams {
            work_item_claim: claim.cloned(),
            pending_suspension: None,
            run_id,
            writer,
            artifact_store,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            notify_deliverer: self.notify_deliverer.clone(),
            graph,
            worktree_path,
            run_config,
            event_tx,
            cancel: registration.cancel,
            resume_cursor: None,
            resume_memory: None,
            resume_memory_applied_seq: None,
            resume_frames: None,
            resume_root_traversal_counts: None,
            resume_applied_graph_revision_seq: None,
            gate_resolutions: registration.gate_resolutions,
            tool_resolutions: registration.tool_resolutions,
            roadmap_amendments: registration.roadmap_amendment_rx,
            pending_elevations: registration.pending_elevations,
            pending_steers: registration.pending_steers,
            mcp_registry: per_run_mcp_registry,
            mcp_servers: mcp_servers_clone,
            profile_registry: self.config.profile_registry.clone(),
            agent_registry: run_agent_registry.clone(),
            capacity_ledger,
            capacity_estimator,
            capacity_policy: self.config.capacity.clone(),
            storage: self.storage.clone(),
            // A fresh run was never parked — nothing to bypass.
            capacity_precheck_bypass_once: std::sync::atomic::AtomicBool::new(false),
        };

        let runs_for_cleanup = self.runs.clone();
        // Post-completion ledger mirror: after the run task finishes, mirror
        // its folded task-ledger into the cross-run registry index so
        // `surge ready` / `surge ledger` see it without opening the run DB.
        // Best-effort — a mirror failure never affects the run outcome.
        let storage_for_ledger = self.storage.clone();
        let join = tokio::spawn(async move {
            let outcome = execute(params).await;
            runs_for_cleanup.write().await.remove(&run_id);
            record_terminal_status(&storage_for_ledger, run_id, &outcome).await;
            mirror_task_ledger(&storage_for_ledger, run_id, &project_path_for_ledger).await;
            outcome
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    async fn prepare_run_start(
        &self,
        run_id: RunId,
        graph: &Graph,
        worktree_path: &Path,
        mut run_config: EngineRunConfig,
        claim: Option<&surge_persistence::work_items::WorkItemLaunchClaim>,
    ) -> Result<
        (
            surge_persistence::runs::run_writer::RunWriter,
            surge_persistence::artifacts::ArtifactStore,
            EngineRunConfig,
        ),
        EngineError,
    > {
        use crate::engine::validate::{validate_for_m6, validate_for_m6_with_resolver};
        let task_context = if let Some(claim) = claim {
            let (_, _, context, _) = super::work_items::pinned(&self.storage, claim)?;
            Some(context)
        } else {
            if self
                .storage
                .work_items()
                .for_run(run_id)
                .map_err(|error| EngineError::Storage(error.to_string()))?
                .is_some()
            {
                return Err(EngineError::Storage(
                    "reserved task run requires host launch ownership".into(),
                ));
            }
            None
        };

        tracing::info!(
            target: "surge.path.exercised",
            path = "engine",
            kind = "start",
            run_id = %run_id,
            "entered engine path",
        );

        // A profile registry lets validation resolve profile references and
        // agent-runtime identity (e.g. `ValidationErrorKind::SameRuntimeVerification`,
        // `ProfileNotFound`) — see `profile_loader::resolver`'s `ReferenceResolver`
        // impl. No registry keeps today's resolver-free behavior unchanged.
        match self.config.profile_registry.as_ref() {
            Some(registry) => {
                validate_for_m6_with_resolver(graph, registry.as_ref())?;
                crate::engine::validate::validate_profile_inputs(graph, registry)?;
            },
            None => validate_for_m6(graph)?,
        }
        crate::engine::validate::validate_quota_policy(
            graph,
            &run_config.quota_recovery,
            self.config.profile_registry.as_deref(),
        )?;

        run_config.memory_store_path =
            self.resolve_memory_store_path(run_config.memory_store_path.take());

        // No registry wired keeps today's behavior unchanged — see
        // `seed_profile_catalog`'s doc for what this seeds and why.
        if task_context.is_none() {
            self.seed_profile_catalog(&mut run_config).await?;
        }

        crate::engine::validate::validate_loop_seeds(graph, &run_config.seed_artifacts)?;

        if self.runs.read().await.contains_key(&run_id) {
            return Err(EngineError::RunAlreadyActive(run_id));
        }

        if !worktree_path.exists() {
            return Err(EngineError::WorktreeMissing(worktree_path.to_path_buf()));
        }

        let writer = if claim.is_some()
            && self
                .storage
                .get_run(&run_id)
                .await
                .map_err(|error| EngineError::Storage(error.to_string()))?
                .is_some()
        {
            let inspected = self
                .storage
                .inspect_run(run_id)
                .await
                .map_err(|error| EngineError::Storage(error.to_string()))?;
            if !matches!(inspected.database, surge_persistence::runs::inspection::RunDatabaseInspection::Present{ref events} if events.is_empty())
            {
                return Err(EngineError::Storage(
                    "task start refuses nonempty existing journal".into(),
                ));
            }
            self.storage
                .open_run_writer(run_id)
                .await
                .map_err(|error| EngineError::Storage(error.to_string()))?
        } else {
            self.storage
                .create_run(run_id, worktree_path, None)
                .await
                .map_err(|error| EngineError::Storage(error.to_string()))?
        };
        self.spawn_event_forwarder(run_id, &writer);
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(self.storage.home().join("runs"));

        let mut events = self
            .build_startup_events(&artifact_store, run_id, graph, worktree_path, &run_config)
            .await?;
        if let Some(context) = task_context {
            if let Some(claim) = claim {
                self.storage
                    .work_items()
                    .validate_claim(claim)
                    .map_err(|error| EngineError::Storage(error.to_string()))?;
            }
            append_task_binding(&artifact_store, run_id, context, &mut events).await?;
        }
        writer
            .append_events(events)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        Ok((writer, artifact_store, run_config))
    }

    /// Resolve the per-run MCP registry: prefer the run-config-supplied list
    /// over the engine-level fallback. If `mcp_servers` is non-empty a fresh
    /// `McpRegistry` is built for this run; otherwise fall back to the
    /// engine-wide registry (typically `None` for daemon mode, where the
    /// per-run list is the only source).
    fn resolve_run_mcp_registry(
        &self,
        mcp_servers: &[McpServerRef],
        worktree_path: &Path,
        writer: &surge_persistence::runs::RunWriter,
    ) -> Option<Arc<surge_mcp::McpRegistry>> {
        let configs = if mcp_servers.is_empty() {
            self.mcp_registry
                .as_ref()
                .map(|registry| registry.configured_servers())?
        } else {
            mcp_servers.to_vec()
        };
        let observer: Arc<dyn surge_mcp::writer_observer::HostWriterObserver> =
            Arc::new(crate::engine::writer_coverage::McpWriterObserver {
                recorder: writer.event_recorder(),
                invocation: surge_core::id::StageInvocationId::new(),
            });
        Some(Arc::new(surge_mcp::McpRegistry::from_config_owned(
            &configs,
            Some(worktree_path),
            &observer,
        )))
    }

    /// Resolve the effective memory-store override for a run: the per-run
    /// `EngineRunConfig::memory_store_path` if the caller set one, else
    /// this engine's own `EngineConfig::memory_store_path` fallback (Task
    /// 12 M4 review). One fact, one place — both `start_run` and
    /// `resume_run` call this rather than each re-deriving the same
    /// "prefer per-run, fall back to engine-level" rule, which is exactly
    /// the kind of two-call-site invariant that drifts apart if it is
    /// spelled out twice.
    fn resolve_memory_store_path(&self, per_run: Option<PathBuf>) -> Option<PathBuf> {
        per_run.or_else(|| self.config.memory_store_path.clone())
    }

    /// Agent catalog a run launches with: its own (project-scoped) catalog
    /// when the launcher supplied one, else the engine-wide catalog.
    fn agent_registry_for(&self, run_config: &EngineRunConfig) -> Option<Arc<surge_acp::Registry>> {
        run_config
            .agent_registry
            .clone()
            .or_else(|| self.config.agent_registry.clone())
    }

    /// Seed the resolved profile registry as the `profile_catalog` run
    /// artifact so bootstrap profiles (`flow-generator@1.0` above all) can
    /// bind Agent nodes to profiles that actually exist
    /// (`profile_loader::render_profile_catalog`) instead of naming them
    /// from prompt text. A no-op when no registry is wired — `start_run`'s
    /// resolver-free default behavior is unchanged in that case.
    ///
    /// Runtimes whose provider quota is exhausted right now are marked
    /// unavailable in the catalog, so the plan routes around them instead of
    /// parking on the first stage bound to one.
    async fn seed_profile_catalog(
        &self,
        run_config: &mut EngineRunConfig,
    ) -> Result<(), EngineError> {
        let Some(registry) = self.config.profile_registry.as_ref() else {
            return Ok(());
        };
        let agents = run_config
            .agent_registry
            .clone()
            .or_else(|| self.config.agent_registry.clone())
            .unwrap_or_else(|| Arc::new(surge_acp::Registry::builtin()));
        let mut unavailable = std::collections::BTreeMap::new();
        for runtime in crate::profile_loader::catalog_runtimes(registry) {
            // A runtime that cannot even start (e.g. a required environment
            // variable is unset) would fail its stage at spawn.
            if let Some(entry) = agents.find_normalized(&runtime)
                && let Err(error) = surge_acp::agent_env::resolve(&entry.id, &entry.env)
            {
                unavailable.insert(runtime, format!("not configured: {error}"));
                continue;
            }
            if let Ok(status) = self.storage.capacity_status(&runtime).await
                && let Some(why) =
                    crate::engine::capacity::exhausted_reason(&status, chrono::Utc::now())
            {
                unavailable.insert(runtime, why);
            }
        }
        let catalog = crate::profile_loader::render_profile_catalog_with(registry, &unavailable);
        let seed = RunSeedArtifact::new(
            PROFILE_CATALOG_ARTIFACT_NAME,
            PROFILE_CATALOG_ARTIFACT_RELPATH,
            catalog,
            PROFILE_CATALOG_PRODUCER_NODE,
        )
        .map_err(|e| EngineError::Internal(format!("profile catalog producer key: {e}")))?;
        run_config.seed_artifacts.push(seed);
        Ok(())
    }

    async fn build_startup_events(
        &self,
        artifact_store: &surge_persistence::artifacts::ArtifactStore,
        run_id: RunId,
        graph: &Graph,
        worktree_path: &Path,
        run_config: &EngineRunConfig,
    ) -> Result<Vec<VersionedEventPayload>, EngineError> {
        let mut events = Self::startup_run_events(graph, worktree_path, run_config)?;
        events.extend(
            self.collect_startup_artifact_events(artifact_store, run_id, worktree_path, run_config)
                .await?,
        );
        Ok(events)
    }

    fn startup_run_events(
        graph: &Graph,
        worktree_path: &Path,
        run_config: &EngineRunConfig,
    ) -> Result<Vec<VersionedEventPayload>, EngineError> {
        use surge_core::approvals::ApprovalPolicy;
        use surge_core::content_hash::ContentHash;
        use surge_core::run_event::RunConfig as CoreRunConfig;
        use surge_core::sandbox::SandboxMode;

        let graph_bytes = serde_json::to_vec(graph)
            .map_err(|e| EngineError::Internal(format!("graph serialize: {e}")))?;
        let graph_hash = ContentHash::compute(&graph_bytes);
        let core_run_config = CoreRunConfig {
            bootstrap_edit_loop_cap: Some(run_config.bootstrap.edit_loop_cap),
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: run_config.mcp_servers.clone(),
            // Persist the frozen budget so a daemon-restart resume re-arms spend
            // enforcement instead of reverting to the unlimited default.
            budget: run_config.budget,
        };

        Ok(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree_path.to_path_buf(),
                initial_prompt: run_config.initial_prompt.clone(),
                config: core_run_config,
            }),
            VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                graph: Box::new(graph.clone()),
                graph_hash,
            }),
        ])
    }

    async fn collect_startup_artifact_events(
        &self,
        artifact_store: &surge_persistence::artifacts::ArtifactStore,
        run_id: RunId,
        worktree_path: &Path,
        run_config: &EngineRunConfig,
    ) -> Result<Vec<VersionedEventPayload>, EngineError> {
        let mut events = Vec::new();
        if let Some(parent_run_id) = run_config.bootstrap_parent {
            events.extend(
                self.bootstrap_parent_artifact_events(artifact_store, run_id, parent_run_id)
                    .await?,
            );
        }
        if let Some(event) =
            project_context_artifact_event(artifact_store, run_id, run_config).await?
        {
            events.push(event);
        }
        if let Some(event) =
            project_memory_artifact_event(artifact_store, run_id, run_config).await?
        {
            events.push(event);
        }
        if let Some(event) =
            memory_claim_candidates_artifact_event(artifact_store, run_id, run_config).await?
        {
            events.push(event);
        }
        events.extend(run_seed_artifact_events(run_id, worktree_path, run_config).await?);
        if let Some(event) =
            initial_prompt_artifact_event(run_id, worktree_path, run_config).await?
        {
            events.push(event);
        }
        Ok(events)
    }

    async fn bootstrap_parent_artifact_events(
        &self,
        artifact_store: &surge_persistence::artifacts::ArtifactStore,
        child_run_id: RunId,
        parent_run_id: RunId,
    ) -> Result<Vec<VersionedEventPayload>, EngineError> {
        use surge_core::keys::NodeKey;
        use surge_persistence::runs::EventSeq;

        const BOOTSTRAP_PARENT_NODE: &str = "bootstrap_parent";

        let reader = self
            .storage
            .open_run_reader(parent_run_id)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let current = reader
            .current_seq()
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let parent_events = reader
            .read_events(EventSeq(1)..current.next())
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        let (parent_worktree, artifacts) =
            collect_bootstrap_parent_artifacts(parent_events, parent_run_id)?;
        let inherited_node = NodeKey::try_from(BOOTSTRAP_PARENT_NODE).map_err(|e| {
            EngineError::Internal(format!("invalid bootstrap parent producer key: {e}"))
        })?;

        let mut inherited_events = Vec::with_capacity(BOOTSTRAP_PARENT_ARTIFACTS.len());
        for name in BOOTSTRAP_PARENT_ARTIFACTS {
            let artifact = artifacts.get(name).ok_or_else(|| {
                EngineError::Internal(format!(
                    "bootstrap parent {parent_run_id} is missing required artifact {name}"
                ))
            })?;
            let bytes = artifact_store
                .open_ref(parent_run_id, artifact, &parent_worktree)
                .await
                .map_err(|e| EngineError::Storage(e.to_string()))?;
            let child_ref = artifact_store
                .put(child_run_id, name, &bytes)
                .await
                .map_err(|e| EngineError::Storage(e.to_string()))?;

            tracing::debug!(
                target: "engine::startup",
                run_id = %child_run_id,
                bootstrap_parent = %parent_run_id,
                artifact = name,
                hash = %child_ref.hash,
                "inherited bootstrap artifact",
            );

            inherited_events.push(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: inherited_node.clone(),
                artifact: child_ref.hash,
                path: child_ref.path,
                name: name.to_owned(),
                source_path: None,
            }));
        }

        Ok(inherited_events)
    }

    async fn prepare_run_resume(
        &self,
        run_id: RunId,
        claim: Option<&surge_persistence::work_items::WorkItemLaunchClaim>,
    ) -> Result<
        (
            surge_persistence::runs::run_writer::RunWriter,
            super::replay::ReplayedState,
            bool,
        ),
        EngineError,
    > {
        use crate::engine::replay::replay;
        if let Some(claim) = claim {
            self.storage
                .work_items()
                .validate_claim(claim)
                .map_err(|error| EngineError::Storage(error.to_string()))?;
        } else if self
            .storage
            .work_items()
            .for_run(run_id)
            .map_err(|error| EngineError::Storage(error.to_string()))?
            .is_some()
        {
            return Err(EngineError::Storage(
                "task resume requires host ownership".into(),
            ));
        }

        tracing::info!(
            target: "surge.path.exercised",
            path = "engine",
            kind = "resume",
            run_id = %run_id,
            "entered engine path",
        );

        if self.runs.read().await.contains_key(&run_id) {
            return Err(EngineError::RunAlreadyActive(run_id));
        }

        // Task 12 M3 review, BLOCKING #3: a parked run's registry row
        // (`status = 'parked'`, `wake_at` = the original park time) must
        // not survive unchanged into this resumed execution — otherwise it
        // lies forever afterward: `due_parked` keeps returning it (a
        // second resume attempt racing this one), `snapshot_active_runs`
        // never sees it (its filter excludes `Parked`), and a crash right
        // after this resume is unrecoverable via the normal stale-pid path
        // (which only rewrites `Running`/`Bootstrapping`). Cleared here,
        // atomically with the status transition (`Storage::clear_parked`
        // — the same "both columns in one write" discipline
        // `set_run_parked` itself already applies), before anything else
        // about the resume proceeds. Also the source of truth for whether
        // this resumed run's first agent dispatch bypasses the capacity
        // precheck once (see `RunTaskParams::capacity_precheck_bypass_once`'s
        // doc) — a resumed *non*-parked run (e.g. crash recovery of a
        // `Crashed` row) gets no such bypass.
        // Neither of these two may be best-effort. If the row still reads
        // `parked` with its old `wake_at` while the run proceeds, the
        // `RunWokeFromPark` written below sets `parked_worktree = None` in
        // the event-log fold (`runs/query.rs`), so the next `due_parked`
        // scan selects this same — now *running* — run, finds no recorded
        // worktree, and `wake_scheduler`'s `fail_honestly` kills it. A
        // transient storage error would fail a healthy run, which is worse
        // than refusing to resume. Both the read and the clear therefore
        // propagate.
        let summary = self
            .storage
            .get_run(&run_id)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let was_parked = matches!(&summary, Some(s) if s.status == surge_core::RunStatus::Parked);
        if was_parked {
            self.storage
                .clear_parked(&run_id, surge_core::RunStatus::Running)
                .await
                .map_err(|e| {
                    EngineError::Storage(format!(
                        "resume refused: could not clear the Parked registry row for \
                         {run_id} ({e}); resuming would leave it due for the wake scan, \
                         which would then fail this run for an unrecorded worktree"
                    ))
                })?;
        }

        let writer = self
            .storage
            .open_run_writer(run_id)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        self.spawn_event_forwarder(run_id, &writer);

        // Task 12 M4: `RunWokeFromPark` is declared (`run_event.rs`) and
        // folded (`run_state.rs` clears `RunState::Pipeline.parked`, which
        // is what turns off `Attention::Waiting`), but nothing wrote it —
        // this is that write. `Engine::resume_run` is the single choke
        // point every real wake goes through (the daemon's wake scheduler
        // and crash recovery's due-parked fallthrough both resume only via
        // `server::resume_run_tracked` → this method), so writing it here,
        // gated on the *same* `was_parked` fact that already clears the
        // registry row above, covers both triggers without a second write
        // site. Best-effort like `RunParked`'s own append in
        // `run_task.rs::parked` — a failed append here must not abort an
        // otherwise-successful resume.
        if was_parked
            && let Err(error) = writer
                .append_event(VersionedEventPayload::new(EventPayload::RunWokeFromPark {}))
                .await
        {
            tracing::warn!(
                target: "engine::capacity",
                %run_id,
                %error,
                "failed to append RunWokeFromPark; the run resumed but its log will not show \
                 when it woke, and RunState::Pipeline.parked will not fold clear"
            );
        }

        let reader = self
            .storage
            .open_run_reader(run_id)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        let replayed = replay(&reader).await?;

        Ok((writer, replayed, was_parked))
    }

    fn capacity_continue_reserved(&self, run_id: RunId) -> Result<bool, EngineError> {
        Ok(self
            .storage
            .work_items()
            .execution_control(run_id)
            .map_err(|error| EngineError::Storage(error.to_string()))?
            .is_some_and(|control| {
                control.state
                    == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
                    && control.fence.as_ref().is_some_and(|fence| {
                        matches!(
                            fence.reason,
                            surge_core::execution_recovery::SuspensionReason::Capacity { .. }
                        )
                    })
            }))
    }

    /// Resume an existing run from its latest snapshot + event tail.
    ///
    /// Opens the persisted event log, replays snapshots and events to
    /// reconstruct the last known cursor and memory, then resumes execution
    /// from that point. Returns immediately if the run is already active in
    /// this process.
    #[allow(clippy::too_many_lines)]
    pub async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        self.resume_run_owned(run_id, worktree_path, None).await
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
        let attempt = self
            .storage
            .work_items()
            .validate_claim(claim)
            .map_err(|error| EngineError::Storage(error.to_string()))?;
        if attempt.state == surge_core::work_item::WorkItemAttemptState::Suspended {
            let control = self
                .storage
                .work_items()
                .execution_control(claim.run())
                .map_err(|error| EngineError::Storage(error.to_string()))?;
            if !control.is_some_and(|control| {
                control.state
                    == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
                    && control.attempt_generation == claim.binding().generation
            }) {
                return Err(EngineError::Storage(
                    "suspended task requires its durable Continue reservation".into(),
                ));
            }
        }
        let path = super::work_items::validate_resume(&self.storage, claim).await?;
        self.resume_run_owned(claim.run(), path, Some(claim)).await
    }

    async fn resume_run_owned(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
        claim: Option<&surge_persistence::work_items::WorkItemLaunchClaim>,
    ) -> Result<RunHandle, EngineError> {
        use crate::engine::run_task::{RunTaskParams, execute};
        use tokio::sync::broadcast;
        let capacity_continue = claim.is_some() && self.capacity_continue_reserved(run_id)?;
        let (writer, mut replayed, was_parked) = self.prepare_run_resume(run_id, claim).await?;

        // If the run already reached a terminal state, return a handle that
        // resolves immediately without re-executing any stages.
        if let Some(terminal_outcome) = replayed.already_terminal {
            drop(writer);
            return Ok(terminal_resume_handle(run_id, terminal_outcome));
        }

        // A committed unanswered gate survives an explicit Suspend/Continue
        // with its original identity. After an unplanned owner loss, retire
        // only unanswered HumanGate occurrences so a stale card cannot
        // authorize the resumed run; normal stage execution emits a fresh ID.
        if claim.is_none() {
            replayed.memory.gate_decisions.retain(|_, decision| {
                decision.response.is_some()
                    || decision.suspended
                    || decision.purpose != surge_core::run_state::GateDecisionPurpose::HumanGate
            });
        }

        let (event_tx, event_rx) = broadcast::channel(256);
        let registration = self.register_active_run(run_id).await;

        // Reconstruct EngineRunConfig from the persisted RunConfig so that
        // mcp_servers and the frozen budget survive a daemon restart + resume.
        // Falls back to defaults (empty mcp list, unlimited budget) for runs that
        // predate those fields.
        let mut resume_run_config = if let Some(claim) = claim {
            super::work_items::pinned(&self.storage, claim)?.3
        } else {
            EngineRunConfig::default()
        };
        if let Some(persisted) = &replayed.run_config {
            resume_run_config
                .mcp_servers
                .clone_from(&persisted.mcp_servers);
            // Re-arm spend enforcement with the run's frozen budget.
            resume_run_config.budget = persisted.budget;
            if let Some(cap) = persisted.bootstrap_edit_loop_cap {
                resume_run_config.bootstrap.edit_loop_cap = cap;
            }
        }
        crate::engine::validate::validate_quota_policy(
            &replayed.graph,
            &resume_run_config.quota_recovery,
            self.config.profile_registry.as_deref(),
        )?;
        // Task 12 M4 review: `memory_store_path` is deliberately never
        // persisted (see `EngineRunConfig::memory_store_path`'s own doc),
        // so there is nothing to recover from `replayed.run_config` above —
        // this resumed run's only source for it is the engine-level
        // fallback, the same one `start_run` consults.
        resume_run_config.memory_store_path =
            self.resolve_memory_store_path(resume_run_config.memory_store_path.take());

        // Build a per-run McpRegistry exactly like start_run does.
        let per_run_mcp_registry =
            self.resolve_run_mcp_registry(&resume_run_config.mcp_servers, &worktree_path, &writer);
        let mcp_servers_for_resume = resume_run_config.mcp_servers.clone();
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(self.storage.home().join("runs"));

        // Captured before `worktree_path` moves into params (post-completion
        // task-ledger mirror; matches RunStarted's project_path).
        let project_path_for_ledger = worktree_path.clone();
        let (capacity_ledger, capacity_estimator) = self.capacity_ports_for(&writer);
        let run_agent_registry = self.agent_registry_for(&resume_run_config);
        let params = RunTaskParams {
            work_item_claim: claim.cloned(),
            pending_suspension: None,
            run_id,
            writer,
            artifact_store,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            notify_deliverer: self.notify_deliverer.clone(),
            graph: replayed.graph,
            worktree_path,
            run_config: resume_run_config,
            event_tx,
            cancel: registration.cancel,
            resume_cursor: Some(replayed.cursor),
            resume_memory: Some(replayed.memory),
            resume_memory_applied_seq: Some(replayed.memory_applied_seq),
            resume_frames: Some(replayed.frames),
            resume_root_traversal_counts: Some(replayed.root_traversal_counts),
            resume_applied_graph_revision_seq: Some(replayed.applied_graph_revision_seq),
            gate_resolutions: registration.gate_resolutions,
            tool_resolutions: registration.tool_resolutions,
            roadmap_amendments: registration.roadmap_amendment_rx,
            pending_elevations: registration.pending_elevations,
            pending_steers: registration.pending_steers,
            mcp_registry: per_run_mcp_registry,
            mcp_servers: mcp_servers_for_resume,
            profile_registry: self.config.profile_registry.clone(),
            agent_registry: run_agent_registry.clone(),
            capacity_ledger,
            capacity_estimator,
            capacity_policy: self.config.capacity.clone(),
            storage: self.storage.clone(),
            capacity_precheck_bypass_once: std::sync::atomic::AtomicBool::new(
                was_parked || capacity_continue,
            ),
        };

        let runs_for_cleanup = self.runs.clone();
        let storage_for_ledger = self.storage.clone();
        let join = tokio::spawn(async move {
            let outcome = execute(params).await;
            runs_for_cleanup.write().await.remove(&run_id);
            record_terminal_status(&storage_for_ledger, run_id, &outcome).await;
            mirror_task_ledger(&storage_for_ledger, run_id, &project_path_for_ledger).await;
            outcome
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
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
    let _ = event_tx.send(crate::engine::handle::EngineRunEvent::Terminal {
        outcome: outcome.clone(),
    });
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

async fn project_context_artifact_event(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    let Some(seed) = &run_config.project_context else {
        return Ok(None);
    };
    let project_context_artifact = artifact_store
        .put(
            run_id,
            PROJECT_CONTEXT_ARTIFACT_NAME,
            seed.content.as_bytes(),
        )
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    let producer = surge_core::keys::NodeKey::try_from(PROJECT_CONTEXT_PRODUCER_NODE)
        .map_err(|e| EngineError::Internal(format!("project context producer key: {e}")))?;
    tracing::debug!(
        target: "engine::startup",
        run_id = %run_id,
        path = %seed.path.display(),
        bytes = seed.content.len(),
        hash = %project_context_artifact.hash,
        "seeded project_context artifact"
    );
    tracing::info!(
        target: "engine::startup",
        run_id = %run_id,
        path = %seed.path.display(),
        hash = %project_context_artifact.hash,
        "project context captured for run"
    );
    Ok(Some(artifact_produced_event(
        producer,
        project_context_artifact.hash,
        project_context_artifact.path,
        PROJECT_CONTEXT_ARTIFACT_NAME,
    )))
}

async fn project_memory_artifact_event(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    let Some(seed) = &run_config.project_memory else {
        return Ok(None);
    };
    let artifact = artifact_store
        .put(
            run_id,
            PROJECT_MEMORY_ARTIFACT_NAME,
            seed.content.as_bytes(),
        )
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    let producer = surge_core::keys::NodeKey::try_from(PROJECT_CONTEXT_PRODUCER_NODE)
        .map_err(|e| EngineError::Internal(format!("project memory producer key: {e}")))?;
    tracing::info!(
        target: "engine::startup",
        run_id = %run_id,
        path = %seed.path.display(),
        hash = %artifact.hash,
        "project memory captured for run"
    );
    Ok(Some(artifact_produced_event(
        producer,
        artifact.hash,
        artifact.path,
        PROJECT_MEMORY_ARTIFACT_NAME,
    )))
}

async fn memory_claim_candidates_artifact_event(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    let Some(candidates) = &run_config.memory_claim_candidates else {
        return Ok(None);
    };
    let snapshot = crate::project_context::MemoryClaimSnapshot {
        budget: run_config.context_pack.unwrap_or_default(),
        claims: candidates.clone(),
    };
    let body = serde_json::to_vec(&snapshot).map_err(|error| {
        EngineError::Internal(format!("memory claim snapshot serialize: {error}"))
    })?;
    let artifact = artifact_store
        .put(run_id, MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME, &body)
        .await
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    let producer = surge_core::keys::NodeKey::try_from(PROJECT_CONTEXT_PRODUCER_NODE)
        .map_err(|error| EngineError::Internal(format!("project memory producer key: {error}")))?;
    Ok(Some(artifact_produced_event(
        producer,
        artifact.hash,
        artifact.path,
        MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME,
    )))
}

async fn run_seed_artifact_events(
    run_id: RunId,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Vec<VersionedEventPayload>, EngineError> {
    let mut events = Vec::with_capacity(run_config.seed_artifacts.len());
    for seed in &run_config.seed_artifacts {
        let seeded_artifact = synthesise_run_seed_artifact(worktree_path, seed)
            .await
            .map_err(|e| {
                EngineError::Internal(format!("seed artifact {} synthesis failed: {e}", seed.name))
            })?;
        tracing::debug!(
            target: "engine::startup",
            run_id = %run_id,
            name = seed.name.as_str(),
            path = %seed.relative_path.display(),
            bytes = seed.content.len(),
            hash = %seeded_artifact.hash,
            "seeded run artifact"
        );
        events.push(artifact_produced_event(
            seeded_artifact.producer,
            seeded_artifact.hash,
            seeded_artifact.relative_path,
            &seed.name,
        ));
    }
    Ok(events)
}

async fn append_task_binding(
    store: &surge_persistence::artifacts::ArtifactStore,
    run: RunId,
    context: surge_core::work_item::WorkItemContext,
    events: &mut Vec<VersionedEventPayload>,
) -> Result<(), EngineError> {
    let bytes = serde_json::to_vec(context.requirements())
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    let artifact = store
        .put(run, "accepted_requirements", &bytes)
        .await
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    events.push(artifact_produced_event(
        "task_requirements"
            .parse()
            .map_err(|error: surge_core::keys::KeyParseError| {
                EngineError::Internal(error.to_string())
            })?,
        artifact.hash,
        artifact.path,
        "accepted_requirements",
    ));
    events.push(VersionedEventPayload::new(
        EventPayload::WorkItemAttemptBound { context },
    ));
    Ok(())
}

async fn initial_prompt_artifact_event(
    run_id: RunId,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    if run_config.initial_prompt.is_empty() {
        return Ok(None);
    }
    let prompt_artifact =
        synthesise_initial_prompt_artifact(worktree_path, run_config.initial_prompt.as_bytes())
            .await
            .map_err(|e| {
                EngineError::Internal(format!("initial-prompt artifact synthesis failed: {e}"))
            })?;
    tracing::debug!(
        target: "engine::startup",
        run_id = %run_id,
        prompt_len = run_config.initial_prompt.len(),
        "seeded user_prompt artifact",
    );
    Ok(Some(artifact_produced_event(
        prompt_artifact.producer,
        prompt_artifact.hash,
        prompt_artifact.relative_path,
        INITIAL_PROMPT_ARTIFACT_NAME,
    )))
}

fn artifact_produced_event(
    node: surge_core::keys::NodeKey,
    artifact: surge_core::content_hash::ContentHash,
    path: PathBuf,
    name: &str,
) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::ArtifactProduced {
        node,
        artifact,
        path,
        name: name.to_owned(),
        source_path: None,
    })
}

/// Canonical artifact name under which the run's free-form initial prompt is
/// surfaced to agent stages. Bootstrap profiles bind to this name (either via
/// `ArtifactSource::InitialPrompt` or `ArtifactSource::RunArtifact { name }`).
/// The parent run's worktree and the description / roadmap / flow artifacts a
/// follow-up run inherits, keyed by logical name.
///
/// The roadmap planner emits the roadmap under an alias (`roadmap_toml`); it is
/// inherited as `roadmap`, refusing competing aliases rather than guessing.
fn collect_bootstrap_parent_artifacts(
    parent_events: Vec<surge_persistence::runs::ReadEvent>,
    parent_run_id: RunId,
) -> Result<
    (
        std::path::PathBuf,
        std::collections::BTreeMap<String, surge_core::run_state::ArtifactRef>,
    ),
    EngineError,
> {
    use surge_core::run_state::ArtifactRef;

    let mut parent_worktree = None;
    let mut artifacts = std::collections::BTreeMap::new();
    for event in parent_events {
        match event.payload.payload {
            EventPayload::RunStarted { project_path, .. } => {
                parent_worktree = Some(project_path);
            },
            EventPayload::ArtifactProduced {
                node,
                artifact,
                path,
                name,
                ..
            } if BOOTSTRAP_PARENT_ARTIFACTS.contains(&name.as_str())
                || crate::bootstrap_driver::is_roadmap_toml_alias(&name) =>
            {
                artifacts.insert(
                    name.clone(),
                    ArtifactRef {
                        hash: artifact,
                        path,
                        name,
                        produced_by: node,
                        produced_at_seq: event.seq.as_u64(),
                    },
                );
            },
            _ => {},
        }
    }

    if let Some(roadmap) =
        crate::bootstrap_driver::canonical_roadmap_ref(artifacts.values(), "roadmap")
            .map_err(|()| {
                EngineError::Internal(format!(
                    "bootstrap parent {parent_run_id} has competing roadmap artifacts"
                ))
            })?
            .cloned()
    {
        artifacts.insert("roadmap".to_owned(), roadmap);
    }

    let parent_worktree = parent_worktree.ok_or_else(|| {
        EngineError::Internal(format!(
            "bootstrap parent {parent_run_id} has no RunStarted event"
        ))
    })?;
    Ok((parent_worktree, artifacts))
}

/// Logical names a follow-up run inherits from its bootstrap parent.
const BOOTSTRAP_PARENT_ARTIFACTS: [&str; 3] = ["description", "roadmap", "flow"];

pub(crate) const INITIAL_PROMPT_ARTIFACT_NAME: &str = "user_prompt";

/// Canonical artifact name for the stable project context captured at run start.
pub(crate) const PROJECT_CONTEXT_ARTIFACT_NAME: &str = "project_context";

/// Canonical artifact name for the accumulating project memory captured at
/// run start (`.surge/memory/`).
pub(crate) const PROJECT_MEMORY_ARTIFACT_NAME: &str = "project_memory";

/// Artifact containing immutable source claims for per-node context packs.
pub const MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME: &str = "memory_claim_candidates";

/// Canonical artifact name for the resolved profile-registry catalogue
/// (`profile_loader::render_profile_catalog`) seeded at run start.
/// `flow-generator@1.0` binds this to pick profiles by what the registry
/// actually holds instead of from names memorized in its own prompt.
pub(crate) const PROFILE_CATALOG_ARTIFACT_NAME: &str = "profile_catalog";

/// Synthetic producer node for the run-level project context seed.
const PROJECT_CONTEXT_PRODUCER_NODE: &str = "project_context_seed";

/// Relative path within the worktree where the seeded profile catalogue is
/// stored.
const PROFILE_CATALOG_ARTIFACT_RELPATH: &str = ".surge/profile_catalog.md";

/// Synthetic producer node id recorded on the seeded profile-catalogue
/// `ArtifactProduced` event. No real node produces it — same reasoning as
/// `PROJECT_CONTEXT_PRODUCER_NODE` above.
const PROFILE_CATALOG_PRODUCER_NODE: &str = "profile_catalog_seed";

/// Relative path within the worktree where the seeded prompt body is stored.
const INITIAL_PROMPT_ARTIFACT_RELPATH: &str = ".surge/user_prompt.txt";

/// Write a finished run's terminal status into the cross-run registry.
///
/// The per-run event log is the source of truth, but `surge engine ls`,
/// liveness probes and every registry reader otherwise kept seeing
/// `running`/`bootstrapping` until the next daemon restart ran recovery.
/// Parked runs are written by `set_run_parked`; a stream error is not a
/// terminal fact, so neither is recorded here. Best-effort, like the ledger
/// mirror: a failed write is logged and never changes the outcome.
async fn record_terminal_status(
    storage: &Arc<surge_persistence::runs::Storage>,
    run_id: RunId,
    outcome: &crate::engine::handle::RunOutcome,
) {
    use crate::engine::handle::RunOutcome;
    use surge_core::RunStatus;
    let status = match outcome {
        RunOutcome::Completed { .. } => RunStatus::Completed,
        RunOutcome::Failed { .. } => RunStatus::Failed,
        RunOutcome::Aborted { .. } => RunStatus::Aborted,
        _ => return,
    };
    let ended_at_ms = chrono::Utc::now().timestamp_millis();
    if let Err(error) = storage
        .set_run_status(&run_id, status, Some(ended_at_ms))
        .await
    {
        tracing::warn!(%run_id, ?status, %error, "could not record terminal run status in registry");
    }
}

/// Synthetic producer node id recorded on the seeded `ArtifactProduced` event.
/// Bootstrap graphs do not have a real `start_node` user node, so the
/// engine attributes the prompt to a stable synthetic key.
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

const INITIAL_PROMPT_PRODUCER_NODE: &str = "start_node";

/// Output of [`synthesise_initial_prompt_artifact`].
pub(crate) struct InitialPromptArtifact {
    pub hash: surge_core::content_hash::ContentHash,
    pub relative_path: PathBuf,
    pub producer: surge_core::keys::NodeKey,
}

/// Persist the operator-supplied prompt body to a stable location inside the
/// run's worktree and compute the metadata needed to record an
/// `ArtifactProduced` event. Returns the content hash, the relative on-disk
/// path, and the synthetic producer node key.
///
/// Writes `<worktree>/.surge/user_prompt.txt`, creating the parent directory
/// when missing. Idempotent for identical content (the file is overwritten
/// each call, and `ContentHash::compute` is purely a function of bytes).
pub(crate) async fn synthesise_initial_prompt_artifact(
    worktree_path: &std::path::Path,
    prompt_bytes: &[u8],
) -> std::io::Result<InitialPromptArtifact> {
    use surge_core::content_hash::ContentHash;
    use surge_core::keys::NodeKey;

    let relative_path = PathBuf::from(INITIAL_PROMPT_ARTIFACT_RELPATH);
    let absolute_path = worktree_path.join(&relative_path);
    if let Some(parent) = absolute_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&absolute_path, prompt_bytes).await?;
    let hash = ContentHash::compute(prompt_bytes);
    let producer = NodeKey::try_from(INITIAL_PROMPT_PRODUCER_NODE).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("initial prompt producer node key: {e}"),
        )
    })?;
    Ok(InitialPromptArtifact {
        hash,
        relative_path,
        producer,
    })
}

pub(crate) async fn synthesise_run_seed_artifact(
    worktree_path: &std::path::Path,
    seed: &crate::engine::config::RunSeedArtifact,
) -> std::io::Result<InitialPromptArtifact> {
    use std::path::Component;

    if seed.relative_path.is_absolute()
        || seed.relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "seed artifact path must stay inside the worktree: {}",
                seed.relative_path.display()
            ),
        ));
    }

    let absolute_path = worktree_path.join(&seed.relative_path);
    if let Some(parent) = absolute_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&absolute_path, seed.content.as_bytes()).await?;
    let hash = surge_core::ContentHash::compute(seed.content.as_bytes());
    Ok(InitialPromptArtifact {
        hash,
        relative_path: seed.relative_path.clone(),
        producer: seed.producer.clone(),
    })
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
mod memory_claim_snapshot_artifact_tests {
    use super::*;

    #[tokio::test]
    async fn frozen_candidates_are_stored_as_a_reportable_run_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let store = surge_persistence::artifacts::ArtifactStore::new(temp.path());
        let run_id = RunId::new();
        let config = EngineRunConfig {
            memory_claim_candidates: Some(Vec::new()),
            ..EngineRunConfig::default()
        };

        let event = memory_claim_candidates_artifact_event(&store, run_id, &config)
            .await
            .unwrap()
            .unwrap();
        let surge_core::run_event::EventPayload::ArtifactProduced { artifact, name, .. } =
            event.payload
        else {
            panic!("candidate snapshot must be linked into replay as an artifact event");
        };
        assert_eq!(name, MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME);
        let bytes = store.open(run_id, artifact).await.unwrap();
        let snapshot: crate::project_context::MemoryClaimSnapshot =
            serde_json::from_slice(&bytes).unwrap();
        assert!(snapshot.claims.is_empty());
        assert_eq!(
            snapshot.budget,
            surge_core::context_pack::ContextPackConfig::default()
        );
    }
}
