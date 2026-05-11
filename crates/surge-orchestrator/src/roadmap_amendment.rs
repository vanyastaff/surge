//! Roadmap amendment orchestration helpers.
//!
//! This module owns the application-layer glue for storing amendment artifacts
//! and appending compact lifecycle events. Pure patch modeling stays in
//! `surge-core`; durable bytes stay in the content-addressed artifact store.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use surge_core::ContentHash;
use surge_core::approvals::{ApprovalChannel, ApprovalChannelKind};
use surge_core::keys::NodeKey;
use surge_core::roadmap_patch::{
    ActivePickupPolicy, InsertionPoint, OperatorConflictChoice, RoadmapItemRef,
    RoadmapPatchApplyConflict, RoadmapPatchApplyResult, RoadmapPatchApprovalDecision,
    RoadmapPatchConflict, RoadmapPatchDependency, RoadmapPatchId, RoadmapPatchItem,
    RoadmapPatchOperation, RoadmapPatchStatus, RoadmapPatchTarget,
};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{Graph, RoadmapArtifact, RoadmapMilestone, RoadmapPatch, RoadmapStatus, RunId};
use surge_notify::{
    NotifyMessage, RoadmapAmendmentNotificationKind, RoadmapAmendmentNotificationPayload,
};
use surge_persistence::artifacts::{ArtifactStore, ArtifactStoreError};
use surge_persistence::runs::{RunWriter, StorageError};

use crate::engine::config::{EngineRunConfig, ProjectContextSeed, RunSeedArtifact};
use crate::engine::facade::EngineFacade;
use crate::engine::handle::RunHandle;
use crate::flow_amendment::{
    FlowAmendmentError, amend_active_flow, create_follow_up_flow, graph_content_hash,
};

const ROADMAP_PATCH_ARTIFACT_NAME: &str = "roadmap-patch.toml";
const AMENDED_ROADMAP_ARTIFACT_NAME: &str = "amended-roadmap.toml";
const AMENDED_FLOW_ARTIFACT_NAME: &str = "amended-flow.toml";
const FOLLOW_UP_ROADMAP_ARTIFACT_NAME: &str = "roadmap_amendment";
const FOLLOW_UP_ROADMAP_ARTIFACT_RELPATH: &str = ".surge/roadmap_amendment.md";
const FOLLOW_UP_ROADMAP_PRODUCER_NODE: &str = "roadmap_amendment_seed";
const APPROVE_DECISION: &str = "approve";
const EDIT_DECISION: &str = "edit";
const REJECT_DECISION: &str = "reject";

/// Stored artifact refs for an applied roadmap amendment.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredAmendmentArtifacts {
    /// Stored amended roadmap artifact.
    pub roadmap: surge_core::run_state::ArtifactRef,
    /// Stored amended flow artifact, when a flow was generated.
    pub flow: Option<surge_core::run_state::ArtifactRef>,
}

/// Materialized inputs for starting a follow-up amendment run.
#[derive(Debug, Clone)]
pub struct FollowUpRunRequest {
    /// Newly allocated follow-up run id.
    pub run_id: RunId,
    /// Patch that caused the follow-up run.
    pub patch_id: RoadmapPatchId,
    /// Target being followed up.
    pub target: RoadmapPatchTarget,
    /// Validated follow-up graph.
    pub graph: Graph,
    /// Hash of the serialized follow-up graph.
    pub graph_hash: ContentHash,
    /// Worktree path for the follow-up run.
    pub worktree_path: std::path::PathBuf,
    /// Run config including project context and amendment seed artifacts.
    pub run_config: EngineRunConfig,
}

/// Result of applying an approved roadmap patch to an active run boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveRunAmendmentOutcome {
    /// Run that accepted the amendment.
    pub run_id: RunId,
    /// Patch that caused the amendment.
    pub patch_id: RoadmapPatchId,
    /// Hash of the graph before applying the amendment.
    pub previous_graph_hash: ContentHash,
    /// Hash of the amended graph.
    pub graph_hash: ContentHash,
    /// Nodes inserted for amendment work.
    pub inserted_nodes: Vec<NodeKey>,
    /// Stored amended roadmap artifact hash.
    pub roadmap_artifact: ContentHash,
    /// Stored amended flow artifact hash.
    pub flow_artifact: ContentHash,
}

/// Operator-facing prompt data for a roadmap patch approval card.
#[derive(Debug, Clone, PartialEq)]
pub struct RoadmapPatchApprovalPrompt {
    /// Patch being reviewed.
    pub patch_id: RoadmapPatchId,
    /// Target roadmap/run being amended.
    pub target: RoadmapPatchTarget,
    /// Stable, concise summary for human review.
    pub summary: String,
    /// Hash of `summary`, stored in lifecycle events.
    pub summary_hash: ContentHash,
    /// JSON response schema mirroring the existing HumanGate response shape.
    pub schema: serde_json::Value,
}

/// Operator response to a roadmap patch approval prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoadmapPatchApprovalResolution {
    /// Requested lifecycle decision.
    pub decision: RoadmapPatchApprovalDecision,
    /// Optional operator comment or edit feedback.
    pub comment: Option<String>,
    /// Optional conflict resolution selected by the operator.
    pub conflict_choice: Option<OperatorConflictChoice>,
}

