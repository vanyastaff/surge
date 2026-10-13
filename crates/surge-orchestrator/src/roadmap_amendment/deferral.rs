//! "Defer to next milestone": rewrite a patch so no operation lands inside a
//! milestone the engine is already running (#103).
//!
//! - An insertion anchored on a running milestone moves before the next
//!   pending milestone, else to the end of the roadmap.
//! - A task for a running milestone moves to the next pending milestone, else
//!   into a new `<id>-deferred` milestone appended to the roadmap.
//! - A replacement of a running item becomes new pending work placed the same
//!   way; the running item itself is never rewritten.

use std::collections::BTreeSet;

use surge_core::roadmap_patch::{
    InsertionPoint, RoadmapItemRef, RoadmapPatchConflict, RoadmapPatchConflictCode,
    RoadmapPatchItem, RoadmapPatchOperation,
};
use surge_core::{MilestoneId, RoadmapArtifact, RoadmapMilestone, RoadmapPatch, RoadmapStatus};

use super::unique_id;

/// `patch` with every operation that targets a running milestone (per
/// `conflicts`) rewritten to land after it.
pub(super) fn defer_to_next_milestone(
    roadmap: &RoadmapArtifact,
    patch: &RoadmapPatch,
    conflicts: &[RoadmapPatchConflict],
) -> RoadmapPatch {
    let mut deferred = patch.clone();
    let mut used: BTreeSet<MilestoneId> = roadmap
        .milestones
        .iter()
        .map(|milestone| milestone.id.clone())
        .collect();
    for operation in &mut deferred.operations {
        rewrite(operation, roadmap, conflicts, &mut used);
    }
    tracing::debug!(
        target: "roadmap_amendment",
        patch_id = %patch.id,
        "roadmap_patch_deferred_to_next_milestone"
    );
    deferred
}

fn rewrite(
    operation: &mut RoadmapPatchOperation,
    roadmap: &RoadmapArtifact,
    conflicts: &[RoadmapPatchConflict],
    used: &mut BTreeSet<MilestoneId>,
) {
    match operation {
        RoadmapPatchOperation::AddMilestone { insertion, .. } => {
            if let Some(milestone_id) = insertion
                .as_ref()
                .and_then(insertion_milestone_id)
                .filter(|milestone_id| running(conflicts, milestone_id))
            {
                *insertion = Some(safe_insertion_after(roadmap, milestone_id));
            }
        },
        RoadmapPatchOperation::AddTask {
            milestone_id,
            task,
            insertion,
        } => {
            if !running(conflicts, milestone_id) {
                return;
            }
            if let Some(next_id) = next_pending_after(roadmap, milestone_id) {
                *milestone_id = next_id;
                *insertion = Some(InsertionPoint::AppendToMilestone {
                    milestone_id: milestone_id.clone(),
                });
            } else {
                let source = milestone_id.clone();
                let new_id = unique_id(&format!("{source}-deferred"), used);
                let mut milestone =
                    RoadmapMilestone::new(new_id, format!("Deferred work after {source}"));
                task.status = RoadmapStatus::Pending;
                milestone.tasks.push(task.clone());
                *operation = RoadmapPatchOperation::AddMilestone {
                    milestone,
                    insertion: Some(InsertionPoint::AppendToRoadmap),
                };
            }
        },
        RoadmapPatchOperation::ReplaceDraftItem {
            target,
            replacement,
            ..
        } => {
            let milestone_id = item_milestone(target);
            if running(conflicts, milestone_id) {
                *operation = deferred_replacement(milestone_id, replacement, roadmap, used);
            }
        },
    }
}

fn deferred_replacement(
    milestone_id: &MilestoneId,
    replacement: &RoadmapPatchItem,
    roadmap: &RoadmapArtifact,
    used: &mut BTreeSet<MilestoneId>,
) -> RoadmapPatchOperation {
    match replacement {
        RoadmapPatchItem::Milestone { milestone } => {
            let mut milestone = milestone.clone();
            milestone.status = RoadmapStatus::Pending;
            for task in &mut milestone.tasks {
                task.status = RoadmapStatus::Pending;
            }
            RoadmapPatchOperation::AddMilestone {
                milestone,
                insertion: Some(safe_insertion_after(roadmap, milestone_id)),
            }
        },
        RoadmapPatchItem::Task { task } => {
            let mut task = task.clone();
            task.status = RoadmapStatus::Pending;
            if let Some(next_id) = next_pending_after(roadmap, milestone_id) {
                RoadmapPatchOperation::AddTask {
                    milestone_id: next_id.clone(),
                    task,
                    insertion: Some(InsertionPoint::AppendToMilestone {
                        milestone_id: next_id,
                    }),
                }
            } else {
                let new_id = unique_id(&format!("{milestone_id}-deferred"), used);
                let mut milestone =
                    RoadmapMilestone::new(new_id, format!("Deferred rework after {milestone_id}"));
                milestone.tasks.push(task);
                RoadmapPatchOperation::AddMilestone {
                    milestone,
                    insertion: Some(InsertionPoint::AppendToRoadmap),
                }
            }
        },
    }
}

/// The milestone an item belongs to.
pub(super) fn item_milestone(item: &RoadmapItemRef) -> &MilestoneId {
    match item {
        RoadmapItemRef::Milestone { milestone_id } | RoadmapItemRef::Task { milestone_id, .. } => {
            milestone_id
        },
    }
}

fn running(conflicts: &[RoadmapPatchConflict], milestone_id: &MilestoneId) -> bool {
    conflicts.iter().any(|conflict| {
        conflict.code == RoadmapPatchConflictCode::RunningMilestone
            && conflict.item.as_ref().map(item_milestone) == Some(milestone_id)
    })
}

fn insertion_milestone_id(insertion: &InsertionPoint) -> Option<&MilestoneId> {
    match insertion {
        InsertionPoint::BeforeMilestone { milestone_id }
        | InsertionPoint::AfterMilestone { milestone_id }
        | InsertionPoint::AppendToMilestone { milestone_id }
        | InsertionPoint::BeforeTask { milestone_id, .. }
        | InsertionPoint::AfterTask { milestone_id, .. } => Some(milestone_id),
        InsertionPoint::AppendToRoadmap => None,
    }
}

fn safe_insertion_after(roadmap: &RoadmapArtifact, milestone_id: &MilestoneId) -> InsertionPoint {
    next_pending_after(roadmap, milestone_id).map_or(InsertionPoint::AppendToRoadmap, |next_id| {
        InsertionPoint::BeforeMilestone {
            milestone_id: next_id,
        }
    })
}

fn next_pending_after(
    roadmap: &RoadmapArtifact,
    milestone_id: &MilestoneId,
) -> Option<MilestoneId> {
    let start = roadmap
        .milestones
        .iter()
        .position(|milestone| milestone.id == *milestone_id)?;
    roadmap
        .milestones
        .iter()
        .skip(start + 1)
        .find(|milestone| milestone.status == RoadmapStatus::Pending)
        .map(|milestone| milestone.id.clone())
}
