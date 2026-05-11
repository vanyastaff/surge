//! Roadmap types — project-level planning across multiple specs.

use serde::{Deserialize, Serialize};

use crate::artifact_contract::ARTIFACT_SCHEMA_VERSION;
use crate::id::SpecId;
use crate::spec::Complexity;

/// Machine-readable `roadmap.toml` artifact.
///
/// This is the planning artifact that agents exchange before a concrete
/// `flow.toml` is generated. The older [`Timeline`] scheduling model remains
/// available for runtime planning; this wrapper captures the authored roadmap
/// shape and schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapArtifact {
    /// Artifact contract schema version.
    #[serde(default = "default_artifact_schema_version")]
    pub schema_version: u32,
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
            schema_version: ARTIFACT_SCHEMA_VERSION,
            milestones,
            dependencies: Vec::new(),
            risks: Vec::new(),
        }
    }

    /// Render a deterministic `roadmap.md` representation.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Roadmap\n\n");
        for milestone in &self.milestones {
            out.push_str(&format!("## {}: {}\n", milestone.id, milestone.title));
            if milestone.status != RoadmapStatus::Pending {
                out.push_str(&format!("Status: {}\n", milestone.status));
            }
            if milestone.tasks.is_empty() {
                out.push('\n');
                continue;
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
                out.push('\n');
                if let Some(description) = &task.description {
                    out.push_str(&format!("  - {}\n", description));
                }
                for criterion in &task.acceptance_criteria {
                    out.push_str(&format!("  - AC: {criterion}\n"));
                }
            }
            out.push('\n');
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

impl Default for RoadmapArtifact {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// One deliverable-focused roadmap milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapMilestone {
    /// Stable human-authored identifier, for example `m1`.
    pub id: String,
    /// Human-readable milestone title.
    pub title: String,
    /// Current execution status for amendment safety checks.
    #[serde(default, skip_serializing_if = "RoadmapStatus::is_pending")]
    pub status: RoadmapStatus,
    /// Ordered tasks within this milestone.
    #[serde(default)]
    pub tasks: Vec<RoadmapTask>,
}

impl RoadmapMilestone {
    /// Create an empty milestone.
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status: RoadmapStatus::Pending,
            tasks: Vec::new(),
        }
    }
}

/// One task within a roadmap milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapTask {
    /// Stable human-authored identifier, for example `m1-t1`.
    pub id: String,
    /// Human-readable task title.
    pub title: String,
    /// Current execution status for amendment safety checks.
    #[serde(default, skip_serializing_if = "RoadmapStatus::is_pending")]
    pub status: RoadmapStatus,
    /// Optional short description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Acceptance criteria that downstream spec/story authors can refine.
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

impl RoadmapTask {
    /// Create a task with no optional description or criteria.
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status: RoadmapStatus::Pending,
            description: None,
            acceptance_criteria: Vec::new(),
        }
    }
}

/// Directed dependency between roadmap milestones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapDependency {
    /// Milestone that must finish first.
    pub from: String,
    /// Milestone that depends on `from`.
    pub to: String,
    /// Human-readable reason for the dependency.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Risk tracked by the roadmap artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapRisk {
    /// Risk description.
    pub description: String,
    /// Optional mitigation plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mitigation: Option<String>,
}

/// A single item in a project roadmap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoadmapItem {
    /// Spec this item refers to.
    pub spec_id: SpecId,
    /// Human-readable title.
    pub title: String,
    /// Estimated complexity.
    pub complexity: Complexity,
    /// Priority for scheduling (higher = more important).
    #[serde(default)]
    pub priority: Priority,
    /// Specs that must complete before this one can start.
    #[serde(default)]
    pub depends_on: Vec<SpecId>,
    /// Current execution status.
    #[serde(default)]
    pub status: RoadmapStatus,
}

/// Priority level for roadmap scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Critical,
    High,
    #[default]
    Medium,
    Low,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

/// Execution status of a roadmap item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoadmapStatus {
    /// Not yet started.
    #[default]
    Pending,
    /// Currently being executed by the orchestrator.
    Running,
    /// Paused, waiting for human input or gate.
    Paused,
    /// Completed successfully.
    Completed,
    /// Failed — may be retried.
    Failed,
    /// Skipped (dependency failed or user cancelled).
    Skipped,
}

