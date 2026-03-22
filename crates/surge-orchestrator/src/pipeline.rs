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

use crate::executor::{ExecutorConfig, SubtaskExecutor, SubtaskResult};
use crate::gates::{GateAction, GateManager};
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

        let worktree_info = match git.create_worktree(&spec_id_str) {
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
        ) {
            Ok(p) => p,
            Err(e) => {
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("Failed to create agent pool: {e}"),
                };
            }
        };

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

        // 5. Get topological order
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

        let order = match graph.topological_order() {
            Ok(o) => o,
            Err(e) => {
                pool.shutdown().await;
                let _ = git.discard(&spec_id_str);
                return PipelineResult::Failed {
                    reason: format!("Topological sort failed: {e}"),
                };
            }
        };

        // 6. Create executor and gate manager
        let mut executor = SubtaskExecutor::new(ExecutorConfig::default());

        let specs_dir = self
            .config
            .working_dir
            .join(".surge")
            .join("specs");
        let gate_manager = GateManager::new(
            self.config.surge_config.pipeline.gates.clone(),
            specs_dir,
        );

        // 7. Execute subtasks in topological order
        let total = order.len();
        let mut completed = 0usize;

        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: TaskState::Draft,
            new_state: TaskState::Executing {
                completed: 0,
                total,
            },
        });

        for subtask_id in &order {
            // Check circuit breaker
            if executor.is_circuit_broken() {
                pool.shutdown().await;
                return PipelineResult::Paused {
                    phase: Phase::Executing,
                    reason: "Circuit breaker tripped — too many consecutive failures".into(),
                };
            }

            // Check gate
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

            // Find the subtask in the spec
            let subtask = match spec.subtasks.iter().find(|s| s.id == *subtask_id) {
                Some(s) => s,
                None => {
                    warn!(subtask_id = %subtask_id, "subtask not found in spec, skipping");
                    continue;
                }
            };

            let result = executor
                .execute(spec, subtask, task_id, &pool, &session, &git, &self.event_tx)
                .await;

            match result {
                SubtaskResult::Success { .. } => {
                    completed += 1;
                    let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
                        task_id,
                        old_state: TaskState::Executing {
                            completed: completed - 1,
                            total,
                        },
                        new_state: TaskState::Executing { completed, total },
                    });
                }
                SubtaskResult::Failed { reason, .. } => {
                    warn!(subtask_id = %subtask_id, %reason, "subtask failed");
                }
            }
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

                if let Err(e) = git.merge(&spec_id_str, None) {
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
