//! Outer acceptance test for Autonomous Task Orchestration v1 (T15).
//!
//! Drives the daemon's task scheduler end to end over a real git repository:
//! a three-task dependent roadmap plus an injected external task, with
//! mid-flight pause/reprioritise. This is the spec-level scenario the ledger
//! names (`OUTER`), exercising the whole layer rather than one function.
//!
//! Shape:
//!
//! ```text
//!  t1 (high) ──► t2 (medium)      t3 (low, independent)
//! ```
//!
//! The stub engine writes real events (`RunCompleted` with a `TaskVerified`
//! for verified tasks) and commits a file per task, so the merge path is the
//! production one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::roadmap::{Priority, RoadmapArtifact, RoadmapMilestone, RoadmapTask, TaskSize};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_daemon::admission::AdmissionController;
use surge_daemon::task_scheduler::{PROJECT_ROADMAP_RELPATH, TaskScheduler};
use surge_orchestrator::engine::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::runs::{MockClock, Storage};
use surge_persistence::task_queue::{DispatchState, TaskQueueFilter};
use tempfile::{TempDir, tempdir};
use tokio::sync::broadcast;

const NOW: i64 = 1_700_000_000_000;

struct OuterFacade {
    storage: Arc<Storage>,
    dispatches: Mutex<Vec<(RunId, PathBuf, String)>>,
    /// `RunOrigin.roadmap_hash` recorded at each dispatch, by task id.
    origins: Mutex<HashMap<String, ContentHash>>,
    commits: Mutex<HashMap<String, String>>,
}

impl OuterFacade {
    fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            dispatches: Mutex::new(Vec::new()),
            origins: Mutex::new(HashMap::new()),
            commits: Mutex::new(HashMap::new()),
        }
    }

    fn task_ids(&self) -> Vec<String> {
        self.dispatches
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, task)| task.clone())
            .collect()
    }

    fn worktree_of(&self, task_id: &str) -> PathBuf {
        self.dispatches
            .lock()
            .unwrap()
            .iter()
            .find(|(_, _, task)| task == task_id)
            .map(|(_, worktree, _)| worktree.clone())
            .expect("task was dispatched")
    }
}

#[async_trait::async_trait]
impl EngineFacade for OuterFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: surge_core::graph::Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        let task_id = match run_config.origin.as_ref() {
            Some(surge_core::run_event::RunOrigin::Task { task_id, .. }) => task_id.clone(),
            _ => String::new(),
        };
        self.dispatches
            .lock()
            .unwrap()
            .push((run_id, worktree_path.clone(), task_id.clone()));
        if let Some(surge_core::run_event::RunOrigin::Task { roadmap_hash, .. }) =
            run_config.origin.as_ref()
        {
            self.origins
                .lock()
                .unwrap()
                .insert(task_id.clone(), *roadmap_hash);
        }

        self.storage
            .create_run(run_id, &worktree_path, None)
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        let writer = self
            .storage
            .open_run_writer(run_id)
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        writer
            .append_event(VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree_path.clone(),
                initial_prompt: String::new(),
                config: surge_core::run_event::RunConfig {
                    origin: run_config.origin.clone(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    budget: Default::default(),
                },
            }))
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;

        if let Some(contents) = self.commits.lock().unwrap().get(&task_id).cloned() {
            let file = format!("{task_id}.txt");
            commit_in_worktree(&worktree_path, &file, &contents);
        }
        writer
            .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                terminal_node: NodeKey::try_from("end").unwrap(),
            }))
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        writer
            .append_event(VersionedEventPayload::new(EventPayload::TaskVerified {
                task_id: task_id.clone(),
                node: NodeKey::try_from("verify_1").unwrap(),
                evidence: ContentHash::compute(task_id.as_bytes()),
            }))
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        writer
            .close()
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        self.storage
            .set_run_status(&run_id, surge_core::RunStatus::Completed, Some(NOW))
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;

        Ok(immediate_handle(run_id))
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        Ok(immediate_handle(run_id))
    }

    async fn stop_run(
        &self,
        _run_id: RunId,
        _reason: String,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }

    async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }

    async fn list_runs(
        &self,
    ) -> Result<
        Vec<surge_orchestrator::engine::handle::RunSummary>,
        surge_orchestrator::engine::EngineError,
    > {
        Ok(vec![])
    }
}