/// Next action selected by the roadmap patch approval loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoadmapPatchApprovalAction {
    /// Patch is approved and can be applied.
    Apply,
    /// Patch needs another Feature Planner draft using this feedback.
    Redraft {
        /// Operator feedback passed back into Feature Planner.
        feedback: String,
        /// 1-based edit attempt count.
        attempt: u32,
    },
    /// Patch is rejected and should not be applied.
    Rejected,
}

/// Bounded approval-loop state for one roadmap patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoadmapPatchApprovalLoop {
    max_edit_rounds: u32,
    edit_rounds: u32,
}

impl RoadmapPatchApprovalLoop {
    /// Create a bounded approval loop. `0` disables the edit-loop cap.
    #[must_use]
    pub const fn new(max_edit_rounds: u32) -> Self {
        Self {
            max_edit_rounds,
            edit_rounds: 0,
        }
    }

    /// Number of edit decisions already accepted.
    #[must_use]
    pub const fn edit_rounds(&self) -> u32 {
        self.edit_rounds
    }

    /// Maximum accepted edit decisions before escalation. `0` means unbounded.
    #[must_use]
    pub const fn max_edit_rounds(&self) -> u32 {
        self.max_edit_rounds
    }

    /// Append an approval-request event and return the prompt payload that a
    /// CLI, UI, or notification channel can render.
    ///
    /// # Errors
    /// Returns [`RoadmapAmendmentError`] when the event writer fails.
    pub async fn request_approval(
        &self,
        writer: &RunWriter,
        patch: &RoadmapPatch,
        channel: ApprovalChannel,
    ) -> Result<RoadmapPatchApprovalPrompt, RoadmapAmendmentError> {
        request_patch_approval(writer, patch, channel).await
    }

    /// Persist an operator decision and translate it to the next loop action.
    ///
    /// # Errors
    /// Returns [`RoadmapAmendmentError`] when the event writer fails, an edit
    /// response lacks feedback, or the edit-loop cap has been exceeded.
    pub async fn record_decision(
        &mut self,
        writer: &RunWriter,
        patch_id: &RoadmapPatchId,
        channel_used: ApprovalChannelKind,
        resolution: RoadmapPatchApprovalResolution,
    ) -> Result<RoadmapPatchApprovalAction, RoadmapAmendmentError> {
        record_patch_approval_decision(writer, patch_id, channel_used, &resolution).await?;
        match resolution.decision {
            RoadmapPatchApprovalDecision::Approve => Ok(RoadmapPatchApprovalAction::Apply),
            RoadmapPatchApprovalDecision::Reject => Ok(RoadmapPatchApprovalAction::Rejected),
            RoadmapPatchApprovalDecision::Edit => {
                let feedback = edit_feedback(patch_id, resolution.comment.as_deref())?;
                self.accept_edit_or_escalate(writer, patch_id, feedback)
                    .await
            },
        }
    }

    async fn accept_edit_or_escalate(
        &mut self,
        writer: &RunWriter,
        patch_id: &RoadmapPatchId,
        feedback: String,
    ) -> Result<RoadmapPatchApprovalAction, RoadmapAmendmentError> {
        if self.max_edit_rounds > 0 && self.edit_rounds >= self.max_edit_rounds {
            append_edit_loop_escalation(writer, patch_id, self.max_edit_rounds).await?;
            tracing::warn!(
                target: "roadmap_amendment",
                patch_id = %patch_id,
                max_edit_rounds = self.max_edit_rounds,
                "roadmap_patch_approval_edit_loop_exceeded"
            );
            return Err(RoadmapAmendmentError::ApprovalLoopExceeded {
                patch_id: patch_id.clone(),
                max_edit_rounds: self.max_edit_rounds,
            });
        }

        self.edit_rounds += 1;
        tracing::info!(
            target: "roadmap_amendment",
            patch_id = %patch_id,
            attempt = self.edit_rounds,
            "roadmap_patch_redraft_requested"
        );
        Ok(RoadmapPatchApprovalAction::Redraft {
            feedback,
            attempt: self.edit_rounds,
        })
    }
}

