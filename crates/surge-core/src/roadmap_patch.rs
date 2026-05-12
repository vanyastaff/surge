//! Typed roadmap amendment patches.
//!
//! This module is pure domain code. It models the `roadmap-patch.toml` artifact
//! that Feature Planner agents produce, plus stable IDs, lifecycle status, and
//! shape diagnostics that callers can log without parsing prose.

use crate::artifact_contract::ARTIFACT_SCHEMA_VERSION;
use crate::content_hash::ContentHash;
use crate::id::RunId;
use crate::roadmap::{
    RoadmapArtifact, RoadmapDependency, RoadmapMilestone, RoadmapStatus, RoadmapTask,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

/// Current schema version for `roadmap-patch.toml`.
pub const ROADMAP_PATCH_SCHEMA_VERSION: u32 = ARTIFACT_SCHEMA_VERSION;

/// Stable identifier for a roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoadmapPatchId(String);

impl RoadmapPatchId {
    /// Create a validated patch ID.
    ///
    /// # Errors
    /// Returns [`RoadmapPatchIdError`] when the ID is empty or contains
    /// unsupported characters.
    pub fn new(value: impl Into<String>) -> Result<Self, RoadmapPatchIdError> {
        let value = value.into();
        let value = value.trim().to_owned();
        validate_patch_id(&value)?;
        Ok(Self(value))
    }

    /// Build a deterministic patch ID from a content hash.
    #[must_use]
    pub fn from_content_hash(hash: ContentHash) -> Self {
        let prefix = hash.to_hex().chars().take(16).collect::<String>();
        Self(format!("rpatch-{prefix}"))
    }

    /// Borrow the stable string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoadmapPatchId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for RoadmapPatchId {
    type Err = RoadmapPatchIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for RoadmapPatchId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RoadmapPatchId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for RoadmapPatchId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("RoadmapPatchId")
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("surge::roadmap_patch::RoadmapPatchId")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Stable roadmap patch identifier. ASCII alphanumeric plus `-`, `_`, `.`. Non-empty after trimming.",
            "minLength": 1,
            "pattern": r"^[A-Za-z0-9._-]+$"
        })
    }
}

/// Error returned when a roadmap patch ID is malformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoadmapPatchIdError {
    /// Patch ID is empty after trimming.
    #[error("roadmap patch id must not be empty")]
    Empty,
    /// Patch ID contains a character outside the stable portable set.
    #[error("roadmap patch id contains unsupported character {character:?}")]
    UnsupportedCharacter {
        /// Unsupported character.
        character: char,
    },
}

/// Machine-readable roadmap amendment artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    title = "RoadmapPatch",
    description = "Surge `roadmap-patch.toml` artifact: an ordered sequence of operations amending an existing roadmap, plus dependencies, conflicts, and lifecycle status."
)]
pub struct RoadmapPatch {
    /// Artifact schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Stable patch identifier.
    pub id: RoadmapPatchId,
    /// Roadmap or run this patch amends.
    pub target: RoadmapPatchTarget,
    /// Human-readable reason for the amendment.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
    /// Ordered patch operations.
    #[serde(default)]
    pub operations: Vec<RoadmapPatchOperation>,
    /// Cross-item dependencies introduced by this patch.
    #[serde(default)]
    pub dependencies: Vec<RoadmapPatchDependency>,
    /// Conflicts detected while drafting or applying the patch.
    #[serde(default)]
    pub conflicts: Vec<RoadmapPatchConflict>,
    /// Current lifecycle state.
    #[serde(default)]
    pub status: RoadmapPatchStatus,
}

impl RoadmapPatch {
    /// Create a patch in the drafted state.
    #[must_use]
    pub fn new(
        id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        operations: Vec<RoadmapPatchOperation>,
    ) -> Self {
        Self {
            schema_version: ROADMAP_PATCH_SCHEMA_VERSION,
            id,
            target,
            rationale: String::new(),
            operations,
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            status: RoadmapPatchStatus::Drafted,
        }
    }

    /// Compute the stable hash used for idempotency.
    ///
    /// The hash excludes `id` and `status`; lifecycle transitions should not
    /// change whether two patches describe the same amendment.
    ///
    /// # Errors
    /// Returns [`RoadmapPatchHashError`] if canonical TOML serialization fails.
    pub fn content_hash(&self) -> Result<ContentHash, RoadmapPatchHashError> {
        let input = RoadmapPatchHashInput {
            schema_version: self.schema_version,
            target: &self.target,
            rationale: &self.rationale,
            operations: &self.operations,
            dependencies: &self.dependencies,
            conflicts: &self.conflicts,
        };
        let canonical = toml::to_string(&input)?;
        Ok(ContentHash::compute(canonical.as_bytes()))
    }

    /// Return shape-level diagnostics that do not require reading a roadmap.
    #[must_use]
    pub fn validate_shape(&self) -> Vec<RoadmapPatchValidationIssue> {
        let mut issues = Vec::new();
        push_schema_issue(self.schema_version, &mut issues);
        push_empty_operations_issue(self.operations.is_empty(), &mut issues);

        for (index, operation) in self.operations.iter().enumerate() {
            operation.validate_shape(index, &mut issues);
        }
        for (index, dependency) in self.dependencies.iter().enumerate() {
            dependency.validate_shape(index, &mut issues);
        }
        for (index, conflict) in self.conflicts.iter().enumerate() {
            conflict.validate_shape(index, &mut issues);
        }

        issues
    }

    /// Apply this patch to a roadmap artifact.
    ///
    /// # Errors
    /// Returns [`RoadmapPatchApplyError`] when the patch shape is invalid or
    /// when one or more operations would mutate completed/running history,
    /// reference missing items, duplicate IDs, or introduce dependency cycles.
    pub fn apply_to_roadmap(
        &self,
        roadmap: &RoadmapArtifact,
    ) -> Result<RoadmapPatchApplyResult, RoadmapPatchApplyError> {
        apply_roadmap_patch(roadmap, self)
    }
}

