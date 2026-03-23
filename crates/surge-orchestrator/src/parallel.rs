//! Parallel batch executor — runs subtask batches with bounded concurrency.

use agent_client_protocol::{ContentBlock, TextContent};
use surge_acp::pool::{AgentPool, SessionHandle};
use surge_core::event::SurgeEvent;
use surge_core::id::{SubtaskId, TaskId};
use surge_core::spec::{Spec, Subtask};
use surge_git::worktree::GitManager;
use tokio::sync::{broadcast, Semaphore};
use tracing::{info, warn};

use crate::context::SubtaskContext;
use crate::executor::ExecutorConfig;

/// Result of executing a single batch of subtasks.
#[derive(Debug, Clone)]
pub struct BatchResult {
    /// Subtask IDs that completed successfully.
    pub successes: Vec<SubtaskId>,
    /// Subtask IDs that failed, along with the failure reason.
    pub failures: Vec<(SubtaskId, String)>,
}

impl BatchResult {
    /// Returns true if all subtasks in the batch succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Executes batches of subtasks with bounded parallelism.
///
/// Within a batch, subtasks are independent and can run concurrently
/// (bounded by `max_parallel` via a [`Semaphore`]). Batches themselves
/// run sequentially.
pub struct ParallelExecutor {
    /// Maximum number of subtasks to run concurrently within a batch.
    max_parallel: usize,
    /// Configuration for individual subtask execution (retries, circuit breaker).
    executor_config: ExecutorConfig,
}

impl ParallelExecutor {
    /// Create a new parallel executor.
    ///
    /// `max_parallel` is clamped to a minimum of 1.
    #[must_use]
    pub fn new(max_parallel: usize, executor_config: ExecutorConfig) -> Self {
        Self {
            max_parallel: max_parallel.max(1),
            executor_config,
        }
    }

    /// Returns the effective max parallelism.
    #[must_use]
    pub fn max_parallel(&self) -> usize {
        self.max_parallel
    }

    /// Returns a reference to the executor config.
    #[must_use]
    pub fn executor_config(&self) -> &ExecutorConfig {
        &self.executor_config
    }

    /// Execute a single batch of subtasks with bounded concurrency.
    ///
    /// Since `AgentPool`/sessions may not be `Send`-safe for true `JoinSet`
    /// parallelism, this currently uses a sequential loop bounded by the
    /// semaphore. Each subtask builds a prompt, sends it to the agent, and
    /// commits the result via git.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_batch(
        &self,
        spec: &Spec,
        subtasks: &[Subtask],
        task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
    ) -> BatchResult {
        let semaphore = Semaphore::new(self.max_parallel);
        let mut successes = Vec::new();
        let mut failures = Vec::new();

        for subtask in subtasks {
            // Acquire semaphore permit (sequential loop, so this is mostly
            // a placeholder for future true-parallel upgrade).
            let _permit = semaphore.acquire().await.expect("semaphore closed");

            let subtask_id = subtask.id;

            // Emit start event
            let _ = event_tx.send(SurgeEvent::SubtaskStarted {
                task_id,
                subtask_id,
            });

            let ctx = SubtaskContext::new(spec, subtask);
            let prompt_text = ctx.build_prompt();

            let content = vec![ContentBlock::Text(TextContent::new(prompt_text))];

            let mut last_error = String::from("unknown error");
            let mut succeeded = false;

            for attempt in 0..=self.executor_config.max_retries {
                if attempt > 0 {
                    info!(
                        subtask_id = %subtask_id,
                        attempt,
                        "retrying subtask"
                    );
                }

                match pool.prompt(session, content.clone()).await {
                    Ok(_response) => {
                        let commit_msg =
                            format!("surge: subtask {} — {}", subtask.title, subtask_id);
                        let spec_id_str = spec.id.to_string();

                        match git.commit(&spec_id_str, &commit_msg) {
                            Ok(oid) => {
                                info!(
                                    subtask_id = %subtask_id,
                                    %oid,
                                    "subtask completed and committed"
                                );
                                succeeded = true;
                                break;
                            }
                            Err(e) => {
                                warn!(
                                    subtask_id = %subtask_id,
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
                            error = %e,
                            "agent prompt failed"
                        );
                        last_error = format!("agent prompt failed: {e}");
                    }
                }
            }

            if succeeded {
                let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
                    task_id,
                    subtask_id,
                    success: true,
                });
                successes.push(subtask_id);
            } else {
                let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
                    task_id,
                    subtask_id,
                    success: false,
                });
                failures.push((subtask_id, last_error));
            }
        }

        BatchResult {
            successes,
            failures,
        }
    }

    /// Execute all batches sequentially.
    ///
    /// Each batch is a slice of subtasks that can run concurrently within
    /// the batch. Batches are processed one at a time in order.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_all_batches(
        &self,
        spec: &Spec,
        batches: &[Vec<Subtask>],
        task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
    ) -> Vec<BatchResult> {
        let mut results = Vec::with_capacity(batches.len());

        for (i, batch) in batches.iter().enumerate() {
            info!(batch_index = i, batch_size = batch.len(), "executing batch");
            let result = self
                .execute_batch(spec, batch, task_id, pool, session, git, event_tx)
                .await;

            let all_ok = result.all_succeeded();
            results.push(result);

            if !all_ok {
                warn!(
                    batch_index = i,
                    "batch had failures, stopping further batches"
                );
                break;
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_executor_creation() {
        let config = ExecutorConfig::default();
        let executor = ParallelExecutor::new(4, config);
        assert_eq!(executor.max_parallel(), 4);
        assert_eq!(executor.executor_config().max_retries, 3);
    }

    #[test]
    fn test_parallel_executor_min_one() {
        let config = ExecutorConfig::default();
        let executor = ParallelExecutor::new(0, config);
        assert_eq!(executor.max_parallel(), 1);
    }
}
