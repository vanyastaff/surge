//! Subtask executor — runs a single subtask via ACP agent.

use std::path::Path;
use std::sync::Arc;

use agent_client_protocol::{ContentBlock, TextContent};
use surge_acp::pool::{AgentPool, SessionHandle};
use surge_core::event::SurgeEvent;
use surge_core::id::{SubtaskId, TaskId};
use surge_core::spec::{Spec, Subtask};
use surge_core::state::TaskState;
use surge_git::worktree::GitManager;
use surge_persistence::store::Store;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, warn};

use crate::context::SubtaskContext;

/// Result of executing a single subtask.
#[derive(Debug, Clone)]
pub enum SubtaskResult {
    /// Subtask completed successfully.
    Success { subtask_id: SubtaskId },
    /// Subtask failed after all retries.
    Failed {
        subtask_id: SubtaskId,
        reason: String,
    },
}

/// Configuration for subtask execution.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum number of retries per subtask.
    pub max_retries: u32,
    /// Number of consecutive failures before the circuit breaker trips.
    pub circuit_breaker_threshold: u32,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            circuit_breaker_threshold: 3,
        }
    }
}

/// Executes individual subtasks via an ACP agent.
pub struct SubtaskExecutor {
    config: ExecutorConfig,
    consecutive_failures: u32,
}

impl SubtaskExecutor {
    /// Create a new executor with the given configuration.
    #[must_use]
    pub fn new(config: ExecutorConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
        }
    }

    /// Check whether the circuit breaker has tripped.
    #[must_use]
    pub fn is_circuit_broken(&self) -> bool {
        self.consecutive_failures >= self.config.circuit_breaker_threshold
    }

    /// Reset the consecutive failure counter.
    pub fn reset_failures(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Execute a subtask: build prompt, send to agent, commit on success.
    ///
    /// Retries up to `max_retries` times on failure.
    /// If `human_input` is provided it is appended to the prompt.
    /// After successful completion, saves a checkpoint to the store.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &mut self,
        spec: &Spec,
        subtask: &Subtask,
        task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
        human_input: Option<&str>,
        spec_dir: Option<&Path>,
        store: Option<&Arc<Mutex<Store>>>,
        completed_count: usize,
        total_count: usize,
    ) -> SubtaskResult {
        let subtask_id = subtask.id;

        let _ = event_tx.send(SurgeEvent::SubtaskStarted {
            task_id,
            subtask_id,
        });

        // Check circuit breaker before attempting execution
        if self.is_circuit_broken() {
            warn!(
                subtask_id = %subtask_id,
                consecutive_failures = self.consecutive_failures,
                "circuit breaker tripped, failing fast without retries"
            );
            let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
                task_id,
                subtask_id,
                success: false,
            });
            return SubtaskResult::Failed {
                subtask_id,
                reason: format!(
                    "circuit breaker tripped after {} consecutive failures",
                    self.consecutive_failures
                ),
            };
        }

        let ctx = SubtaskContext::new(spec, subtask, spec_dir);
        let mut prompt_text = ctx.build_prompt();

        if let Some(input) = human_input {
            prompt_text.push_str(&format!(
                "\n## Human Input\n\n{input}\n\nPlease incorporate this guidance into your implementation.\n"
            ));
        }

        let content = vec![ContentBlock::Text(TextContent::new(prompt_text))];

        let mut last_error = String::from("unknown error");
        let retry_start = std::time::Instant::now();

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                info!(
                    subtask_id = %subtask_id,
                    attempt,
                    max_retries = self.config.max_retries,
                    elapsed_ms = retry_start.elapsed().as_millis() as u64,
                    "retrying subtask"
                );
            }

            match pool.prompt(session, content.clone()).await {
                Ok(_response) => {
                    let commit_msg = format!("surge: subtask {} — {}", subtask.title, subtask_id);
                    let spec_id_str = spec.id.to_string();

                    match git.commit(&spec_id_str, &commit_msg) {
                        Ok(oid) => {
                            info!(
                                subtask_id = %subtask_id,
                                %oid,
                                "subtask completed and committed"
                            );
                            self.consecutive_failures = 0;

                            // Save checkpoint to enable task resumption
                            if let Some(store_ref) = store {
                                let new_completed = completed_count + 1;
                                let state = TaskState::Executing {
                                    completed: new_completed,
                                    total: total_count,
                                };
                                let mut store_guard = store_ref.lock().await;
                                if let Err(e) =
                                    store_guard.checkpoint_task_state(task_id, spec.id, &state)
                                {
                                    warn!(
                                        subtask_id = %subtask_id,
                                        error = %e,
                                        "failed to save task checkpoint"
                                    );
                                } else {
                                    info!(
                                        task_id = %task_id,
                                        completed = new_completed,
                                        total = total_count,
                                        "task checkpoint saved"
                                    );
                                }
                            }

                            let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
                                task_id,
                                subtask_id,
                                success: true,
                            });
                            return SubtaskResult::Success { subtask_id };
                        }
                        Err(e) => {
                            warn!(
                                subtask_id = %subtask_id,
                                attempt,
                                max_retries = self.config.max_retries,
                                elapsed_ms = retry_start.elapsed().as_millis() as u64,
                                error = %e,
                                "commit failed after agent prompt"
                            );
                            last_error = format!("commit failed: {e}");
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        subtask_id = %subtask_id,
                        attempt,
                        max_retries = self.config.max_retries,
                        elapsed_ms = retry_start.elapsed().as_millis() as u64,
                        error = %e,
                        "agent prompt failed"
                    );
                    last_error = format!("agent prompt failed: {e}");
                }
            }
        }

        // All retries exhausted
        warn!(
            subtask_id = %subtask_id,
            max_retries = self.config.max_retries,
            total_elapsed_ms = retry_start.elapsed().as_millis() as u64,
            "all subtask retries exhausted"
        );
        self.consecutive_failures += 1;
        let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
            task_id,
            subtask_id,
            success: false,
        });

        SubtaskResult::Failed {
            subtask_id,
            reason: last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_config_defaults() {
        let config = ExecutorConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.circuit_breaker_threshold, 3);
    }

    #[test]
    fn test_circuit_breaker() {
        let config = ExecutorConfig {
            max_retries: 1,
            circuit_breaker_threshold: 2,
        };
        let mut executor = SubtaskExecutor::new(config);

        assert!(!executor.is_circuit_broken());

        executor.consecutive_failures = 1;
        assert!(!executor.is_circuit_broken());

        executor.consecutive_failures = 2;
        assert!(executor.is_circuit_broken());

        executor.reset_failures();
        assert!(!executor.is_circuit_broken());
    }
}
