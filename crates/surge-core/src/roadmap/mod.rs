//! Roadmap types — project-level planning across multiple specs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::collections::{HashMap, HashSet};

use crate::artifact_contract::{ARTIFACT_SCHEMA_VERSION, ROADMAP_SCHEMA_VERSION};
use crate::id::SpecId;
use crate::spec::Complexity;

/// Schema version in which a per-task `size` became mandatory. Pinned to the
/// exact version rather than `ROADMAP_SCHEMA_VERSION` so a future bump can't
/// silently drop the requirement for v2 roadmaps.
const SIZE_REQUIRED_FROM_VERSION: u32 = 2;

/// Machine-readable `roadmap.toml` artifact.
///
/// This is the planning artifact that agents exchange before a concrete
/// `flow.toml` is generated. The older [`Timeline`] scheduling model remains
/// available for runtime planning; this wrapper captures the authored roadmap
/// shape and schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    title = "RoadmapArtifact",
    description = "Surge `roadmap.toml` artifact: optional release stages, optional missions with validation contracts, ordered milestones, cross-milestone dependencies, and tracked risks."
)]
pub struct RoadmapArtifact {
    /// Artifact contract schema version.
    #[serde(default = "default_artifact_schema_version")]
    pub schema_version: u32,
    /// Release stages, in delivery order.
    ///
    /// Optional, and one stage is as valid as several: the planner picks the
    /// smallest structure that fits the work. When present, the stages
    /// partition [`Self::milestones`] the same way missions do: every
    /// milestone belongs to exactly one stage and concatenating the stages'
    /// milestone lists reproduces the milestone order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<RoadmapStage>,
    /// One sentence justifying the number of stages (or why there is only
    /// one). Informational; never validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages_rationale: Option<String>,
    /// Missions grouping the milestones, in execution order.
    ///
    /// Optional. When present, the missions partition [`Self::milestones`]:
    /// every milestone belongs to exactly one mission and concatenating the
    /// missions' milestone lists reproduces the milestone order. Milestones
    /// stay a flat list so flows keep iterating `milestones` directly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missions: Vec<RoadmapMission>,
    /// Deliverable-focused milestones in execution order.
    #[serde(default)]
    pub milestones: Vec<RoadmapMilestone>,
    /// Cross-milestone dependencies.
    #[serde(default)]
    pub dependencies: Vec<RoadmapDependency>,
    /// Known delivery risks.
    #[serde(default)]
    pub risks: Vec<RoadmapRisk>,
}

impl RoadmapArtifact {
    /// Create a roadmap artifact with the current schema version.
    #[must_use]
    pub fn new(milestones: Vec<RoadmapMilestone>) -> Self {
        Self {
            schema_version: ROADMAP_SCHEMA_VERSION,
            stages: Vec::new(),
            stages_rationale: None,
            missions: Vec::new(),
            milestones,
            dependencies: Vec::new(),
            risks: Vec::new(),
        }
    }

