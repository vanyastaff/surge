use std::collections::BTreeMap;

use chrono::Utc;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone};
use surge_core::roadmap_patch::{ActivePickupPolicy, RoadmapPatch, RoadmapPatchStatus};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_core::{
    RoadmapPatchApplyResult, RoadmapPatchId, RoadmapPatchTarget, RunId, validate_artifact_text,
};
use surge_orchestrator::roadmap_amendment::{
    apply_active_run_patch, record_roadmap_updated, store_applied_artifacts, store_patch_draft,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, Storage};

const PATCH_TOML: &str = r#"schema_version = 1
id = "rpatch-artifacts"
rationale = "Store patch artifacts through the content-addressed store."
status = "drafted"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "add_milestone"

[operations.milestone]
id = "m2"
title = "Artifact storage"

[operations.insertion]
kind = "append_to_roadmap"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stores_patch_and_amended_artifacts_with_lifecycle_events() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, tmp.path(), None).await.unwrap();
    let artifact_store = ArtifactStore::new(tmp.path().join("runs"));
    let patch: RoadmapPatch = toml::from_str(PATCH_TOML).unwrap();

    let patch_ref = store_patch_draft(
        &artifact_store,
        &writer,
        run_id,
        &patch,
        PATCH_TOML.as_bytes(),
    )
    .await
    .unwrap();
    let artifacts = store_applied_artifacts(
        &artifact_store,
        &writer,
        run_id,
        &patch.id,
        &patch.target,
        b"schema_version = 1\nmilestones = []\n",
        Some(b"schema_version = 1\nstart = \"end\"\n"),
    )
    .await
    .unwrap();
    record_roadmap_updated(
        &writer,
        &patch.id,
        &patch.target,
        &artifacts,
        ActivePickupPolicy::Allowed,
    )
    .await
    .unwrap();
    writer.flush().await.unwrap();

    let stored_patch = artifact_store.open(run_id, patch_ref.hash).await.unwrap();
    assert_eq!(stored_patch, PATCH_TOML.as_bytes());
    assert!(validate_artifact_text(surge_core::ArtifactKind::RoadmapPatch, PATCH_TOML).is_valid());

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
    assert!(kinds.contains(&"RoadmapPatchDrafted"));
    assert!(kinds.contains(&"RoadmapPatchApplied"));
    assert!(kinds.contains(&"RoadmapUpdated"));

    let records = reader.roadmap_patches().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].patch_id, patch.id);
    assert_eq!(records[0].status, RoadmapPatchStatus::Applied);
    assert_eq!(records[0].patch_artifact, Some(patch_ref.hash));
    assert_eq!(records[0].roadmap_artifact, Some(artifacts.roadmap.hash));
    assert_eq!(
        records[0].flow_artifact,
        artifacts.flow.as_ref().map(|artifact| artifact.hash)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_run_patch_records_graph_revision_in_target_log() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, tmp.path(), None).await.unwrap();
    let artifact_store = ArtifactStore::new(tmp.path().join("runs"));
    let patch_id = RoadmapPatchId::new("rpatch-active-log").unwrap();
    let target = RoadmapPatchTarget::RunRoadmap {
        run_id,
        roadmap_artifact: None,
        flow_artifact: None,
        active_pickup: ActivePickupPolicy::Allowed,
    };
    let patch_result = patch_result_with_milestone();

    let outcome = apply_active_run_patch(
        &artifact_store,
        &writer,
        run_id,
        &terminal_only_graph(),
        &patch_id,
        &target,
        &patch_result,
    )
    .await
    .unwrap();
    writer.flush().await.unwrap();

    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.patch_id, patch_id);
    assert_eq!(outcome.inserted_nodes, vec![node_key("amend_001")]);

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
    assert!(kinds.contains(&"RoadmapPatchApplied"));
    assert!(kinds.contains(&"RoadmapUpdated"));
    assert!(kinds.contains(&"GraphRevisionAccepted"));

    let records = reader.roadmap_patches().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, RoadmapPatchStatus::Applied);
    assert_eq!(records[0].roadmap_artifact, Some(outcome.roadmap_artifact));
    assert_eq!(records[0].flow_artifact, Some(outcome.flow_artifact));
}

fn patch_result_with_milestone() -> RoadmapPatchApplyResult {
    let roadmap = RoadmapArtifact::new(vec![RoadmapMilestone::new("m2", "Metrics")]);
    RoadmapPatchApplyResult {
        markdown: roadmap.to_markdown(),
        roadmap,
        inserted_milestones: vec!["m2".into()],
        inserted_tasks: Vec::new(),
        replaced_items: Vec::new(),
        dependencies_added: Vec::new(),
    }
}

fn terminal_only_graph() -> Graph {
    let end = node_key("end");
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("terminal-only", Utc::now()),
        start: end.clone(),
        nodes: [(end.clone(), success_terminal_node("end"))].into(),
        edges: Vec::new(),
        subgraphs: BTreeMap::new(),
    }
}

fn success_terminal_node(id: &str) -> Node {
    let key = node_key(id);
    Node {
        id: key,
        position: Position::default(),
        declared_outcomes: Vec::new(),
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn node_key(value: &str) -> NodeKey {
    NodeKey::try_from(value).expect("valid node key")
}