/// Errors from roadmap amendment artifact helpers.
#[derive(Debug, thiserror::Error)]
pub enum RoadmapAmendmentError {
    /// Artifact store failed.
    #[error("artifact store failed: {0}")]
    ArtifactStore(#[from] ArtifactStoreError),
    /// Event writer failed.
    #[error("event writer failed: {0}")]
    Storage(#[from] StorageError),
    /// Flow generation failed.
    #[error("flow amendment failed: {0}")]
    Flow(#[from] FlowAmendmentError),
    /// Engine start failed.
    #[error("engine failed to start follow-up run: {0}")]
    Engine(#[from] crate::engine::error::EngineError),
    /// Generated key was not valid.
    #[error("invalid generated key: {0}")]
    Key(#[from] surge_core::keys::KeyParseError),
    /// Active amendment storage did not return an amended flow artifact.
    #[error("active amendment did not store an amended flow artifact")]
    MissingActiveFlowArtifact,
    /// Operator selected edit without useful feedback.
    #[error("roadmap patch {patch_id} edit decision requires feedback")]
    MissingEditFeedback {
        /// Patch that needs feedback.
        patch_id: RoadmapPatchId,
    },
    /// Edit loop exceeded the configured cap.
    #[error("roadmap patch {patch_id} approval edit-loop exceeded cap {max_edit_rounds}")]
    ApprovalLoopExceeded {
        /// Patch whose edit loop exceeded the cap.
        patch_id: RoadmapPatchId,
        /// Configured maximum accepted edit rounds.
        max_edit_rounds: u32,
    },
}

/// Build the graph and run configuration for a follow-up amendment run.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] if follow-up graph generation fails or
/// the synthetic seed producer key is invalid.
pub fn build_follow_up_run_request(
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    patch_result: &RoadmapPatchApplyResult,
    worktree_path: impl Into<std::path::PathBuf>,
    project_context: Option<ProjectContextSeed>,
    created_at: chrono::DateTime<chrono::Utc>,
) -> Result<FollowUpRunRequest, RoadmapAmendmentError> {
    let flow = create_follow_up_flow(patch_result, created_at)?;
    let amendment_seed = RunSeedArtifact::new(
        FOLLOW_UP_ROADMAP_ARTIFACT_NAME,
        FOLLOW_UP_ROADMAP_ARTIFACT_RELPATH,
        follow_up_roadmap_seed_markdown(patch_id, patch_result),
        FOLLOW_UP_ROADMAP_PRODUCER_NODE,
    )?;
    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        initial_prompt: format!(
            "Follow-up roadmap amendment {patch_id}. Use the `{FOLLOW_UP_ROADMAP_ARTIFACT_NAME}` run artifact as the appended work seed."
        ),
        project_context,
        seed_artifacts: vec![amendment_seed],
        ..EngineRunConfig::default()
    };

    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %patch_id,
        target = ?target,
        run_id = %run_id,
        graph_hash = %flow.graph_hash,
        "follow_up_run_request_materialized"
    );
    Ok(FollowUpRunRequest {
        run_id,
        patch_id: patch_id.clone(),
        target: target.clone(),
        graph: flow.graph,
        graph_hash: flow.graph_hash,
        worktree_path: worktree_path.into(),
        run_config,
    })
}

/// Start a materialized follow-up amendment run through the engine facade.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when the engine facade rejects the run.
#[must_use = "await the follow-up run start and handle failures"]
pub async fn start_follow_up_run(
    engine: &dyn EngineFacade,
    request: FollowUpRunRequest,
) -> Result<RunHandle, RoadmapAmendmentError> {
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %request.patch_id,
        target = ?request.target,
        run_id = %request.run_id,
        graph_hash = %request.graph_hash,
        "follow_up_run_starting"
    );
    let handle = engine
        .start_run(
            request.run_id,
            request.graph,
            request.worktree_path,
            request.run_config,
        )
        .await?;
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %request.patch_id,
        run_id = %handle.run_id,
        "follow_up_run_started"
    );
    Ok(handle)
}

/// Apply an approved patch to an active run's durable log.
///
/// The caller supplies the currently active graph. This helper generates the
/// amended graph, stores roadmap/flow artifacts, and appends both the roadmap
/// update and graph revision events to the target run writer. The engine task
/// can then pick up the revision at a safe boundary.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when graph amendment, artifact storage, or
/// event appends fail.
#[must_use = "await active run amendment application and handle failures"]
pub async fn apply_active_run_patch(
    artifact_store: &ArtifactStore,
    writer: &RunWriter,
    run_id: RunId,
    active_graph: &Graph,
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    patch_result: &RoadmapPatchApplyResult,
) -> Result<ActiveRunAmendmentOutcome, RoadmapAmendmentError> {
    let previous_graph_hash = graph_content_hash(active_graph).map_err(FlowAmendmentError::from)?;
    let amendment = amend_active_flow(active_graph, patch_result)?;
    let artifacts = store_applied_artifacts(
        artifact_store,
        writer,
        run_id,
        patch_id,
        target,
        patch_result.markdown.as_bytes(),
        Some(amendment.flow_toml.as_bytes()),
    )
    .await?;
    record_roadmap_updated(
        writer,
        patch_id,
        target,
        &artifacts,
        ActivePickupPolicy::Allowed,
    )
    .await?;
    record_graph_revision_accepted(
        writer,
        patch_id,
        target,
        previous_graph_hash,
        &amendment.graph,
        amendment.graph_hash,
        ActivePickupPolicy::Allowed,
    )
    .await?;

    let Some(flow_artifact) = artifacts.flow.as_ref().map(|artifact| artifact.hash) else {
        return Err(RoadmapAmendmentError::MissingActiveFlowArtifact);
    };

    tracing::info!(
        target: "roadmap_amendment",
        run_id = %run_id,
        patch_id = %patch_id,
        previous_graph_hash = %previous_graph_hash,
        graph_hash = %amendment.graph_hash,
        inserted_nodes = amendment.inserted_nodes.len(),
        "active_run_roadmap_patch_applied"
    );
    Ok(ActiveRunAmendmentOutcome {
        run_id,
        patch_id: patch_id.clone(),
        previous_graph_hash,
        graph_hash: amendment.graph_hash,
        inserted_nodes: amendment.inserted_nodes,
        roadmap_artifact: artifacts.roadmap.hash,
        flow_artifact,
    })
}

/// Convert apply-time conflicts into operator-facing patch conflicts.
#[must_use]
pub fn apply_conflicts_as_patch_conflicts(
    conflicts: &[RoadmapPatchApplyConflict],
) -> Vec<RoadmapPatchConflict> {
    conflicts
        .iter()
        .map(RoadmapPatchApplyConflict::to_patch_conflict)
        .collect()
}

/// Build a standalone follow-up roadmap result directly from patch operations.
///
/// This is used when an operator resolves a running/completed-roadmap conflict
/// by choosing a follow-up run. The original roadmap is intentionally not
/// mutated; the follow-up seed contains only the new or replacement work.
#[must_use]
pub fn follow_up_result_from_patch(patch: &RoadmapPatch) -> RoadmapPatchApplyResult {
    let mut roadmap = RoadmapArtifact::default();
    let mut used_milestone_ids = BTreeSet::new();
    let mut inserted_milestones = Vec::new();
    let mut inserted_tasks = Vec::new();
    let mut replaced_items = Vec::new();

    for operation in &patch.operations {
        match operation {
            RoadmapPatchOperation::AddMilestone { milestone, .. } => {
                let mut milestone = pending_follow_up_milestone(milestone);
                milestone.id = unique_id(&milestone.id, &mut used_milestone_ids);
                inserted_milestones.push(milestone.id.clone());
                roadmap.milestones.push(milestone);
            },
            RoadmapPatchOperation::AddTask {
                milestone_id, task, ..
            } => {
                let synthetic_id = unique_id(
                    &format!("{milestone_id}-follow-up"),
                    &mut used_milestone_ids,
                );
                let mut milestone = RoadmapMilestone::new(
                    synthetic_id.clone(),
                    format!("Follow-up for {milestone_id}"),
                );
                let mut task = task.clone();
                task.status = RoadmapStatus::Pending;
                let task_id = task.id.clone();
                milestone.tasks.push(task);
                inserted_milestones.push(synthetic_id.clone());
                inserted_tasks.push(RoadmapItemRef::Task {
                    milestone_id: synthetic_id,
                    task_id,
                });
                roadmap.milestones.push(milestone);
            },
            RoadmapPatchOperation::ReplaceDraftItem {
                target,
                replacement,
                ..
            } => {
                replaced_items.push(target.clone());
                match replacement {
                    RoadmapPatchItem::Milestone { milestone } => {
                        let mut milestone = pending_follow_up_milestone(milestone);
                        milestone.id = unique_id(&milestone.id, &mut used_milestone_ids);
                        inserted_milestones.push(milestone.id.clone());
                        roadmap.milestones.push(milestone);
                    },
                    RoadmapPatchItem::Task { task } => {
                        let milestone_seed = match target {
                            RoadmapItemRef::Milestone { milestone_id }
                            | RoadmapItemRef::Task { milestone_id, .. } => milestone_id,
                        };
                        let synthetic_id = unique_id(
                            &format!("{milestone_seed}-follow-up"),
                            &mut used_milestone_ids,
                        );
                        let mut milestone = RoadmapMilestone::new(
                            synthetic_id.clone(),
                            format!("Follow-up rework for {milestone_seed}"),
                        );
                        let mut task = task.clone();
                        task.status = RoadmapStatus::Pending;
                        let task_id = task.id.clone();
                        milestone.tasks.push(task);
                        inserted_milestones.push(synthetic_id.clone());
                        inserted_tasks.push(RoadmapItemRef::Task {
                            milestone_id: synthetic_id,
                            task_id,
                        });
                        roadmap.milestones.push(milestone);
                    },
                }
            },
        }
    }

    RoadmapPatchApplyResult {
        markdown: roadmap.to_markdown(),
        roadmap,
        inserted_milestones,
        inserted_tasks,
        replaced_items,
        dependencies_added: Vec::new(),
    }
}

/// Build a notification message for a patch approval request.
#[must_use]
pub fn patch_approval_requested_notification(prompt: &RoadmapPatchApprovalPrompt) -> NotifyMessage {
    NotifyMessage::RoadmapAmendment(Box::new(RoadmapAmendmentNotificationPayload {
        kind: RoadmapAmendmentNotificationKind::ApprovalRequested,
        patch_id: prompt.patch_id.clone(),
        target: prompt.target.clone(),
        run_id: target_run_id(&prompt.target),
        follow_up_run_id: None,
        status: Some(RoadmapPatchStatus::PendingApproval),
        summary: "Roadmap patch needs approval".into(),
        detail: Some(prompt.summary.clone()),
        conflict_codes: Vec::new(),
        conflict_choices: Vec::new(),
        artifact_paths: Vec::new(),
    }))
}

/// Build a notification message for a successfully applied patch.
#[must_use]
pub fn patch_applied_notification(
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    artifacts: &StoredAmendmentArtifacts,
) -> NotifyMessage {
    NotifyMessage::RoadmapAmendment(Box::new(RoadmapAmendmentNotificationPayload {
        kind: RoadmapAmendmentNotificationKind::PatchApplied,
        patch_id: patch_id.clone(),
        target: target.clone(),
        run_id: target_run_id(target),
        follow_up_run_id: None,
        status: Some(RoadmapPatchStatus::Applied),
        summary: "Roadmap patch applied".into(),
        detail: None,
        conflict_codes: Vec::new(),
        conflict_choices: Vec::new(),
        artifact_paths: applied_artifact_paths(artifacts),
    }))
}

/// Build a notification message when an active runner sees an amendment.
#[must_use]
pub fn runner_pickup_notification(
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    active_pickup: ActivePickupPolicy,
) -> NotifyMessage {
    NotifyMessage::RoadmapAmendment(Box::new(RoadmapAmendmentNotificationPayload {
        kind: RoadmapAmendmentNotificationKind::RunnerPickup,
        patch_id: patch_id.clone(),
        target: target.clone(),
        run_id: target_run_id(target),
        follow_up_run_id: None,
        status: Some(RoadmapPatchStatus::Applied),
        summary: "Runner observed roadmap amendment".into(),
        detail: Some(format!("active_pickup={}", pickup_label(active_pickup))),
        conflict_codes: Vec::new(),
        conflict_choices: Vec::new(),
        artifact_paths: Vec::new(),
    }))
}

/// Build a notification message for a materialized follow-up run.
#[must_use]
pub fn follow_up_run_created_notification(request: &FollowUpRunRequest) -> NotifyMessage {
    NotifyMessage::RoadmapAmendment(Box::new(RoadmapAmendmentNotificationPayload {
        kind: RoadmapAmendmentNotificationKind::FollowUpRunCreated,
        patch_id: request.patch_id.clone(),
        target: request.target.clone(),
        run_id: target_run_id(&request.target),
        follow_up_run_id: Some(request.run_id),
        status: Some(RoadmapPatchStatus::Approved),
        summary: "Follow-up run created for roadmap amendment".into(),
        detail: Some(format!("graph_hash={}", request.graph_hash)),
        conflict_codes: Vec::new(),
        conflict_choices: Vec::new(),
        artifact_paths: request
            .run_config
            .seed_artifacts
            .iter()
            .map(|seed| seed.relative_path.clone())
            .collect(),
    }))
}

/// Build a notification message for conflicts requiring operator choice.
#[must_use]
pub fn patch_conflict_notification(
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    conflicts: &[RoadmapPatchConflict],
) -> NotifyMessage {
    let mut choices = Vec::new();
    for conflict in conflicts {
        choices.extend(conflict.choices.iter().copied());
    }
    choices.sort_by_key(|choice| conflict_choice_label(*choice));
    choices.dedup();
    NotifyMessage::RoadmapAmendment(Box::new(RoadmapAmendmentNotificationPayload {
        kind: RoadmapAmendmentNotificationKind::ConflictDetected,
        patch_id: patch_id.clone(),
        target: target.clone(),
        run_id: target_run_id(target),
        follow_up_run_id: None,
        status: None,
        summary: "Roadmap patch has conflicts".into(),
        detail: Some(
            conflicts
                .iter()
                .map(|conflict| conflict.message.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        conflict_codes: conflicts
            .iter()
            .map(|conflict| format!("{:?}", conflict.code))
            .collect(),
        conflict_choices: choices,
        artifact_paths: Vec::new(),
    }))
}

/// Build a notification message for a rejected patch.
#[must_use]
pub fn patch_rejected_notification(
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    comment: Option<&str>,
) -> NotifyMessage {
    NotifyMessage::RoadmapAmendment(Box::new(RoadmapAmendmentNotificationPayload {
        kind: RoadmapAmendmentNotificationKind::PatchRejected,
        patch_id: patch_id.clone(),
        target: target.clone(),
        run_id: target_run_id(target),
        follow_up_run_id: None,
        status: Some(RoadmapPatchStatus::Rejected),
        summary: "Roadmap patch rejected".into(),
        detail: comment.map(str::to_owned),
        conflict_codes: Vec::new(),
        conflict_choices: Vec::new(),
        artifact_paths: Vec::new(),
    }))
}

/// Store a drafted roadmap patch and append `RoadmapPatchDrafted`.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when artifact storage or event append
/// fails.
#[must_use = "await the store operation and handle failures"]
pub async fn store_patch_draft(
    artifact_store: &ArtifactStore,
    writer: &RunWriter,
    run_id: RunId,
    patch: &RoadmapPatch,
    patch_bytes: &[u8],
) -> Result<surge_core::run_state::ArtifactRef, RoadmapAmendmentError> {
    let artifact = artifact_store
        .put(run_id, ROADMAP_PATCH_ARTIFACT_NAME, patch_bytes)
        .await?;
    tracing::info!(
        target: "roadmap_amendment",
        run_id = %run_id,
        patch_id = %patch.id,
        hash = %artifact.hash,
        path = %artifact.path.display(),
        "roadmap_patch_stored"
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::RoadmapPatchDrafted {
                patch_id: patch.id.clone(),
                target: patch.target.clone(),
                patch_artifact: artifact.hash,
                patch_path: artifact.path.clone(),
            },
        ))
        .await?;
    Ok(artifact)
}

/// Store amended roadmap/flow artifacts and append `RoadmapPatchApplied`.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when artifact storage or event append
/// fails.
#[must_use = "await the store operation and handle failures"]
pub async fn store_applied_artifacts(
    artifact_store: &ArtifactStore,
    writer: &RunWriter,
    run_id: RunId,
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    roadmap_bytes: &[u8],
    flow_bytes: Option<&[u8]>,
) -> Result<StoredAmendmentArtifacts, RoadmapAmendmentError> {
    let roadmap = artifact_store
        .put(run_id, AMENDED_ROADMAP_ARTIFACT_NAME, roadmap_bytes)
        .await?;
    let flow = match flow_bytes {
        Some(bytes) => Some(
            artifact_store
                .put(run_id, AMENDED_FLOW_ARTIFACT_NAME, bytes)
                .await?,
        ),
        None => None,
    };
    tracing::info!(
        target: "roadmap_amendment",
        run_id = %run_id,
        patch_id = %patch_id,
        roadmap_hash = %roadmap.hash,
        flow_hash = flow.as_ref().map(|artifact| artifact.hash.to_string()).as_deref(),
        "roadmap_amendment_artifacts_stored"
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::RoadmapPatchApplied {
                patch_id: patch_id.clone(),
                target: target.clone(),
                amended_roadmap_artifact: roadmap.hash,
                amended_roadmap_path: roadmap.path.clone(),
                amended_flow_artifact: flow.as_ref().map(|artifact| artifact.hash),
                amended_flow_path: flow.as_ref().map(|artifact| artifact.path.clone()),
            },
        ))
        .await?;
    Ok(StoredAmendmentArtifacts { roadmap, flow })
}

/// Append `RoadmapUpdated` after an amended roadmap/flow has become visible.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when the event append fails.
#[must_use = "await the update record and handle failures"]
pub async fn record_roadmap_updated(
    writer: &RunWriter,
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    artifacts: &StoredAmendmentArtifacts,
    active_pickup: ActivePickupPolicy,
) -> Result<(), RoadmapAmendmentError> {
    writer
        .append_event(VersionedEventPayload::new(EventPayload::RoadmapUpdated {
            patch_id: patch_id.clone(),
            target: target.clone(),
            roadmap_artifact: artifacts.roadmap.hash,
            roadmap_path: artifacts.roadmap.path.clone(),
            flow_artifact: artifacts.flow.as_ref().map(|artifact| artifact.hash),
            flow_path: artifacts
                .flow
                .as_ref()
                .map(|artifact| artifact.path.clone()),
            active_pickup,
        }))
        .await?;
    Ok(())
}

/// Append `GraphRevisionAccepted` for an active amendment graph update.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when the event append fails.
#[must_use = "await the graph revision record and handle failures"]
pub async fn record_graph_revision_accepted(
    writer: &RunWriter,
    patch_id: &RoadmapPatchId,
    target: &RoadmapPatchTarget,
    previous_graph_hash: ContentHash,
    graph: &Graph,
    graph_hash: ContentHash,
    active_pickup: ActivePickupPolicy,
) -> Result<(), RoadmapAmendmentError> {
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %patch_id,
        previous_graph_hash = %previous_graph_hash,
        graph_hash = %graph_hash,
        active_pickup = ?active_pickup,
        "graph_revision_accepted"
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::GraphRevisionAccepted {
                patch_id: patch_id.clone(),
                target: target.clone(),
                previous_graph_hash,
                graph: Box::new(graph.clone()),
                graph_hash,
                active_pickup,
            },
        ))
        .await?;
    Ok(())
}

