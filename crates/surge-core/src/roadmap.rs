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
    description = "Surge `roadmap.toml` artifact: ordered milestones, cross-milestone dependencies, and tracked risks."
)]
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
            schema_version: ROADMAP_SCHEMA_VERSION,
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

impl RoadmapArtifact {
    /// Validate the task-ledger invariants of this roadmap.
    ///
    /// Pure structural checks, independent of any I/O or diagnostics
    /// plumbing: unique milestone/task ids, referential integrity of
    /// task-level `depends_on` / `discovered_from` and milestone-level
    /// `dependencies`, acyclic task dependencies, and — at schema v2 —
    /// a required `size` on every task. Returns an empty vector when the
    /// ledger is well-formed.
    #[must_use]
    pub fn validate_ledger(&self) -> Vec<RoadmapLedgerIssue> {
        let mut issues = Vec::new();

        let mut milestone_ids: HashSet<&str> = HashSet::new();
        for milestone in &self.milestones {
            if !milestone_ids.insert(milestone.id.as_str()) {
                issues.push(RoadmapLedgerIssue::DuplicateMilestoneId {
                    milestone: milestone.id.clone(),
                });
            }
        }

        let mut has_duplicate_task_ids = false;
        let mut task_ids: HashSet<&str> = HashSet::new();
        for task in self.tasks() {
            if !task_ids.insert(task.id.as_str()) {
                has_duplicate_task_ids = true;
                issues.push(RoadmapLedgerIssue::DuplicateTaskId {
                    task: task.id.clone(),
                });
            }
        }

        for task in self.tasks() {
            for dependency in &task.depends_on {
                if dependency == &task.id {
                    issues.push(RoadmapLedgerIssue::SelfDependency {
                        task: task.id.clone(),
                    });
                } else if !task_ids.contains(dependency.as_str()) {
                    issues.push(RoadmapLedgerIssue::UnknownDependsOn {
                        task: task.id.clone(),
                        missing: dependency.clone(),
                    });
                }
            }
            if let Some(origin) = &task.discovered_from {
                if origin == &task.id {
                    issues.push(RoadmapLedgerIssue::SelfDiscovery {
                        task: task.id.clone(),
                    });
                } else if !task_ids.contains(origin.as_str()) {
                    issues.push(RoadmapLedgerIssue::UnknownDiscoveredFrom {
                        task: task.id.clone(),
                        missing: origin.clone(),
                    });
                }
            }
            if self.schema_version >= SIZE_REQUIRED_FROM_VERSION && task.size.is_none() {
                issues.push(RoadmapLedgerIssue::MissingSize {
                    task: task.id.clone(),
                });
            }
        }

        for dependency in &self.dependencies {
            if dependency.from == dependency.to {
                issues.push(RoadmapLedgerIssue::MilestoneSelfDependency {
                    milestone: dependency.from.clone(),
                });
            }
            for milestone in [&dependency.from, &dependency.to] {
                if !milestone_ids.contains(milestone.as_str()) {
                    issues.push(RoadmapLedgerIssue::UnknownMilestoneDependency {
                        missing: milestone.clone(),
                    });
                }
            }
        }

        // Cycle detection is unreliable when duplicate task ids exist (the
        // HashMap in find_task_cycle uses last-write-wins, which can mask
        // edges). Skip it so we don't report a false-negative.
        if !has_duplicate_task_ids && let Some(cycle) = self.find_task_cycle() {
            issues.push(RoadmapLedgerIssue::DependencyCycle { cycle });
        }

        issues
    }

    /// Iterate every task across all milestones in declaration order.
    pub fn tasks(&self) -> impl Iterator<Item = &RoadmapTask> {
        self.milestones
            .iter()
            .flat_map(|milestone| milestone.tasks.iter())
    }

