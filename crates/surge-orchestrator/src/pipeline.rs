//! Orchestrator — main pipeline driving spec execution.

use std::path::PathBuf;

use surge_acp::pool::AgentPool;
use surge_acp::PermissionPolicy;
use surge_core::event::SurgeEvent;
use surge_core::id::TaskId;
use surge_core::state::TaskState;
use surge_core::SurgeConfig;
use surge_git::worktree::GitManager;
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
    pub async fn execute(&self, spec_file: &SpecFile) -> PipelineResult {
        let spec = &spec_file.spec;
        let task_id = TaskId::new();
        let spec_id_str = spec.id.to_string();

        // 1. Validate spec
        let validation = validate_spec(spec);
        if !validation.is_ok() {
            return PipelineResult::Failed {
                reason: format!("Spec validation failed: {}", validation.errors.join("; ")),
            };
        }
        info!(spec_id = %spec.id, "spec validated");

        // 2. Create git worktree
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

        // 3. Create AgentPool
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

        // Forward pool events (TokensConsumed, AgentHealthChanged, etc.) to
        // the orchestrator's broadcast channel so subscribers get full coverage.
        let mut pool_rx = pool.subscribe();
        let pipeline_event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            while let Ok(event) = pool_rx.recv().await {
                let _ = pipeline_event_tx.send(event);
            }
        });

        // Pre-connect the default agent in the background so the first prompt
        // doesn't pay the full connection latency.
        pool.warm_up();

        // 4. Create ACP session
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

        // 5. Build dependency graph and get batches
        let graph = match DependencyGraph::from_spec(spec) {
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

        // Resolve SubtaskId batches into Subtask batches
        let batches: Vec<Vec<_>> = batch_ids
            .iter()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| spec.subtasks.iter().find(|s| s.id == *id).cloned())
                    .collect()
            })
            .collect();

        let total: usize = batches.iter().map(|b| b.len()).sum();

        // 6. Create parallel executor and gate manager
        let parallel_exec = ParallelExecutor::new(
            self.config.surge_config.pipeline.max_parallel,
            ExecutorConfig::default(),
        );

        let specs_dir = self
            .config
            .working_dir
            .join(".surge")
            .join("specs");
        let gate_manager = GateManager::new(
            self.config.surge_config.pipeline.gates.clone(),
            specs_dir,
        );

        // 7. Execute subtasks in batches
        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: TaskState::Draft,
            new_state: TaskState::Executing {
                completed: 0,
                total,
            },
        });

        // Check gate once before batch execution
        match gate_manager.check_gate(Phase::Executing, spec.id) {
            GateAction::Pause { reason } => {
                pool.shutdown().await;
                return PipelineResult::Paused {
                    phase: Phase::Executing,
                    reason,
                };
            }
            GateAction::HumanInput { .. } | GateAction::Continue => {}
        }

        let batch_results = parallel_exec
            .execute_all_batches(spec, &batches, task_id, &pool, &session, &git, &self.event_tx)
            .await;

        let completed: usize = batch_results.iter().map(|r| r.successes.len()).sum();
        let failed: usize = batch_results.iter().map(|r| r.failures.len()).sum();

        if failed > 0 {
            warn!(completed, failed, "some subtasks failed during batch execution");
        }

        // 8. QA review
        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: TaskState::Executing { completed, total },
            new_state: TaskState::QaReview,
        });

        let qa_reviewer = QaReviewer::new(self.config.surge_config.pipeline.max_qa_iterations);
        let qa_result = qa_reviewer
            .run(spec, task_id, &pool, &session, &git, &self.event_tx)
            .await;

        info!(
            iterations = qa_result.iterations,
            verdict = ?qa_result.verdict,
            "QA review complete"
        );

        // 9. If QA approved → merge
        match qa_result.verdict {
            QaVerdict::Approved => {
                let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
                    task_id,
                    old_state: TaskState::QaReview,
                    new_state: TaskState::Merging,
                });

                if let Err(e) = git.merge(&spec_id_str, None, true) {
                    pool.shutdown().await;
                    let _ = git.discard(&spec_id_str);
                    return PipelineResult::Failed {
                        reason: format!("Merge failed: {e}"),
                    };
                }
                info!("merged successfully");
            }
            QaVerdict::NeedsFix { issues } => {
                pool.shutdown().await;
                return PipelineResult::Failed {
                    reason: format!("QA review failed after max iterations: {issues}"),
                };
            }
        }

        // 10. Cleanup
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
