//! Integration tests for `surge_daemon::task_scheduler` — the project task
//! dispatcher (ADR-0020, T5/T6).
//!
//! These drive real git repositories (a queue task's worktree is a real
//! worktree whose branch is merged back), a real registry, and a stub engine
//! facade that records dispatches and writes the events a run would write.
//!
//! Acceptance coverage:
//! - G3: a dependent task is not dispatched before its dependency is
//!   completed **and** merged (`dependent_task_dispatches_after_merge`).
//! - G10/G18: a claimed-but-never-started dispatch is re-queued exactly once
//!   (`claimed_but_unstarted_task_requeues_exactly_once`), and a live run is
//!   left alone (`live_run_is_left_dispatched`).
//! - G11: a merge conflict parks the project
//!   (`merge_conflict_parks_the_project`).

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
use surge_daemon::task_scheduler::{PROJECT_ROADMAP_RELPATH, TaskScheduler, TaskTemplateSource};
use surge_orchestrator::engine::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::runs::{MockClock, Storage};
use surge_persistence::task_queue::{DispatchState, TaskQueueFilter};
use tempfile::{TempDir, tempdir};
use tokio::sync::broadcast;

const NOW: i64 = 1_700_000_000_000;

/// A run behavior the stub facade applies when `start_run` is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StubBehavior {
    /// Write startup + completed + verified events, and commit a file in the
    /// worktree so the branch merges cleanly.
    CompleteVerified,
    /// Write startup + completed events but no `TaskVerified`.
    CompleteUnverified,
    /// Write startup events only (run appears live).
    Live,
    /// Write nothing at all — the crash-between-claim-and-start case.
    WriteNothing,
}

/// Records dispatches and fabricates the events a real run would write.
struct StubFacade {
    storage: Arc<Storage>,
    behavior: Mutex<StubBehavior>,
    dispatches: Mutex<Vec<(RunId, PathBuf, EngineRunConfig)>>,
    /// The graph each dispatch was given, for template-selection assertions.
    graphs: Mutex<Vec<surge_core::graph::Graph>>,
    /// Files to write into the worktree before completing, keyed by task id.
    commits: Mutex<HashMap<String, (String, String)>>,
}

impl StubFacade {
    fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            behavior: Mutex::new(StubBehavior::CompleteVerified),
            dispatches: Mutex::new(Vec::new()),
            graphs: Mutex::new(Vec::new()),
            commits: Mutex::new(HashMap::new()),
        }
    }

    fn set_behavior(&self, behavior: StubBehavior) {
        *self.behavior.lock().unwrap() = behavior;
    }

    fn task_ids(&self) -> Vec<String> {
        self.dispatches
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, cfg)| {
                cfg.origin
                    .as_ref()
                    .and_then(|origin| match origin {
                        surge_core::run_event::RunOrigin::Task { task_id, .. } => {
                            Some(task_id.clone())
                        },
                        _ => None,
                    })
                    .unwrap_or_default()
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl EngineFacade for StubFacade {
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
            .push((run_id, worktree_path.clone(), run_config.clone()));
        self.graphs.lock().unwrap().push(_graph.clone());

        let behavior = *self.behavior.lock().unwrap();
        if behavior == StubBehavior::WriteNothing {
            return Ok(immediate_handle(run_id));
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

        if behavior != StubBehavior::Live {
            // Commit a task-named file so the merge has something to bring
            // over (and a same-file conflict is controllable from the test).
            if let Some((file, contents)) = self.commits.lock().unwrap().get(&task_id).cloned() {
                commit_in_worktree(&worktree_path, &file, &contents);
            }
            writer
                .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("end").unwrap(),
                }))
                .await
                .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
            if behavior == StubBehavior::CompleteVerified {
                writer
                    .append_event(VersionedEventPayload::new(EventPayload::TaskVerified {
                        task_id: task_id.clone(),
                        node: NodeKey::try_from("verify_1").unwrap(),
                        evidence: ContentHash::compute(task_id.as_bytes()),
                    }))
                    .await
                    .map_err(|e| {
                        surge_orchestrator::engine::EngineError::Internal(e.to_string())
                    })?;
            }
        }
        writer
            .flush()
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        writer
            .close()
            .await
            .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        if behavior != StubBehavior::Live {
            self.storage
                .set_run_status(&run_id, surge_core::RunStatus::Completed, Some(NOW))
                .await
                .map_err(|e| surge_orchestrator::engine::EngineError::Internal(e.to_string()))?;
        }
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