/// Error returned when patch hash computation fails.
#[derive(Debug, thiserror::Error)]
pub enum RoadmapPatchHashError {
    /// Canonical TOML serialization failed.
    #[error("failed to serialize roadmap patch for hashing: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Result of applying a roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapPatchApplyResult {
    /// Amended roadmap artifact.
    pub roadmap: RoadmapArtifact,
    /// Deterministic markdown rendering for `roadmap.md`.
    pub markdown: String,
    /// Milestone ids inserted by the patch.
    pub inserted_milestones: Vec<String>,
    /// Task refs inserted by the patch.
    pub inserted_tasks: Vec<RoadmapItemRef>,
    /// Existing draft refs replaced by the patch.
    pub replaced_items: Vec<RoadmapItemRef>,
    /// Dependency edges accepted by the patch.
    pub dependencies_added: Vec<RoadmapPatchDependency>,
}

/// One typed conflict found while applying a roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapPatchApplyConflict {
    /// Stable conflict code.
    pub code: RoadmapPatchConflictCode,
    /// Item involved in the conflict, when known.
    pub item: Option<RoadmapItemRef>,
    /// Human-readable conflict summary.
    pub message: String,
}

impl RoadmapPatchApplyConflict {
    fn new(
        code: RoadmapPatchConflictCode,
        item: Option<RoadmapItemRef>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            item,
            message: message.into(),
        }
    }

    /// Convert an apply-time conflict into operator-facing patch metadata.
    #[must_use]
    pub fn to_patch_conflict(&self) -> RoadmapPatchConflict {
        RoadmapPatchConflict {
            code: self.code,
            item: self.item.clone(),
            message: self.message.clone(),
            choices: conflict_choices_for_code(self.code),
            selected_choice: None,
        }
    }
}

/// Error returned by pure roadmap patch application.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoadmapPatchApplyError {
    /// Patch shape failed validation before apply.
    #[error("roadmap patch shape is invalid")]
    InvalidShape {
        /// Shape validation issues.
        issues: Vec<RoadmapPatchValidationIssue>,
    },
    /// Patch conflicts with the supplied roadmap.
    #[error("roadmap patch has apply conflicts")]
    Conflicts {
        /// Typed apply conflicts.
        conflicts: Vec<RoadmapPatchApplyConflict>,
    },
}

/// Apply a roadmap patch to a typed roadmap artifact.
///
/// # Errors
/// Returns [`RoadmapPatchApplyError`] if the patch is malformed for apply or
/// conflicts with the supplied roadmap state.
pub fn apply_roadmap_patch(
    roadmap: &RoadmapArtifact,
    patch: &RoadmapPatch,
) -> Result<RoadmapPatchApplyResult, RoadmapPatchApplyError> {
    let issues = patch.validate_shape();
    if !issues.is_empty() {
        return Err(RoadmapPatchApplyError::InvalidShape { issues });
    }

    let mut context = RoadmapPatchApplyContext::new(roadmap.clone());
    for operation in &patch.operations {
        context.apply_operation(operation);
    }
    if context.conflicts.is_empty() {
        context.apply_dependencies(&patch.dependencies);
    }
    context.finish()
}

struct RoadmapPatchApplyContext {
    roadmap: RoadmapArtifact,
    inserted_milestones: Vec<String>,
    inserted_tasks: Vec<RoadmapItemRef>,
    replaced_items: Vec<RoadmapItemRef>,
    dependencies_added: Vec<RoadmapPatchDependency>,
    conflicts: Vec<RoadmapPatchApplyConflict>,
}

