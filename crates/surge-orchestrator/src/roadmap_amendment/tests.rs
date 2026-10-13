//! In-crate tests of the helpers behind "apply an approved roadmap patch":
//! the follow-up request builder and starter, and the active-run helper the
//! engine's amendment queue calls.

use chrono::{TimeZone, Utc};
use std::collections::BTreeMap;
use surge_core::graph::{GraphMetadata, SCHEMA_VERSION};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::roadmap_patch::{ActivePickupPolicy, RoadmapPatchStatus};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, Storage};

use super::apply_active_run_patch;

use super::{build_follow_up_run_request, follow_up_result_from_patch, start_follow_up_run};
use crate::engine::config::{EngineRunConfig, ProjectContextSeed};
use crate::engine::error::EngineError;
use crate::engine::facade::EngineFacade;
use crate::engine::handle::{RunHandle, RunOutcome, RunSummary};
use crate::engine::validate::validate_for_m6;
use std::path::PathBuf;
use std::sync::Mutex;
use surge_core::keys::NodeKey;
use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone, RoadmapTask};
use surge_core::roadmap_patch::{
    InsertionPoint, RoadmapItemRef, RoadmapPatch, RoadmapPatchApplyResult, RoadmapPatchId,
    RoadmapPatchItem, RoadmapPatchOperation,
};
use surge_core::{Graph, RoadmapPatchTarget, RunId};

#[test]
fn follow_up_run_request_contains_valid_graph_and_seed_artifacts() {
    let patch_id = RoadmapPatchId::new("rpatch-follow-up").unwrap();
    let target = RoadmapPatchTarget::ProjectRoadmap {
        roadmap_path: ".ai-factory/ROADMAP.md".into(),
    };
    let project_context = ProjectContextSeed::new(
        "project.md".into(),
        "# Project\n\nStable project context.".into(),
    );
    let created_at = Utc
        .with_ymd_and_hms(2026, 5, 11, 13, 0, 0)
        .single()
        .unwrap();

    let request = build_follow_up_run_request(
        &patch_id,
        &target,
        &patch_result_with_task(),
        ".worktrees/follow-up",
        Some(project_context.clone()),
        created_at,
    )
    .expect("follow-up request builds");

    validate_for_m6(&request.graph).expect("follow-up graph validates");
    assert_eq!(request.patch_id, patch_id);
    assert_eq!(request.target, target);
    assert_eq!(request.graph.metadata.created_at, created_at);
    assert_eq!(
        request.run_config.project_context.as_ref(),
        Some(&project_context)
    );
    assert_eq!(request.run_config.seed_artifacts.len(), 1);
    let seed = &request.run_config.seed_artifacts[0];
    assert_eq!(seed.name, "roadmap_amendment");
    assert_eq!(
        seed.relative_path,
        std::path::PathBuf::from(".surge/roadmap_amendment.md")
    );
    assert_eq!(seed.producer.as_ref(), "roadmap_amendment_seed");
    assert!(seed.content.contains("m2/t1: Add runtime counters"));
    assert!(seed.content.contains("## Amended roadmap"));
}

#[tokio::test]
async fn start_follow_up_run_forwards_materialized_request_to_engine() {
    let patch_id = RoadmapPatchId::new("rpatch-forward").unwrap();
    let target = RoadmapPatchTarget::ProjectRoadmap {
        roadmap_path: ".ai-factory/ROADMAP.md".into(),
    };
    let created_at = Utc
        .with_ymd_and_hms(2026, 5, 11, 14, 0, 0)
        .single()
        .unwrap();
    let request = build_follow_up_run_request(
        &patch_id,
        &target,
        &patch_result_with_task(),
        ".worktrees/follow-up-forward",
        None,
        created_at,
    )
    .expect("follow-up request builds");
    let expected_run_id = request.run_id;
    let engine = RecordingEngine::default();

    let handle = start_follow_up_run(&engine, request)
        .await
        .expect("follow-up run starts");

    assert_eq!(handle.run_id, expected_run_id);
    let recorded = engine.take_recorded_start();
    assert_eq!(recorded.run_id, expected_run_id);
    assert_eq!(
        recorded.worktree_path,
        PathBuf::from(".worktrees/follow-up-forward")
    );
    assert_eq!(recorded.graph_name, "roadmap-amendment-follow-up");
    assert_eq!(recorded.seed_names, vec!["roadmap_amendment"]);
    assert!(recorded.initial_prompt.contains("rpatch-forward"));
}