/// Append `RoadmapPatchApprovalRequested` and return a renderable prompt.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when the event append fails.
pub async fn request_patch_approval(
    writer: &RunWriter,
    patch: &RoadmapPatch,
    channel: ApprovalChannel,
) -> Result<RoadmapPatchApprovalPrompt, RoadmapAmendmentError> {
    let summary = render_patch_approval_summary(patch);
    let summary_hash = ContentHash::compute(summary.as_bytes());
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %patch.id,
        channel = channel.kind().as_str(),
        summary_hash = %summary_hash,
        "roadmap_patch_approval_requested"
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::RoadmapPatchApprovalRequested {
                patch_id: patch.id.clone(),
                target: patch.target.clone(),
                channel,
                summary_hash,
            },
        ))
        .await?;
    Ok(RoadmapPatchApprovalPrompt {
        patch_id: patch.id.clone(),
        target: patch.target.clone(),
        summary,
        summary_hash,
        schema: approval_response_schema(),
    })
}

/// Append `RoadmapPatchApprovalDecided`.
///
/// # Errors
/// Returns [`RoadmapAmendmentError`] when the event append fails.
pub async fn record_patch_approval_decision(
    writer: &RunWriter,
    patch_id: &RoadmapPatchId,
    channel_used: ApprovalChannelKind,
    resolution: &RoadmapPatchApprovalResolution,
) -> Result<(), RoadmapAmendmentError> {
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %patch_id,
        decision = approval_decision_label(resolution.decision),
        channel = channel_used.as_str(),
        conflict_choice = resolution.conflict_choice.map(conflict_choice_label),
        "roadmap_patch_approval_decided"
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::RoadmapPatchApprovalDecided {
                patch_id: patch_id.clone(),
                decision: resolution.decision,
                channel_used,
                comment: resolution.comment.clone(),
                conflict_choice: resolution.conflict_choice,
            },
        ))
        .await?;
    Ok(())
}