impl RoadmapPatchApplyContext {
    fn new(roadmap: RoadmapArtifact) -> Self {
        Self {
            roadmap,
            inserted_milestones: Vec::new(),
            inserted_tasks: Vec::new(),
            replaced_items: Vec::new(),
            dependencies_added: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    fn apply_operation(&mut self, operation: &RoadmapPatchOperation) {
        match operation {
            RoadmapPatchOperation::AddMilestone {
                milestone,
                insertion,
            } => self.add_milestone(milestone, insertion.as_ref()),
            RoadmapPatchOperation::AddTask {
                milestone_id,
                task,
                insertion,
            } => self.add_task(milestone_id, task, insertion.as_ref()),
            RoadmapPatchOperation::ReplaceDraftItem {
                target,
                replacement,
                ..
            } => self.replace_draft_item(target, replacement),
        }
    }

    fn add_milestone(&mut self, milestone: &RoadmapMilestone, insertion: Option<&InsertionPoint>) {
        if self.has_milestone(&milestone.id) {
            self.push_duplicate(RoadmapItemRef::Milestone {
                milestone_id: milestone.id.clone(),
            });
            return;
        }

        let Some(insertion) = insertion else {
            self.push_unsupported("missing milestone insertion point");
            return;
        };
        let milestone = pending_milestone(milestone);
        match insertion {
            InsertionPoint::AppendToRoadmap => {
                self.inserted_milestones.push(milestone.id.clone());
                self.roadmap.milestones.push(milestone);
            },
            InsertionPoint::BeforeMilestone { milestone_id } => {
                self.insert_milestone_near(milestone_id, milestone, InsertSide::Before);
            },
            InsertionPoint::AfterMilestone { milestone_id } => {
                self.insert_milestone_near(milestone_id, milestone, InsertSide::After);
            },
            _ => self.push_unsupported("milestone insertions must target roadmap milestones"),
        }
    }

    fn insert_milestone_near(
        &mut self,
        target_id: &str,
        milestone: RoadmapMilestone,
        side: InsertSide,
    ) {
        let target_ref = RoadmapItemRef::Milestone {
            milestone_id: target_id.to_owned(),
        };
        let Some(index) = self.find_milestone_index(target_id) else {
            self.push_missing(target_ref);
            return;
        };
        if self.push_status_conflict(target_ref, self.roadmap.milestones[index].status) {
            return;
        }
        let insert_at = match side {
            InsertSide::Before => index,
            InsertSide::After => index + 1,
        };
        self.inserted_milestones.push(milestone.id.clone());
        self.roadmap.milestones.insert(insert_at, milestone);
    }

    fn add_task(
        &mut self,
        milestone_id: &str,
        task: &RoadmapTask,
        insertion: Option<&InsertionPoint>,
    ) {
        let Some(milestone_index) = self.find_milestone_index(milestone_id) else {
            self.push_missing(RoadmapItemRef::Milestone {
                milestone_id: milestone_id.to_owned(),
            });
            return;
        };
        let milestone_ref = RoadmapItemRef::Milestone {
            milestone_id: milestone_id.to_owned(),
        };
        if self.push_status_conflict(
            milestone_ref,
            self.roadmap.milestones[milestone_index].status,
        ) {
            return;
        }
        if self.has_task_in_milestone(milestone_index, &task.id) {
            self.push_duplicate(RoadmapItemRef::Task {
                milestone_id: milestone_id.to_owned(),
                task_id: task.id.clone(),
            });
            return;
        }
        let Some(insert_at) = self.task_insert_index(milestone_index, milestone_id, insertion)
        else {
            return;
        };
        let task = pending_task(task);
        self.inserted_tasks.push(RoadmapItemRef::Task {
            milestone_id: milestone_id.to_owned(),
            task_id: task.id.clone(),
        });
        self.roadmap.milestones[milestone_index]
            .tasks
            .insert(insert_at, task);
    }

    fn task_insert_index(
        &mut self,
        milestone_index: usize,
        milestone_id: &str,
        insertion: Option<&InsertionPoint>,
    ) -> Option<usize> {
        match insertion {
            Some(InsertionPoint::AppendToMilestone {
                milestone_id: insertion_milestone,
            }) if insertion_milestone == milestone_id => {
                Some(self.roadmap.milestones[milestone_index].tasks.len())
            },
            Some(InsertionPoint::BeforeTask {
                milestone_id: insertion_milestone,
                task_id,
            }) if insertion_milestone == milestone_id => self.task_insert_index_near(
                milestone_index,
                milestone_id,
                task_id,
                InsertSide::Before,
            ),
            Some(InsertionPoint::AfterTask {
                milestone_id: insertion_milestone,
                task_id,
            }) if insertion_milestone == milestone_id => self.task_insert_index_near(
                milestone_index,
                milestone_id,
                task_id,
                InsertSide::After,
            ),
            _ => {
                self.push_unsupported("task insertions must target tasks in the target milestone");
                None
            },
        }
    }

    fn task_insert_index_near(
        &mut self,
        milestone_index: usize,
        milestone_id: &str,
        task_id: &str,
        side: InsertSide,
    ) -> Option<usize> {
        let target_ref = RoadmapItemRef::Task {
            milestone_id: milestone_id.to_owned(),
            task_id: task_id.to_owned(),
        };
        let Some(task_index) = find_task_index(&self.roadmap.milestones[milestone_index], task_id)
        else {
            self.push_missing(target_ref);
            return None;
        };
        let status = self.roadmap.milestones[milestone_index].tasks[task_index].status;
        if self.push_status_conflict(target_ref, status) {
            return None;
        }
        Some(match side {
            InsertSide::Before => task_index,
            InsertSide::After => task_index + 1,
        })
    }

    fn replace_draft_item(&mut self, target: &RoadmapItemRef, replacement: &RoadmapPatchItem) {
        match (target, replacement) {
            (
                RoadmapItemRef::Milestone { milestone_id },
                RoadmapPatchItem::Milestone { milestone },
            ) => self.replace_milestone(milestone_id, milestone),
            (
                RoadmapItemRef::Task {
                    milestone_id,
                    task_id,
                },
                RoadmapPatchItem::Task { task },
            ) => self.replace_task(milestone_id, task_id, task),
            _ => self.push_unsupported("replacement item kind must match target item kind"),
        }
    }

    fn replace_milestone(&mut self, milestone_id: &str, replacement: &RoadmapMilestone) {
        let target_ref = RoadmapItemRef::Milestone {
            milestone_id: milestone_id.to_owned(),
        };
        let Some(index) = self.find_milestone_index(milestone_id) else {
            self.push_missing(target_ref);
            return;
        };
        if self.push_status_conflict(target_ref.clone(), self.roadmap.milestones[index].status) {
            return;
        }
        if replacement.id != milestone_id && self.has_milestone(&replacement.id) {
            self.push_duplicate(RoadmapItemRef::Milestone {
                milestone_id: replacement.id.clone(),
            });
            return;
        }
        self.roadmap.milestones[index] = pending_milestone(replacement);
        self.replaced_items.push(target_ref);
    }

    fn replace_task(&mut self, milestone_id: &str, task_id: &str, replacement: &RoadmapTask) {
        let milestone_ref = RoadmapItemRef::Milestone {
            milestone_id: milestone_id.to_owned(),
        };
        let Some(milestone_index) = self.find_milestone_index(milestone_id) else {
            self.push_missing(milestone_ref);
            return;
        };
        if self.push_status_conflict(
            milestone_ref,
            self.roadmap.milestones[milestone_index].status,
        ) {
            return;
        }
        let target_ref = RoadmapItemRef::Task {
            milestone_id: milestone_id.to_owned(),
            task_id: task_id.to_owned(),
        };
        let Some(task_index) = find_task_index(&self.roadmap.milestones[milestone_index], task_id)
        else {
            self.push_missing(target_ref);
            return;
        };
        let task_status = self.roadmap.milestones[milestone_index].tasks[task_index].status;
        if self.push_status_conflict(target_ref.clone(), task_status) {
            return;
        }
        if replacement.id != task_id && self.has_task_in_milestone(milestone_index, &replacement.id)
        {
            self.push_duplicate(RoadmapItemRef::Task {
                milestone_id: milestone_id.to_owned(),
                task_id: replacement.id.clone(),
            });
            return;
        }
        self.roadmap.milestones[milestone_index].tasks[task_index] = pending_task(replacement);
        self.replaced_items.push(target_ref);
    }

    fn apply_dependencies(&mut self, dependencies: &[RoadmapPatchDependency]) {
        for dependency in dependencies {
            if !self.dependency_refs_exist(dependency) {
                continue;
            }
            if let (
                RoadmapItemRef::Milestone { milestone_id: from },
                RoadmapItemRef::Milestone { milestone_id: to },
            ) = (&dependency.from, &dependency.to)
            {
                self.add_milestone_dependency(from, to, &dependency.reason);
            }
            self.dependencies_added.push(dependency.clone());
        }
        if has_milestone_dependency_cycle(&self.roadmap) {
            self.conflicts.push(RoadmapPatchApplyConflict::new(
                RoadmapPatchConflictCode::DependencyCycle,
                None,
                "patch would introduce a milestone dependency cycle",
            ));
        }
    }

    fn dependency_refs_exist(&mut self, dependency: &RoadmapPatchDependency) -> bool {
        let mut ok = true;
        for reference in [&dependency.from, &dependency.to] {
            if !self.contains_ref(reference) {
                self.push_missing(reference.clone());
                ok = false;
            }
        }
        ok
    }

    fn add_milestone_dependency(&mut self, from: &str, to: &str, reason: &str) {
        let exists = self
            .roadmap
            .dependencies
            .iter()
            .any(|dependency| dependency.from == from && dependency.to == to);
        if exists {
            return;
        }
        self.roadmap.dependencies.push(RoadmapDependency {
            from: from.to_owned(),
            to: to.to_owned(),
            reason: reason.to_owned(),
        });
    }

    fn finish(self) -> Result<RoadmapPatchApplyResult, RoadmapPatchApplyError> {
        if !self.conflicts.is_empty() {
            return Err(RoadmapPatchApplyError::Conflicts {
                conflicts: self.conflicts,
            });
        }
        Ok(RoadmapPatchApplyResult {
            markdown: self.roadmap.to_markdown(),
            roadmap: self.roadmap,
            inserted_milestones: self.inserted_milestones,
            inserted_tasks: self.inserted_tasks,
            replaced_items: self.replaced_items,
            dependencies_added: self.dependencies_added,
        })
    }

    fn has_milestone(&self, milestone_id: &str) -> bool {
        self.roadmap
            .milestones
            .iter()
            .any(|milestone| milestone.id == milestone_id)
    }

    fn has_task_in_milestone(&self, milestone_index: usize, task_id: &str) -> bool {
        self.roadmap.milestones[milestone_index]
            .tasks
            .iter()
            .any(|task| task.id == task_id)
    }

    fn find_milestone_index(&self, milestone_id: &str) -> Option<usize> {
        self.roadmap
            .milestones
            .iter()
            .position(|milestone| milestone.id == milestone_id)
    }

    fn contains_ref(&self, reference: &RoadmapItemRef) -> bool {
        match reference {
            RoadmapItemRef::Milestone { milestone_id } => self.has_milestone(milestone_id),
            RoadmapItemRef::Task {
                milestone_id,
                task_id,
            } => self
                .find_milestone_index(milestone_id)
                .is_some_and(|index| self.has_task_in_milestone(index, task_id)),
        }
    }

    fn push_missing(&mut self, item: RoadmapItemRef) {
        self.conflicts.push(RoadmapPatchApplyConflict::new(
            RoadmapPatchConflictCode::MissingTarget,
            Some(item),
            "patch references a missing roadmap item",
        ));
    }

    fn push_duplicate(&mut self, item: RoadmapItemRef) {
        self.conflicts.push(RoadmapPatchApplyConflict::new(
            RoadmapPatchConflictCode::DuplicateItem,
            Some(item),
            "patch would duplicate an existing roadmap item id",
        ));
    }

    fn push_unsupported(&mut self, message: impl Into<String>) {
        self.conflicts.push(RoadmapPatchApplyConflict::new(
            RoadmapPatchConflictCode::UnsupportedOperation,
            None,
            message,
        ));
    }

    fn push_status_conflict(&mut self, item: RoadmapItemRef, status: RoadmapStatus) -> bool {
        let Some(code) = status_conflict_code(status) else {
            return false;
        };
        self.conflicts.push(RoadmapPatchApplyConflict::new(
            code,
            Some(item),
            format!("patch cannot amend item with status {status}"),
        ));
        true
    }
}

#[derive(Debug, Clone, Copy)]
enum InsertSide {
    Before,
    After,
}

fn pending_milestone(milestone: &RoadmapMilestone) -> RoadmapMilestone {
    let mut milestone = milestone.clone();
    milestone.status = RoadmapStatus::Pending;
    for task in &mut milestone.tasks {
        task.status = RoadmapStatus::Pending;
    }
    milestone
}

fn pending_task(task: &RoadmapTask) -> RoadmapTask {
    let mut task = task.clone();
    task.status = RoadmapStatus::Pending;
    task
}

fn find_task_index(milestone: &RoadmapMilestone, task_id: &str) -> Option<usize> {
    milestone.tasks.iter().position(|task| task.id == task_id)
}

const fn status_conflict_code(status: RoadmapStatus) -> Option<RoadmapPatchConflictCode> {
    match status {
        RoadmapStatus::Running | RoadmapStatus::Paused => {
            Some(RoadmapPatchConflictCode::RunningMilestone)
        },
        RoadmapStatus::Completed | RoadmapStatus::Failed | RoadmapStatus::Skipped => {
            Some(RoadmapPatchConflictCode::CompletedHistory)
        },
        RoadmapStatus::Pending => None,
    }
}

fn has_milestone_dependency_cycle(roadmap: &RoadmapArtifact) -> bool {
    let mut graph: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for milestone in &roadmap.milestones {
        graph.entry(milestone.id.as_str()).or_default();
    }
    for dependency in &roadmap.dependencies {
        graph
            .entry(dependency.from.as_str())
            .or_default()
            .push(dependency.to.as_str());
    }

    let mut visited = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    graph
        .keys()
        .any(|node| dependency_dfs_has_cycle(node, &graph, &mut visiting, &mut visited))
}

fn dependency_dfs_has_cycle<'a>(
    node: &'a str,
    graph: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> bool {
    if visited.contains(node) {
        return false;
    }
    if !visiting.insert(node) {
        return true;
    }
    for next in graph.get(node).into_iter().flatten() {
        if dependency_dfs_has_cycle(next, graph, visiting, visited) {
            return true;
        }
    }
    visiting.remove(node);
    visited.insert(node);
    false
}

/// Target amended by a roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoadmapPatchTarget {
    /// Project-level roadmap file.
    ProjectRoadmap {
        /// Portable path relative to the project root.
        roadmap_path: String,
    },
    /// Roadmap artifact belonging to a run.
    RunRoadmap {
        /// Target run.
        run_id: RunId,
        /// Stored roadmap artifact hash, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roadmap_artifact: Option<ContentHash>,
        /// Stored flow artifact hash, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow_artifact: Option<ContentHash>,
        /// Whether the active runner may pick up this amendment.
        #[serde(default)]
        active_pickup: ActivePickupPolicy,
    },
}

/// Active-run pickup policy for a patch target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivePickupPolicy {
    /// Runner may pick up new work at a safe loop boundary.
    #[default]
    Allowed,
    /// Always create a follow-up run rather than amending the active graph.
    FollowUpOnly,
    /// Keep the patch pending until an operator chooses a resolution.
    Disabled,
}