    /// Find one cycle in the task-level `depends_on` graph, if any.
    ///
    /// Deterministic: tasks are visited in declaration order, and each task's
    /// dependencies in their declared order, so the same roadmap always
    /// reports the same cycle. Unknown dependency ids are ignored here — they
    /// are reported separately as [`RoadmapLedgerIssue::UnknownDependsOn`].
    ///
    /// Uses an explicit-stack iterative DFS (not recursion): a linear chain of
    /// tens of thousands of tasks would blow a recursive call stack, and a
    /// depth cap would silently miss deep cycles.
    fn find_task_cycle(&self) -> Option<Vec<String>> {
        let dependencies: HashMap<&str, &[String]> = self
            .tasks()
            .map(|task| (task.id.as_str(), task.depends_on.as_slice()))
            .collect();

        let mut marks: HashMap<&str, CycleMark> = HashMap::new();

        for task in self.tasks() {
            if let Some(cycle) = find_cycle_from(task.id.as_str(), &dependencies, &mut marks) {
                return Some(cycle);
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CycleMark {
    Visiting,
    Done,
}

/// One entry in the explicit DFS stack: a node and the index of the next
/// child (dependency) to visit from it.
struct CycleFrame<'a> {
    node: &'a str,
    next_child: usize,
}

/// Iterative depth-first search from `start` looking for a back edge into the
/// active path. Returns the cycle path (first == last) on the first one found.
///
/// Mirrors a recursive coloring DFS: a node on the active path is `Visiting`,
/// a fully-explored node is `Done`. Encountering a `Visiting` node means the
/// active path plus that node closes a cycle.
fn find_cycle_from<'a>(
    start: &'a str,
    dependencies: &HashMap<&'a str, &'a [String]>,
    marks: &mut HashMap<&'a str, CycleMark>,
) -> Option<Vec<String>> {
    if marks.get(start) == Some(&CycleMark::Done) {
        return None;
    }

    let mut path: Vec<CycleFrame<'a>> = vec![CycleFrame {
        node: start,
        next_child: 0,
    }];
    marks.insert(start, CycleMark::Visiting);

    while let Some(frame) = path.last_mut() {
        let node = frame.node;
        let children = dependencies.get(node).copied().unwrap_or_default();

        if frame.next_child < children.len() {
            let target = children[frame.next_child].as_str();
            frame.next_child += 1;

            // Only follow edges to known tasks; unknown ids are a separate
            // diagnostic and must not appear in the reported cycle.
            if !dependencies.contains_key(target) {
                continue;
            }
            match marks.get(target) {
                Some(CycleMark::Done) => continue,
                Some(CycleMark::Visiting) => {
                    // Back edge: close the cycle at `target`.
                    let start_idx = path.iter().position(|f| f.node == target)?;
                    let mut cycle: Vec<String> = path[start_idx..]
                        .iter()
                        .map(|f| f.node.to_owned())
                        .collect();
                    cycle.push(target.to_owned());
                    return Some(cycle);
                },
                None => {
                    marks.insert(target, CycleMark::Visiting);
                    path.push(CycleFrame {
                        node: target,
                        next_child: 0,
                    });
                },
            }
        } else {
            marks.insert(node, CycleMark::Done);
            path.pop();
        }
    }
    None
}

/// One structural problem found by [`RoadmapArtifact::validate_ledger`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RoadmapLedgerIssue {
    /// Two milestones share the same id.
    DuplicateMilestoneId {
        /// The duplicated milestone id.
        milestone: String,
    },
    /// Two tasks share the same id (across all milestones).
    DuplicateTaskId {
        /// The duplicated task id.
        task: String,
    },
    /// A task depends on itself.
    SelfDependency {
        /// The task id.
        task: String,
    },
    /// A task depends on a task id that does not exist.
    UnknownDependsOn {
        /// The task declaring the dependency.
        task: String,
        /// The missing task id.
        missing: String,
    },
    /// A task claims to be discovered from itself.
    SelfDiscovery {
        /// The task id.
        task: String,
    },
    /// A task's `discovered_from` references a task id that does not exist.
    UnknownDiscoveredFrom {
        /// The task declaring the origin.
        task: String,
        /// The missing task id.
        missing: String,
    },
    /// A milestone-level dependency references a missing milestone id.
    UnknownMilestoneDependency {
        /// The missing milestone id.
        missing: String,
    },
    /// A milestone dependency references itself.
    MilestoneSelfDependency {
        /// The milestone id.
        milestone: String,
    },
    /// The task-level `depends_on` graph contains a cycle.
    DependencyCycle {
        /// The cycle as a task-id path; first and last entries are equal.
        cycle: Vec<String>,
    },
    /// A schema-v2 task is missing its required `size`.
    MissingSize {
        /// The task id.
        task: String,
    },
}