/// Render a concise operator-facing approval summary.
#[must_use]
pub fn render_patch_approval_summary(patch: &RoadmapPatch) -> String {
    let mut summary = String::new();
    summary.push_str(&format!("Roadmap patch: {}\n", patch.id));
    summary.push_str(&format!("Target: {}\n", target_label(&patch.target)));
    if !patch.rationale.trim().is_empty() {
        summary.push_str(&format!("Rationale: {}\n", patch.rationale.trim()));
    }
    push_operations_summary(&mut summary, &patch.operations);
    push_dependencies_summary(&mut summary, &patch.dependencies);
    push_conflicts_summary(&mut summary, &patch.conflicts);
    summary
}

fn approval_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "decision": {
                "type": "string",
                "enum": [APPROVE_DECISION, EDIT_DECISION, REJECT_DECISION],
            },
            "comment": { "type": "string" },
            "conflict_choice": {
                "type": "string",
                "enum": [
                    "defer_to_next_milestone",
                    "abort_current_run",
                    "create_follow_up_run",
                    "reject_patch",
                ],
            },
        },
        "required": ["decision"],
    })
}

fn edit_feedback(
    patch_id: &RoadmapPatchId,
    comment: Option<&str>,
) -> Result<String, RoadmapAmendmentError> {
    let feedback = comment.map(str::trim).filter(|value| !value.is_empty());
    match feedback {
        Some(value) => Ok(value.to_owned()),
        None => Err(RoadmapAmendmentError::MissingEditFeedback {
            patch_id: patch_id.clone(),
        }),
    }
}