/// Insertion point for append/insert operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InsertionPoint {
    /// Append a milestone to the end of the roadmap.
    AppendToRoadmap,
    /// Insert before an existing milestone.
    BeforeMilestone {
        /// Existing milestone ID.
        milestone_id: String,
    },
    /// Insert after an existing milestone.
    AfterMilestone {
        /// Existing milestone ID.
        milestone_id: String,
    },
    /// Append a task to an existing milestone.
    AppendToMilestone {
        /// Existing milestone ID.
        milestone_id: String,
    },
    /// Insert before an existing task.
    BeforeTask {
        /// Existing milestone ID.
        milestone_id: String,
        /// Existing task ID.
        task_id: String,
    },
    /// Insert after an existing task.
    AfterTask {
        /// Existing milestone ID.
        milestone_id: String,
        /// Existing task ID.
        task_id: String,
    },
}

impl InsertionPoint {
    fn is_empty_reference(&self) -> bool {
        match self {
            Self::AppendToRoadmap => false,
            Self::BeforeMilestone { milestone_id }
            | Self::AfterMilestone { milestone_id }
            | Self::AppendToMilestone { milestone_id } => milestone_id.trim().is_empty(),
            Self::BeforeTask {
                milestone_id,
                task_id,
            }
            | Self::AfterTask {
                milestone_id,
                task_id,
            } => milestone_id.trim().is_empty() || task_id.trim().is_empty(),
        }
    }
}

