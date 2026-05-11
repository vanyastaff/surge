use std::path::Path;

use surge_core::keys::NodeKey;
use surge_core::roadmap_patch::{ActivePickupPolicy, RoadmapPatchTarget};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{ContentHash, RunId, RunStatus};
use surge_orchestrator::roadmap_target::{
    RoadmapAmendmentPoint, RoadmapTargetError, RoadmapTargetResolver, RoadmapTargetSelector,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::Storage;

async fn create_run_with_artifacts(
    storage: &std::sync::Arc<Storage>,
    storage_home: &Path,
    project_path: &Path,
    status: RunStatus,
) -> (RunId, ContentHash, ContentHash) {
    let run_id = RunId::new();
    let writer = storage
        .create_run(run_id, project_path, None)
        .await
        .unwrap();
    let artifact_store = ArtifactStore::new(storage_home.join("runs"));
    let roadmap = artifact_store
        .put(run_id, "roadmap", b"# Roadmap\n")
        .await
        .unwrap();
    let flow = artifact_store
        .put(run_id, "flow", b"schema_version = 1\n")
        .await
        .unwrap();
    let node = NodeKey::try_from("roadmap_planner").unwrap();
    writer
        .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
            node: node.clone(),
            artifact: roadmap.hash,
            path: roadmap.path,
            name: "roadmap".into(),
        }))
        .await
        .unwrap();
    writer
        .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
            node,
            artifact: flow.hash,
            path: flow.path,
            name: "flow".into(),
        }))
        .await
        .unwrap();
    writer.flush().await.unwrap();

    let conn = storage.acquire_registry_conn().unwrap();
    let status = status.as_str().to_owned();
    let run_id_string = run_id.to_string();
    conn.execute(
        "UPDATE runs SET status = ? WHERE id = ?",
        [status.as_str(), run_id_string.as_str()],
    )
    .unwrap();
    (run_id, roadmap.hash, flow.hash)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_project_target_reads_project_roadmap() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".ai-factory")).unwrap();
    std::fs::write(
        project.join(".ai-factory").join("ROADMAP.md"),
        "# Roadmap\n",
    )
    .unwrap();
    let storage = Storage::open(tmp.path().join("home")).await.unwrap();

    let resolver =
        RoadmapTargetResolver::new(storage, &project, Path::new(".ai-factory/ROADMAP.md"));
    let candidate = resolver
        .resolve(RoadmapTargetSelector::ProjectFile)
        .await
        .unwrap();

    assert_eq!(candidate.run_id, None);
    assert_eq!(
        candidate.amendment_point,
        RoadmapAmendmentPoint::ProjectFile
    );
    assert_eq!(candidate.active_pickup, ActivePickupPolicy::FollowUpOnly);
    assert!(candidate.roadmap_hash.is_some());
    assert!(matches!(
        candidate.target,
        RoadmapPatchTarget::ProjectRoadmap { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_run_target_returns_artifacts_and_active_pickup() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let storage_home = tmp.path().join("home");
    let storage = Storage::open(&storage_home).await.unwrap();
    let (run_id, roadmap_hash, flow_hash) =
        create_run_with_artifacts(&storage, &storage_home, &project, RunStatus::Running).await;

    let resolver =
        RoadmapTargetResolver::new(storage, &project, Path::new(".ai-factory/ROADMAP.md"));
    let candidate = resolver
        .resolve(RoadmapTargetSelector::Run { run_id })
        .await
        .unwrap();

    assert_eq!(candidate.run_id, Some(run_id));
    assert_eq!(candidate.run_status, Some(RunStatus::Running));
    assert_eq!(candidate.roadmap_hash, Some(roadmap_hash));
    assert_eq!(candidate.flow_hash, Some(flow_hash));
    assert_eq!(candidate.active_pickup, ActivePickupPolicy::Allowed);
    assert_eq!(
        candidate.amendment_point,
        RoadmapAmendmentPoint::ActiveRunBoundary
    );
    assert!(matches!(
        candidate.target,
        RoadmapPatchTarget::RunRoadmap {
            active_pickup: ActivePickupPolicy::Allowed,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_run_auto_target_uses_follow_up_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let storage_home = tmp.path().join("home");
    let storage = Storage::open(&storage_home).await.unwrap();
    let (run_id, _, _) =
        create_run_with_artifacts(&storage, &storage_home, &project, RunStatus::Completed).await;

    let resolver =
        RoadmapTargetResolver::new(storage, &project, Path::new(".ai-factory/ROADMAP.md"));
    let candidate = resolver.resolve(RoadmapTargetSelector::Auto).await.unwrap();

    assert_eq!(candidate.run_id, Some(run_id));
    assert_eq!(candidate.run_status, Some(RunStatus::Completed));
    assert_eq!(candidate.active_pickup, ActivePickupPolicy::FollowUpOnly);
    assert_eq!(
        candidate.amendment_point,
        RoadmapAmendmentPoint::FollowUpRun
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_target_is_ambiguous_when_project_and_run_candidates_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".ai-factory")).unwrap();
    std::fs::write(
        project.join(".ai-factory").join("ROADMAP.md"),
        "# Roadmap\n",
    )
    .unwrap();
    let storage_home = tmp.path().join("home");
    let storage = Storage::open(&storage_home).await.unwrap();
    create_run_with_artifacts(&storage, &storage_home, &project, RunStatus::Running).await;

    let resolver =
        RoadmapTargetResolver::new(storage, &project, Path::new(".ai-factory/ROADMAP.md"));
    let err = resolver
        .resolve(RoadmapTargetSelector::Auto)
        .await
        .unwrap_err();

    match err {
        RoadmapTargetError::Ambiguous { count, candidates } => {
            assert_eq!(count, 2);
            assert_eq!(candidates.len(), 2);
        },
        other => panic!("expected ambiguous target, got {other:?}"),
    }
}
