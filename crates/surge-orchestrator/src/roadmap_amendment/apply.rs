//! Apply an approved roadmap patch (#103): the one place that decides where
//! an approved patch lands and records how it got there.
//!
//! # Routing
//! The route is decided before anything is written, from the patch, the
//! operator's approval and the target's *current* roadmap:
//!
//! | Situation | Route | Index status |
//! |---|---|---|
//! | malformed patch shape | refused, nothing recorded | unchanged |
//! | conflicts, no choice | refused (lists choices), nothing recorded | unchanged |
//! | choice no conflict offers | refused, nothing recorded | unchanged |
//! | `reject_patch` | reject decision with the choice | rejected |
//! | `create_follow_up_run` | follow-up run seeded with the new work only | applied |
//! | `abort_current_run` | approved, not applied: abort required | approved |
//! | `defer_to_next_milestone` | patch rewritten past running milestones, then by amendment point | per amendment point |
//! | no conflicts, project file | roadmap file rewritten, amended roadmap stored | applied |
//! | no conflicts, active run | submitted to the engine's amendment queue | applied |
//! | no conflicts, follow-up run | follow-up run started | applied |
//! | no conflicts, deferred target | approved, not applied: not at a pickup boundary | approved |
//!
//! # Event order
//! On the owning run's log: `RoadmapPatchApprovalRequested` (unless already
//! recorded) → `RoadmapPatchApprovalDecided` (unless an approval is already
//! recorded) → the route's events (`RoadmapPatchApplied` → `RoadmapUpdated`
//! for the project file). An active run's amendment events are written by
//! the engine on the target run's log. The patch index is updated after the
//! log, so it stays a rebuildable projection; a failure after the decision
//! leaves the index at `approved`, never `applied`.
//!
//! # Re-entrancy
//! Applying an already-approved patch again reads the recorded approval
//! instead of recording a second one and re-checks against the current
//! roadmap and target, so an approved-but-not-applied patch can be applied
//! once the target allows it. An applied or rejected patch is refused.

use std::path::PathBuf;
use std::sync::Arc;

use surge_core::approvals::{ApprovalChannel, ApprovalChannelKind};
use surge_core::roadmap_patch::{
    ActivePickupPolicy, OperatorConflictChoice, RoadmapPatchApplyError, RoadmapPatchApplyResult,
    RoadmapPatchApprovalDecision, RoadmapPatchConflict, RoadmapPatchStatus, RoadmapPatchTarget,
};
use surge_core::run_event::EventPayload;
use surge_core::{ContentHash, RoadmapArtifact, RoadmapPatch, RunId};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::roadmap_patches::RoadmapPatchIndexUpsert;
use surge_persistence::runs::{RunWriter, Storage};

use super::{
    ActiveRunAmendmentOutcome, RoadmapAmendmentError, RoadmapPatchApprovalResolution,
    build_follow_up_run_request, conflict_choice_label, follow_up_result_from_patch,
    record_patch_approval_decision, record_roadmap_updated, request_patch_approval,
    start_follow_up_run, store_applied_artifacts,
};
use crate::engine::config::ProjectContextSeed;
use crate::engine::facade::EngineFacade;
use crate::roadmap_target::RoadmapAmendmentPoint;

/// A drafted patch checked against a roadmap: planner-declared conflicts
/// merged with the ones detected now, and the conflict choices offered.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchAssessment {
    /// The patch with merged conflicts.
    pub patch: RoadmapPatch,
    /// Conflict choices offered by any conflict, deduplicated and sorted.
    pub choices: Vec<OperatorConflictChoice>,
    /// Why the patch shape is invalid, when it is.
    pub malformed: Option<String>,
}