/// One operation inside a roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RoadmapPatchOperation {
    /// Insert a new milestone.
    AddMilestone {
        /// New milestone.
        milestone: RoadmapMilestone,
        /// Position for the new milestone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        insertion: Option<InsertionPoint>,
    },
    /// Insert a new task into an existing milestone.
    AddTask {
        /// Existing milestone ID.
        milestone_id: String,
        /// New task.
        task: RoadmapTask,
        /// Position for the new task.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        insertion: Option<InsertionPoint>,
    },
    /// Replace an item that is still draft-only and not part of completed history.
    ReplaceDraftItem {
        /// Existing draft item to replace.
        target: RoadmapItemRef,
        /// Replacement item.
        replacement: RoadmapPatchItem,
        /// Human-readable replacement reason.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
}

impl RoadmapPatchOperation {
    fn validate_shape(&self, index: usize, issues: &mut Vec<RoadmapPatchValidationIssue>) {
        match self {
            Self::AddMilestone {
                milestone,
                insertion,
            } => validate_add_milestone(index, milestone, insertion.as_ref(), issues),
            Self::AddTask {
                milestone_id,
                task,
                insertion,
            } => validate_add_task(index, milestone_id, task, insertion.as_ref(), issues),
            Self::ReplaceDraftItem {
                target,
                replacement,
                ..
            } => validate_replace_draft_item(index, target, replacement, issues),
        }
    }
}

/// Reference to a roadmap milestone or task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoadmapItemRef {
    /// Milestone reference.
    Milestone {
        /// Milestone ID.
        milestone_id: String,
    },
    /// Task reference.
    Task {
        /// Milestone ID containing the task.
        milestone_id: String,
        /// Task ID.
        task_id: String,
    },
}

impl RoadmapItemRef {
    fn is_empty(&self) -> bool {
        match self {
            Self::Milestone { milestone_id } => milestone_id.trim().is_empty(),
            Self::Task {
                milestone_id,
                task_id,
            } => milestone_id.trim().is_empty() || task_id.trim().is_empty(),
        }
    }
}

/// Replacement item for a draft-only roadmap item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoadmapPatchItem {
    /// Replacement milestone.
    Milestone {
        /// Milestone contents.
        milestone: RoadmapMilestone,
    },
    /// Replacement task.
    Task {
        /// Task contents.
        task: RoadmapTask,
    },
}

/// Dependency edge introduced by a patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapPatchDependency {
    /// Source item that must complete first.
    pub from: RoadmapItemRef,
    /// Dependent item.
    pub to: RoadmapItemRef,
    /// Human-readable reason.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl RoadmapPatchDependency {
    /// Convert a milestone dependency into a patch dependency.
    #[must_use]
    pub fn from_milestone_dependency(dependency: RoadmapDependency) -> Self {
        Self {
            from: RoadmapItemRef::Milestone {
                milestone_id: dependency.from,
            },
            to: RoadmapItemRef::Milestone {
                milestone_id: dependency.to,
            },
            reason: dependency.reason,
        }
    }

    fn validate_shape(&self, index: usize, issues: &mut Vec<RoadmapPatchValidationIssue>) {
        if self.from.is_empty() {
            issues.push(RoadmapPatchValidationIssue::new(
                RoadmapPatchValidationCode::MissingTargetReference,
                format!("dependencies[{index}].from"),
                "dependency source reference must not be empty",
            ));
        }
        if self.to.is_empty() {
            issues.push(RoadmapPatchValidationIssue::new(
                RoadmapPatchValidationCode::MissingTargetReference,
                format!("dependencies[{index}].to"),
                "dependency target reference must not be empty",
            ));
        }
    }
}

