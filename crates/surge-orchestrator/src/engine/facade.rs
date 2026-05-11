//! `EngineFacade` — abstraction over `Engine` so CLI / tests can
//! switch between in-process (`LocalEngineFacade`) and out-of-process
//! (`DaemonEngineFacade`, Phase 5) hosting without touching the
//! engine's public API.

use crate::engine::config::EngineRunConfig;
use crate::engine::engine::Engine;
use crate::engine::error::EngineError;
use crate::engine::handle::{RunHandle, RunSummary};
use crate::roadmap_amendment::ActiveRunAmendmentOutcome;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::roadmap_patch::{RoadmapPatchApplyResult, RoadmapPatchId, RoadmapPatchTarget};

/// Engine-facing surface used by CLI commands and tests. All futures
/// are `Send`. Implementations: [`LocalEngineFacade`] (in-process,
/// straight delegation to [`Engine`]) and `DaemonEngineFacade`
/// (forwards every method as an IPC request — Phase 5).
#[async_trait]
pub trait EngineFacade: Send + Sync {
    /// Start a new run.
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError>;

    /// Resume an existing run from its latest snapshot.
    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError>;

    /// Cancel an in-flight run.
    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError>;

    /// Submit an approved roadmap amendment to a live run. Implementations
    /// must route this to the run task that owns the target run writer.
    async fn submit_roadmap_amendment(
        &self,
        run_id: RunId,
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        patch_result: RoadmapPatchApplyResult,
    ) -> Result<ActiveRunAmendmentOutcome, EngineError> {
        let _ = (patch_id, target, patch_result);
        Err(EngineError::RunNotFound(run_id))
    }

    /// Provide an answer to a paused run waiting on human input.
    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError>;

    /// List runs visible to this facade. For the local facade, this
    /// is the in-memory active set. For the daemon facade, the
    /// daemon reports its full view.
    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError>;
}

/// In-process `EngineFacade`. Wraps an `Arc<Engine>` and forwards
/// every call directly. Default for the M6-style CLI invocation.
pub struct LocalEngineFacade {
    engine: Arc<Engine>,
}

impl LocalEngineFacade {
    /// Construct a facade around the given engine.
    #[must_use]
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl EngineFacade for LocalEngineFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        self.engine
            .start_run(run_id, graph, worktree_path, run_config)
            .await
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        self.engine.resume_run(run_id, worktree_path).await
    }

    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError> {
        self.engine.stop_run(run_id, reason).await
    }

    async fn submit_roadmap_amendment(
        &self,
        run_id: RunId,
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        patch_result: RoadmapPatchApplyResult,
    ) -> Result<ActiveRunAmendmentOutcome, EngineError> {
        self.engine
            .submit_roadmap_amendment(run_id, patch_id, target, patch_result)
            .await
    }

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        self.engine
            .resolve_human_input(run_id, call_id, response)
            .await
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(self.engine.snapshot_active_runs().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn _facade_is_object_safe() {
        // Compile-time check: must be usable behind Arc<dyn>.
        let _: Option<Arc<dyn EngineFacade>> = None;
    }
}