fn immediate_handle(run_id: RunId) -> RunHandle {
    let (tx, rx) = broadcast::channel(8);
    let _ = tx.send(EngineRunEvent::Terminal {
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_from("end").unwrap(),
        },
    });
    drop(tx);
    let completion = tokio::spawn(async move {
        RunOutcome::Completed {
            terminal: NodeKey::try_from("end").unwrap(),
        }
    });
    RunHandle {
        run_id,
        events: rx,
        completion,
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
        path.join(PROJECT_ROADMAP_RELPATH),
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

fn commit_in_worktree(worktree: &Path, file: &str, contents: &str) {
    std::fs::write(worktree.join(file), contents).unwrap();
    for args in [vec!["add", "."], vec!["commit", "-m", "task work"]] {
        Command::new("git")
            .args(&args)
            .current_dir(worktree)
            .output()
            .unwrap();
    }
}

fn three_task_roadmap() -> RoadmapArtifact {
    let mut milestone = RoadmapMilestone::new("m1", "Deliver");
    let mut t1 = RoadmapTask::new("t1", "Root task");
    t1.size = Some(TaskSize::M);
    t1.priority = Priority::High;
    let mut t2 = RoadmapTask::new("t2", "Dependent task");
    t2.size = Some(TaskSize::M);
    t2.priority = Priority::Medium;
    t2.depends_on = vec!["t1".to_string()];
    let mut t3 = RoadmapTask::new("t3", "Independent task");
    t3.size = Some(TaskSize::M);
    t3.priority = Priority::Low;
    milestone.tasks.push(t1);
    milestone.tasks.push(t2);
    milestone.tasks.push(t3);
    RoadmapArtifact::new(vec![milestone])
}

fn scheduler(
    storage: Arc<Storage>,
    facade: Arc<OuterFacade>,
    clock: Arc<MockClock>,
) -> TaskScheduler {
    let mut scheduler = TaskScheduler::new(
        storage,
        facade as Arc<dyn EngineFacade>,
        Arc::new(AdmissionController::new(8, 16)),
        clock,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
    );
    scheduler.runtime_count = 1;
    scheduler
}

#[tokio::test(flavor = "multi_thread")]
async fn outer_scenario_pause_reprioritise_and_dependency_order() {
    let (repo_dir, repo) = init_repo(&three_task_roadmap());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(OuterFacade::new(storage.clone()));
    for task in ["t1", "t2", "t3"] {
        facade
            .commits
            .lock()
            .unwrap()
            .insert(task.into(), format!("work by {task}"));
    }
    let clock = Arc::new(MockClock::new(NOW));
    let queue = storage.task_queue_store();
    queue.register_project(&repo, NOW).unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    // Tick 1: t1 (high) dispatches; t2 is blocked on it, t3 waits because
    // only one task is dispatched per tick.
    sched.tick().await.unwrap();
    assert_eq!(facade.task_ids(), vec!["t1"], "highest priority first");

    // Mid-flight: pause the project. The next tick must not dispatch even
    // though t3 is ready and admission has slots.
    queue.set_project_paused(&repo, true, NOW).unwrap();
    sched.tick().await.unwrap();
    assert_eq!(
        facade.task_ids(),
        vec!["t1"],
        "a paused project dispatches nothing"
    );

    // Reprioritise t3 to critical while paused (the file and the row), then
    // resume: t3 must now dispatch before t2, which is still blocked.
    let mut roadmap = three_task_roadmap();
    roadmap.milestones[0].tasks[2].priority = Priority::Critical;
    std::fs::write(
        repo.join(PROJECT_ROADMAP_RELPATH),
        toml::to_string_pretty(&roadmap).unwrap(),
    )
    .unwrap();
    queue
        .set_priority(&repo, "t3", Priority::Critical, NOW)
        .unwrap();
    queue.set_project_paused(&repo, false, NOW).unwrap();

    sched.tick().await.unwrap();
    assert_eq!(
        facade.task_ids(),
        vec!["t1", "t3"],
        "after resume the reprioritised t3 dispatches next"
    );

    // Next tick: t3 settles, t2 unblocks (t1 verified and merged) and runs.
    sched.tick().await.unwrap();
    assert_eq!(facade.task_ids(), vec!["t1", "t3", "t2"]);

    // t2's worktree must contain t1's merged file — the integration branch
    // advanced before the dependent dispatched.
    let t2_worktree = facade.worktree_of("t2");
    assert!(
        t2_worktree.join("t1.txt").exists(),
        "t2 must be based on the integration branch after t1 merged"
    );
    assert!(
        t2_worktree.join("t3.txt").exists(),
        "t3 merged before t2 too"
    );

    // G13: t1 dispatched under the pre-edit roadmap hash; the mid-flight
    // reprioritisation must not retroactively rewrite what its run recorded.
    let origin_hash_of_t1 = facade.origins.lock().unwrap().get("t1").copied();
    let roadmap_after_edit = std::fs::read(repo.join(PROJECT_ROADMAP_RELPATH)).unwrap();
    let hash_after_edit = ContentHash::compute(&roadmap_after_edit);
    assert!(
        origin_hash_of_t1.is_some_and(|h| h != hash_after_edit),
        "t1's recorded roadmap_hash must be the pre-edit one, not the edited file's"
    );

    // Final state: all three settled done.
    sched.tick().await.unwrap();
    let rows = queue
        .list(&TaskQueueFilter {
            project_root: Some(repo.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|r| r.dispatch_state == DispatchState::Done),
        "every task settles done: {rows:?}"
    );
    let _ = repo_dir;
}

/// The dependency rule under failure: t1 fails → t2 is reported blocked and
/// never dispatched, while t3 completes normally.
#[tokio::test(flavor = "multi_thread")]
async fn outer_scenario_failed_dependency_blocks_dependent() {
    let (repo_dir, repo) = init_repo(&three_task_roadmap());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(OuterFacade::new(storage.clone()));
    let clock = Arc::new(MockClock::new(NOW));
    let queue = storage.task_queue_store();
    queue.register_project(&repo, NOW).unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    sched.tick().await.unwrap(); // t1 dispatches
    // Simulate t1's run failing.
    {
        let row = queue.get(&repo, "t1").unwrap().unwrap();
        let run_id = row.run_id.unwrap();
        let writer = storage.open_run_writer(run_id).await.unwrap();
        writer
            .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
                error: "implementation failed".into(),
            }))
            .await
            .unwrap();
        writer.flush().await.unwrap();
        writer.close().await.unwrap();
        storage
            .set_run_status(&run_id, surge_core::RunStatus::Failed, Some(NOW))
            .await
            .unwrap();
    }
    sched.tick().await.unwrap(); // reconcile: t1 failed, t3 dispatches

    assert_eq!(facade.task_ids(), vec!["t1", "t3"], "t2 stays blocked");

    let t1 = queue.get(&repo, "t1").unwrap().unwrap();
    assert_eq!(t1.dispatch_state, DispatchState::Failed);
    let t2 = queue.get(&repo, "t2").unwrap().unwrap();
    assert_eq!(
        t2.dispatch_state,
        DispatchState::Queued,
        "a dependent of a failed task is not dispatched and not failed itself"
    );
    let _ = repo_dir;
}
