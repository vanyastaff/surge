//! Intake → roadmap planning (ADR-0020 T11).
//!
//! An external task (GitHub/Linear ticket, `surge task add`) must not start a
//! detached run that bypasses the project's roadmap. It enters the **same
//! queue**: the feature planner drafts a `roadmap-patch.toml`, the patch is
//! applied to `.surge/roadmap.toml` (with `RunningMilestone` conflicts
//! deferred to the next milestone automatically), and the mirror projects the
//! new tasks into the registry queue.
//!
//! ## The seam, and why
//!
//! The planner itself is an LLM agent turn through ACP. Keeping it behind
//! [`RoadmapPlanner`] means the route from "ticket text" to "queue rows" is
//! testable without a live agent, and the daemon's launcher can stay ignorant
//! of ACP wiring. [`FeaturePlannerRoadmapPlanner`] is the production adapter;
//! tests use a stub that returns a fixed patch.
//!
//! ## What this module owns
//!
//! - reading and parsing the project roadmap;
//! - applying the patch and, on `RunningMilestone`, rewriting the operations
//!   to land after the running milestone (the "defer" choice the spec makes
//!   automatic for intake);
//! - rendering the amended roadmap back to TOML and returning the queue rows
//!   for the caller to mirror;
//! - the named errors for every refusal (no roadmap, invalid patch, conflict
//!   the deferral cannot resolve, planner failure).

pub mod feature_planner;

use std::path::{Path, PathBuf};

use surge_core::roadmap::RoadmapArtifact;
use surge_core::roadmap_patch::{
    InsertionPoint, RoadmapPatch, RoadmapPatchApplyError, RoadmapPatchConflictCode,
    RoadmapPatchOperation,
};

/// Relative path of the project roadmap inside the repository.
pub const PROJECT_ROADMAP_RELPATH: &str = ".surge/roadmap.toml";

/// A planner that turns a feature request plus the current roadmap into a
/// patch. Implemented by the ACP feature-planner driver in production.
#[async_trait::async_trait]
pub trait RoadmapPlanner: Send + Sync {
    /// Draft a patch for `request` against `roadmap_toml`.
    ///
    /// # Errors
    /// A human-readable reason when the planner cannot produce a patch (no
    /// agent binary, model refusal, unparseable output).
    async fn plan(
        &self,
        request: String,
        roadmap_toml: String,
    ) -> Result<RoadmapPatch, PlannerError>;
}

/// Why a planner could not produce a patch.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlannerError {
    /// The underlying ACP/agent call failed.
    #[error("planner call failed: {0}")]
    Call(String),
    /// The planner's output was not a parseable patch.
    #[error("planner output is not a valid roadmap patch: {0}")]
    Output(String),
}

/// Outcome of planning one request into the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedIntake {
    /// The patch as applied (post-deferral when a conflict was resolved).
    pub patch: RoadmapPatch,
    /// The amended roadmap.
    pub roadmap: RoadmapArtifact,
    /// TOML rendering of `roadmap` — what the caller writes to disk.
    pub roadmap_toml: String,
    /// Queue rows the caller mirrors after writing the roadmap.
    pub queue_entries: Vec<surge_core::roadmap::QueueMirrorEntry>,
    /// True when a `RunningMilestone` conflict was auto-deferred.
    pub deferred: bool,
}

