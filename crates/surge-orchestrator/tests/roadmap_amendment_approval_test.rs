use surge_core::RunId;
use surge_core::approvals::{ApprovalChannel, ApprovalDuration};
use surge_core::roadmap::RoadmapMilestone;
use surge_core::roadmap_patch::{
    InsertionPoint, OperatorConflictChoice, RoadmapItemRef, RoadmapPatch,
    RoadmapPatchApprovalDecision, RoadmapPatchConflict, RoadmapPatchConflictCode,
    RoadmapPatchDependency, RoadmapPatchId, RoadmapPatchOperation, RoadmapPatchStatus,
    RoadmapPatchTarget,
};
use surge_orchestrator::roadmap_amendment::{
    RoadmapAmendmentError, RoadmapPatchApprovalAction, RoadmapPatchApprovalLoop,
    RoadmapPatchApprovalResolution, patch_approval_requested_notification, store_patch_draft,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, Storage};

fn approval_patch(id: &str) -> RoadmapPatch {
    let mut patch = RoadmapPatch::new(
        RoadmapPatchId::new(id).unwrap(),
        RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        },
        vec![RoadmapPatchOperation::AddMilestone {
            milestone: RoadmapMilestone::new("m2", "Approval loop"),
            insertion: Some(InsertionPoint::AppendToRoadmap),
        }],
    );
    patch.rationale = "Let operators approve roadmap amendments before apply.".into();
    patch.dependencies.push(RoadmapPatchDependency {
        from: RoadmapItemRef::Milestone {
            milestone_id: "m1".into(),
        },
        to: RoadmapItemRef::Milestone {
            milestone_id: "m2".into(),
        },
        reason: "approval metadata depends on patch drafts".into(),
    });
    patch.conflicts.push(RoadmapPatchConflict {
        code: RoadmapPatchConflictCode::RunningMilestone,
        item: Some(RoadmapItemRef::Milestone {
            milestone_id: "m1".into(),
        }),
        message: "m1 is currently running".into(),
        choices: vec![
            OperatorConflictChoice::CreateFollowUpRun,
            OperatorConflictChoice::RejectPatch,
        ],
        selected_choice: None,
    });
    patch
}

async fn create_drafted_patch(
    tmp: &tempfile::TempDir,
    storage: &std::sync::Arc<Storage>,
    run_id: RunId,
    patch: &RoadmapPatch,
) -> surge_persistence::runs::RunWriter {
    let writer = storage.create_run(run_id, tmp.path(), None).await.unwrap();
    let artifact_store = ArtifactStore::new(tmp.path().join("runs"));
    store_patch_draft(
        &artifact_store,
        &writer,
        run_id,
        patch,
        b"roadmap patch approval fixture",
    )
    .await
    .unwrap();
    writer
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_loop_records_approve_decision_in_patch_view() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let run_id = RunId::new();
    let patch = approval_patch("rpatch-approve");
    let writer = create_drafted_patch(&tmp, &storage, run_id, &patch).await;
    let channel = ApprovalChannel::Desktop {
        duration: ApprovalDuration::Transient,
    };

    let mut approval = RoadmapPatchApprovalLoop::new(2);
    let prompt = approval
        .request_approval(&writer, &patch, channel.clone())
        .await
        .unwrap();
    assert!(prompt.summary.contains("Approval loop"));
    assert!(prompt.summary.contains("append to roadmap"));
    assert!(prompt.summary.contains("create_follow_up_run"));
    assert_eq!(prompt.schema["required"][0], "decision");
    let rendered_notification = patch_approval_requested_notification(&prompt).render();
    assert!(rendered_notification.body.contains("rpatch-approve"));
    assert!(rendered_notification.body.contains("Approval loop"));

    let action = approval
        .record_decision(
            &writer,
            &patch.id,
            channel.kind(),
            RoadmapPatchApprovalResolution {
                decision: RoadmapPatchApprovalDecision::Approve,
                comment: Some("ship it".into()),
                conflict_choice: Some(OperatorConflictChoice::CreateFollowUpRun),
            },
        )
        .await
        .unwrap();
    assert_eq!(action, RoadmapPatchApprovalAction::Apply);
    writer.flush().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let records = reader.roadmap_patches().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, RoadmapPatchStatus::Approved);
    assert_eq!(records[0].summary_hash, Some(prompt.summary_hash));
    assert_eq!(
        records[0].decision,
        Some(RoadmapPatchApprovalDecision::Approve)
    );
    assert_eq!(records[0].decision_comment.as_deref(), Some("ship it"));
    assert_eq!(
        records[0].conflict_choice,
        Some(OperatorConflictChoice::CreateFollowUpRun)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_loop_edit_returns_feedback_and_escalates_after_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let run_id = RunId::new();
    let patch = approval_patch("rpatch-edit");
    let writer = create_drafted_patch(&tmp, &storage, run_id, &patch).await;
    let channel = ApprovalChannel::Desktop {
        duration: ApprovalDuration::Transient,
    };

    let mut approval = RoadmapPatchApprovalLoop::new(1);
    approval
        .request_approval(&writer, &patch, channel.clone())
        .await
        .unwrap();
    let action = approval
        .record_decision(
            &writer,
            &patch.id,
            channel.kind(),
            RoadmapPatchApprovalResolution {
                decision: RoadmapPatchApprovalDecision::Edit,
                comment: Some("split the milestone into smaller work".into()),
                conflict_choice: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        action,
        RoadmapPatchApprovalAction::Redraft {
            feedback: "split the milestone into smaller work".into(),
            attempt: 1,
        }
    );
    assert_eq!(approval.edit_rounds(), 1);

    approval
        .request_approval(&writer, &patch, channel.clone())
        .await
        .unwrap();
    let err = approval
        .record_decision(
            &writer,
            &patch.id,
            channel.kind(),
            RoadmapPatchApprovalResolution {
                decision: RoadmapPatchApprovalDecision::Edit,
                comment: Some("still too broad".into()),
                conflict_choice: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        RoadmapAmendmentError::ApprovalLoopExceeded {
            max_edit_rounds: 1,
            ..
        }
    ));
    assert_eq!(approval.edit_rounds(), 1);
    writer.flush().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let max_seq = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(1)..EventSeq(max_seq.as_u64() + 1))
        .await
        .unwrap();
    let kinds = events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"EscalationRequested"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_loop_records_reject_as_terminal_patch_state() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let run_id = RunId::new();
    let patch = approval_patch("rpatch-reject");
    let writer = create_drafted_patch(&tmp, &storage, run_id, &patch).await;
    let channel = ApprovalChannel::Desktop {
        duration: ApprovalDuration::Transient,
    };

    let mut approval = RoadmapPatchApprovalLoop::new(2);
    approval
        .request_approval(&writer, &patch, channel.clone())
        .await
        .unwrap();
    let action = approval
        .record_decision(
            &writer,
            &patch.id,
            channel.kind(),
            RoadmapPatchApprovalResolution {
                decision: RoadmapPatchApprovalDecision::Reject,
                comment: Some("not part of this roadmap".into()),
                conflict_choice: Some(OperatorConflictChoice::RejectPatch),
            },
        )
        .await
        .unwrap();
    assert_eq!(action, RoadmapPatchApprovalAction::Rejected);
    writer.flush().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let record = reader.roadmap_patch(&patch.id).await.unwrap().unwrap();
    assert_eq!(record.status, RoadmapPatchStatus::Rejected);
    assert_eq!(record.decision, Some(RoadmapPatchApprovalDecision::Reject));
    assert_eq!(
        record.conflict_choice,
        Some(OperatorConflictChoice::RejectPatch)
    );
}
