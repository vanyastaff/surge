use chrono::{TimeZone, Utc};
use std::path::PathBuf;
use std::sync::Mutex;
use surge_core::keys::NodeKey;
use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone, RoadmapTask};
use surge_core::roadmap_patch::{
    InsertionPoint, RoadmapItemRef, RoadmapPatch, RoadmapPatchApplyResult, RoadmapPatchId,
    RoadmapPatchItem, RoadmapPatchOperation,
};
use surge_core::{Graph, RoadmapPatchTarget, RunId};
use surge_orchestrator::engine::config::{EngineRunConfig, ProjectContextSeed};
use surge_orchestrator::engine::error::EngineError;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{RunHandle, RunOutcome, RunSummary};
use surge_orchestrator::engine::validate::validate_for_m6;
use surge_orchestrator::roadmap_amendment::{
    build_follow_up_run_request, follow_up_result_from_patch, start_follow_up_run,
};

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
