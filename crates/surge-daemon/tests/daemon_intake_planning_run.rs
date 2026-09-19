//! Integration tests for T11: a tracker ticket planned into the project queue.
//!
//! Drives `TicketRunLauncher` (through the inbox consumer's path) against a
//! real git repository with `.surge/roadmap.toml` and a stub planner, and
//! asserts the acceptance criteria:
//!
//! - G10: the ticket starts a **planning** transaction, not a work run — no
//!   `engine.start_run` call happens, `.surge/roadmap.toml` gains the task at
//!   the planner's insertion point, and a `task_queue` row exists.
//! - G11: a patch conflicting with the running milestone is deferred to the
//!   next milestone without operator input.
//!
//! The planner is the seam: production wires the ACP feature-planner behind
//! `RoadmapPlanner`; these tests script it, so the roadmap/queue behavior is
//! proven without a live agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, Mutex as StdMutex};

use surge_core::id::RunId;
use surge_core::roadmap::{
    RoadmapArtifact, RoadmapMilestone, RoadmapStatus, RoadmapTask, TaskSize,
};
use surge_core::roadmap_patch::{
    InsertionPoint, RoadmapPatch, RoadmapPatchId, RoadmapPatchOperation, RoadmapPatchStatus,
    RoadmapPatchTarget,
};
use surge_daemon::inbox::ticket_run_launcher::{PLANNED_INTO_QUEUE, TicketRunLauncher};
use surge_intake::TaskSource;
use surge_intake::types::{TaskDetails, TaskId};
use surge_orchestrator::archetype_registry::ArchetypeRegistry;
use surge_orchestrator::bootstrap::MinimalBootstrapGraphBuilder;
use surge_orchestrator::engine::EngineError;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{RunHandle, RunSummary};
use surge_orchestrator::intake_planner::{PlannerError, RoadmapPlanner};
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use surge_persistence::runs::Storage;
use surge_persistence::task_queue::TaskQueueFilter;
use tempfile::{TempDir, tempdir};

/// A planner that returns a fixed patch and records the requests it saw.
struct FixedPlanner {
    patch: Mutex<Option<RoadmapPatch>>,
    requests: StdMutex<Vec<String>>,
}

impl FixedPlanner {
    fn new(patch: RoadmapPatch) -> Self {
        Self {
            patch: Mutex::new(Some(patch)),
            requests: StdMutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl RoadmapPlanner for FixedPlanner {
    async fn plan(
        &self,
        request: String,
        _roadmap_toml: String,
    ) -> Result<RoadmapPatch, PlannerError> {
        self.requests.lock().unwrap().push(request);
        self.patch
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| PlannerError::Call("no scripted patch".into()))
    }
}

/// A facade that records `start_run` calls and always fails. The planning
/// path must not reach it; the fallback path does, and the test asserts the
/// difference through the recorded calls.
#[derive(Default)]
struct RecordingFacade {
    starts: StdMutex<Vec<RunId>>,
}

#[async_trait::async_trait]
impl EngineFacade for RecordingFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: surge_orchestrator::engine::EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        self.starts.lock().unwrap().push(run_id);
        Err(EngineError::Internal("stub: start_run always fails".into()))
    }

    async fn resume_run(
        &self,
        _run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        Err(EngineError::RunNotFound(_run_id))
    }

    async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Ok(())
    }

    async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(vec![])
    }
}

fn init_repo(roadmap: &RoadmapArtifact) -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let path = dir.path().to_path_buf();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(&path)
            .output()
            .unwrap();
    }
    std::fs::write(path.join("README.md"), "# Test\n").unwrap();
    std::fs::create_dir_all(path.join(".surge")).unwrap();
    std::fs::write(
        path.join(".surge/roadmap.toml"),
        toml::to_string_pretty(roadmap).unwrap(),
    )
    .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&path)
        .output()
        .unwrap();
    (dir, path)
}

fn running_first_roadmap() -> RoadmapArtifact {
    let mut m1 = RoadmapMilestone::new("m1", "Running now");
    m1.status = RoadmapStatus::Running;
    m1.tasks.push(RoadmapTask::new("m1-t1", "In flight"));
    let mut m2 = RoadmapMilestone::new("m2", "Next");
    m2.tasks.push(RoadmapTask::new("m2-t1", "Later"));
    RoadmapArtifact::new(vec![m1, m2])
}