impl RoadmapStatus {
    /// Returns `true` when the item has not started.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Returns `true` if no further execution will happen.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Skipped)
    }
}

impl std::fmt::Display for RoadmapStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        })
    }
}

const fn markdown_checkbox(status: RoadmapStatus) -> &'static str {
    match status {
        RoadmapStatus::Completed => "x",
        RoadmapStatus::Running => "~",
        _ => " ",
    }
}

/// A project timeline — ordered batches of roadmap items.
///
/// Items within a batch are independent and can run in parallel.
/// Batches are executed sequentially.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Timeline {
    /// Ordered batches of roadmap items.
    pub batches: Vec<TimelineBatch>,
}

/// A single batch in a timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineBatch {
    /// Batch execution order (0-based).
    pub order: usize,
    /// Items in this batch (can run in parallel).
    pub items: Vec<RoadmapItem>,
    /// Why this batch is ordered this way.
    #[serde(default)]
    pub reason: String,
}

fn default_artifact_schema_version() -> u32 {
    ARTIFACT_SCHEMA_VERSION
}

impl Timeline {
    /// Create a new empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of items across all batches.
    #[must_use]
    pub fn total_items(&self) -> usize {
        self.batches.iter().map(|b| b.items.len()).sum()
    }

    /// Count items by status.
    #[must_use]
    pub fn count_by_status(&self, status: RoadmapStatus) -> usize {
        self.batches
            .iter()
            .flat_map(|b| &b.items)
            .filter(|item| item.status == status)
            .count()
    }

    /// Find a mutable reference to a roadmap item by spec ID.
    pub fn find_item_mut(&mut self, spec_id: SpecId) -> Option<&mut RoadmapItem> {
        self.batches
            .iter_mut()
            .flat_map(|b| &mut b.items)
            .find(|item| item.spec_id == spec_id)
    }