impl std::fmt::Display for RoadmapLedgerIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateMilestoneId { milestone } => {
                write!(formatter, "duplicate milestone id {milestone:?}")
            },
            Self::DuplicateTaskId { task } => write!(formatter, "duplicate task id {task:?}"),
            Self::MilestoneSelfDependency { milestone } => {
                write!(formatter, "milestone {milestone:?} depends on itself")
            },
            Self::SelfDependency { task } => {
                write!(formatter, "task {task:?} depends on itself")
            },
            Self::UnknownDependsOn { task, missing } => write!(
                formatter,
                "task {task:?} depends on unknown task {missing:?}"
            ),
            Self::SelfDiscovery { task } => {
                write!(formatter, "task {task:?} is discovered from itself")
            },
            Self::UnknownDiscoveredFrom { task, missing } => write!(
                formatter,
                "task {task:?} is discovered from unknown task {missing:?}"
            ),
            Self::UnknownMilestoneDependency { missing } => write!(
                formatter,
                "milestone dependency references unknown milestone {missing:?}"
            ),
            Self::DependencyCycle { cycle } => {
                write!(formatter, "task dependency cycle: {}", cycle.join(" -> "))
            },
            Self::MissingSize { task } => write!(
                formatter,
                "task {task:?} is missing its required size (schema v2)"
            ),
        }
    }
}

impl Default for RoadmapArtifact {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// One deliverable-focused roadmap milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// Task ids (in any milestone) that must complete before this task.
    ///
    /// Schema v2. Task-granularity edges for the ledger; milestone-level
    /// ordering stays in [`RoadmapArtifact::dependencies`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Task id this task was discovered from while executing that task.
    ///
    /// Schema v2. Captures mid-task discovered work instead of dropping it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_from: Option<String>,
    /// Context-budget size class.
    ///
    /// Schema v2, required by the validator at v2: every task must be
    /// completable in one fresh agent session; oversized work is split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<TaskSize>,
    /// True only when a verification-authority node reported the task
    /// verified. Distinct from [`RoadmapStatus::Completed`], which any
    /// stage can claim.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verified: bool,
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
            depends_on: Vec::new(),
            discovered_from: None,
            size: None,
            verified: false,
        }
    }
}

/// Context-budget size class for one roadmap task.
///
/// The contract is qualitative, not a token count: an `S`/`M` task fits one
/// fresh agent session comfortably; `L` is the upper bound and a planner
/// signal to consider splitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskSize {
    /// Small — a focused change, well under one session.
    S,
    /// Medium — a typical task, fits one session with room for verification.
    M,
    /// Large — fills one session; anything bigger must be split.
    L,
}

impl std::fmt::Display for TaskSize {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
        })
    }
}

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
        id: String,
    },
    /// A task entry has an empty title.
    EmptyTitle {
        /// The id of the task with the empty title.
        id: String,
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
        let mut seen: HashSet<&str> = HashSet::new();
        for entry in &self.tasks {
            if entry.id.trim().is_empty() {
                issues.push(DiscoveredTaskIssue::EmptyId);
            } else if !seen.insert(entry.id.as_str()) {
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
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Optional short description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The `verification-report.toml` artifact — a sealed verifier's record of
/// the checks it ran against a task and the outcome it reached. Produced only
/// by verifier nodes (read-only sandbox + verification authority); the hash of
/// this artifact is the `evidence` carried on the matching `TaskVerified`
/// ledger event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    title = "VerificationReportArtifact",
    description = "Surge `verification-report.toml` artifact: a sealed verifier's record of the checks run against a task."
)]
pub struct VerificationReportArtifact {
    /// Artifact contract schema version.
    #[serde(default = "default_verification_report_schema_version")]
    pub schema_version: u32,
    /// The task this report covers.
    pub task_id: String,
    /// Verifier outcome.
    pub outcome: VerificationReportOutcome,
    /// Short human-readable summary of the verification.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Checks the verifier ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<VerificationCheck>,
    /// References to evidence (content hashes or artifact paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

impl VerificationReportArtifact {
    /// Validate the artifact: `task_id` and `outcome` are required and every
    /// check has a non-empty command. Returns an empty vector when
    /// well-formed.
    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.task_id.trim().is_empty() {
            issues.push("verification-report has an empty task_id".to_owned());
        }
        if self.summary.trim().is_empty() && self.checks.is_empty() {
            issues.push(format!(
                "verification-report for {:?} has no summary and no checks",
                self.task_id
            ));
        }
        for (idx, check) in self.checks.iter().enumerate() {
            if check.command.trim().is_empty() {
                issues.push(format!(
                    "verification-report check #{idx} has an empty command"
                ));
            }
            if check.result.trim().is_empty() {
                issues.push(format!(
                    "verification-report check #{idx} ({:?}) has an empty result",
                    check.command
                ));
            }
        }
        issues
    }
}

/// Outcome a sealed verifier reached for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerificationReportOutcome {
    /// The task passed verification; the engine maps this to a `TaskVerified`
    /// ledger transition.
    #[default]
    Passed,
    /// The task failed verification; the engine maps this to a
    /// `FailedVerification` status.
    Failed,
}