/// Conflict metadata attached to a roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoadmapPatchConflict {
    /// Stable conflict code.
    pub code: RoadmapPatchConflictCode,
    /// Roadmap item involved in the conflict, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<RoadmapItemRef>,
    /// Operator-facing explanation.
    pub message: String,
    /// Available operator choices.
    #[serde(default)]
    pub choices: Vec<OperatorConflictChoice>,
    /// Selected choice, once the operator has resolved the conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_choice: Option<OperatorConflictChoice>,
}

impl RoadmapPatchConflict {
    fn validate_shape(&self, index: usize, issues: &mut Vec<RoadmapPatchValidationIssue>) {
        if self.message.trim().is_empty() {
            issues.push(RoadmapPatchValidationIssue::new(
                RoadmapPatchValidationCode::MissingConflictMessage,
                format!("conflicts[{index}].message"),
                "conflict message must not be empty",
            ));
        }
        if self.choices.is_empty() {
            issues.push(RoadmapPatchValidationIssue::new(
                RoadmapPatchValidationCode::MissingConflictChoice,
                format!("conflicts[{index}].choices"),
                "conflict must expose at least one operator choice",
            ));
        }
    }
}

/// Stable conflict classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapPatchConflictCode {
    /// Referenced item could not be found.
    MissingTarget,
    /// Patch references an item already running.
    RunningMilestone,
    /// Patch would mutate completed history.
    CompletedHistory,
    /// Patch would duplicate an existing item ID.
    DuplicateItem,
    /// Patch would introduce a dependency cycle.
    DependencyCycle,
    /// Operation cannot be represented safely.
    UnsupportedOperation,
}

/// Operator choice for resolving an amendment conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorConflictChoice {
    /// Defer the new work to the next safe milestone.
    DeferToNextMilestone,
    /// Abort the current run before applying the patch.
    AbortCurrentRun,
    /// Create a follow-up run instead of amending the active run.
    CreateFollowUpRun,
    /// Reject the patch.
    RejectPatch,
}

/// Operator choices that are safe to present for one conflict class.
#[must_use]
pub fn conflict_choices_for_code(code: RoadmapPatchConflictCode) -> Vec<OperatorConflictChoice> {
    match code {
        RoadmapPatchConflictCode::RunningMilestone => vec![
            OperatorConflictChoice::DeferToNextMilestone,
            OperatorConflictChoice::AbortCurrentRun,
            OperatorConflictChoice::CreateFollowUpRun,
            OperatorConflictChoice::RejectPatch,
        ],
        RoadmapPatchConflictCode::CompletedHistory => vec![
            OperatorConflictChoice::CreateFollowUpRun,
            OperatorConflictChoice::RejectPatch,
        ],
        RoadmapPatchConflictCode::MissingTarget
        | RoadmapPatchConflictCode::DuplicateItem
        | RoadmapPatchConflictCode::DependencyCycle
        | RoadmapPatchConflictCode::UnsupportedOperation => {
            vec![OperatorConflictChoice::RejectPatch]
        },
    }
}

/// Lifecycle state for a roadmap patch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapPatchStatus {
    /// Patch has been drafted by Feature Planner.
    #[default]
    Drafted,
    /// Patch is waiting for operator approval.
    PendingApproval,
    /// Operator approved the patch.
    Approved,
    /// Patch was applied to a roadmap or follow-up run.
    Applied,
    /// Operator rejected the patch.
    Rejected,
    /// Patch was superseded by a later draft.
    Superseded,
}

/// Operator approval decision for a drafted patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapPatchApprovalDecision {
    /// Approve and apply the patch.
    Approve,
    /// Ask the Feature Planner to revise the patch.
    Edit,
    /// Reject the patch.
    Reject,
}

/// Shape-level validation codes for roadmap patches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapPatchValidationCode {
    /// Schema version is not supported.
    UnsupportedSchemaVersion,
    /// Patch has no operations.
    MissingOperation,
    /// An add operation has no insertion point.
    MissingInsertionPoint,
    /// A milestone/task/reference ID is empty.
    MissingTargetReference,
    /// A milestone or task title is empty.
    MissingTitle,
    /// Conflict message is empty.
    MissingConflictMessage,
    /// Conflict has no operator choice.
    MissingConflictChoice,
}

impl RoadmapPatchValidationCode {
    /// Stable snake-case code for logs and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedSchemaVersion => "unsupported_schema_version",
            Self::MissingOperation => "missing_operation",
            Self::MissingInsertionPoint => "missing_insertion_point",
            Self::MissingTargetReference => "missing_target_reference",
            Self::MissingTitle => "missing_title",
            Self::MissingConflictMessage => "missing_conflict_message",
            Self::MissingConflictChoice => "missing_conflict_choice",
        }
    }
}

impl fmt::Display for RoadmapPatchValidationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One shape-level validation issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapPatchValidationIssue {
    /// Stable machine-readable code.
    pub code: RoadmapPatchValidationCode,
    /// Field path or logical location.
    pub location: String,
    /// Short user-safe explanation.
    pub message: String,
}

impl RoadmapPatchValidationIssue {
    /// Create a validation issue.
    #[must_use]
    pub fn new(
        code: RoadmapPatchValidationCode,
        location: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            location: location.into(),
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct RoadmapPatchHashInput<'a> {
    schema_version: u32,
    target: &'a RoadmapPatchTarget,
    rationale: &'a str,
    operations: &'a [RoadmapPatchOperation],
    dependencies: &'a [RoadmapPatchDependency],
    conflicts: &'a [RoadmapPatchConflict],
}

fn validate_patch_id(value: &str) -> Result<(), RoadmapPatchIdError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(RoadmapPatchIdError::Empty);
    }

    for character in trimmed.chars() {
        if is_patch_id_character(character) {
            continue;
        }
        return Err(RoadmapPatchIdError::UnsupportedCharacter { character });
    }

    Ok(())
}