fn init_repo_with_roadmap(roadmap: &RoadmapArtifact) -> (TempDir, PathBuf) {
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
    let roadmap_dir = path.join(".surge");
    std::fs::create_dir_all(&roadmap_dir).unwrap();
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
    let full = worktree.join(file);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, contents).unwrap();
    for args in [vec!["add", "."], vec!["commit", "-m", "task work"]] {
        Command::new("git")
            .args(&args)
            .current_dir(worktree)
            .output()
            .unwrap();
    }
}

fn scheduler(
    storage: Arc<Storage>,
    facade: Arc<StubFacade>,
    clock: Arc<MockClock>,
) -> TaskScheduler {
    let mut scheduler = TaskScheduler::new(
        storage,
        facade as Arc<dyn EngineFacade>,
        Arc::new(AdmissionController::new(8, 16)),
        clock,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
    );
    scheduler.template = TaskTemplateSource::BundledSingleTask;
    // One runtime: same-vendor verification is a warning, not a refusal, so
    // the fixture flows are selectable regardless of what is installed on
    // the test host.
    scheduler.runtime_count = 1;
    scheduler
}

fn two_task_chain() -> RoadmapArtifact {
    let mut milestone = RoadmapMilestone::new("m1", "Chain");
    let mut t1 = RoadmapTask::new("t1", "First");
    t1.size = Some(TaskSize::S);
    t1.priority = Priority::High;
    let mut t2 = RoadmapTask::new("t2", "Second");
    t2.size = Some(TaskSize::S);
    t2.depends_on = vec!["t1".to_string()];
    milestone.tasks.push(t1);
    milestone.tasks.push(t2);
    RoadmapArtifact::new(vec![milestone])
}

#[tokio::test(flavor = "multi_thread")]
async fn dependent_task_dispatches_after_merge() {
    let (repo_dir, repo) = init_repo_with_roadmap(&two_task_chain());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    facade
        .commits
        .lock()
        .unwrap()
        .insert("t1".into(), ("t1.txt".into(), "from t1".into()));
    facade
        .commits
        .lock()
        .unwrap()
        .insert("t2".into(), ("t2.txt".into(), "from t2".into()));
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    // Tick 1: t1 dispatches, t2 is blocked by dependencies.
    sched.tick().await.unwrap();
    assert_eq!(facade.task_ids(), vec!["t1"], "only the root dispatches");

    // Tick 2: t1 is completed+verified and merged, so t2 dispatches.
    sched.tick().await.unwrap();
    assert_eq!(
        facade.task_ids(),
        vec!["t1", "t2"],
        "dependent dispatches only after the dependency is done and merged"
    );

    // t1's commit must be on the integration branch before t2 ran: the
    // worktree t2 branched from contains t1.txt.
    let t2_worktree = {
        let dispatches = facade.dispatches.lock().unwrap();
        dispatches
            .iter()
            .find(|(_, _, cfg)| {
                matches!(
                    cfg.origin.as_ref(),
                    Some(surge_core::run_event::RunOrigin::Task { task_id, .. }) if task_id == "t2"
                )
            })
            .map(|(_, wt, _)| wt.clone())
            .unwrap()
    };
    assert!(
        t2_worktree.join("t1.txt").exists(),
        "t2's worktree must be based on the integration branch after t1 merged"
    );

    // Tick 3: t2's own run is reconciled and settled.
    sched.tick().await.unwrap();

    // Queue state: both settled done.
    let queue = storage.task_queue_store();
    let rows = queue
        .list(&TaskQueueFilter {
            project_root: Some(repo.clone()),
            ..Default::default()
        })
        .unwrap();
    assert!(rows.iter().all(|r| r.dispatch_state == DispatchState::Done));
    let _ = repo_dir;
}

