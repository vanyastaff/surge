//! Parallel batch executor — runs subtask batches with bounded concurrency.

use std::path::Path;
use std::sync::Arc;

use surge_acp::pool::{AgentPool, SessionHandle};
use surge_core::event::SurgeEvent;
use surge_core::id::{SubtaskId, TaskId};
use surge_core::spec::{Spec, Subtask};
use surge_git::worktree::GitManager;
use surge_persistence::store::Store;
use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::executor::{ExecutorConfig, SubtaskExecutor, SubtaskResult};

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
/// run sequentially — see [`crate::pipeline`] for the inter-batch loop.
pub struct ParallelExecutor {
    /// Maximum number of subtasks to run concurrently within a batch.
    max_parallel: usize,
    /// Configuration for individual subtask execution (retries, circuit breaker).
    executor_config: ExecutorConfig,
}

impl ParallelExecutor {
    /// Create a new parallel executor.
    ///
    /// `max_parallel` is clamped to the range `[1, 64]`.
    /// Most agent API providers throttle at 3-5 concurrent sessions;
    /// 64 is a generous upper bound for multi-agent setups.
    #[must_use]
    pub fn new(max_parallel: usize, executor_config: ExecutorConfig) -> Self {
        Self {
            max_parallel: max_parallel.clamp(1, 64),
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
    /// Uses [`SubtaskExecutor`] for per-subtask retry logic and circuit breaker.
    /// If the circuit breaker trips mid-batch, remaining subtasks are skipped.
    ///
    /// `human_input` is injected into the first subtask's prompt only.
    /// `store` is used to save checkpoints after each successful subtask.
    /// `completed_offset` indicates how many subtasks were completed before this batch.
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
        human_input: Option<&str>,
        spec_dir: Option<&Path>,
        store: Option<&Arc<Mutex<Store>>>,
        completed_offset: usize,
        total_count: usize,
    ) -> BatchResult {
        let semaphore = Semaphore::new(self.max_parallel);
        let mut executor = SubtaskExecutor::new(self.executor_config.clone());
        let mut successes = Vec::new();
        let mut failures = Vec::new();

        for (i, subtask) in subtasks.iter().enumerate() {
            let _permit = match semaphore.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    tracing::error!("concurrency semaphore closed unexpectedly");
                    failures.push((subtask.id, "semaphore closed".to_string()));
                    continue;
                }
            };

            // Human input only goes to the first subtask in the batch.
            let input = if i == 0 { human_input } else { None };

            // Calculate how many subtasks completed before this one
            let completed_so_far = completed_offset + successes.len();

            match executor
                .execute(
                    spec,
                    subtask,
                    task_id,
                    pool,
                    session,
                    git,
                    event_tx,
                    input,
                    spec_dir,
                    store,
                    completed_so_far,
                    total_count,
                )
                .await
            {
                SubtaskResult::Success { subtask_id } => successes.push(subtask_id),
                SubtaskResult::Failed { subtask_id, reason } => {
                    failures.push((subtask_id, reason));
                }
            }
        }

        BatchResult {
            successes,
            failures,
        }
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
