#[path = "fixtures/runtime_home.rs"]
mod runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use surge_core::roadmap::RoadmapTask;
use surge_core::roadmap_patch::{
    InsertionPoint, RoadmapPatch, RoadmapPatchId, RoadmapPatchOperation, RoadmapPatchStatus,
    RoadmapPatchTarget,
};
use surge_core::{ArtifactDiagnosticCode, ArtifactKind, ContentHash, validate_artifact_text};
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
    let tmp = FixtureHome::new().unwrap();
    {
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
    tmp.close().unwrap();
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