#[tokio::test(flavor = "multi_thread")]
async fn unverified_completion_does_not_merge_or_unblock() {
    let (repo_dir, repo) = init_repo_with_roadmap(&two_task_chain());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    facade
        .commits
        .lock()
        .unwrap()
        .insert("t1".into(), ("t1.txt".into(), "from t1".into()));
    facade.set_behavior(StubBehavior::CompleteUnverified);
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    sched.tick().await.unwrap(); // dispatch t1
    sched.tick().await.unwrap(); // reconcile t1: completed, unverified

    assert_eq!(
        facade.task_ids(),
        vec!["t1"],
        "an unverified completion must not unblock the dependent"
    );
    let queue = storage.task_queue_store();
    let row = queue.get(&repo, "t1").unwrap().unwrap();
    assert_eq!(row.dispatch_state, DispatchState::Failed);
    assert!(
        !repo.join("t1.txt").exists(),
        "unverified completion must not advance the integration branch"
    );
    let _ = repo_dir;
}

#[tokio::test(flavor = "multi_thread")]
async fn claimed_but_unstarted_task_requeues_exactly_once() {
    let (repo_dir, repo) = init_repo_with_roadmap(&two_task_chain());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    facade.set_behavior(StubBehavior::WriteNothing);
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    sched.tick().await.unwrap(); // dispatch t1 (no events written)
    let queue = storage.task_queue_store();

    // First reconcile: the row goes back to queued, attempt 2.
    sched.tick().await.unwrap();
    let row = queue.get(&repo, "t1").unwrap().unwrap();
    assert_eq!(
        row.dispatch_state,
        DispatchState::Queued,
        "requeued after crash"
    );
    assert_eq!(row.attempt, 2, "attempt increments exactly once per crash");
    let _ = repo_dir;
}

#[tokio::test(flavor = "multi_thread")]
async fn live_run_is_left_dispatched() {
    let (repo_dir, repo) = init_repo_with_roadmap(&two_task_chain());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    facade.set_behavior(StubBehavior::Live);
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    sched.tick().await.unwrap(); // dispatch t1, run stays live
    sched.tick().await.unwrap(); // reconcile: live

    let queue = storage.task_queue_store();
    let row = queue.get(&repo, "t1").unwrap().unwrap();
    assert_eq!(row.dispatch_state, DispatchState::Dispatched);
    assert_eq!(row.attempt, 1, "a live run is neither requeued nor failed");
    assert_eq!(facade.task_ids(), vec!["t1"], "t2 waits for a live t1");
    let _ = repo_dir;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_conflict_parks_the_project() {
    let mut milestone = RoadmapMilestone::new("m1", "Conflict");
    let mut t1 = RoadmapTask::new("t1", "First");
    t1.size = Some(TaskSize::S);
    milestone.tasks.push(t1);
    let (repo_dir, repo) = init_repo_with_roadmap(&RoadmapArtifact::new(vec![milestone]));

    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    // The task commits a change to a file the integration branch also
    // changed after the worktree was cut, so the merge conflicts.
    facade
        .commits
        .lock()
        .unwrap()
        .insert("t1".into(), ("shared.txt".into(), "task version".into()));
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    sched.tick().await.unwrap(); // dispatch t1
    // Advance the integration branch on t1's file (after the worktree was cut).
    std::fs::write(repo.join("shared.txt"), "integration version").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-m", "conflicting change"]] {
        Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
    }
    sched.tick().await.unwrap(); // reconcile → merge → conflict → park

    let queue = storage.task_queue_store();
    assert!(
        queue.is_project_paused(&repo).unwrap(),
        "a merge conflict must pause the project queue"
    );
    let row = queue.get(&repo, "t1").unwrap().unwrap();
    assert_eq!(row.dispatch_state, DispatchState::Failed);
    let _ = repo_dir;
}

#[tokio::test(flavor = "multi_thread")]
async fn paused_project_dispatches_nothing() {
    let (repo_dir, repo) = init_repo_with_roadmap(&two_task_chain());
    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    let clock = Arc::new(MockClock::new(NOW));
    let sched = scheduler(storage.clone(), facade.clone(), clock);

    let queue = storage.task_queue_store();
    queue.register_project(&repo, NOW).unwrap();
    queue.set_project_paused(&repo, true, NOW).unwrap();
    sched.tick().await.unwrap();

    assert!(
        facade.task_ids().is_empty(),
        "a paused project must not dispatch"
    );
    let _ = repo_dir;
}

