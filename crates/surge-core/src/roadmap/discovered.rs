//! `discovered-tasks` artifact: work found while executing a task.

use super::*;

/// The `discovered-tasks.toml` artifact — work an agent found mid-task and
/// wants appended to the ledger. The engine attaches each entry to the current
/// task via a `discovered_from` edge; the artifact itself carries only the new
/// task identity, not the origin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    title = "DiscoveredTasksArtifact",
    description = "Surge `discovered-tasks.toml` artifact: tasks discovered mid-execution to append to the ledger."
)]
pub struct DiscoveredTasksArtifact {
    /// Artifact contract schema version.
    #[serde(default = "default_discovered_tasks_schema_version")]
    pub schema_version: u32,
    /// Newly discovered tasks.
    #[serde(default)]
    pub tasks: Vec<DiscoveredTaskEntry>,
}

/// One issue found by [`DiscoveredTasksArtifact::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveredTaskIssue {
    /// A task entry has an empty id.
    EmptyId,
    /// Two task entries share the same id.
    DuplicateId {
        /// The duplicated id.
        id: RoadmapTaskId,
    },
    /// A task entry has an empty title.
    EmptyTitle {
        /// The id of the task with the empty title.
        id: RoadmapTaskId,
    },
}

impl std::fmt::Display for DiscoveredTaskIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyId => write!(f, "discovered task has an empty id"),
            Self::DuplicateId { id } => {
                write!(f, "duplicate discovered task id {id:?}")
            },
            Self::EmptyTitle { id } => {
                write!(f, "discovered task {id:?} has an empty title")
            },
        }
    }
}

impl DiscoveredTasksArtifact {
    /// Validate the artifact: every entry needs a non-empty id and title, and
    /// ids must be unique within the artifact. Returns an empty vector when
    /// well-formed.
    #[must_use]
    pub fn validate(&self) -> Vec<DiscoveredTaskIssue> {
        let mut issues = Vec::new();
        let mut seen: HashSet<&RoadmapTaskId> = HashSet::new();
        for entry in &self.tasks {
            if entry.id.as_str().trim().is_empty() {
                issues.push(DiscoveredTaskIssue::EmptyId);
            } else if !seen.insert(&entry.id) {
                issues.push(DiscoveredTaskIssue::DuplicateId {
                    id: entry.id.clone(),
                });
            }
            if entry.title.trim().is_empty() {
                issues.push(DiscoveredTaskIssue::EmptyTitle {
                    id: entry.id.clone(),
                });
            }
        }
        issues
    }
}

/// One entry in a [`DiscoveredTasksArtifact`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveredTaskEntry {
    /// Stable id for the discovered task.
    pub id: RoadmapTaskId,
    /// Human-readable title.
    pub title: String,
    /// Optional short description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional acceptance criteria. A split planner fills them so each
    /// replacement task can be verified on its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
}
