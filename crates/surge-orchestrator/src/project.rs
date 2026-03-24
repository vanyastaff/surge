//! Project executor — runs a timeline of specs through the orchestrator.
//!
//! A [`ProjectExecutor`] takes a [`Timeline`] (ordered batches of specs) and
//! drives each one through the [`Orchestrator`] pipeline. Specs within a batch
//! are independent and can run concurrently (bounded by `max_parallel_specs`).

use std::path::PathBuf;

use surge_core::SurgeConfig;
use surge_core::event::SurgeEvent;
use surge_core::id::SpecId;
use surge_core::roadmap::{RoadmapStatus, Timeline};
use surge_spec::SpecFile;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::pipeline::{Orchestrator, OrchestratorConfig, PipelineResult};

/// Configuration for project-level execution.
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    /// Surge configuration (agents, pipeline settings, etc.).
    pub surge_config: SurgeConfig,
    /// Root working directory of the project.
    pub working_dir: PathBuf,
    /// Maximum number of specs to run concurrently within a batch.
    /// Defaults to 1 (sequential).
    pub max_parallel_specs: usize,
}

/// Result of executing the entire project timeline.
#[derive(Debug, Clone)]
pub struct ProjectResult {
    /// Number of specs that completed successfully.
    pub completed: usize,
    /// Number of specs that failed.
    pub failed: usize,
    /// Number of specs that were skipped.
    pub skipped: usize,
    /// Number of specs that paused (awaiting human input).
    pub paused: usize,
}

impl ProjectResult {
    /// Returns `true` if all specs completed without failure.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0 && self.paused == 0
    }
}

/// Drives a project timeline through the orchestrator.
///
/// For each batch in the timeline, runs all pending specs through the
/// single-spec [`Orchestrator`]. Updates roadmap item statuses as specs
/// complete. Stops on the first batch that has failures (unless skipped).
pub struct ProjectExecutor {
    config: ProjectConfig,
    event_tx: broadcast::Sender<SurgeEvent>,
}

impl ProjectExecutor {
    /// Create a new project executor.
    #[must_use]
    pub fn new(config: ProjectConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { config, event_tx }
    }

