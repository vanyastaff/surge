//! Orchestrator — main pipeline driving spec execution.

use std::path::PathBuf;

use surge_acp::pool::AgentPool;
use surge_acp::PermissionPolicy;
use surge_core::event::SurgeEvent;
use surge_core::id::TaskId;
use surge_core::spec::SubtaskState;
use surge_core::state::TaskState;
use surge_core::SurgeConfig;
use surge_git::worktree::GitManager;
use surge_persistence::aggregator::{SessionContext, UsageAggregator};
use surge_persistence::store::Store;
use surge_spec::{validate_spec, DependencyGraph, SpecFile};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::executor::ExecutorConfig;
use crate::gates::{GateAction, GateManager};
use crate::parallel::ParallelExecutor;
use crate::phases::Phase;
use crate::qa::{QaReviewer, QaVerdict};

/// Configuration for the Orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub surge_config: SurgeConfig,
    pub working_dir: PathBuf,
}

/// Result of a pipeline execution.
#[derive(Debug, Clone)]
pub enum PipelineResult {
    /// Pipeline completed successfully — all subtasks done, QA passed, merged.
    Completed,
    /// Pipeline paused at a phase, waiting for external signal.
    Paused { phase: Phase, reason: String },
    /// Pipeline failed with an error.
    Failed { reason: String },
}

/// The Orchestrator drives a spec through the full pipeline:
/// validate → worktree → agent session → execute subtasks → QA → merge.
pub struct Orchestrator {
    config: OrchestratorConfig,
    event_tx: broadcast::Sender<SurgeEvent>,
}

impl Orchestrator {
    /// Create a new Orchestrator with the given configuration.
    #[must_use]
    pub fn new(config: OrchestratorConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { config, event_tx }
    }

