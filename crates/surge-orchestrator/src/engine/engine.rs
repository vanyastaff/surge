//! `Engine` — the public API. Methods are stubbed in this task and
//! implemented incrementally in Phase 5 (lifecycle), Phase 9 (resolve),
//! Phase 11 (stop).

use crate::engine::config::{EngineConfig, EngineRunConfig};
use crate::engine::error::EngineError;
use crate::engine::handle::RunHandle;
use crate::engine::tools::ToolDispatcher;
use crate::profile_loader::ProfileRegistry;
use crate::roadmap_amendment::ActiveRunAmendmentOutcome;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;
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
}

pub(crate) struct ActiveRun {
    pub cancel: tokio_util::sync::CancellationToken,
    pub gate_resolutions: Arc<
        tokio::sync::Mutex<
            HashMap<
                surge_core::keys::NodeKey,
                tokio::sync::oneshot::Sender<crate::engine::stage::human_gate::HumanGateResolution>,
            >,
        >,
    >,
    pub tool_resolutions:
        Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    pub roadmap_amendments:
        tokio::sync::mpsc::Sender<crate::engine::run_task::RoadmapAmendmentCommand>,
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
        Self {
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            config: Arc::new(config),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
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
        Self {
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            config: Arc::new(config),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
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

    /// Start a new run.
    #[allow(clippy::too_many_lines)]
    pub async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        use crate::engine::handle::RunHandle;
        use crate::engine::run_task::{RunTaskParams, execute};
        use crate::engine::validate::validate_for_m6;
        use surge_core::approvals::ApprovalPolicy;
        use surge_core::content_hash::ContentHash;
        use surge_core::run_event::{
            EventPayload, RunConfig as CoreRunConfig, VersionedEventPayload,
        };
        use surge_core::sandbox::SandboxMode;
        use tokio::sync::broadcast;
        use tokio_util::sync::CancellationToken;

        validate_for_m6(&graph)?;

        if self.runs.read().await.contains_key(&run_id) {
            return Err(EngineError::RunAlreadyActive(run_id));
        }

        if !worktree_path.exists() {
            return Err(EngineError::WorktreeMissing(worktree_path));
        }

        let writer = self
            .storage
            .create_run(run_id, &worktree_path, None)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(self.storage.home().join("runs"));

        // Emit RunStarted + PipelineMaterialized atomically.
        let core_run_config = CoreRunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: run_config.mcp_servers.clone(),
        };
        let graph_bytes = serde_json::to_vec(&graph)
            .map_err(|e| EngineError::Internal(format!("graph serialize: {e}")))?;
        let graph_hash = ContentHash::compute(&graph_bytes);

        let mut events = vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree_path.clone(),
                initial_prompt: run_config.initial_prompt.clone(),
                config: core_run_config,
            }),
            VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                graph: Box::new(graph.clone()),
                graph_hash,
            }),
        ];

        if let Some(parent_run_id) = run_config.bootstrap_parent {
            let inherited = self
                .bootstrap_parent_artifact_events(&artifact_store, run_id, parent_run_id)
                .await?;
            events.extend(inherited);
        }

        if let Some(seed) = &run_config.project_context {
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
            events.push(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: producer,
                artifact: project_context_artifact.hash,
                path: project_context_artifact.path,
                name: PROJECT_CONTEXT_ARTIFACT_NAME.to_string(),
            }));
        }

        for seed in &run_config.seed_artifacts {
            let seeded_artifact = synthesise_run_seed_artifact(&worktree_path, seed)
                .await
                .map_err(|e| {
                    EngineError::Internal(format!(
                        "seed artifact {} synthesis failed: {e}",
                        seed.name
                    ))
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
            events.push(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: seeded_artifact.producer,
                artifact: seeded_artifact.hash,
                path: seeded_artifact.relative_path,
                name: seed.name.clone(),
            }));
        }

        // Surface the operator's free-form prompt as a first-class artifact so
        // bootstrap (or any) agent stage can pull it via the standard binding
        // path through `ArtifactSource::InitialPrompt` (and equivalently via
        // `ArtifactSource::RunArtifact { name: "user_prompt" }`). The artifact
        // body is stored at `<worktree>/.surge/user_prompt.txt`; the
        // `ArtifactProduced` event records its content hash, relative path,
        // and a synthetic producer node so the existing fold rule populates
        // `RunMemory.artifacts["user_prompt"]` deterministically.
        if !run_config.initial_prompt.is_empty() {
            let prompt_artifact = synthesise_initial_prompt_artifact(
                &worktree_path,
                run_config.initial_prompt.as_bytes(),
            )
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
            events.push(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: prompt_artifact.producer,
                artifact: prompt_artifact.hash,
                path: prompt_artifact.relative_path,
                name: INITIAL_PROMPT_ARTIFACT_NAME.to_string(),
            }));
        }

        writer
            .append_events(events)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        // Per-run MCP registry: prefer the run-config-supplied list over
        // the engine-level fallback. If `run_config.mcp_servers` is
        // non-empty a fresh `McpRegistry` is built for this run; otherwise
        // we fall back to the engine-wide registry (typically `None` for
        // daemon mode, where the per-run list is the only source).
        let per_run_mcp_registry = if run_config.mcp_servers.is_empty() {
            self.mcp_registry.clone()
        } else {
            Some(Arc::new(surge_mcp::McpRegistry::from_config(
                &run_config.mcp_servers,
            )))
        };
        let mcp_servers_clone = run_config.mcp_servers.clone();

        let (event_tx, event_rx) = broadcast::channel(256);
        let cancel = CancellationToken::new();

        let gate_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let tool_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let (roadmap_amendment_tx, roadmap_amendment_rx) = tokio::sync::mpsc::channel(16);
        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
            roadmap_amendments: roadmap_amendment_tx,
        };
        self.runs.write().await.insert(run_id, active);

        let params = RunTaskParams {
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
            cancel,
            resume_cursor: None,
            resume_memory: None,
            resume_frames: None,
            resume_root_traversal_counts: None,
            gate_resolutions,
            tool_resolutions,
            roadmap_amendments: roadmap_amendment_rx,
            mcp_registry: per_run_mcp_registry,
            mcp_servers: mcp_servers_clone,
            profile_registry: self.config.profile_registry.clone(),
        };

        let runs_for_cleanup = self.runs.clone();
        let join = tokio::spawn(async move {
            let outcome = execute(params).await;
            runs_for_cleanup.write().await.remove(&run_id);
            outcome
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    async fn bootstrap_parent_artifact_events(
        &self,
        artifact_store: &surge_persistence::artifacts::ArtifactStore,
        child_run_id: RunId,
        parent_run_id: RunId,
    ) -> Result<Vec<VersionedEventPayload>, EngineError> {
        use surge_core::keys::NodeKey;
        use surge_core::run_state::ArtifactRef;
        use surge_persistence::runs::EventSeq;

        const BOOTSTRAP_PARENT_ARTIFACTS: [&str; 3] = ["description", "roadmap", "flow"];
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
                } if BOOTSTRAP_PARENT_ARTIFACTS.contains(&name.as_str()) => {
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

        let parent_worktree = parent_worktree.ok_or_else(|| {
            EngineError::Internal(format!(
                "bootstrap parent {parent_run_id} has no RunStarted event"
            ))
        })?;
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
            }));
        }

        Ok(inherited_events)
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
        use crate::engine::handle::RunHandle;
        use crate::engine::replay::replay;
        use crate::engine::run_task::{RunTaskParams, execute};
        use tokio::sync::broadcast;
        use tokio_util::sync::CancellationToken;

        if self.runs.read().await.contains_key(&run_id) {
            return Err(EngineError::RunAlreadyActive(run_id));
        }

        let writer = self
            .storage
            .open_run_writer(run_id)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        let reader = self
            .storage
            .open_run_reader(run_id)
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        let replayed = replay(&reader).await?;

        // If the run already reached a terminal state, return a handle that
        // resolves immediately without re-executing any stages.
        if let Some(terminal_outcome) = replayed.already_terminal {
            // Drop the writer; we won't be writing anything.
            drop(writer);
            let (event_tx, event_rx) = broadcast::channel(1);
            // Immediately send the terminal event (best-effort; receiver may
            // not be listening yet, which is fine — the future resolves).
            let _ = event_tx.send(crate::engine::handle::EngineRunEvent::Terminal {
                outcome: terminal_outcome.clone(),
            });
            let join = tokio::spawn(async move { terminal_outcome });
            return Ok(RunHandle {
                run_id,
                events: event_rx,
                completion: join,
            });
        }

        let (event_tx, event_rx) = broadcast::channel(256);
        let cancel = CancellationToken::new();
        let gate_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let tool_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let (roadmap_amendment_tx, roadmap_amendment_rx) = tokio::sync::mpsc::channel(16);

        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
            roadmap_amendments: roadmap_amendment_tx,
        };
        self.runs.write().await.insert(run_id, active);

        // Reconstruct EngineRunConfig from the persisted RunConfig so that
        // mcp_servers survive a daemon restart + resume. Falls back to an
        // empty list for runs that predate the mcp_servers field.
        let mut resume_run_config = EngineRunConfig::default();
        if let Some(persisted) = &replayed.run_config {
            resume_run_config
                .mcp_servers
                .clone_from(&persisted.mcp_servers);
        }

        // Build a per-run McpRegistry exactly like start_run does.
        let per_run_mcp_registry = if resume_run_config.mcp_servers.is_empty() {
            self.mcp_registry.clone()
        } else {
            Some(Arc::new(surge_mcp::McpRegistry::from_config(
                &resume_run_config.mcp_servers,
            )))
        };
        let mcp_servers_for_resume = resume_run_config.mcp_servers.clone();
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(self.storage.home().join("runs"));

        let params = RunTaskParams {
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
            cancel,
            resume_cursor: Some(replayed.cursor),
            resume_memory: Some(replayed.memory),
            resume_frames: None,
            resume_root_traversal_counts: None,
            gate_resolutions,
            tool_resolutions,
            roadmap_amendments: roadmap_amendment_rx,
            mcp_registry: per_run_mcp_registry,
            mcp_servers: mcp_servers_for_resume,
            profile_registry: self.config.profile_registry.clone(),
        };

        let runs_for_cleanup = self.runs.clone();
        let join = tokio::spawn(async move {
            let outcome = execute(params).await;
            runs_for_cleanup.write().await.remove(&run_id);
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
            // HumanGate resolution. Look up by extracting outcome from response.
            let outcome_str = response
                .get("outcome")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    EngineError::Internal("HumanGate resolution missing 'outcome' field".into())
                })?;
            let outcome = surge_core::keys::OutcomeKey::try_from(outcome_str)
                .map_err(|e| EngineError::Internal(format!("invalid outcome: {e}")))?;

            // M5 simplification: only one HumanGate active per run at a
            // time, so take the first entry from the map.
            let mut gates = active.gate_resolutions.lock().await;
            let key = gates.keys().next().cloned();
            if let Some(k) = key {
                let Some(tx) = gates.remove(&k) else {
                    return Err(EngineError::Internal(format!(
                        "pending HumanGate disappeared before resolving {k:?}"
                    )));
                };
                tx.send(crate::engine::stage::human_gate::HumanGateResolution {
                    outcome,
                    response,
                })
                .map_err(|_| EngineError::Internal("gate resolution receiver dropped".into()))?;
                Ok(())
            } else {
                Err(EngineError::Internal(
                    "no pending HumanGate to resolve".into(),
                ))
            }
        }
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