/// Merge `patch`'s declared conflicts with those `roadmap` produces now.
/// A detected conflict fills an empty message and empty choices of a
/// declared one with the same code and item; others are appended.
#[must_use]
pub fn assess_patch(patch: &RoadmapPatch, roadmap: &RoadmapArtifact) -> PatchAssessment {
    let mut merged = patch.clone();
    let mut malformed = None;
    match patch.apply_to_roadmap(roadmap) {
        Ok(_) => {},
        Err(RoadmapPatchApplyError::InvalidShape { issues }) => {
            malformed = Some(
                issues
                    .iter()
                    .map(|issue| format!("{} at {}", issue.code.as_str(), issue.location))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        },
        Err(RoadmapPatchApplyError::Conflicts { conflicts }) => merge_conflicts(
            &mut merged,
            conflicts
                .iter()
                .map(|conflict| conflict.to_patch_conflict()),
        ),
    }
    let choices = offered_choices(&merged.conflicts);
    PatchAssessment {
        patch: merged,
        choices,
        malformed,
    }
}

/// The operator's approval of a patch.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchApproval {
    /// How to resolve conflicts, when the patch has any.
    pub conflict_choice: Option<OperatorConflictChoice>,
    /// Optional operator comment.
    pub comment: Option<String>,
    /// Channel the approval request is recorded on, when it was not yet.
    pub channel: ApprovalChannel,
    /// Channel the decision came through.
    pub channel_used: ApprovalChannelKind,
}

/// Where the patch is meant to land, as resolved now.
#[derive(Debug, Clone)]
pub struct PatchApplyTarget {
    /// The patch target.
    pub target: RoadmapPatchTarget,
    /// Safe amendment point implied by the target's current state.
    pub amendment_point: RoadmapAmendmentPoint,
    /// The roadmap file.
    pub roadmap_path: PathBuf,
    /// The roadmap file's current text.
    pub roadmap_text: String,
    /// The active run, for an active-run target.
    pub active_run: Option<RunId>,
    /// The project the patch belongs to.
    pub project_path: PathBuf,
    /// Worktree a follow-up run starts in.
    pub worktree: PathBuf,
    /// Project context seeded into a follow-up run.
    pub project_context: Option<ProjectContextSeed>,
}

/// Supplies the engine only when a route needs it (starting a daemon is not
/// free).
#[async_trait::async_trait]
pub trait EngineSource: Send + Sync {
    /// The engine facade to start follow-up runs and submit active-run
    /// amendments through.
    ///
    /// # Errors
    /// Why the engine is unavailable.
    async fn engine(&self) -> Result<Arc<dyn EngineFacade>, crate::engine::error::EngineError>;
}

#[async_trait::async_trait]
impl EngineSource for Arc<dyn EngineFacade> {
    async fn engine(&self) -> Result<Arc<dyn EngineFacade>, crate::engine::error::EngineError> {
        Ok(Arc::clone(self))
    }
}

/// What applying a patch needs besides the patch.
pub struct PatchApplyPorts<'a> {
    /// Content-addressed artifact store.
    pub artifact_store: &'a ArtifactStore,
    /// Run storage: the owning run's log and the patch index.
    pub storage: &'a Arc<Storage>,
    /// Writer of the run whose log records the patch lifecycle.
    pub writer: &'a RunWriter,
    /// That run.
    pub owner_run: RunId,
    /// The engine, for follow-up and active-run routes.
    pub engine: &'a dyn EngineSource,
}

/// Why an approved patch was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAppliedReason {
    /// The operator chose to abort the current run first; stop it, then
    /// apply again (it then routes to a follow-up run).
    AbortRequired {
        /// The run to stop, when the target names one.
        run_id: Option<RunId>,
    },
    /// The target is not at a pickup boundary yet; apply again once it is.
    TargetNotAtPickupBoundary,
}