    /// Returns the next batch of pending items ready for execution.
    ///
    /// A batch is ready when all previous batches have completed.
    #[must_use]
    pub fn next_ready_batch(&self) -> Option<&TimelineBatch> {
        for batch in &self.batches {
            let all_terminal = batch.items.iter().all(|i| i.status.is_terminal());
            if all_terminal {
                continue;
            }
            let has_pending = batch
                .items
                .iter()
                .any(|i| i.status == RoadmapStatus::Pending);
            if has_pending {
                return Some(batch);
            }
            // Batch has running/paused items — wait for them.
            return None;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::SpecId;

    fn make_item(title: &str, status: RoadmapStatus) -> RoadmapItem {
        RoadmapItem {
            spec_id: SpecId::new(),
            title: title.to_string(),
            complexity: Complexity::Standard,
            priority: Priority::Medium,
            depends_on: vec![],
            status,
        }
    }

    #[test]
    fn test_timeline_total_items() {
        let timeline = Timeline {
            batches: vec![
                TimelineBatch {
                    order: 0,
                    items: vec![
                        make_item("A", RoadmapStatus::Pending),
                        make_item("B", RoadmapStatus::Pending),
                    ],
                    reason: String::new(),
                },
                TimelineBatch {
                    order: 1,
                    items: vec![make_item("C", RoadmapStatus::Pending)],
                    reason: String::new(),
                },
            ],
        };
        assert_eq!(timeline.total_items(), 3);
    }

    #[test]
    fn test_timeline_count_by_status() {
        let timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![
                    make_item("A", RoadmapStatus::Completed),
                    make_item("B", RoadmapStatus::Pending),
                    make_item("C", RoadmapStatus::Completed),
                ],
                reason: String::new(),
            }],
        };
        assert_eq!(timeline.count_by_status(RoadmapStatus::Completed), 2);
        assert_eq!(timeline.count_by_status(RoadmapStatus::Pending), 1);
    }

    #[test]
    fn test_timeline_next_ready_batch() {
        let timeline = Timeline {
            batches: vec![
                TimelineBatch {
                    order: 0,
                    items: vec![make_item("A", RoadmapStatus::Completed)],
                    reason: String::new(),
                },
                TimelineBatch {
                    order: 1,
                    items: vec![make_item("B", RoadmapStatus::Pending)],
                    reason: String::new(),
                },
            ],
        };
        let batch = timeline.next_ready_batch().unwrap();
        assert_eq!(batch.order, 1);
    }

    #[test]
    fn test_timeline_next_ready_batch_none_when_running() {
        let timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![make_item("A", RoadmapStatus::Running)],
                reason: String::new(),
            }],
        };
        assert!(timeline.next_ready_batch().is_none());
    }

    #[test]
    fn test_timeline_next_ready_batch_none_when_all_done() {
        let timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![make_item("A", RoadmapStatus::Completed)],
                reason: String::new(),
            }],
        };
        assert!(timeline.next_ready_batch().is_none());
    }

    #[test]
    fn test_roadmap_status_is_terminal() {
        assert!(RoadmapStatus::Completed.is_terminal());
        assert!(RoadmapStatus::Failed.is_terminal());
        assert!(RoadmapStatus::Skipped.is_terminal());
        assert!(!RoadmapStatus::Pending.is_terminal());
        assert!(!RoadmapStatus::Running.is_terminal());
        assert!(!RoadmapStatus::Paused.is_terminal());
    }

    #[test]
    fn test_priority_display() {
        assert_eq!(Priority::Critical.to_string(), "critical");
        assert_eq!(Priority::High.to_string(), "high");
        assert_eq!(Priority::Medium.to_string(), "medium");
        assert_eq!(Priority::Low.to_string(), "low");
    }

    #[test]
    fn test_roadmap_item_toml_roundtrip() {
        let item = make_item("Test item", RoadmapStatus::Pending);
        let toml_str = toml::to_string(&item).unwrap();
        let deserialized: RoadmapItem = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.title, "Test item");
        assert_eq!(deserialized.status, RoadmapStatus::Pending);
        assert_eq!(deserialized.priority, Priority::Medium);
    }

    #[test]
    fn test_roadmap_artifact_toml_roundtrip() {
        let mut milestone = RoadmapMilestone::new("m1", "Artifact contracts");
        let mut task = RoadmapTask::new("m1-t1", "Define schema");
        task.acceptance_criteria
            .push("schema_version is present".to_string());
        milestone.tasks.push(task);

        let mut artifact = RoadmapArtifact::new(vec![milestone]);
        artifact.dependencies.push(RoadmapDependency {
            from: "m1".to_string(),
            to: "m2".to_string(),
            reason: "contracts unblock validators".to_string(),
        });
        artifact.risks.push(RoadmapRisk {
            description: "legacy markdown drift".to_string(),
            mitigation: Some("keep compatibility docs".to_string()),
        });

        let toml_str = toml::to_string(&artifact).unwrap();
        let deserialized: RoadmapArtifact = toml::from_str(&toml_str).unwrap();

        assert_eq!(deserialized.schema_version, ARTIFACT_SCHEMA_VERSION);
        assert_eq!(deserialized.milestones[0].id, "m1");
        assert_eq!(deserialized.milestones[0].tasks[0].id, "m1-t1");
        assert_eq!(deserialized.dependencies[0].to, "m2");
        assert_eq!(
            deserialized.risks[0].mitigation.as_deref(),
            Some("keep compatibility docs")
        );
    }

    #[test]
    fn test_find_item_mut() {
        let spec_id = SpecId::new();
        let mut timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![RoadmapItem {
                    spec_id,
                    title: "Find me".to_string(),
                    complexity: Complexity::Simple,
                    priority: Priority::High,
                    depends_on: vec![],
                    status: RoadmapStatus::Pending,
                }],
                reason: String::new(),
            }],
        };

        let item = timeline.find_item_mut(spec_id).unwrap();
        item.status = RoadmapStatus::Running;
        assert_eq!(timeline.batches[0].items[0].status, RoadmapStatus::Running);
    }

    #[test]
    fn to_markdown_preserves_running_checkbox_marker() {
        let mut milestone = RoadmapMilestone::new("m1", "Active");
        milestone
            .tasks
            .push(RoadmapTask::new("m1-t1", "Currently running"));
        milestone.tasks[0].status = RoadmapStatus::Running;
        let roadmap = RoadmapArtifact::new(vec![milestone]);

        let markdown = roadmap.to_markdown();

        assert!(markdown.contains("- [~] m1-t1: Currently running (running)"));
    }
}