    /// Render a deterministic `roadmap.md` representation.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Roadmap\n\n");
        if !self.missions.is_empty() {
            out.push_str("## Missions\n");
            for mission in &self.missions {
                out.push_str(&format!(
                    "### {}: {} ({})\n",
                    mission.id,
                    mission.title,
                    join_ids(&mission.milestones, ", ")
                ));
                if mission.status != RoadmapStatus::Pending {
                    out.push_str(&format!("Status: {}\n", mission.status));
                }
                if !mission.goal.trim().is_empty() {
                    out.push_str(&format!("Goal: {}\n", mission.goal));
                }
                for assertion in &mission.validation_contract {
                    out.push_str(&format!(
                        "- {}: {} — {}\n",
                        assertion.id, assertion.title, assertion.pass_condition
                    ));
                }
                out.push('\n');
            }
        }
        if self.stages.is_empty() {
            for milestone in &self.milestones {
                push_milestone_markdown(&mut out, milestone);
            }
        } else {
            if let Some(rationale) = self
                .stages_rationale
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                out.push_str(&format!("Stages: {rationale}\n\n"));
            }
            let mut rendered: HashSet<&MilestoneId> = HashSet::new();
            for stage in &self.stages {
                out.push_str(&format!("# Stage {}: {}\n", stage.id, stage.title));
                if !stage.goal.trim().is_empty() {
                    out.push_str(&format!("Goal: {}\n", stage.goal));
                }
                for criterion in &stage.exit_criteria {
                    out.push_str(&format!("- Exit: {criterion}\n"));
                }
                out.push('\n');
                let owned = stage
                    .milestones
                    .iter()
                    .filter_map(|id| self.milestones.iter().find(|milestone| milestone.id == *id));
                for milestone in owned.filter(|milestone| rendered.insert(&milestone.id)) {
                    push_milestone_markdown(&mut out, milestone);
                }
            }
            // Milestones a malformed roadmap left outside every stage still
            // show up, so the rendering never hides work.
            for milestone in &self.milestones {
                if !rendered.contains(&milestone.id) {
                    push_milestone_markdown(&mut out, milestone);
                }
            }
        }
        if !self.dependencies.is_empty() {
            out.push_str("## Dependencies\n");
            for dependency in &self.dependencies {
                out.push_str(&format!("- {} -> {}", dependency.from, dependency.to));
                if !dependency.reason.trim().is_empty() {
                    out.push_str(&format!(": {}", dependency.reason));
                }
                out.push('\n');
            }
            out.push('\n');
        }
        if !self.risks.is_empty() {
            out.push_str("## Risks\n");
            for risk in &self.risks {
                out.push_str(&format!("- {}", risk.description));
                if let Some(mitigation) = &risk.mitigation {
                    out.push_str(&format!(" (mitigation: {mitigation})"));
                }
                out.push('\n');
            }
        }
        out
    }
}

fn join_ids<T: std::fmt::Display>(ids: &[T], separator: &str) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

fn push_milestone_markdown(out: &mut String, milestone: &RoadmapMilestone) {
    out.push_str(&format!("## {}: {}", milestone.id, milestone.title));
    if let Some(priority) = milestone.priority {
        out.push_str(&format!(" [{priority}]"));
    }
    out.push('\n');
    if milestone.status != RoadmapStatus::Pending {
        out.push_str(&format!("Status: {}\n", milestone.status));
    }
    if milestone.tasks.is_empty() {
        out.push('\n');
        return;
    }
    for task in &milestone.tasks {
        out.push_str(&format!(
            "- [{}] {}: {}",
            markdown_checkbox(task.status),
            task.id,
            task.title
        ));
        if task.status != RoadmapStatus::Pending {
            out.push_str(&format!(" ({})", task.status));
        }
        if let Some(priority) = task.priority {
            out.push_str(&format!(" [{priority}]"));
        }
        if let Some(group) = &task.parallel_group {
            out.push_str(&format!(" [parallel: {group}]"));
        }
        out.push('\n');
        if let Some(description) = &task.description {
            out.push_str(&format!("  - {}\n", description));
        }
        for criterion in &task.acceptance_criteria {
            out.push_str(&format!("  - AC: {criterion}\n"));
        }
        if !task.fulfills.is_empty() {
            out.push_str(&format!(
                "  - Fulfills: {}\n",
                join_ids(&task.fulfills, ", ")
            ));
        }
    }
    out.push('\n');
}

impl Default for RoadmapArtifact {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

fn default_artifact_schema_version() -> u32 {
    ROADMAP_SCHEMA_VERSION
}

fn default_discovered_tasks_schema_version() -> u32 {
    ARTIFACT_SCHEMA_VERSION
}

// verification-report is a v1 artifact; its validator requires
// `ARTIFACT_SCHEMA_VERSION` exactly. Must NOT reuse
// `default_artifact_schema_version`, which the roadmap bump raised to v2.
fn default_verification_report_schema_version() -> u32 {
    ARTIFACT_SCHEMA_VERSION
}

mod discovered;
mod ids;
mod ledger;
mod plan;
mod timeline;
mod verification;

pub use discovered::*;
pub use ids::{AssertionId, MilestoneId, MissionId, RoadmapTaskId, StageId};
pub use ledger::*;
use plan::markdown_checkbox;
pub use plan::*;
pub use timeline::*;
pub use verification::*;

#[cfg(test)]
mod tests;