fn add_task_patch(milestone_id: &str, task_id: &str) -> RoadmapPatch {
    let mut task = RoadmapTask::new(task_id, "From tracker");
    task.size = Some(TaskSize::S);
    task.description = Some("Planned from a tracker ticket.".into());
    RoadmapPatch {
        schema_version: 1,
        id: RoadmapPatchId::new("tracker-p1").expect("valid id"),
        target: RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".surge/roadmap.toml".into(),
        },
        rationale: "ticket".into(),
        operations: vec![RoadmapPatchOperation::AddTask {
            milestone_id: milestone_id.into(),
            task,
            insertion: Some(InsertionPoint::AppendToMilestone {
                milestone_id: milestone_id.into(),
            }),
        }],
        dependencies: Vec::new(),
        conflicts: Vec::new(),
        status: RoadmapPatchStatus::Drafted,
    }
}

fn insert_ticket(storage: &Storage, task_id: &str, callback_token: &str) {
    let conn = storage.acquire_registry_conn().unwrap();
    let repo = IntakeRepo::new(&conn);
    let now = chrono::Utc::now();
    repo.insert(&IntakeRow {
        task_id: task_id.into(),
        source_id: "mock:t".into(),
        provider: "mock".into(),
        run_id: None,
        triage_decision: None,
        duplicate_of: None,
        priority: Some("medium".into()),
        state: TicketState::InboxNotified,
        first_seen: now,
        last_seen: now,
        snooze_until: None,
        callback_token: Some(callback_token.into()),
        tg_chat_id: None,
        tg_message_id: None,
    })
    .unwrap();
}

fn launcher(
    storage: Arc<Storage>,
    project_root: &Path,
    planner: Arc<dyn RoadmapPlanner>,
) -> TicketRunLauncher {
    launcher_with(
        storage,
        project_root,
        planner,
        Arc::new(RecordingFacade::default()),
    )
}

fn launcher_with(
    storage: Arc<Storage>,
    project_root: &Path,
    planner: Arc<dyn RoadmapPlanner>,
    facade: Arc<dyn EngineFacade>,
) -> TicketRunLauncher {
    let archetypes = Arc::new(
        ArchetypeRegistry::from_dir(Path::new("definitely-missing-dir-for-tests"))
            .expect("empty registry"),
    );
    let mut launcher = TicketRunLauncher::new(
        storage,
        facade,
        Arc::new(MinimalBootstrapGraphBuilder::new()),
        archetypes,
        std::env::temp_dir().join("ato_t11_worktrees"),
        project_root.to_path_buf(),
        surge_core::SurgeConfig::default(),
    );
    launcher.planner = Some(planner);
    launcher
}

fn details() -> TaskDetails {
    TaskDetails {
        task_id: TaskId::try_new("mock:1").unwrap(),
        source_id: "mock:t".into(),
        title: "Add CSV export".into(),
        description: "Users want to export the table as CSV.".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: "https://mock.invalid/1".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    }
}

async fn source() -> Arc<dyn TaskSource> {
    let source = surge_intake::testing::MockTaskSource::new("mock:t", "mock");
    source.put_task(details()).await;
    Arc::new(source)
}