async fn append_edit_loop_escalation(
    writer: &RunWriter,
    patch_id: &RoadmapPatchId,
    max_edit_rounds: u32,
) -> Result<(), RoadmapAmendmentError> {
    let reason = format!(
        "roadmap patch approval edit-loop exceeded for {patch_id} (cap = {max_edit_rounds})"
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::EscalationRequested {
                stage: None,
                reason,
            },
        ))
        .await?;
    Ok(())
}

fn push_operations_summary(summary: &mut String, operations: &[RoadmapPatchOperation]) {
    if operations.is_empty() {
        summary.push_str("Operations: none\n");
        return;
    }
    summary.push_str("Operations:\n");
    for operation in operations {
        summary.push_str("- ");
        summary.push_str(&operation_label(operation));
        summary.push('\n');
    }
}

fn push_dependencies_summary(summary: &mut String, dependencies: &[RoadmapPatchDependency]) {
    if dependencies.is_empty() {
        return;
    }
    summary.push_str("Dependencies:\n");
    for dependency in dependencies {
        summary.push_str("- ");
        summary.push_str(&dependency_label(dependency));
        summary.push('\n');
    }
}

fn push_conflicts_summary(summary: &mut String, conflicts: &[RoadmapPatchConflict]) {
    if conflicts.is_empty() {
        return;
    }
    summary.push_str("Conflicts:\n");
    for conflict in conflicts {
        summary.push_str("- ");
        summary.push_str(&format!("{:?}: {}", conflict.code, conflict.message));
        if !conflict.choices.is_empty() {
            let choices = conflict
                .choices
                .iter()
                .map(|choice| conflict_choice_label(*choice))
                .collect::<Vec<_>>()
                .join(", ");
            summary.push_str(&format!(" (choices: {choices})"));
        }
        summary.push('\n');
    }
}