    /// Subscribe to pipeline events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SurgeEvent> {
        self.event_tx.subscribe()
    }

    /// Execute the full pipeline for a spec.
    ///
    /// `spec_file` is taken by mutable reference so that subtask execution
    /// states can be persisted to disk after each batch.
    pub async fn execute(&self, spec_file: &mut SpecFile) -> PipelineResult {
        // Clone the spec so we can hold a read-only reference while also
        // mutably updating spec_file for state persistence within the loop.
        let spec = spec_file.spec.clone();
        let task_id = TaskId::new();
        let spec_id_str = spec.id.to_string();

        // 1. Validate spec
        let validation = validate_spec(&spec);
        if !validation.is_ok() {
            return PipelineResult::Failed {
                reason: format!("Spec validation failed: {}", validation.errors.join("; ")),
            };
        }
        info!(spec_id = %spec.id, "spec validated");

        // 2. Set up persistence layer for token tracking
        let db_dir = self.config.working_dir.join(".surge");
        let db_path = db_dir.join("usage.db");

        let store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to create usage store, continuing without persistence");
                Store::in_memory().unwrap()
            }
        };

        let aggregator = UsageAggregator::new(store);
        let aggregator_rx = self.event_tx.subscribe();
        let _aggregator_handle = aggregator.start_listening(aggregator_rx);

        // 3. Create git worktree
        let git = match GitManager::new(self.config.working_dir.clone()) {
            Ok(g) => g,
            Err(e) => {
                return PipelineResult::Failed {
                    reason: format!("Failed to initialise git manager: {e}"),
                };
            }
        };

        let worktree_info = match git.create_worktree(&spec_id_str, None) {
            Ok(wt) => wt,
            Err(e) => {
                return PipelineResult::Failed {
                    reason: format!("Failed to create worktree: {e}"),
                };
            }
        };
        let worktree_path = worktree_info.path.clone();
        info!(path = %worktree_path.display(), "worktree created");

        // 4. Create AgentPool
        let pool = match AgentPool::new(
            self.config.surge_config.agents.clone(),
            self.config.surge_config.default_agent.clone(),
            worktree_path.clone(),
            PermissionPolicy::default(),
            self.config.surge_config.resilience.clone(),
        ) {
            Ok(p) => p,
            Err(e) => {
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("Failed to create agent pool: {e}"),
                };
            }
        };

        // Forward pool events to the orchestrator broadcast channel.
        let mut pool_rx = pool.subscribe();
        let pipeline_event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            while let Ok(event) = pool_rx.recv().await {
                let _ = pipeline_event_tx.send(event);
            }
        });

        pool.warm_up();

        // 5. Create ACP session
        let session = match pool.create_session(None, None, &worktree_path).await {
            Ok(s) => s,
            Err(e) => {
                pool.shutdown().await;
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("Failed to create ACP session: {e}"),
                };
            }
        };
        info!("ACP session created");

        // 6. Register session with usage aggregator
        aggregator
            .register_session(
                session.session_id.clone(),
                SessionContext {
                    task_id,
                    subtask_id: None,
                    spec_id: spec.id,
                },
            )
            .await;

        // 7. Build dependency graph and topological batches
        let graph = match DependencyGraph::from_spec(&spec) {
            Ok(g) => g,
            Err(e) => {
                pool.shutdown().await;
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("Failed to build dependency graph: {e}"),
                };
            }
        };

        let batch_ids = match graph.topological_batches() {
            Ok(b) => b,
            Err(e) => {
                pool.shutdown().await;
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("Topological batches failed: {e}"),
                };
            }
        };

        let batches: Vec<Vec<_>> = batch_ids
            .iter()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| spec.subtasks.iter().find(|s| s.id == *id).cloned())
                    .collect()
            })
            .collect();

        let total: usize = batches.iter().map(|b| b.len()).sum();

        // 8. Create parallel executor and gate manager
        let parallel_exec = ParallelExecutor::new(
            self.config.surge_config.pipeline.max_parallel,
            ExecutorConfig::default(),
        );

        let specs_dir = self.config.working_dir.join(".surge").join("specs");
        let gate_manager = GateManager::new(
            self.config.surge_config.pipeline.gates.clone(),
            specs_dir,
        );

        // 9. Execute subtasks in batches
        let spec_id = spec.id;
        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: TaskState::Draft,
            new_state: TaskState::Executing { completed: 0, total },
        });

        let mut completed: usize = 0;
        let mut failed_batches: usize = 0;
        // Human input to inject into the next batch's first subtask (consumed once).
        let mut pending_human_input: Option<String> = None;

        for (i, batch) in batches.iter().enumerate() {
            // Gate check between batches (including before the first).
            match gate_manager.check_gate(Phase::Executing, spec_id) {
                GateAction::Pause { reason } => {
                    pool.shutdown().await;
                    return PipelineResult::Paused {
                        phase: Phase::Executing,
                        reason,
                    };
                }
                GateAction::HumanInput { content } => {
                    info!("human input received, will inject into next batch");
                    pending_human_input = Some(content);
                }
                GateAction::Continue => {}
            }

            info!(batch_index = i, batch_size = batch.len(), "executing batch");

            let result = parallel_exec
                .execute_batch(
                    &spec,
                    batch,
                    task_id,
                    &pool,
                    &session,
                    &git,
                    &self.event_tx,
                    pending_human_input.as_deref(),
                )
                .await;

            // Human input consumed after first batch that received it.
            pending_human_input = None;

            completed += result.successes.len();

            // Persist subtask states to disk (best-effort — log and continue on error).
            if let Some(ref path) = spec_file.path.clone() {
                for subtask_id in &result.successes {
                    if let Err(e) =
                        spec_file.update_subtask_state(path, *subtask_id, SubtaskState::Completed)
                    {
                        warn!(subtask_id = %subtask_id, error = %e, "failed to persist subtask state");
                    }
                }
                for (subtask_id, _) in &result.failures {
                    if let Err(e) =
                        spec_file.update_subtask_state(path, *subtask_id, SubtaskState::Failed)
                    {
                        warn!(subtask_id = %subtask_id, error = %e, "failed to persist subtask state");
                    }
                }
            }

            if !result.all_succeeded() {
                failed_batches += 1;
                warn!(
                    batch_index = i,
                    failures = result.failures.len(),
                    "batch had failures, stopping further batches"
                );
                break;
            }
        }

        if failed_batches > 0 {
            warn!(completed, "some subtasks failed during batch execution");
        }

        // 10. QA review
        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: TaskState::Executing { completed, total },
            new_state: TaskState::QaReview,
        });

        let qa_reviewer = QaReviewer::new(self.config.surge_config.pipeline.max_qa_iterations);
        let qa_result = qa_reviewer
            .run(&spec, task_id, &pool, &session, &git, &self.event_tx)
            .await;

        info!(
            iterations = qa_result.iterations,
            verdict = ?qa_result.verdict,
            "QA review complete"
        );

        // 11. If QA approved → merge
        match qa_result.verdict {
            QaVerdict::Approved => {
                let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
                    task_id,
                    old_state: TaskState::QaReview,
                    new_state: TaskState::Merging,
                });

                if let Err(e) = git.merge(&spec_id_str, None, true) {
                    aggregator.unregister_session(&session.session_id).await;
                    pool.shutdown().await;
                    let _ = git.discard(&spec_id_str);
                    return PipelineResult::Failed {
                        reason: format!("Merge failed: {e}"),
                    };
                }
                info!("merged successfully");
            }
            QaVerdict::NeedsFix { issues } => {
                aggregator.unregister_session(&session.session_id).await;
                pool.shutdown().await;
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("QA review failed after max iterations: {issues}"),
                };
            }
        }

        // 12. Cleanup
        aggregator.unregister_session(&session.session_id).await;
        let _ = git.discard(&spec_id_str);
        pool.shutdown().await;

        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: TaskState::Merging,
            new_state: TaskState::Completed,
        });

        PipelineResult::Completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_creation() {
        let config = OrchestratorConfig {
            surge_config: SurgeConfig::default(),
            working_dir: PathBuf::from("/tmp"),
        };
        let orch = Orchestrator::new(config);
        let _rx = orch.subscribe();
    }
}