/// A task whose size is `l` selects `linear-3` — the dispatch must carry the
/// selected template's graph (its node keys differ from every other bundled
/// flow), not a project-wide one.
#[tokio::test(flavor = "multi_thread")]
async fn dispatch_uses_the_size_selected_template() {
    let mut milestone = RoadmapMilestone::new("m1", "One");
    let mut t1 = RoadmapTask::new("t1", "Big work");
    t1.size = Some(TaskSize::L);
    let (repo_dir, repo) = init_repo_with_roadmap(&RoadmapArtifact::new(vec![milestone.clone()]));
    milestone.tasks.push(t1);
    std::fs::write(
        repo.join(PROJECT_ROADMAP_RELPATH),
        toml::to_string_pretty(&RoadmapArtifact::new(vec![milestone])).unwrap(),
    )
    .unwrap();

    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);
    sched.tick().await.unwrap();

    let graphs = facade.graphs.lock().unwrap();
    let graph = graphs.first().expect("one dispatch");
    assert!(
        graph
            .nodes
            .contains_key(&surge_core::keys::NodeKey::try_from("verify_1").unwrap()),
        "linear-3 carries a verifier; the dispatched graph must be the selected template"
    );
    let _ = repo_dir;
}

/// A pinned flow is resolved from the catalog; an unresolvable pin is a
/// named refusal (the row is requeued, nothing dispatches).
#[tokio::test(flavor = "multi_thread")]
async fn unresolvable_pin_refuses_dispatch() {
    let mut milestone = RoadmapMilestone::new("m1", "One");
    let mut t1 = RoadmapTask::new("t1", "Pinned");
    t1.size = Some(TaskSize::M);
    t1.flow = Some("does-not-exist@1".parse().unwrap());
    milestone.tasks.push(t1);
    let (repo_dir, repo) = init_repo_with_roadmap(&RoadmapArtifact::new(vec![milestone]));

    let storage = Storage::open(repo_dir.path().join("surge-home"))
        .await
        .unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let sched = scheduler(storage.clone(), facade.clone(), clock);
    sched.tick().await.unwrap();

    assert!(facade.task_ids().is_empty(), "nothing may dispatch");
    let row = storage
        .task_queue_store()
        .get(&repo, "t1")
        .unwrap()
        .unwrap();
    assert_eq!(
        row.dispatch_state,
        DispatchState::Queued,
        "the row is requeued"
    );
    assert_eq!(row.attempt, 2, "the refusal is one attempt");
    let _ = repo_dir;
}

/// T13/G15: an untrusted `.surge/` file stops dispatch; pinning it with the
/// trust store lets the next tick proceed.
#[tokio::test(flavor = "multi_thread")]
async fn untrusted_project_file_blocks_dispatch_until_accepted() {
    use surge_core::artifact_contract::RelPath;
    use surge_persistence::trust_store::{TrustStore, repo_id};

    let (repo_dir, repo) = init_repo_with_roadmap(&two_task_chain());
    // A project profile makes the repository carry composed context.
    std::fs::create_dir_all(repo.join(".surge/profiles")).unwrap();
    std::fs::write(
        repo.join(".surge/profiles/custom-1.0.toml"),
        "role = { id = \"custom\", version = \"1.0.0\" }\n",
    )
    .unwrap();

    let surge_home = repo_dir.path().join("surge-home");
    let storage = Storage::open(&surge_home).await.unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let mut sched = scheduler(storage.clone(), facade.clone(), clock);
    sched.surge_home = Some(surge_home.clone());

    // Unpinned: nothing dispatches.
    sched.tick().await.unwrap();
    assert!(
        facade.task_ids().is_empty(),
        "an untrusted project must not dispatch"
    );

    // Pin every project file; the next tick dispatches.
    let mut store = TrustStore::open(&surge_home, &repo_id(None, &repo)).unwrap();
    store
        .accept_file(
            &repo,
            &RelPath::new(".surge/profiles/custom-1.0.toml").unwrap(),
            NOW,
        )
        .unwrap();
    sched.tick().await.unwrap();
    assert_eq!(
        facade.task_ids(),
        vec!["t1"],
        "pinning the files must unblock dispatch"
    );
    let _ = repo_dir;
}

/// Only executable composition is gated: editing `roadmap.toml` must not
/// prompt (it is planning data), while adding a profile must.
#[test]
fn trust_gate_covers_composition_not_planning_files() {
    use surge_persistence::trust_store::is_trust_gated_path;

    assert!(is_trust_gated_path(".surge/profiles/x-1.0.toml"));
    assert!(is_trust_gated_path(".surge/flows/bug-fix-1.0.toml"));
    assert!(is_trust_gated_path(".surge/skills/foo/SKILL.md"));
    assert!(is_trust_gated_path(".surge/mcp.toml"));
    assert!(!is_trust_gated_path(".surge/roadmap.toml"));
    assert!(!is_trust_gated_path(".surge/memory/note.md"));
}