#[tokio::test(flavor = "multi_thread")]
async fn ticket_is_planned_into_the_queue_not_started_as_a_run() {
    let (repo_dir, repo) = init_repo(&running_first_roadmap());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    insert_ticket(&storage, "mock:1", "tok-1");
    // `fetch_ticket_for_start` reads the full task from the source; the mock
    // returns `details()` for any id.
    let planner = Arc::new(FixedPlanner::new(add_task_patch("m2", "ticket-csv")));
    let facade = Arc::new(RecordingFacade::default());
    let launcher = launcher_with(
        storage.clone(),
        &repo,
        planner.clone() as Arc<dyn RoadmapPlanner>,
        facade.clone(),
    );

    let start = launcher
        .fetch_ticket_for_start(
            &HashMap::from([("mock:t".to_string(), source().await)]),
            "tok-1",
        )
        .await
        .unwrap()
        .expect("ticket found");
    let outcome = launcher.launch(start, "telegram", None).await;
    let error = match outcome {
        Ok(_) => panic!("the planning path returns Err(PLANNED_INTO_QUEUE), not Ok"),
        Err(error) => error,
    };
    assert_eq!(
        error, PLANNED_INTO_QUEUE,
        "the planning path is success-with-no-run"
    );

    // G10: the roadmap file gained the task at the planner's insertion point.
    let roadmap: RoadmapArtifact =
        toml::from_str(&std::fs::read_to_string(repo.join(".surge/roadmap.toml")).unwrap())
            .unwrap();
    let m2 = roadmap.milestones.iter().find(|m| m.id == "m2").unwrap();
    assert!(
        m2.tasks.iter().any(|t| t.id == "ticket-csv"),
        "the planned task must be in the roadmap"
    );

    // G10: a queue row exists for it.
    let queue = storage.task_queue_store();
    let rows = queue
        .list(&TaskQueueFilter {
            project_root: Some(repo.clone()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        rows.iter().any(|row| row.task_id == "ticket-csv"),
        "the planned task must be mirrored into the queue: {rows:?}"
    );

    // FSM: the ticket is no longer awaiting a decision.
    assert_eq!(fetch_state(&storage, "mock:1"), TicketState::RunStarted);

    // The planner saw the ticket's own words, and no run was started.
    let requests = planner.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("Add CSV export"));
    assert!(
        facade.starts.lock().unwrap().is_empty(),
        "the planning path must not start a run"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn running_milestone_conflict_defers_without_operator_input() {
    let (repo_dir, repo) = init_repo(&running_first_roadmap());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    insert_ticket(&storage, "mock:1", "tok-2");
    // The patch targets the *running* milestone m1.
    let planner = Arc::new(FixedPlanner::new(add_task_patch("m1", "ticket-hot")));
    let launcher = launcher(
        storage.clone(),
        &repo,
        planner.clone() as Arc<dyn RoadmapPlanner>,
    );

    let start = launcher
        .fetch_ticket_for_start(
            &HashMap::from([("mock:t".to_string(), source().await)]),
            "tok-2",
        )
        .await
        .unwrap()
        .expect("ticket found");
    assert!(launcher.launch(start, "auto", None).await.is_err());

    // G11: the task landed after the running milestone, not inside it.
    let roadmap: RoadmapArtifact =
        toml::from_str(&std::fs::read_to_string(repo.join(".surge/roadmap.toml")).unwrap())
            .unwrap();
    let m1 = roadmap.milestones.iter().find(|m| m.id == "m1").unwrap();
    assert!(
        !m1.tasks.iter().any(|t| t.id == "ticket-hot"),
        "the task must not land in the running milestone"
    );
    let m2 = roadmap.milestones.iter().find(|m| m.id == "m2").unwrap();
    assert!(
        m2.tasks.iter().any(|t| t.id == "ticket-hot"),
        "the task must be deferred into the next pending milestone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn planner_failure_falls_back_to_the_legacy_path() {
    struct FailingPlanner;

    #[async_trait::async_trait]
    impl RoadmapPlanner for FailingPlanner {
        async fn plan(
            &self,
            _request: String,
            _roadmap_toml: String,
        ) -> Result<RoadmapPatch, PlannerError> {
            Err(PlannerError::Call("no agent binary".into()))
        }
    }

    let (repo_dir, repo) = init_repo(&running_first_roadmap());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    insert_ticket(&storage, "mock:1", "tok-3");
    let facade = Arc::new(RecordingFacade::default());
    let launcher = launcher_with(
        storage.clone(),
        &repo,
        Arc::new(FailingPlanner) as Arc<dyn RoadmapPlanner>,
        facade.clone(),
    );

    let start = launcher
        .fetch_ticket_for_start(
            &HashMap::from([("mock:t".to_string(), source().await)]),
            "tok-3",
        )
        .await
        .unwrap()
        .expect("ticket found");
    // The fallback path tries to start a run, and `NoRunsFacade` panics if it
    // does — so reaching the facade at all proves the fallback engaged. The
    // test asserts on the error shape instead: the legacy launcher returns
    // the facade's error, not the planned sentinel.
    let error = match launcher.launch(start, "telegram", None).await {
        Ok(_) => panic!("the stub facade must fail"),
        Err(error) => error,
    };
    assert_ne!(error, PLANNED_INTO_QUEUE);
    assert_eq!(
        facade.starts.lock().unwrap().len(),
        1,
        "the fallback must reach the legacy start_run path"
    );
    // The ticket was not transitioned: the fallback failed before the FSM.
    assert_eq!(fetch_state(&storage, "mock:1"), TicketState::InboxNotified);
    let _ = repo_dir;
}

fn fetch_state(storage: &Storage, task_id: &str) -> TicketState {
    let conn = storage.acquire_registry_conn().unwrap();
    IntakeRepo::new(&conn)
        .fetch(task_id)
        .unwrap()
        .unwrap()
        .state
}