/// One check recorded in a [`VerificationReportArtifact`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationCheck {
    /// What was checked (e.g. `cargo nextest run`).
    pub command: String,
    /// `passed` | `failed` | `skipped`.
    pub result: String,
    /// Optional note / observed output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Directed dependency between roadmap milestones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RoadmapStatus {
    /// Not yet started.
    #[default]
    Pending,
    /// Currently being executed by the orchestrator.
    Running,
    /// Paused, waiting for human input or gate.
    Paused,
    /// Implementation reported done; awaiting a verification-authority node.
    #[serde(rename = "ready_for_verification")]
    ReadyForVerification,
    /// Verification rejected the implementation; routes back to execution.
    #[serde(rename = "failed_verification")]
    FailedVerification,
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
            Self::ReadyForVerification => "ready_for_verification",
            Self::FailedVerification => "failed_verification",
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

    #[test]
    fn verification_report_defaults_to_v1_not_roadmap_v2() {
        // A verifier writes verification-report.toml with no schema_version.
        // It must default to ARTIFACT_SCHEMA_VERSION (1) — the version its
        // contract validator requires — not the roadmap's bumped v2.
        let toml = "task_id = \"m1-t1\"\noutcome = \"passed\"\nsummary = \"all green\"\n";
        let report: VerificationReportArtifact = toml::from_str(toml).unwrap();
        assert_eq!(report.schema_version, ARTIFACT_SCHEMA_VERSION);
        assert_ne!(report.schema_version, ROADMAP_SCHEMA_VERSION);
    }

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

        assert_eq!(deserialized.schema_version, ROADMAP_SCHEMA_VERSION);
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

    fn ledger_roadmap(tasks: Vec<RoadmapTask>) -> RoadmapArtifact {
        let mut milestone = RoadmapMilestone::new("m1", "Ledger");
        milestone.tasks = tasks;
        RoadmapArtifact::new(vec![milestone])
    }

    fn sized_task(id: &str, depends_on: &[&str]) -> RoadmapTask {
        let mut task = RoadmapTask::new(id, id);
        task.size = Some(TaskSize::M);
        task.depends_on = depends_on.iter().map(ToString::to_string).collect();
        task
    }

    #[test]
    fn validate_ledger_accepts_well_formed_v2() {
        let roadmap = ledger_roadmap(vec![
            sized_task("t1", &[]),
            sized_task("t2", &["t1"]),
            sized_task("t3", &["t1", "t2"]),
        ]);
        assert_eq!(roadmap.validate_ledger(), Vec::new());
    }

    #[test]
    fn validate_ledger_flags_duplicate_ids() {
        let mut roadmap = ledger_roadmap(vec![sized_task("t1", &[]), sized_task("t1", &[])]);
        roadmap
            .milestones
            .push(RoadmapMilestone::new("m1", "Duplicate"));

        let issues = roadmap.validate_ledger();

        assert!(issues.contains(&RoadmapLedgerIssue::DuplicateTaskId {
            task: "t1".to_string()
        }));
        assert!(issues.contains(&RoadmapLedgerIssue::DuplicateMilestoneId {
            milestone: "m1".to_string()
        }));
    }

    #[test]
    fn validate_ledger_flags_unknown_and_self_references() {
        let mut discovered = sized_task("t2", &["missing"]);
        discovered.discovered_from = Some("t2".to_string());
        let mut self_dep = sized_task("t1", &["t1"]);
        self_dep.discovered_from = Some("ghost".to_string());
        let roadmap = ledger_roadmap(vec![self_dep, discovered]);

        let issues = roadmap.validate_ledger();

        assert!(issues.contains(&RoadmapLedgerIssue::SelfDependency {
            task: "t1".to_string()
        }));
        assert!(issues.contains(&RoadmapLedgerIssue::UnknownDependsOn {
            task: "t2".to_string(),
            missing: "missing".to_string()
        }));
        assert!(issues.contains(&RoadmapLedgerIssue::SelfDiscovery {
            task: "t2".to_string()
        }));
        assert!(issues.contains(&RoadmapLedgerIssue::UnknownDiscoveredFrom {
            task: "t1".to_string(),
            missing: "ghost".to_string()
        }));
    }

    #[test]
    fn validate_ledger_reports_cycle_deterministically() {
        let roadmap = ledger_roadmap(vec![
            sized_task("t1", &["t3"]),
            sized_task("t2", &["t1"]),
            sized_task("t3", &["t2"]),
        ]);

        let issues = roadmap.validate_ledger();

        assert_eq!(
            issues,
            vec![RoadmapLedgerIssue::DependencyCycle {
                cycle: vec![
                    "t1".to_string(),
                    "t3".to_string(),
                    "t2".to_string(),
                    "t1".to_string()
                ]
            }]
        );
    }

    #[test]
    fn validate_ledger_detects_deep_cycle_without_stack_overflow() {
        // A long linear chain t0 -> t1 -> ... -> t_{n-1} -> t0 forms one big
        // cycle far deeper than any recursion cap would allow. The iterative
        // DFS must both avoid a stack overflow and still report the cycle.
        let n = 20_000;
        let tasks: Vec<RoadmapTask> = (0..n)
            .map(|i| {
                let next = (i + 1) % n;
                let mut task = RoadmapTask::new(format!("t{i}"), format!("Task {i}"));
                task.size = Some(TaskSize::M);
                task.depends_on = vec![format!("t{next}")];
                task
            })
            .collect();
        let roadmap = ledger_roadmap(tasks);

        let issues = roadmap.validate_ledger();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, RoadmapLedgerIssue::DependencyCycle { .. })),
            "deep cycle must be reported, got {issues:?}"
        );
    }

    #[test]
    fn validate_ledger_requires_size_only_at_v2() {
        let mut roadmap = ledger_roadmap(vec![RoadmapTask::new("t1", "No size")]);
        assert_eq!(
            roadmap.validate_ledger(),
            vec![RoadmapLedgerIssue::MissingSize {
                task: "t1".to_string()
            }]
        );

        roadmap.schema_version = 1;
        assert_eq!(roadmap.validate_ledger(), Vec::new());
    }

    #[test]
    fn validate_ledger_flags_unknown_milestone_dependency() {
        let mut roadmap = ledger_roadmap(vec![sized_task("t1", &[])]);
        roadmap.dependencies.push(RoadmapDependency {
            from: "m1".to_string(),
            to: "m9".to_string(),
            reason: String::new(),
        });

        assert_eq!(
            roadmap.validate_ledger(),
            vec![RoadmapLedgerIssue::UnknownMilestoneDependency {
                missing: "m9".to_string()
            }]
        );
    }

    #[test]
    fn validate_ledger_flags_self_referencing_milestone_dependency() {
        let mut roadmap = ledger_roadmap(vec![sized_task("t1", &[])]);
        roadmap.dependencies.push(RoadmapDependency {
            from: "m1".to_string(),
            to: "m1".to_string(),
            reason: String::new(),
        });

        let issues = roadmap.validate_ledger();
        assert!(
            issues.contains(&RoadmapLedgerIssue::MilestoneSelfDependency {
                milestone: "m1".to_string()
            })
        );
    }

    #[test]
    fn task_ledger_fields_roundtrip_via_toml() {
        let mut task = sized_task("t2", &["t1"]);
        task.discovered_from = Some("t1".to_string());
        task.verified = true;
        task.status = RoadmapStatus::ReadyForVerification;
        let roadmap = ledger_roadmap(vec![sized_task("t1", &[]), task]);

        let toml_str = toml::to_string(&roadmap).unwrap();
        let parsed: RoadmapArtifact = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed, roadmap);
        assert!(toml_str.contains("ready_for_verification"));
        assert!(toml_str.contains("size = \"m\""));
    }

    #[test]
    fn v1_roadmap_toml_still_parses_with_defaulted_ledger_fields() {
        let parsed: RoadmapArtifact = toml::from_str(
            r#"
schema_version = 1

[[milestones]]
id = "m1"
title = "Legacy"

[[milestones.tasks]]
id = "t1"
title = "Old task"
"#,
        )
        .unwrap();

        let task = &parsed.milestones[0].tasks[0];
        assert_eq!(parsed.schema_version, 1);
        assert!(task.depends_on.is_empty());
        assert!(task.discovered_from.is_none());
        assert!(task.size.is_none());
        assert!(!task.verified);
        assert_eq!(parsed.validate_ledger(), Vec::new());
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