/// Where an approved patch went.
#[derive(Debug, Clone, PartialEq)]
pub enum PatchApplyOutcome {
    /// The project roadmap file was amended.
    AppliedToProjectRoadmap {
        /// Stored amended roadmap artifact.
        roadmap_artifact: ContentHash,
    },
    /// An active run accepted the amendment at its next safe boundary.
    AppliedToActiveRun(ActiveRunAmendmentOutcome),
    /// A follow-up run was started with the new work.
    FollowUpStarted {
        /// The follow-up run.
        run_id: RunId,
        /// Whether a conflict choice routed it here (rather than the
        /// target's amendment point).
        from_conflict_choice: bool,
    },
    /// Approved, but not applied.
    ApprovedNotApplied {
        /// Why.
        reason: NotAppliedReason,
    },
    /// Rejected by the operator's conflict choice.
    Rejected {
        /// The choice that rejected it.
        conflict_choice: OperatorConflictChoice,
    },
}

impl PatchApplyOutcome {
    /// The patch status this outcome records in the index.
    #[must_use]
    pub fn status(&self) -> RoadmapPatchStatus {
        match self {
            Self::AppliedToProjectRoadmap { .. }
            | Self::AppliedToActiveRun(_)
            | Self::FollowUpStarted { .. } => RoadmapPatchStatus::Applied,
            Self::ApprovedNotApplied { .. } => RoadmapPatchStatus::Approved,
            Self::Rejected { .. } => RoadmapPatchStatus::Rejected,
        }
    }
}

/// Why a patch cannot be applied as asked; nothing was recorded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchApplyRefusal {
    /// The patch shape is invalid.
    #[error("roadmap patch shape is invalid: {issues}")]
    MalformedShape {
        /// The shape issues.
        issues: String,
    },
    /// The patch has conflicts and no choice was given.
    #[error("roadmap patch has conflicts; choose one of: {}", choice_list(choices))]
    ConflictChoiceRequired {
        /// The choices the conflicts offer.
        choices: Vec<OperatorConflictChoice>,
    },
    /// No conflict offers the given choice.
    #[error("conflict choice {} is not available for this patch", conflict_choice_label(*choice))]
    ConflictChoiceUnavailable {
        /// The refused choice.
        choice: OperatorConflictChoice,
    },
    /// Deferral left the patch conflicting.
    #[error("roadmap patch still conflicts after deferring to the next milestone")]
    DeferralStillConflicts,
    /// An active-run target without a run id.
    #[error("active-run amendment target is missing its run id")]
    ActiveRunWithoutRunId,
    /// The patch was already applied.
    #[error("roadmap patch was already applied")]
    AlreadyApplied,
    /// The patch was already rejected.
    #[error("roadmap patch was already rejected")]
    AlreadyRejected,
}

