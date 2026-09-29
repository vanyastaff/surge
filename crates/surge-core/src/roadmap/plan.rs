//! The plan model: missions, milestones, tasks, stages, priorities and status.

use super::*;

/// A bounded, multi-milestone effort with its own definition of done.
///
/// The validation contract is written before the work is split into tasks,
/// so it describes the behaviour the user asked for rather than the
/// implementation already planned. Each assertion is claimed by exactly one
/// task through [`RoadmapTask::fulfills`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapMission {
    /// Stable identifier, for example `mission-1`.
    pub id: String,
    /// Human-readable mission title.
    pub title: String,
    /// The outcome this mission delivers, in one or two sentences.
    pub goal: String,
    /// Current execution status.
    #[serde(default, skip_serializing_if = "RoadmapStatus::is_pending")]
    pub status: RoadmapStatus,
    /// Ids of the milestones this mission owns, in execution order.
    pub milestones: Vec<String>,
    /// Behavioural assertions that define the mission as done.
    #[serde(default)]
    pub validation_contract: Vec<ValidationAssertion>,
}

impl RoadmapMission {
    /// Create a pending mission with no milestones or assertions.
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            goal: goal.into(),
            status: RoadmapStatus::Pending,
            milestones: Vec::new(),
            validation_contract: Vec::new(),
        }
    }
}

/// One testable behavioural assertion in a mission's validation contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ValidationAssertion {
    /// Stable id with an area prefix, for example `VAL-AUTH-001`.
    pub id: String,
    /// Short title.
    pub title: String,
    /// Observable pass/fail condition, phrased as behaviour a user or test
    /// can check.
    pub pass_condition: String,
    /// Evidence a verifier must capture (test output, screenshot, HTTP
    /// exchange, log excerpt).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
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
    /// Optional scheduling priority; `p0` is the most urgent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<TaskPriority>,
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
            priority: None,
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
    /// Validation-contract assertion ids this task makes fully testable.
    ///
    /// Only the leaf task that completes an assertion claims it;
    /// infrastructure tasks leave this empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fulfills: Vec<String>,
    /// Optional scheduling priority; `p0` is the most urgent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<TaskPriority>,
    /// Explicit parallelism marker.
    ///
    /// Tasks sharing a group name may run concurrently, so no member may
    /// depend (directly or transitively) on another member of the same group.
    /// Tasks with no group are scheduled purely by `depends_on`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_group: Option<String>,
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
            fulfills: Vec::new(),
            priority: None,
            parallel_group: None,
        }
    }
}

/// Scheduling priority of a roadmap milestone or task.
///
/// Distinct from the legacy [`Priority`] used by [`RoadmapItem`]. Ordering
/// follows urgency: `P0 < P1 < P2 < P3`, so sorting ascending puts the most
/// urgent work first.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum TaskPriority {
    /// Must ship; blocks the release.
    P0,
    /// Important; should ship in this release.
    P1,
    /// Normal.
    P2,
    /// Nice to have; first to be cut.
    P3,
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
        })
    }
}

/// One release stage grouping milestones.
///
/// A stage marks a real release boundary: something usable or shippable
/// exists once it is done. The title is free-form ("MVP", "Public beta",
/// "Launch") and stage order is declaration order; nothing requires a fixed
/// ladder, and a roadmap may have a single stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapStage {
    /// Stable identifier, for example `stage-1`.
    pub id: String,
    /// Human-readable stage title.
    pub title: String,
    /// The outcome this release delivers, in one or two sentences.
    #[serde(default)]
    pub goal: String,
    /// Ids of the milestones this stage owns, in execution order.
    pub milestones: Vec<String>,
    /// Observable conditions that must hold before the stage is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exit_criteria: Vec<String>,
}

impl RoadmapStage {
    /// Create a stage with no milestones or exit criteria.
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            goal: goal.into(),
            milestones: Vec::new(),
            exit_criteria: Vec::new(),
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
    /// Every variant, exhaustive by construction: `all_variants_are_named`
    /// below matches every arm with no wildcard, so a ninth `RoadmapStatus`
    /// variant fails to compile there until this array is updated too — a
    /// reader fixing that match failure sees this array right next to it.
    /// `surge_core::evidence`'s tests build their "every non-`Completed`
    /// status" fixtures from this instead of keeping their own hand-written
    /// list, so a new variant reaches those tests without a second edit.
    pub const ALL: [Self; 8] = [
        Self::Pending,
        Self::Running,
        Self::Paused,
        Self::ReadyForVerification,
        Self::FailedVerification,
        Self::Completed,
        Self::Failed,
        Self::Skipped,
    ];

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

pub(super) const fn markdown_checkbox(status: RoadmapStatus) -> &'static str {
    match status {
        RoadmapStatus::Completed => "x",
        RoadmapStatus::Running => "~",
        _ => " ",
    }
}