/// Why a request could not be planned into the queue.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IntakePlanError {
    /// The project has no `.surge/roadmap.toml` (the caller should fall back
    /// to the bootstrap path).
    #[error("project has no roadmap at {0}")]
    NoRoadmap(PathBuf),
    /// The roadmap file could not be read.
    #[error("read {path}: {source}")]
    Read {
        /// File that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The roadmap file did not parse.
    #[error("parse {path}: {source}")]
    Parse {
        /// File that failed.
        path: PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// The planner failed.
    #[error(transparent)]
    Planner(#[from] PlannerError),
    /// The patch conflicts with the roadmap and deferral could not resolve it.
    #[error("patch conflicts: {0}")]
    Conflict(String),
    /// The patch shape is invalid.
    #[error("patch is invalid: {0}")]
    InvalidPatch(String),
    /// The amended roadmap could not be rendered.
    #[error("render roadmap: {0}")]
    Render(#[from] toml::ser::Error),
}

/// Read the project roadmap, ask `planner` for a patch, apply it, and return
/// the amended roadmap plus the queue rows.
///
/// A `RunningMilestone` conflict is resolved by moving the affected
/// operations to the next pending milestone (or a new "deferred" milestone at
/// the end) — the automatic deferral the spec specifies for intake. Any other
/// conflict is a named error, never a silent drop.
///
/// # Errors
/// [`IntakePlanError`] for every refusal; see its variants.
pub async fn plan_into_queue(
    planner: &dyn RoadmapPlanner,
    project_root: &Path,
    request: &str,
) -> Result<PlannedIntake, IntakePlanError> {
    let roadmap_path = project_root.join(PROJECT_ROADMAP_RELPATH);
    if !roadmap_path.exists() {
        return Err(IntakePlanError::NoRoadmap(roadmap_path));
    }
    let roadmap_toml =
        std::fs::read_to_string(&roadmap_path).map_err(|source| IntakePlanError::Read {
            path: roadmap_path.clone(),
            source,
        })?;
    let roadmap: RoadmapArtifact =
        toml::from_str(&roadmap_toml).map_err(|source| IntakePlanError::Parse {
            path: roadmap_path.clone(),
            source,
        })?;

    let patch = planner
        .plan(request.to_string(), roadmap_toml.clone())
        .await?;
    apply_patch_with_deferral(&roadmap, patch, &roadmap_toml)
}

/// Apply `patch`, deferring a running-milestone conflict if needed.
///
/// Separated from [`plan_into_queue`] so the deferral rule is testable
/// without a planner.
///
/// # Errors
/// [`IntakePlanError`] when the patch is invalid, conflicts beyond what
/// deferral resolves, or the amended roadmap cannot be rendered.
pub fn apply_patch_with_deferral(
    roadmap: &RoadmapArtifact,
    patch: RoadmapPatch,
    _roadmap_toml: &str,
) -> Result<PlannedIntake, IntakePlanError> {
    match patch.apply_to_roadmap(roadmap) {
        Ok(result) => {
            let roadmap_toml = toml::to_string_pretty(&result.roadmap)?;
            let queue_entries = result.roadmap.to_queue_entries();
            Ok(PlannedIntake {
                patch,
                roadmap: result.roadmap,
                roadmap_toml,
                queue_entries,
                deferred: false,
            })
        },
        Err(RoadmapPatchApplyError::InvalidShape { issues }) => {
            let rendered = issues
                .iter()
                .map(|issue| format!("{issue:?}"))
                .collect::<Vec<_>>()
                .join("; ");
            Err(IntakePlanError::InvalidPatch(rendered))
        },
        Err(RoadmapPatchApplyError::Conflicts { conflicts }) => {
            let running: Vec<_> = conflicts
                .iter()
                .filter(|c| c.code == RoadmapPatchConflictCode::RunningMilestone)
                .collect();
            if running.is_empty() {
                let rendered = conflicts
                    .iter()
                    .map(|c| format!("{:?}: {}", c.code, c.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(IntakePlanError::Conflict(rendered));
            }
            // Auto-defer: move the conflicting operations after the running
            // milestone, then re-apply. The deferral is deterministic and
            // local — it never drops an operation.
            let running_ids: Vec<String> = running
                .iter()
                .filter_map(|c| c.item.as_ref())
                .map(|item| match item {
                    surge_core::roadmap_patch::RoadmapItemRef::Milestone { milestone_id } => {
                        milestone_id.clone()
                    },
                    surge_core::roadmap_patch::RoadmapItemRef::Task { milestone_id, .. } => {
                        milestone_id.clone()
                    },
                })
                .collect();
            let deferred_patch = defer_operations(roadmap, patch, &running_ids);
            let result = deferred_patch
                .apply_to_roadmap(roadmap)
                .map_err(|e| IntakePlanError::Conflict(format!("after deferral: {e}")))?;
            let roadmap_toml = toml::to_string_pretty(&result.roadmap)?;
            let queue_entries = result.roadmap.to_queue_entries();
            Ok(PlannedIntake {
                patch: deferred_patch,
                roadmap: result.roadmap,
                roadmap_toml,
                queue_entries,
                deferred: true,
            })
        },
    }
}

/// Rewrite every operation targeting a running milestone to land after it.
///
/// The target is the next pending milestone when one exists, else a new
/// `"<source>-deferred"` milestone appended at the end.
fn defer_operations(
    roadmap: &RoadmapArtifact,
    mut patch: RoadmapPatch,
    running_ids: &[String],
) -> RoadmapPatch {
    for operation in &mut patch.operations {
        match operation {
            RoadmapPatchOperation::AddTask {
                milestone_id,
                insertion,
                ..
            } if running_ids.iter().any(|id| id == milestone_id) => {
                let source = milestone_id.clone();
                let target = next_pending_milestone(roadmap, &source)
                    .unwrap_or_else(|| format!("{source}-deferred"));
                *milestone_id = target.clone();
                *insertion = Some(InsertionPoint::AppendToMilestone {
                    milestone_id: target,
                });
            },
            RoadmapPatchOperation::AddMilestone { insertion, .. } => {
                // A new milestone cannot conflict on its own id; nothing to do.
                let _ = insertion;
            },
            _ => {},
        }
    }
    // Ensure the deferred target exists as a milestone when it is synthetic.
    let synthetic: Vec<String> = patch
        .operations
        .iter()
        .filter_map(|operation| match operation {
            RoadmapPatchOperation::AddTask { milestone_id, .. }
                if milestone_id.ends_with("-deferred")
                    && !roadmap.milestones.iter().any(|m| &m.id == milestone_id)
                    && !patch.operations.iter().any(|op| {
                        matches!(
                            op,
                            RoadmapPatchOperation::AddMilestone { milestone, .. }
                                if &milestone.id == milestone_id
                        )
                    }) =>
            {
                Some(milestone_id.clone())
            },
            _ => None,
        })
        .collect();
    for milestone_id in synthetic.into_iter().rev() {
        let milestone = surge_core::roadmap::RoadmapMilestone::new(milestone_id, "Deferred work");
        patch.operations.insert(
            0,
            RoadmapPatchOperation::AddMilestone {
                milestone,
                insertion: Some(InsertionPoint::AppendToRoadmap),
            },
        );
    }
    patch
}

/// The first milestone after `after` whose status is `Pending`.
fn next_pending_milestone(roadmap: &RoadmapArtifact, after: &str) -> Option<String> {
    use surge_core::roadmap::RoadmapStatus;
    let position = roadmap.milestones.iter().position(|m| m.id == after)?;
    roadmap
        .milestones
        .iter()
        .skip(position + 1)
        .find(|m| m.status == RoadmapStatus::Pending)
        .map(|m| m.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::roadmap::{RoadmapMilestone, RoadmapTask};
    use surge_core::roadmap_patch::{RoadmapItemRef, RoadmapPatchId, RoadmapPatchStatus};

    fn roadmap_with_running_first() -> RoadmapArtifact {
        let mut m1 = RoadmapMilestone::new("m1", "Running now");
        m1.status = surge_core::roadmap::RoadmapStatus::Running;
        m1.tasks.push(RoadmapTask::new("m1-t1", "In flight"));
        let mut m2 = RoadmapMilestone::new("m2", "Next");
        m2.tasks.push(RoadmapTask::new("m2-t1", "Later"));
        RoadmapArtifact::new(vec![m1, m2])
    }

    fn add_task_patch(milestone_id: &str, task_id: &str) -> RoadmapPatch {
        let mut task = RoadmapTask::new(task_id, "From ticket");
        task.size = Some(surge_core::roadmap::TaskSize::S);
        RoadmapPatch {
            schema_version: 1,
            id: RoadmapPatchId::new("p1").expect("valid test id"),
            target: surge_core::roadmap_patch::RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: PROJECT_ROADMAP_RELPATH.into(),
            },
            rationale: "ticket".into(),
            operations: vec![RoadmapPatchOperation::AddTask {
                milestone_id: milestone_id.into(),
                task,
                insertion: Some(InsertionPoint::AppendToMilestone {
                    milestone_id: milestone_id.into(),
                }),
            }],
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            status: RoadmapPatchStatus::Drafted,
        }
    }

    #[test]
    fn clean_patch_lands_in_the_requested_milestone() {
        let roadmap = roadmap_with_running_first();
        let planned =
            apply_patch_with_deferral(&roadmap, add_task_patch("m2", "m2-ticket"), "").unwrap();
        assert!(!planned.deferred);
        assert_eq!(planned.queue_entries.len(), 3);
        assert!(
            planned
                .queue_entries
                .iter()
                .any(|e| e.task_id == "m2-ticket")
        );
    }

    #[test]
    fn running_milestone_conflict_defers_to_the_next_pending_one() {
        let roadmap = roadmap_with_running_first();
        let planned =
            apply_patch_with_deferral(&roadmap, add_task_patch("m1", "ticket-t1"), "").unwrap();
        assert!(planned.deferred, "the conflict must be auto-deferred");
        let m2 = planned
            .roadmap
            .milestones
            .iter()
            .find(|m| m.id == "m2")
            .unwrap();
        assert!(
            m2.tasks.iter().any(|t| t.id == "ticket-t1"),
            "the task must land in the next pending milestone"
        );
    }

    #[test]
    fn running_milestone_without_a_pending_successor_gets_a_deferred_milestone() {
        let mut m1 = RoadmapMilestone::new("m1", "Running");
        m1.status = surge_core::roadmap::RoadmapStatus::Running;
        m1.tasks.push(RoadmapTask::new("m1-t1", "In flight"));
        let roadmap = RoadmapArtifact::new(vec![m1]);
        let planned =
            apply_patch_with_deferral(&roadmap, add_task_patch("m1", "ticket-t1"), "").unwrap();
        assert!(planned.deferred);
        assert!(
            planned
                .roadmap
                .milestones
                .iter()
                .any(|m| m.id == "m1-deferred" && m.tasks.iter().any(|t| t.id == "ticket-t1")),
            "a synthetic deferred milestone must be created: {:?}",
            planned.roadmap.milestones
        );
    }

    #[test]
    fn missing_roadmap_is_a_named_error() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(plan_into_queue(&StubPlanner, dir.path(), "do it"))
            .unwrap_err();
        assert!(matches!(error, IntakePlanError::NoRoadmap(_)));
    }

    struct StubPlanner;

    #[async_trait::async_trait]
    impl RoadmapPlanner for StubPlanner {
        async fn plan(
            &self,
            _request: String,
            _roadmap_toml: String,
        ) -> Result<RoadmapPatch, PlannerError> {
            Err(PlannerError::Call("stub".into()))
        }
    }

    #[tokio::test]
    async fn planner_failure_propagates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".surge")).unwrap();
        std::fs::write(
            dir.path().join(PROJECT_ROADMAP_RELPATH),
            toml::to_string(&roadmap_with_running_first()).unwrap(),
        )
        .unwrap();
        let error = plan_into_queue(&StubPlanner, dir.path(), "do it")
            .await
            .unwrap_err();
        assert!(matches!(error, IntakePlanError::Planner(_)));
    }

    #[tokio::test]
    async fn planned_roadmap_is_written_and_mirrors_to_the_queue() {
        struct FixedPlanner;
        #[async_trait::async_trait]
        impl RoadmapPlanner for FixedPlanner {
            async fn plan(
                &self,
                _request: String,
                _roadmap_toml: String,
            ) -> Result<RoadmapPatch, PlannerError> {
                Ok(add_task_patch("m1", "ticket-t1"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".surge")).unwrap();
        std::fs::write(
            dir.path().join(PROJECT_ROADMAP_RELPATH),
            toml::to_string(&roadmap_with_running_first()).unwrap(),
        )
        .unwrap();

        let planned = plan_into_queue(&FixedPlanner, dir.path(), "fix the thing")
            .await
            .unwrap();
        assert!(planned.deferred);
        // The returned TOML is what the caller writes; it must parse back to
        // the same task set (the queue mirror is built from the file).
        let reparsed: RoadmapArtifact = toml::from_str(&planned.roadmap_toml).unwrap();
        assert!(
            reparsed
                .milestones
                .iter()
                .any(|m| m.id == "m2" && m.tasks.iter().any(|t| t.id == "ticket-t1"))
        );
        assert!(
            planned
                .queue_entries
                .iter()
                .any(|e| e.task_id == "ticket-t1")
        );
    }

    #[test]
    fn non_running_conflict_is_not_deferred() {
        // A duplicate task id in a clean milestone is a `DuplicateItem`
        // conflict; deferral has no answer for it and must refuse.
        let roadmap = roadmap_with_running_first();
        let mut patch = add_task_patch("m2", "m2-t1");
        patch
            .conflicts
            .push(surge_core::roadmap_patch::RoadmapPatchConflict {
                code: RoadmapPatchConflictCode::DuplicateItem,
                item: Some(RoadmapItemRef::Task {
                    milestone_id: "m2".into(),
                    task_id: "m2-t1".into(),
                }),
                message: "already exists".into(),
                choices: surge_core::roadmap_patch::conflict_choices_for_code(
                    RoadmapPatchConflictCode::DuplicateItem,
                ),
                selected_choice: None,
            });
        let error = apply_patch_with_deferral(&roadmap, patch, "").unwrap_err();
        assert!(
            matches!(error, IntakePlanError::Conflict(_)),
            "expected Conflict, got {error:?}"
        );
    }
}
