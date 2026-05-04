//! `Engine` — the public API. Methods are stubbed in this task and
//! implemented incrementally in Phase 5 (lifecycle), Phase 9 (resolve),
//! Phase 11 (stop).

use crate::engine::config::{EngineConfig, EngineRunConfig};
use crate::engine::error::EngineError;
use crate::engine::handle::RunHandle;
use crate::engine::tools::ToolDispatcher;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;

/// Central orchestration engine. Drives one or more concurrent runs, each
/// executing a frozen [`Graph`] through ACP sessions and persistence writes.
pub struct Engine {
    bridge: Arc<dyn BridgeFacade>,
    storage: Arc<surge_persistence::runs::Storage>,
    tool_dispatcher: Arc<dyn ToolDispatcher>,
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
}

impl Engine {
    /// Create a new `Engine` wired to the given bridge, storage, and tool dispatcher.
    pub fn new(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        config: EngineConfig,
    ) -> Self {
        Self {
            bridge,
            storage,
            tool_dispatcher,
            config: Arc::new(config),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
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
        use crate::engine::handle::RunHandle;
        use crate::engine::run_task::{RunTaskParams, execute};
        use crate::engine::validate::validate_for_m5;
        use surge_core::approvals::ApprovalPolicy;
        use surge_core::content_hash::ContentHash;
        use surge_core::run_event::{
            EventPayload, RunConfig as CoreRunConfig, VersionedEventPayload,
        };
        use surge_core::sandbox::SandboxMode;
        use tokio::sync::broadcast;
        use tokio_util::sync::CancellationToken;

        validate_for_m5(&graph)?;

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

        // Emit RunStarted + PipelineMaterialized atomically.
        let core_run_config = CoreRunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
        };
        let graph_bytes = serde_json::to_vec(&graph)
            .map_err(|e| EngineError::Internal(format!("graph serialize: {e}")))?;
        let graph_hash = ContentHash::compute(&graph_bytes);

        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree_path.clone(),
                    initial_prompt: String::new(),
                    config: core_run_config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph.clone()),
                    graph_hash,
                }),
            ])
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;

        let (event_tx, event_rx) = broadcast::channel(256);
        let cancel = CancellationToken::new();

        let gate_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let tool_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
        };
        self.runs.write().await.insert(run_id, active);

        let params = RunTaskParams {
            run_id,
            writer,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            graph,
            worktree_path,
            run_config,
            event_tx,
            cancel,
            resume_cursor: None,
            resume_memory: None,
            gate_resolutions,
            tool_resolutions,
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
            let _ = event_tx.send(crate::engine::handle::EngineRunEvent::Terminal(
                terminal_outcome.clone(),
            ));
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

        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
        };
        self.runs.write().await.insert(run_id, active);

        let params = RunTaskParams {
            run_id,
            writer,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            graph: replayed.graph,
            worktree_path,
            run_config: EngineRunConfig::default(),
            event_tx,
            cancel,
            resume_cursor: Some(replayed.cursor),
            resume_memory: Some(replayed.memory),
            gate_resolutions,
            tool_resolutions,
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
                let tx = gates.remove(&k).expect("just looked up");
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
