use surge_core::approvals::{ApprovalChannel, ApprovalDuration};
use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone, RoadmapStatus, RoadmapTask};
use surge_core::roadmap_patch::{
    InsertionPoint, OperatorConflictChoice, RoadmapPatch, RoadmapPatchApplyError,
    RoadmapPatchApprovalDecision, RoadmapPatchConflictCode, RoadmapPatchId, RoadmapPatchOperation,
    RoadmapPatchStatus, RoadmapPatchTarget,
};
use surge_core::{
    ArtifactDiagnosticCode, ArtifactKind, ContentHash, RunId, validate_artifact_text,
};
use surge_orchestrator::engine::validate::validate_for_m6;
use surge_orchestrator::roadmap_amendment::{
    RoadmapPatchApprovalLoop, RoadmapPatchApprovalResolution, apply_conflicts_as_patch_conflicts,
    build_follow_up_run_request, follow_up_result_from_patch, store_patch_draft,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::roadmap_patches::RoadmapPatchIndexUpsert;
use surge_persistence::runs::Storage;

const INVALID_PATCH: &str = r#"schema_version = 1
id = "rpatch-invalid"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"
"#;

#[test]
fn malformed_patch_is_rejected_before_amendment_lifecycle() {
    let report = validate_artifact_text(ArtifactKind::RoadmapPatch, INVALID_PATCH);

    assert!(!report.is_valid());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == ArtifactDiagnosticCode::MissingOperation })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_patch_content_keeps_original_registry_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let patch = add_task_patch("rpatch-duplicate-a");
    let content_hash = patch.content_hash().unwrap();

    let first = storage
        .roadmap_patch_store()
        .upsert(&index_upsert(
            &patch,
            content_hash,
            RoadmapPatchStatus::Drafted,
        ))
        .unwrap();
    let mut duplicate = add_task_patch("rpatch-duplicate-b");
    duplicate.rationale = patch.rationale.clone();
    let second = storage
        .roadmap_patch_store()
        .upsert(&index_upsert(
            &duplicate,
            content_hash,
            RoadmapPatchStatus::PendingApproval,
        ))
        .unwrap();

    assert_eq!(second.patch_id, first.patch_id);
    assert_eq!(second.status, RoadmapPatchStatus::PendingApproval);
    assert!(
        storage
            .roadmap_patch_store()
            .get(&duplicate.id)
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_milestone_conflict_records_choice_and_builds_follow_up() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, tmp.path(), None).await.unwrap();
    let artifact_store = ArtifactStore::new(tmp.path().join("runs"));
    let roadmap = running_roadmap();
    let patch = add_task_patch("rpatch-running-follow-up");
    let RoadmapPatchApplyError::Conflicts { conflicts } =
        patch.apply_to_roadmap(&roadmap).unwrap_err()
    else {
        panic!("expected running milestone conflict");
    };
    let patch_conflicts = apply_conflicts_as_patch_conflicts(&conflicts);

    assert_eq!(
        patch_conflicts[0].code,
        RoadmapPatchConflictCode::RunningMilestone
    );
    assert!(
        patch_conflicts[0]
            .choices
            .contains(&OperatorConflictChoice::CreateFollowUpRun)
    );

    let patch_toml = toml::to_string(&patch).unwrap();
    store_patch_draft(
        &artifact_store,
        &writer,
        run_id,
        &patch,
        patch_toml.as_bytes(),
    )
    .await
    .unwrap();
    let mut approval = RoadmapPatchApprovalLoop::new(0);
    let channel = ApprovalChannel::Desktop {
        duration: ApprovalDuration::Transient,
    };
    approval
        .request_approval(&writer, &patch, channel.clone())
        .await
        .unwrap();
    approval
        .record_decision(
            &writer,
            &patch.id,
            channel.kind(),
            RoadmapPatchApprovalResolution {
                decision: RoadmapPatchApprovalDecision::Approve,
                comment: Some("run is already in the milestone".into()),
                conflict_choice: Some(OperatorConflictChoice::CreateFollowUpRun),
            },
        )
        .await
        .unwrap();
    writer.flush().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let record = reader.roadmap_patch(&patch.id).await.unwrap().unwrap();
    assert_eq!(record.status, RoadmapPatchStatus::Approved);
    assert_eq!(
        record.conflict_choice,
        Some(OperatorConflictChoice::CreateFollowUpRun)
    );

    let follow_up = follow_up_result_from_patch(&patch);
    let request = build_follow_up_run_request(
        &patch.id,
        &patch.target,
        &follow_up,
        tmp.path(),
        None,
        chrono::Utc::now(),
    )
    .unwrap();
    validate_for_m6(&request.graph).unwrap();
    assert!(
        request.run_config.seed_artifacts[0]
            .content
            .contains("new-task")
    );
}

fn running_roadmap() -> RoadmapArtifact {
    let mut milestone = RoadmapMilestone::new("m1", "Running work");
    milestone.status = RoadmapStatus::Running;
    milestone
        .tasks
        .push(RoadmapTask::new("old-task", "Old task"));
    RoadmapArtifact::new(vec![milestone])
}

fn add_task_patch(id: &str) -> RoadmapPatch {
    let mut patch = RoadmapPatch::new(
        RoadmapPatchId::new(id).unwrap(),
        RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        },
        vec![RoadmapPatchOperation::AddTask {
            milestone_id: "m1".into(),
            task: RoadmapTask::new("new-task", "New task"),
            insertion: Some(InsertionPoint::AppendToMilestone {
                milestone_id: "m1".into(),
            }),
        }],
    );
    patch.rationale = "Add task from feature request".into();
    patch
}

fn index_upsert(
    patch: &RoadmapPatch,
    content_hash: ContentHash,
    status: RoadmapPatchStatus,
) -> RoadmapPatchIndexUpsert {
    RoadmapPatchIndexUpsert {
        patch_id: patch.id.clone(),
        content_hash,
        run_id: None,
        project_path: std::path::PathBuf::from("/tmp/surge-project"),
        target: patch.target.clone(),
        status,
        patch_artifact: None,
        patch_path: None,
        summary_hash: None,
        decision: None,
        decision_comment: None,
        conflict_choice: None,
        observed_at_ms: 1_700_000_000_000,
    }
}