fn target_run_id(target: &RoadmapPatchTarget) -> Option<RunId> {
    match target {
        RoadmapPatchTarget::ProjectRoadmap { .. } => None,
        RoadmapPatchTarget::RunRoadmap { run_id, .. } => Some(*run_id),
    }
}

fn applied_artifact_paths(artifacts: &StoredAmendmentArtifacts) -> Vec<std::path::PathBuf> {
    let mut paths = vec![artifacts.roadmap.path.clone()];
    if let Some(flow) = &artifacts.flow {
        paths.push(flow.path.clone());
    }
    paths
}

fn pending_follow_up_milestone(milestone: &RoadmapMilestone) -> RoadmapMilestone {
    let mut milestone = milestone.clone();
    milestone.status = RoadmapStatus::Pending;
    for task in &mut milestone.tasks {
        task.status = RoadmapStatus::Pending;
    }
    milestone
}

fn unique_id(base: &str, used: &mut BTreeSet<String>) -> String {
    let base = if base.trim().is_empty() {
        "follow-up".to_owned()
    } else {
        base.to_owned()
    };
    if used.insert(base.clone()) {
        return base;
    }

    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base}-{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn follow_up_roadmap_seed_markdown(
    patch_id: &RoadmapPatchId,
    patch_result: &RoadmapPatchApplyResult,
) -> String {
    let mut out = String::new();
    out.push_str("# Follow-up Roadmap Amendment\n\n");
    out.push_str(&format!("Patch: {patch_id}\n\n"));

    if !patch_result.inserted_milestones.is_empty() {
        out.push_str("## Inserted milestones\n");
        for milestone_id in &patch_result.inserted_milestones {
            let title = patch_result
                .roadmap
                .milestones
                .iter()
                .find(|milestone| milestone.id == milestone_id.as_str())
                .map(|milestone| milestone.title.as_str())
                .unwrap_or("");
            if title.is_empty() {
                out.push_str(&format!("- {milestone_id}\n"));
            } else {
                out.push_str(&format!("- {milestone_id}: {title}\n"));
            }
        }
        out.push('\n');
    }

    if !patch_result.inserted_tasks.is_empty() {
        out.push_str("## Inserted tasks\n");
        for task_ref in &patch_result.inserted_tasks {
            out.push_str("- ");
            out.push_str(&task_seed_label(patch_result, task_ref));
            out.push('\n');
        }
        out.push('\n');
    }

    if !patch_result.replaced_items.is_empty() {
        out.push_str("## Reworked draft items\n");
        for item in &patch_result.replaced_items {
            out.push_str("- ");
            out.push_str(&item_ref_label(item));
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("## Amended roadmap\n\n");
    out.push_str(&patch_result.markdown);
    out
}

fn task_seed_label(patch_result: &RoadmapPatchApplyResult, task_ref: &RoadmapItemRef) -> String {
    let RoadmapItemRef::Task {
        milestone_id,
        task_id,
    } = task_ref
    else {
        return item_ref_label(task_ref);
    };
    let title = patch_result
        .roadmap
        .milestones
        .iter()
        .find(|milestone| milestone.id == milestone_id.as_str())
        .and_then(|milestone| {
            milestone
                .tasks
                .iter()
                .find(|task| task.id == task_id.as_str())
                .map(|task| task.title.as_str())
        })
        .unwrap_or("");
    if title.is_empty() {
        format!("{milestone_id}/{task_id}")
    } else {
        format!("{milestone_id}/{task_id}: {title}")
    }
}

fn operation_label(operation: &RoadmapPatchOperation) -> String {
    match operation {
        RoadmapPatchOperation::AddMilestone {
            milestone,
            insertion,
        } => format!(
            "add milestone {} ({}) at {}",
            milestone.id,
            milestone.title,
            insertion_label(insertion.as_ref())
        ),
        RoadmapPatchOperation::AddTask {
            milestone_id,
            task,
            insertion,
        } => format!(
            "add task {} ({}) to milestone {} at {}",
            task.id,
            task.title,
            milestone_id,
            insertion_label(insertion.as_ref())
        ),
        RoadmapPatchOperation::ReplaceDraftItem {
            target,
            replacement,
            reason,
        } => {
            let reason = reason.trim();
            if reason.is_empty() {
                format!(
                    "replace draft {} with {}",
                    item_ref_label(target),
                    replacement_label(replacement)
                )
            } else {
                format!(
                    "replace draft {} with {} because {}",
                    item_ref_label(target),
                    replacement_label(replacement),
                    reason
                )
            }
        },
    }
}

fn dependency_label(dependency: &RoadmapPatchDependency) -> String {
    let reason = dependency.reason.trim();
    if reason.is_empty() {
        format!(
            "{} -> {}",
            item_ref_label(&dependency.from),
            item_ref_label(&dependency.to)
        )
    } else {
        format!(
            "{} -> {} because {}",
            item_ref_label(&dependency.from),
            item_ref_label(&dependency.to),
            reason
        )
    }
}

fn target_label(target: &RoadmapPatchTarget) -> String {
    match target {
        RoadmapPatchTarget::ProjectRoadmap { roadmap_path } => {
            format!("project roadmap {roadmap_path}")
        },
        RoadmapPatchTarget::RunRoadmap {
            run_id,
            roadmap_artifact,
            flow_artifact,
            active_pickup,
        } => format!(
            "run {run_id} roadmap={:?} flow={:?} pickup={}",
            roadmap_artifact,
            flow_artifact,
            pickup_label(*active_pickup)
        ),
    }
}

fn insertion_label(insertion: Option<&InsertionPoint>) -> String {
    match insertion {
        Some(InsertionPoint::AppendToRoadmap) => "append to roadmap".to_owned(),
        Some(InsertionPoint::BeforeMilestone { milestone_id }) => {
            format!("before milestone {milestone_id}")
        },
        Some(InsertionPoint::AfterMilestone { milestone_id }) => {
            format!("after milestone {milestone_id}")
        },
        Some(InsertionPoint::AppendToMilestone { milestone_id }) => {
            format!("append to milestone {milestone_id}")
        },
        Some(InsertionPoint::BeforeTask {
            milestone_id,
            task_id,
        }) => {
            format!("before task {milestone_id}/{task_id}")
        },
        Some(InsertionPoint::AfterTask {
            milestone_id,
            task_id,
        }) => {
            format!("after task {milestone_id}/{task_id}")
        },
        None => "unspecified".to_owned(),
    }
}

fn item_ref_label(reference: &RoadmapItemRef) -> String {
    match reference {
        RoadmapItemRef::Milestone { milestone_id } => format!("milestone {milestone_id}"),
        RoadmapItemRef::Task {
            milestone_id,
            task_id,
        } => {
            format!("task {milestone_id}/{task_id}")
        },
    }
}

fn replacement_label(replacement: &RoadmapPatchItem) -> String {
    match replacement {
        RoadmapPatchItem::Milestone { milestone } => {
            format!("milestone {} ({})", milestone.id, milestone.title)
        },
        RoadmapPatchItem::Task { task } => format!("task {} ({})", task.id, task.title),
    }
}

const fn approval_decision_label(decision: RoadmapPatchApprovalDecision) -> &'static str {
    match decision {
        RoadmapPatchApprovalDecision::Approve => APPROVE_DECISION,
        RoadmapPatchApprovalDecision::Edit => EDIT_DECISION,
        RoadmapPatchApprovalDecision::Reject => REJECT_DECISION,
    }
}

const fn conflict_choice_label(choice: OperatorConflictChoice) -> &'static str {
    match choice {
        OperatorConflictChoice::DeferToNextMilestone => "defer_to_next_milestone",
        OperatorConflictChoice::AbortCurrentRun => "abort_current_run",
        OperatorConflictChoice::CreateFollowUpRun => "create_follow_up_run",
        OperatorConflictChoice::RejectPatch => "reject_patch",
    }
}

const fn pickup_label(policy: ActivePickupPolicy) -> &'static str {
    match policy {
        ActivePickupPolicy::Allowed => "allowed",
        ActivePickupPolicy::FollowUpOnly => "follow_up_only",
        ActivePickupPolicy::Disabled => "disabled",
    }
}