#[test]
fn follow_up_result_from_patch_preserves_conflicted_work_without_mutating_base() {
    let target = RoadmapPatchTarget::ProjectRoadmap {
        roadmap_path: ".ai-factory/ROADMAP.md".into(),
    };
    let patch = RoadmapPatch::new(
        RoadmapPatchId::new("rpatch-conflicted-follow-up").unwrap(),
        target,
        vec![
            RoadmapPatchOperation::AddTask {
                milestone_id: "running".into(),
                task: RoadmapTask::new("running-t2", "Deferred task"),
                insertion: Some(InsertionPoint::AppendToMilestone {
                    milestone_id: "running".into(),
                }),
            },
            RoadmapPatchOperation::ReplaceDraftItem {
                target: RoadmapItemRef::Task {
                    milestone_id: "running".into(),
                    task_id: "running-t1".into(),
                },
                replacement: RoadmapPatchItem::Task {
                    task: RoadmapTask::new("running-t1b", "Reworked task"),
                },
                reason: "running milestone cannot be rewritten in place".into(),
            },
        ],
    );

    let result = follow_up_result_from_patch(&patch);

    assert_eq!(result.roadmap.milestones.len(), 2);
    assert!(result.markdown.contains("Deferred task"));
    assert!(result.markdown.contains("Reworked task"));
    assert_eq!(result.inserted_tasks.len(), 2);
    assert_eq!(result.replaced_items.len(), 1);
}

fn patch_result_with_task() -> RoadmapPatchApplyResult {
    let mut milestone = RoadmapMilestone::new("m2", "Metrics");
    milestone
        .tasks
        .push(RoadmapTask::new("t1", "Add runtime counters"));
    let roadmap = RoadmapArtifact::new(vec![milestone]);
    RoadmapPatchApplyResult {
        markdown: roadmap.to_markdown(),
        roadmap,
        inserted_milestones: Vec::new(),
        inserted_tasks: vec![RoadmapItemRef::Task {
            milestone_id: "m2".into(),
            task_id: "t1".into(),
        }],
        replaced_items: Vec::new(),
        dependencies_added: Vec::new(),
    }
}

#[derive(Default)]
struct RecordingEngine {
    recorded_start: Mutex<Option<RecordedStart>>,
}

impl RecordingEngine {
    fn take_recorded_start(&self) -> RecordedStart {
        self.recorded_start
            .lock()
            .unwrap()
            .take()
            .expect("start_run should be called")
    }
}

struct RecordedStart {
    run_id: RunId,
    worktree_path: PathBuf,
    graph_name: String,
    seed_names: Vec<String>,
    initial_prompt: String,
}

#[async_trait::async_trait]
impl EngineFacade for RecordingEngine {
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        *self.recorded_start.lock().unwrap() = Some(RecordedStart {
            run_id,
            worktree_path,
            graph_name: graph.metadata.name,
            seed_names: run_config
                .seed_artifacts
                .iter()
                .map(|seed| seed.name.clone())
                .collect(),
            initial_prompt: run_config.initial_prompt,
        });

        let (_events_tx, events) = tokio::sync::broadcast::channel(8);
        let completion = tokio::spawn(async {
            RunOutcome::Completed {
                terminal: NodeKey::try_from("success").unwrap(),
            }
        });
        Ok(RunHandle {
            run_id,
            events,
            completion,
        })
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        Err(EngineError::RunNotFound(run_id))
    }

    async fn stop_run(&self, run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Err(EngineError::RunNotFound(run_id))
    }

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Err(EngineError::RunNotFound(run_id))
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(Vec::new())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_run_patch_records_graph_revision_in_target_log() {
    let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    {
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
        let patch_result = active_patch_result();

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

        writer.close().await.unwrap();
    }
    tmp.close().unwrap();
}

fn active_patch_result() -> RoadmapPatchApplyResult {
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

#[test]
fn generated_milestone_ids_are_unique_and_never_empty() {
    let mut used = std::collections::BTreeSet::new();
    assert_eq!(super::unique_id("  ", &mut used).as_str(), "follow-up");
    assert_eq!(super::unique_id("", &mut used).as_str(), "follow-up-2");
    assert_eq!(
        super::unique_id("m1-deferred", &mut used).as_str(),
        "m1-deferred"
    );
    assert_eq!(
        super::unique_id("m1-deferred", &mut used).as_str(),
        "m1-deferred-2"
    );
}