/// Failure of [`apply_approved_patch`].
#[derive(Debug, thiserror::Error)]
pub enum PatchApplyError {
    /// The patch cannot be applied as asked; nothing was recorded.
    #[error(transparent)]
    Refused(#[from] PatchApplyRefusal),
    /// A store, log, roadmap file or engine step failed; if the decision was
    /// recorded, the index says `approved`.
    #[error(transparent)]
    Failed(#[from] RoadmapAmendmentError),
}

impl From<surge_persistence::runs::StorageError> for PatchApplyError {
    fn from(error: surge_persistence::runs::StorageError) -> Self {
        Self::Failed(RoadmapAmendmentError::Storage(error))
    }
}

/// The route decided before anything is written.
enum Route {
    Reject(OperatorConflictChoice),
    Approve(Box<Approved>),
}

/// Where an approved patch goes.
enum Approved {
    AbortRequired,
    FollowUp(RoadmapPatchApplyResult),
    ByAmendmentPoint(RoadmapPatchApplyResult),
}

/// What the owning run's log already says about the patch.
#[derive(Default)]
struct Recorded {
    requested: bool,
    decision: Option<(RoadmapPatchApprovalDecision, Option<OperatorConflictChoice>)>,
    applied: bool,
}

/// Apply an approved roadmap patch (see the module docs for the routing
/// table, event order and re-entrancy).
///
/// # Errors
/// [`PatchApplyError::Refused`] when the patch cannot be applied as asked
/// (nothing recorded), [`PatchApplyError::Failed`] when a step fails.
#[must_use = "the outcome says where the patch went"]
pub async fn apply_approved_patch(
    ports: PatchApplyPorts<'_>,
    patch: &RoadmapPatch,
    approval: PatchApproval,
    target: &PatchApplyTarget,
) -> Result<PatchApplyOutcome, PatchApplyError> {
    let recorded = recorded(ports.storage, ports.owner_run, patch).await?;
    let indexed = ports.storage.roadmap_patch_store().get(&patch.id)?;
    if recorded.applied
        || indexed
            .as_ref()
            .is_some_and(|record| record.status == RoadmapPatchStatus::Applied)
    {
        return Err(PatchApplyRefusal::AlreadyApplied.into());
    }
    if matches!(
        recorded.decision,
        Some((RoadmapPatchApprovalDecision::Reject, _))
    ) {
        return Err(PatchApplyRefusal::AlreadyRejected.into());
    }
    let approved = matches!(
        recorded.decision,
        Some((RoadmapPatchApprovalDecision::Approve, _))
    );
    let choice = approval
        .conflict_choice
        .or_else(|| recorded.decision.and_then(|(_, choice)| choice));

    let parsed =
        crate::roadmap_document::parse_roadmap_document(&target.roadmap_path, &target.roadmap_text)
            .map_err(RoadmapAmendmentError::from)?;
    let assessment = assess_patch(patch, &parsed.roadmap);
    let route = route(patch, &parsed.roadmap, &assessment, choice)?;
    if matches!(&route, Route::Approve(approved) if matches!(**approved, Approved::ByAmendmentPoint(_)))
        && target.amendment_point == RoadmapAmendmentPoint::ActiveRunBoundary
        && target.active_run.is_none()
    {
        return Err(PatchApplyRefusal::ActiveRunWithoutRunId.into());
    }

    if !recorded.requested {
        request_patch_approval(ports.writer, &assessment.patch, approval.channel.clone()).await?;
    }
    let approved_route = match route {
        Route::Approve(approved_route) => *approved_route,
        Route::Reject(choice) => {
            let comment = approval
                .comment
                .clone()
                .or_else(|| Some("rejected by conflict resolution".into()));
            record_patch_approval_decision(
                ports.writer,
                &patch.id,
                approval.channel_used,
                &RoadmapPatchApprovalResolution {
                    decision: RoadmapPatchApprovalDecision::Reject,
                    comment: comment.clone(),
                    conflict_choice: Some(choice),
                },
            )
            .await?;
            index(
                &ports,
                patch,
                target,
                RoadmapPatchStatus::Rejected,
                RoadmapPatchApprovalDecision::Reject,
                comment,
                Some(choice),
            )?;
            return Ok(PatchApplyOutcome::Rejected {
                conflict_choice: choice,
            });
        },
    };
    if !approved {
        record_patch_approval_decision(
            ports.writer,
            &patch.id,
            approval.channel_used,
            &RoadmapPatchApprovalResolution {
                decision: RoadmapPatchApprovalDecision::Approve,
                comment: approval.comment.clone(),
                conflict_choice: choice,
            },
        )
        .await?;
    }
    let approve = |status| {
        index(
            &ports,
            patch,
            target,
            status,
            RoadmapPatchApprovalDecision::Approve,
            approval.comment.clone(),
            choice,
        )
    };
    approve(RoadmapPatchStatus::Approved)?;

    let outcome = match approved_route {
        Approved::AbortRequired => PatchApplyOutcome::ApprovedNotApplied {
            reason: NotAppliedReason::AbortRequired {
                run_id: target.active_run,
            },
        },
        Approved::FollowUp(result) => follow_up(&ports, patch, target, &result, true).await?,
        Approved::ByAmendmentPoint(result) => match target.amendment_point {
            RoadmapAmendmentPoint::ProjectFile => {
                project_file(&ports, patch, target, &parsed, &result).await?
            },
            RoadmapAmendmentPoint::ActiveRunBoundary => {
                let run_id = target
                    .active_run
                    .ok_or(PatchApplyRefusal::ActiveRunWithoutRunId)?;
                let engine = ports
                    .engine
                    .engine()
                    .await
                    .map_err(RoadmapAmendmentError::from)?;
                let outcome = engine
                    .submit_roadmap_amendment(
                        run_id,
                        patch.id.clone(),
                        target.target.clone(),
                        result,
                    )
                    .await
                    .map_err(RoadmapAmendmentError::from)?;
                PatchApplyOutcome::AppliedToActiveRun(outcome)
            },
            RoadmapAmendmentPoint::FollowUpRun => {
                follow_up(&ports, patch, target, &result, false).await?
            },
            RoadmapAmendmentPoint::Deferred => PatchApplyOutcome::ApprovedNotApplied {
                reason: NotAppliedReason::TargetNotAtPickupBoundary,
            },
        },
    };
    if outcome.status() != RoadmapPatchStatus::Approved {
        approve(outcome.status())?;
    }
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %patch.id,
        status = ?outcome.status(),
        "roadmap_patch_apply_finished"
    );
    Ok(outcome)
}

/// Record an operator's rejection of a patch that was not applied: the
/// decision on the owning run's log, then the index.
///
/// # Errors
/// [`PatchApplyRefusal::AlreadyApplied`] for an applied patch;
/// [`PatchApplyError::Failed`] when the log or index write fails.
#[must_use = "the rejection may fail"]
pub async fn reject_patch(
    storage: &Arc<Storage>,
    patch_id: &surge_core::roadmap_patch::RoadmapPatchId,
    comment: Option<String>,
    conflict_choice: Option<OperatorConflictChoice>,
    channel_used: ApprovalChannelKind,
) -> Result<surge_persistence::roadmap_patches::RoadmapPatchIndexRecord, PatchApplyError> {
    let record = storage
        .roadmap_patch_store()
        .get(patch_id)?
        .ok_or_else(|| RoadmapAmendmentError::UnknownPatch(patch_id.clone()))?;
    if record.status == RoadmapPatchStatus::Applied {
        return Err(PatchApplyRefusal::AlreadyApplied.into());
    }
    let owner = record
        .run_id
        .ok_or_else(|| RoadmapAmendmentError::NoOwningRun(patch_id.clone()))?;
    let writer = storage.open_run_writer(owner).await.map_err(|error| {
        RoadmapAmendmentError::OpenOwningRun {
            run_id: owner,
            reason: error.to_string(),
        }
    })?;
    let recorded = record_patch_approval_decision(
        &writer,
        patch_id,
        channel_used,
        &RoadmapPatchApprovalResolution {
            decision: RoadmapPatchApprovalDecision::Reject,
            comment: comment.clone(),
            conflict_choice,
        },
    )
    .await;
    let closed = writer.close().await;
    recorded?;
    closed.map_err(|error| RoadmapAmendmentError::OpenOwningRun {
        run_id: owner,
        reason: error.to_string(),
    })?;
    storage
        .roadmap_patch_store()
        .reject(
            patch_id,
            comment.as_deref(),
            conflict_choice,
            chrono::Utc::now().timestamp_millis(),
        )?
        .ok_or_else(|| RoadmapAmendmentError::UnknownPatch(patch_id.clone()).into())
}

fn route(
    patch: &RoadmapPatch,
    roadmap: &RoadmapArtifact,
    assessment: &PatchAssessment,
    choice: Option<OperatorConflictChoice>,
) -> Result<Route, PatchApplyRefusal> {
    if let Some(issues) = &assessment.malformed {
        return Err(PatchApplyRefusal::MalformedShape {
            issues: issues.clone(),
        });
    }
    if let Some(choice) = choice
        && !assessment.choices.contains(&choice)
    {
        return Err(PatchApplyRefusal::ConflictChoiceUnavailable { choice });
    }
    let detected = match patch.apply_to_roadmap(roadmap) {
        Ok(result) => {
            return Ok(match choice {
                Some(OperatorConflictChoice::RejectPatch) => {
                    Route::Reject(OperatorConflictChoice::RejectPatch)
                },
                _ => Route::Approve(Box::new(Approved::ByAmendmentPoint(result))),
            });
        },
        Err(RoadmapPatchApplyError::InvalidShape { .. }) => {
            return Err(PatchApplyRefusal::MalformedShape {
                issues: "invalid".into(),
            });
        },
        Err(RoadmapPatchApplyError::Conflicts { conflicts }) => conflicts
            .iter()
            .map(|conflict| conflict.to_patch_conflict())
            .collect::<Vec<_>>(),
    };
    let Some(choice) = choice else {
        return Err(PatchApplyRefusal::ConflictChoiceRequired {
            choices: assessment.choices.clone(),
        });
    };
    if !detected
        .iter()
        .any(|conflict| conflict.choices.contains(&choice))
    {
        return Err(PatchApplyRefusal::ConflictChoiceUnavailable { choice });
    }
    tracing::info!(
        target: "roadmap_amendment",
        patch_id = %patch.id,
        conflict_choice = conflict_choice_label(choice),
        "roadmap_patch_conflict_resolution_selected"
    );
    Ok(match choice {
        OperatorConflictChoice::RejectPatch => Route::Reject(choice),
        OperatorConflictChoice::AbortCurrentRun => {
            Route::Approve(Box::new(Approved::AbortRequired))
        },
        OperatorConflictChoice::CreateFollowUpRun => Route::Approve(Box::new(Approved::FollowUp(
            follow_up_result_from_patch(patch),
        ))),
        OperatorConflictChoice::DeferToNextMilestone => {
            let deferred = super::deferral::defer_to_next_milestone(roadmap, patch, &detected);
            Route::Approve(Box::new(Approved::ByAmendmentPoint(
                deferred
                    .apply_to_roadmap(roadmap)
                    .map_err(|_| PatchApplyRefusal::DeferralStillConflicts)?,
            )))
        },
    })
}

async fn follow_up(
    ports: &PatchApplyPorts<'_>,
    patch: &RoadmapPatch,
    target: &PatchApplyTarget,
    result: &RoadmapPatchApplyResult,
    from_conflict_choice: bool,
) -> Result<PatchApplyOutcome, RoadmapAmendmentError> {
    let request = build_follow_up_run_request(
        &patch.id,
        &target.target,
        result,
        &target.worktree,
        target.project_context.clone(),
        chrono::Utc::now(),
    )?;
    let run_id = request.run_id;
    let engine = ports.engine.engine().await?;
    start_follow_up_run(engine.as_ref(), request).await?;
    Ok(PatchApplyOutcome::FollowUpStarted {
        run_id,
        from_conflict_choice,
    })
}

async fn project_file(
    ports: &PatchApplyPorts<'_>,
    patch: &RoadmapPatch,
    target: &PatchApplyTarget,
    parsed: &crate::roadmap_document::ParsedRoadmapDocument,
    result: &RoadmapPatchApplyResult,
) -> Result<PatchApplyOutcome, RoadmapAmendmentError> {
    let amended = crate::roadmap_document::render_amended_roadmap_document(
        &target.roadmap_path,
        &target.roadmap_text,
        parsed,
        result,
    )?;
    tokio::fs::write(&target.roadmap_path, amended.as_bytes())
        .await
        .map_err(|source| RoadmapAmendmentError::RoadmapFile {
            path: target.roadmap_path.clone(),
            source,
        })?;
    let artifacts = store_applied_artifacts(
        ports.artifact_store,
        ports.writer,
        ports.owner_run,
        &patch.id,
        &target.target,
        amended.as_bytes(),
        None,
    )
    .await?;
    record_roadmap_updated(
        ports.writer,
        &patch.id,
        &target.target,
        &artifacts,
        ActivePickupPolicy::FollowUpOnly,
    )
    .await?;
    Ok(PatchApplyOutcome::AppliedToProjectRoadmap {
        roadmap_artifact: artifacts.roadmap.hash,
    })
}

async fn recorded(
    storage: &Arc<Storage>,
    owner: RunId,
    patch: &RoadmapPatch,
) -> Result<Recorded, PatchApplyError> {
    let reader = storage.open_run_reader(owner).await.map_err(|error| {
        RoadmapAmendmentError::OpenOwningRun {
            run_id: owner,
            reason: error.to_string(),
        }
    })?;
    let events = reader.read_run_events().await?;
    drop(reader);
    let mut recorded = Recorded::default();
    for event in events {
        match event.payload {
            EventPayload::RoadmapPatchApprovalRequested { patch_id, .. }
                if patch_id == patch.id =>
            {
                recorded.requested = true;
            },
            EventPayload::RoadmapPatchApprovalDecided {
                patch_id,
                decision,
                conflict_choice,
                ..
            } if patch_id == patch.id => {
                recorded.decision = Some((decision, conflict_choice));
            },
            EventPayload::RoadmapPatchApplied { patch_id, .. } if patch_id == patch.id => {
                recorded.applied = true;
            },
            _ => {},
        }
    }
    Ok(recorded)
}

fn index(
    ports: &PatchApplyPorts<'_>,
    patch: &RoadmapPatch,
    target: &PatchApplyTarget,
    status: RoadmapPatchStatus,
    decision: RoadmapPatchApprovalDecision,
    comment: Option<String>,
    conflict_choice: Option<OperatorConflictChoice>,
) -> Result<(), PatchApplyError> {
    let store = ports.storage.roadmap_patch_store();
    let existing = store.get(&patch.id)?;
    let content_hash = patch
        .content_hash()
        .map_err(|error| RoadmapAmendmentError::PatchHash(error.to_string()))?;
    store.upsert(&RoadmapPatchIndexUpsert {
        patch_id: patch.id.clone(),
        content_hash,
        run_id: Some(ports.owner_run),
        project_path: target.project_path.clone(),
        target: target.target.clone(),
        status,
        patch_artifact: existing.as_ref().and_then(|record| record.patch_artifact),
        patch_path: existing
            .as_ref()
            .and_then(|record| record.patch_path.clone()),
        summary_hash: existing.as_ref().and_then(|record| record.summary_hash),
        decision: Some(decision),
        decision_comment: comment,
        conflict_choice,
        observed_at_ms: chrono::Utc::now().timestamp_millis(),
    })?;
    Ok(())
}

fn merge_conflicts(
    patch: &mut RoadmapPatch,
    detected: impl IntoIterator<Item = RoadmapPatchConflict>,
) {
    for mut conflict in detected {
        match patch
            .conflicts
            .iter_mut()
            .find(|existing| existing.code == conflict.code && existing.item == conflict.item)
        {
            Some(existing) => {
                if existing.message.trim().is_empty() {
                    existing.message = conflict.message;
                }
                if existing.choices.is_empty() {
                    existing.choices = std::mem::take(&mut conflict.choices);
                }
            },
            None => patch.conflicts.push(conflict),
        }
    }
}

fn offered_choices(conflicts: &[RoadmapPatchConflict]) -> Vec<OperatorConflictChoice> {
    let mut choices: Vec<_> = conflicts
        .iter()
        .flat_map(|conflict| conflict.choices.iter().copied())
        .collect();
    choices.sort_by_key(|choice| conflict_choice_label(*choice));
    choices.dedup();
    choices
}

fn choice_list(choices: &[OperatorConflictChoice]) -> String {
    choices
        .iter()
        .map(|choice| conflict_choice_label(*choice))
        .collect::<Vec<_>>()
        .join(", ")
}