/// G14: a project flow edited between two tasks affects only the next task,
/// and the trust gate makes the operator accept the change first.
///
/// The scenario uses the production path throughout: the project template
/// shadows the bundled one (FlowCatalog precedence), the edit changes the
/// project file's hash (TrustStore), and the dispatch refuses until
/// `trust accept` pins the new content.
#[tokio::test(flavor = "multi_thread")]
async fn flow_edit_mid_run_applies_to_the_next_task_after_trust() {
    use surge_core::artifact_contract::RelPath;
    use surge_persistence::trust_store::{TrustStore, repo_id};

    // Two independent M tasks so both select `linear-with-review`.
    let mut milestone = RoadmapMilestone::new("m1", "Flows");
    let mut t1 = RoadmapTask::new("t1", "First");
    t1.size = Some(TaskSize::M);
    let mut t2 = RoadmapTask::new("t2", "Second");
    t2.size = Some(TaskSize::M);
    milestone.tasks.push(t1);
    milestone.tasks.push(t2);
    let (repo_dir, repo) = init_repo_with_roadmap(&RoadmapArtifact::new(vec![milestone]));

    // A project copy of the bundled template, marked with a description the
    // stub facade can observe on the dispatched graph.
    let mut project_flow = surge_core::BundledFlows::by_name_latest("linear-with-review")
        .expect("bundled")
        .graph;
    project_flow.metadata.description = Some("project v1".into());
    let flows_dir = repo.join(".surge/flows");
    std::fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("linear-with-review-1.0.toml");
    std::fs::write(&flow_path, toml::to_string(&project_flow).unwrap()).unwrap();

    let surge_home = repo_dir.path().join("surge-home");
    let storage = Storage::open(&surge_home).await.unwrap();
    let facade = Arc::new(StubFacade::new(storage.clone()));
    facade
        .commits
        .lock()
        .unwrap()
        .insert("t1".into(), ("t1.txt".into(), "from t1".into()));
    facade
        .commits
        .lock()
        .unwrap()
        .insert("t2".into(), ("t2.txt".into(), "from t2".into()));
    let clock = Arc::new(MockClock::new(NOW));
    storage
        .task_queue_store()
        .register_project(&repo, NOW)
        .unwrap();
    let mut sched = scheduler(storage.clone(), facade.clone(), clock);
    sched.surge_home = Some(surge_home.clone());

    // Pin the project flow so the first dispatch is allowed.
    let rel = RelPath::new(".surge/flows/linear-with-review-1.0.toml").unwrap();
    let mut trust = TrustStore::open(&surge_home, &repo_id(None, &repo)).unwrap();
    trust.accept_file(&repo, &rel, NOW).unwrap();

    // Dispatch 1: the project template wins over the bundled one.
    sched.tick().await.unwrap();
    {
        let graphs = facade.graphs.lock().unwrap();
        assert_eq!(
            graphs[0].metadata.description.as_deref(),
            Some("project v1"),
            "the project's flow must shadow the bundled template"
        );
    }

    // Edit the flow while t1 is still in flight.
    project_flow.metadata.description = Some("project v2".into());
    std::fs::write(&flow_path, toml::to_string(&project_flow).unwrap()).unwrap();

    // Tick 2: t1 settles and merges, but t2 does not dispatch — the edited
    // flow is not trusted yet.
    sched.tick().await.unwrap();
    assert_eq!(
        facade.task_ids(),
        vec!["t1"],
        "the edited flow must not dispatch before it is trusted"
    );

    // Accept the edit and tick again: t2 runs on the edited template.
    trust.accept_file(&repo, &rel, NOW + 1).unwrap();
    sched.tick().await.unwrap();
    assert_eq!(facade.task_ids(), vec!["t1", "t2"]);
    {
        let graphs = facade.graphs.lock().unwrap();
        assert_eq!(
            graphs[1].metadata.description.as_deref(),
            Some("project v2"),
            "the next task must get the edited flow content"
        );
    }
    let _ = repo_dir;
}
