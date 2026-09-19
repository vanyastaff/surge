//! Production [`RoadmapPlanner`]: the ACP Feature Planner behind the intake
//! seam (T11).
//!
//! `surge feature describe` has driven `run_feature_planner` from the CLI for
//! a while; this adapter gives the daemon the same capability without
//! duplicating the CLI's output/approval handling. The planner run gets its
//! own event log (so a planning decision is auditable), and the resulting
//! patch travels back through the pure [intake planner](crate::intake_planner)
//! path, which owns the deferral rule and the queue projection.
//!
//! The approval gate for the patch is the existing roadmap-patch approval
//! loop: this adapter only **drafts**; nothing it returns has been applied to
//! a file. The daemon's launcher applies it, and the roadmap-amendment
//! machinery (human gate included) is what the operator sees in the cockpit.

use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::roadmap_patch::{RoadmapPatch, RoadmapPatchTarget};
use surge_core::run_state::RunMemory;
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::Storage;

use crate::engine::tools::worktree::WorktreeToolDispatcher;
use crate::feature_driver::{FeaturePlannerParams, FeaturePlannerResult, run_feature_planner};
use crate::intake_planner::{PlannerError, RoadmapPlanner};

/// Plans feature requests through the ACP Feature Planner.
pub struct FeaturePlannerRoadmapPlanner {
    /// Storage home used for the planner run's own log and artifacts.
    pub storage: Arc<Storage>,
    /// Bridge the planner's agent stage runs on.
    pub bridge: Arc<dyn BridgeFacade>,
    /// Project-wide profile registry (home + bundled); the run's project lane
    /// is bound per planning call from `project_root`.
    pub profile_registry: Arc<crate::profile_loader::ProfileRegistry>,
}

impl FeaturePlannerRoadmapPlanner {
    /// Construct the adapter from the daemon's shared subsystems.
    #[must_use]
    pub fn new(
        storage: Arc<Storage>,
        bridge: Arc<dyn BridgeFacade>,
        profile_registry: Arc<crate::profile_loader::ProfileRegistry>,
    ) -> Self {
        Self {
            storage,
            bridge,
            profile_registry,
        }
    }
}

#[async_trait::async_trait]
impl RoadmapPlanner for FeaturePlannerRoadmapPlanner {
    async fn plan(
        &self,
        request: String,
        roadmap_toml: String,
    ) -> Result<RoadmapPatch, PlannerError> {
        // A scratch worktree for the planner's own artifacts and its
        // `roadmap-patch.toml`. The run log lives under the storage home, so
        // the directory only has to exist.
        let worktree = self
            .storage
            .home()
            .join("intake")
            .join("planner")
            .join(ulid::Ulid::new().to_string());
        std::fs::create_dir_all(&worktree)
            .map_err(|e| PlannerError::Call(format!("planner worktree: {e}")))?;

        let planner_run_id = surge_core::RunId::new();
        let writer = self
            .storage
            .create_run(planner_run_id, &worktree, Some("feature-planner".into()))
            .await
            .map_err(|e| PlannerError::Call(format!("create planner run: {e}")))?;

        let artifact_store = ArtifactStore::new(self.storage.home().join("runs"));
        let tool_dispatcher: Arc<dyn crate::engine::tools::ToolDispatcher> =
            Arc::new(WorktreeToolDispatcher::new(worktree.clone()));
        let hook_executor = crate::engine::hooks::HookExecutor::new();
        let tool_resolutions: crate::feature_driver::ToolResolutionMap =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let memory = RunMemory::default();
        let project_layer = surge_core::ProjectLayer::for_project(&worktree);
        let run_registry = self
            .profile_registry
            .for_run(&project_layer)
            .map_err(|e| PlannerError::Call(format!("project profile lane: {e}")))?;
        let run_registry = Arc::new(run_registry);

        let roadmap_prompt = format!("Current roadmap (`.surge/roadmap.toml`):\n\n{roadmap_toml}");
        let result = run_feature_planner(FeaturePlannerParams {
            request,
            roadmap: roadmap_prompt,
            bridge: &self.bridge,
            writer: &writer,
            artifact_store: &artifact_store,
            worktree_path: &worktree,
            tool_dispatcher: &tool_dispatcher,
            run_memory: &memory,
            run_id: planner_run_id,
            tool_resolutions: &tool_resolutions,
            human_input_timeout: Duration::from_secs(300),
            mcp_registry: None,
            mcp_servers: Vec::new(),
            profile_registry: run_registry,
            hook_executor: &hook_executor,
        })
        .await
        .map_err(|e| PlannerError::Call(e.to_string()))?;

        let patch = match result {
            FeaturePlannerResult::Patched { patch, .. } => *patch,
            FeaturePlannerResult::OutOfScope => {
                return Err(PlannerError::Output(
                    "the planner judged the request out of scope for this roadmap".into(),
                ));
            },
        };

        // The planner's patch may not name its target (the CLI normalizes it
        // after the fact); the intake path always targets the project
        // roadmap, so pin it here.
        let mut patch = patch;
        if matches!(
            patch.target,
            RoadmapPatchTarget::RunRoadmap { .. } | RoadmapPatchTarget::ProjectRoadmap { .. }
        ) {
            patch.target = RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: crate::intake_planner::PROJECT_ROADMAP_RELPATH.into(),
            };
        }

        let _ = writer.close().await;
        Ok(patch)
    }
}

/// Convenience constructor for the daemon's wiring.
#[must_use]
pub fn planner_for_daemon(
    storage: Arc<Storage>,
    bridge: Arc<dyn BridgeFacade>,
    profile_registry: Arc<crate::profile_loader::ProfileRegistry>,
) -> Arc<dyn RoadmapPlanner> {
    Arc::new(FeaturePlannerRoadmapPlanner::new(
        storage,
        bridge,
        profile_registry,
    ))
}