// Suppress unused-field warning for config until Phase 6+ uses it.
#[allow(dead_code)]
fn _engine_config_used(e: &Engine) {
    let _ = &e.config;
}

/// Canonical artifact name under which the run's free-form initial prompt is
/// surfaced to agent stages. Bootstrap profiles bind to this name (either via
/// `ArtifactSource::InitialPrompt` or `ArtifactSource::RunArtifact { name }`).
pub(crate) const INITIAL_PROMPT_ARTIFACT_NAME: &str = "user_prompt";

/// Canonical artifact name for the stable project context captured at run start.
pub(crate) const PROJECT_CONTEXT_ARTIFACT_NAME: &str = "project_context";

/// Synthetic producer node for the run-level project context seed.
const PROJECT_CONTEXT_PRODUCER_NODE: &str = "project_context_seed";

/// Relative path within the worktree where the seeded prompt body is stored.
const INITIAL_PROMPT_ARTIFACT_RELPATH: &str = ".surge/user_prompt.txt";

/// Synthetic producer node id recorded on the seeded `ArtifactProduced` event.
/// Bootstrap graphs do not have a real `start_node` user node, so the
/// engine attributes the prompt to a stable synthetic key.
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
    Ok(InitialPromptArtifact {
        hash: seed.hash,
        relative_path: seed.relative_path.clone(),
        producer: seed.producer.clone(),
    })
}