const fn is_patch_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_' || character == '.'
}

fn push_schema_issue(schema_version: u32, issues: &mut Vec<RoadmapPatchValidationIssue>) {
    if schema_version == ROADMAP_PATCH_SCHEMA_VERSION {
        return;
    }
    issues.push(RoadmapPatchValidationIssue::new(
        RoadmapPatchValidationCode::UnsupportedSchemaVersion,
        "schema_version",
        format!("expected schema_version {ROADMAP_PATCH_SCHEMA_VERSION}"),
    ));
}

fn push_empty_operations_issue(
    operations_empty: bool,
    issues: &mut Vec<RoadmapPatchValidationIssue>,
) {
    if !operations_empty {
        return;
    }
    issues.push(RoadmapPatchValidationIssue::new(
        RoadmapPatchValidationCode::MissingOperation,
        "operations",
        "patch must contain at least one operation",
    ));
}

fn validate_add_milestone(
    index: usize,
    milestone: &RoadmapMilestone,
    insertion: Option<&InsertionPoint>,
    issues: &mut Vec<RoadmapPatchValidationIssue>,
) {
    validate_required_text(
        &milestone.id,
        RoadmapPatchValidationCode::MissingTargetReference,
        format!("operations[{index}].milestone.id"),
        "new milestone id must not be empty",
        issues,
    );
    validate_required_text(
        &milestone.title,
        RoadmapPatchValidationCode::MissingTitle,
        format!("operations[{index}].milestone.title"),
        "new milestone title must not be empty",
        issues,
    );
    validate_insertion(index, insertion, issues);
}

fn validate_add_task(
    index: usize,
    milestone_id: &str,
    task: &RoadmapTask,
    insertion: Option<&InsertionPoint>,
    issues: &mut Vec<RoadmapPatchValidationIssue>,
) {
    validate_required_text(
        milestone_id,
        RoadmapPatchValidationCode::MissingTargetReference,
        format!("operations[{index}].milestone_id"),
        "target milestone id must not be empty",
        issues,
    );
    validate_required_text(
        &task.id,
        RoadmapPatchValidationCode::MissingTargetReference,
        format!("operations[{index}].task.id"),
        "new task id must not be empty",
        issues,
    );
    validate_required_text(
        &task.title,
        RoadmapPatchValidationCode::MissingTitle,
        format!("operations[{index}].task.title"),
        "new task title must not be empty",
        issues,
    );
    validate_insertion(index, insertion, issues);
}

fn validate_replace_draft_item(
    index: usize,
    target: &RoadmapItemRef,
    replacement: &RoadmapPatchItem,
    issues: &mut Vec<RoadmapPatchValidationIssue>,
) {
    if target.is_empty() {
        issues.push(RoadmapPatchValidationIssue::new(
            RoadmapPatchValidationCode::MissingTargetReference,
            format!("operations[{index}].target"),
            "replacement target reference must not be empty",
        ));
    }
    match replacement {
        RoadmapPatchItem::Milestone { milestone } => validate_required_text(
            &milestone.title,
            RoadmapPatchValidationCode::MissingTitle,
            format!("operations[{index}].replacement.milestone.title"),
            "replacement milestone title must not be empty",
            issues,
        ),
        RoadmapPatchItem::Task { task } => validate_required_text(
            &task.title,
            RoadmapPatchValidationCode::MissingTitle,
            format!("operations[{index}].replacement.task.title"),
            "replacement task title must not be empty",
            issues,
        ),
    }
}

fn validate_insertion(
    index: usize,
    insertion: Option<&InsertionPoint>,
    issues: &mut Vec<RoadmapPatchValidationIssue>,
) {
    let Some(insertion) = insertion else {
        issues.push(RoadmapPatchValidationIssue::new(
            RoadmapPatchValidationCode::MissingInsertionPoint,
            format!("operations[{index}].insertion"),
            "add operation must include an insertion point",
        ));
        return;
    };

    if insertion.is_empty_reference() {
        issues.push(RoadmapPatchValidationIssue::new(
            RoadmapPatchValidationCode::MissingTargetReference,
            format!("operations[{index}].insertion"),
            "insertion point reference must not be empty",
        ));
    }
}

fn validate_required_text(
    value: &str,
    code: RoadmapPatchValidationCode,
    location: String,
    message: &'static str,
    issues: &mut Vec<RoadmapPatchValidationIssue>,
) {
    if !value.trim().is_empty() {
        return;
    }
    issues.push(RoadmapPatchValidationIssue::new(code, location, message));
}