    /// Subscribe to project-level events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SurgeEvent> {
        self.event_tx.subscribe()
    }

    /// Execute the timeline, running specs batch by batch.
    ///
    /// Requires a closure that loads a `SpecFile` by `SpecId`.
    /// This keeps the executor decoupled from filesystem layout.
    pub async fn execute<F>(&self, timeline: &mut Timeline, mut load_spec: F) -> ProjectResult
    where
        F: FnMut(SpecId) -> Option<SpecFile>,
    {
        let mut result = ProjectResult {
            completed: 0,
            failed: 0,
            skipped: 0,
            paused: 0,
        };

        for batch_idx in 0..timeline.batches.len() {
            let spec_ids: Vec<(usize, SpecId)> = timeline.batches[batch_idx]
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.status == RoadmapStatus::Pending)
                .map(|(i, item)| (i, item.spec_id))
                .collect();

            if spec_ids.is_empty() {
                continue;
            }

            info!(
                batch = batch_idx,
                specs = spec_ids.len(),
                "executing project batch"
            );

            // Mark items as running.
            for &(item_idx, _) in &spec_ids {
                timeline.batches[batch_idx].items[item_idx].status = RoadmapStatus::Running;
            }

            // Execute specs (sequentially for now; parallel via JoinSet is future work).
            let mut batch_had_failure = false;

            for (item_idx, spec_id) in spec_ids {
                let mut spec_file = match load_spec(spec_id) {
                    Some(sf) => sf,
                    None => {
                        warn!(spec_id = %spec_id, "spec file not found, skipping");
                        timeline.batches[batch_idx].items[item_idx].status = RoadmapStatus::Skipped;
                        result.skipped += 1;
                        continue;
                    }
                };

                let orch_config = OrchestratorConfig {
                    surge_config: self.config.surge_config.clone(),
                    working_dir: self.config.working_dir.clone(),
                };
                let orchestrator = Orchestrator::new(orch_config);

                // Forward orchestrator events to project-level channel.
                let mut orch_rx = orchestrator.subscribe();
                let project_tx = self.event_tx.clone();
                tokio::spawn(async move {
                    while let Ok(event) = orch_rx.recv().await {
                        let _ = project_tx.send(event);
                    }
                });

                let pipeline_result = orchestrator.execute(&mut spec_file).await;

                match pipeline_result {
                    PipelineResult::Completed => {
                        info!(spec_id = %spec_id, "spec completed");
                        timeline.batches[batch_idx].items[item_idx].status =
                            RoadmapStatus::Completed;
                        result.completed += 1;
                    }
                    PipelineResult::Paused { phase, reason } => {
                        info!(spec_id = %spec_id, %phase, %reason, "spec paused");
                        timeline.batches[batch_idx].items[item_idx].status = RoadmapStatus::Paused;
                        result.paused += 1;
                    }
                    PipelineResult::Failed { reason } => {
                        warn!(spec_id = %spec_id, %reason, "spec failed");
                        timeline.batches[batch_idx].items[item_idx].status = RoadmapStatus::Failed;
                        result.failed += 1;
                        batch_had_failure = true;
                    }
                }
            }

            // If any spec in the batch failed, skip remaining batches.
            if batch_had_failure {
                // Mark remaining pending items in future batches as skipped.
                for future_batch in &mut timeline.batches[batch_idx + 1..] {
                    for item in &mut future_batch.items {
                        if item.status == RoadmapStatus::Pending {
                            item.status = RoadmapStatus::Skipped;
                            result.skipped += 1;
                        }
                    }
                }
                warn!(
                    batch = batch_idx,
                    "batch had failures, skipping remaining batches"
                );
                break;
            }
        }

        info!(
            completed = result.completed,
            failed = result.failed,
            skipped = result.skipped,
            paused = result.paused,
            "project execution finished"
        );

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SpecId;
    use surge_core::roadmap::{RoadmapItem, TimelineBatch};
    use surge_core::spec::Complexity;

    fn make_item(title: &str) -> RoadmapItem {
        RoadmapItem {
            spec_id: SpecId::new(),
            title: title.to_string(),
            complexity: Complexity::Standard,
            priority: surge_core::roadmap::Priority::Medium,
            depends_on: vec![],
            status: RoadmapStatus::Pending,
        }
    }

    #[test]
    fn test_project_executor_creation() {
        let config = ProjectConfig {
            surge_config: SurgeConfig::default(),
            working_dir: PathBuf::from("/tmp"),
            max_parallel_specs: 2,
        };
        let executor = ProjectExecutor::new(config);
        let _rx = executor.subscribe();
    }

    #[test]
    fn test_project_result_all_succeeded() {
        let result = ProjectResult {
            completed: 3,
            failed: 0,
            skipped: 0,
            paused: 0,
        };
        assert!(result.all_succeeded());

        let result = ProjectResult {
            completed: 2,
            failed: 1,
            skipped: 0,
            paused: 0,
        };
        assert!(!result.all_succeeded());
    }

    #[tokio::test]
    async fn test_project_executor_empty_timeline() {
        let config = ProjectConfig {
            surge_config: SurgeConfig::default(),
            working_dir: PathBuf::from("/tmp"),
            max_parallel_specs: 1,
        };
        let executor = ProjectExecutor::new(config);
        let mut timeline = Timeline::new();

        let result = executor.execute(&mut timeline, |_| None).await;
        assert_eq!(result.completed, 0);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());
    }

    #[tokio::test]
    async fn test_project_executor_skips_missing_specs() {
        let config = ProjectConfig {
            surge_config: SurgeConfig::default(),
            working_dir: PathBuf::from("/tmp"),
            max_parallel_specs: 1,
        };
        let executor = ProjectExecutor::new(config);

        let mut timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![make_item("Missing spec")],
                reason: String::new(),
            }],
        };

        let result = executor.execute(&mut timeline, |_| None).await;
        assert_eq!(result.skipped, 1);
        assert_eq!(timeline.batches[0].items[0].status, RoadmapStatus::Skipped);
    }
}