const fn default_schema_version() -> u32 {
    ROADMAP_PATCH_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> RoadmapPatchTarget {
        RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        }
    }

    fn base_roadmap() -> RoadmapArtifact {
        let mut milestone = RoadmapMilestone::new("m1", "Foundation");
        milestone
            .tasks
            .push(RoadmapTask::new("m1-t1", "First task"));
        RoadmapArtifact::new(vec![milestone])
    }

    fn patch(operations: Vec<RoadmapPatchOperation>) -> RoadmapPatch {
        RoadmapPatch::new(
            RoadmapPatchId::new("rpatch-apply").unwrap(),
            target(),
            operations,
        )
    }

    fn conflict_codes(error: RoadmapPatchApplyError) -> Vec<RoadmapPatchConflictCode> {
        match error {
            RoadmapPatchApplyError::Conflicts { conflicts } => conflicts
                .into_iter()
                .map(|conflict| conflict.code)
                .collect(),
            other => panic!("expected conflicts, got {other:?}"),
        }
    }

    #[test]
    fn patch_id_new_stores_trimmed_value() {
        let patch_id = RoadmapPatchId::new("  rpatch-trimmed  ").unwrap();

        assert_eq!(patch_id.as_str(), "rpatch-trimmed");
    }

    #[test]
    fn applies_append_milestone_task_dependency_and_markdown() {
        let mut patch = patch(vec![
            RoadmapPatchOperation::AddMilestone {
                milestone: RoadmapMilestone::new("m2", "Approval flow"),
                insertion: Some(InsertionPoint::AppendToRoadmap),
            },
            RoadmapPatchOperation::AddTask {
                milestone_id: "m1".into(),
                task: RoadmapTask::new("m1-t2", "Second task"),
                insertion: Some(InsertionPoint::AppendToMilestone {
                    milestone_id: "m1".into(),
                }),
            },
        ]);
        patch.dependencies.push(RoadmapPatchDependency {
            from: RoadmapItemRef::Milestone {
                milestone_id: "m1".into(),
            },
            to: RoadmapItemRef::Milestone {
                milestone_id: "m2".into(),
            },
            reason: "foundation first".into(),
        });

        let result = patch.apply_to_roadmap(&base_roadmap()).unwrap();

        assert_eq!(
            result
                .roadmap
                .milestones
                .iter()
                .map(|milestone| milestone.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "m2"]
        );
        assert_eq!(result.roadmap.milestones[0].tasks[1].id, "m1-t2");
        assert_eq!(result.roadmap.dependencies.len(), 1);
        assert!(result.markdown.contains("## m2: Approval flow"));
        assert!(result.markdown.contains("- [ ] m1-t2: Second task"));
    }

    #[test]
    fn rejects_task_append_to_completed_milestone() {
        let mut roadmap = base_roadmap();
        roadmap.milestones[0].status = RoadmapStatus::Completed;
        let patch = patch(vec![RoadmapPatchOperation::AddTask {
            milestone_id: "m1".into(),
            task: RoadmapTask::new("m1-t2", "Late task"),
            insertion: Some(InsertionPoint::AppendToMilestone {
                milestone_id: "m1".into(),
            }),
        }]);

        let codes = conflict_codes(patch.apply_to_roadmap(&roadmap).unwrap_err());

        assert_eq!(codes, vec![RoadmapPatchConflictCode::CompletedHistory]);
    }

    #[test]
    fn rejects_insert_after_running_milestone() {
        let mut roadmap = base_roadmap();
        roadmap.milestones[0].status = RoadmapStatus::Running;
        let patch = patch(vec![RoadmapPatchOperation::AddMilestone {
            milestone: RoadmapMilestone::new("m2", "Blocked"),
            insertion: Some(InsertionPoint::AfterMilestone {
                milestone_id: "m1".into(),
            }),
        }]);

        let codes = conflict_codes(patch.apply_to_roadmap(&roadmap).unwrap_err());

        assert_eq!(codes, vec![RoadmapPatchConflictCode::RunningMilestone]);
    }

    #[test]
    fn running_conflict_surfaces_operator_resolution_choices() {
        let mut roadmap = base_roadmap();
        roadmap.milestones[0].status = RoadmapStatus::Running;
        let patch = patch(vec![RoadmapPatchOperation::AddTask {
            milestone_id: "m1".into(),
            task: RoadmapTask::new("m1-t2", "Later task"),
            insertion: Some(InsertionPoint::AppendToMilestone {
                milestone_id: "m1".into(),
            }),
        }]);
        let RoadmapPatchApplyError::Conflicts { conflicts } =
            patch.apply_to_roadmap(&roadmap).unwrap_err()
        else {
            panic!("expected running milestone conflict");
        };

        let conflict = conflicts[0].to_patch_conflict();

        assert_eq!(conflict.code, RoadmapPatchConflictCode::RunningMilestone);
        assert_eq!(
            conflict.choices,
            vec![
                OperatorConflictChoice::DeferToNextMilestone,
                OperatorConflictChoice::AbortCurrentRun,
                OperatorConflictChoice::CreateFollowUpRun,
                OperatorConflictChoice::RejectPatch,
            ]
        );
    }

    #[test]
    fn replaces_pending_task_only() {
        let patch = patch(vec![RoadmapPatchOperation::ReplaceDraftItem {
            target: RoadmapItemRef::Task {
                milestone_id: "m1".into(),
                task_id: "m1-t1".into(),
            },
            replacement: RoadmapPatchItem::Task {
                task: RoadmapTask::new("m1-t1", "Updated first task"),
            },
            reason: "clearer wording".into(),
        }]);

        let result = patch.apply_to_roadmap(&base_roadmap()).unwrap();

        assert_eq!(
            result.roadmap.milestones[0].tasks[0].title,
            "Updated first task"
        );
        assert_eq!(
            result.replaced_items,
            vec![RoadmapItemRef::Task {
                milestone_id: "m1".into(),
                task_id: "m1-t1".into()
            }]
        );
    }

    #[test]
    fn rejects_milestone_dependency_cycle() {
        let mut roadmap = base_roadmap();
        roadmap
            .milestones
            .push(RoadmapMilestone::new("m2", "Second"));
        roadmap.dependencies.push(RoadmapDependency {
            from: "m1".into(),
            to: "m2".into(),
            reason: String::new(),
        });
        let mut patch = patch(vec![RoadmapPatchOperation::AddTask {
            milestone_id: "m2".into(),
            task: RoadmapTask::new("m2-t1", "Noop"),
            insertion: Some(InsertionPoint::AppendToMilestone {
                milestone_id: "m2".into(),
            }),
        }]);
        patch.dependencies.push(RoadmapPatchDependency {
            from: RoadmapItemRef::Milestone {
                milestone_id: "m2".into(),
            },
            to: RoadmapItemRef::Milestone {
                milestone_id: "m1".into(),
            },
            reason: "cycle".into(),
        });

        let codes = conflict_codes(patch.apply_to_roadmap(&roadmap).unwrap_err());

        assert!(codes.contains(&RoadmapPatchConflictCode::DependencyCycle));
    }
}
