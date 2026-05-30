//! Integration test for crash recovery (`surge_daemon::recovery`).
//!
//! Seeds a registry with two non-terminal runs — one whose worktree
//! still exists (resumable) and one whose worktree is gone (un-resumable)
//! — then drives [`recover_on_startup`] against a stub engine facade and
//! asserts the resumable run was resumed and the worktree-lost run was
//! marked `Failed` in the registry.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::RunStatus;
use surge_orchestrator::engine::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::runs::Storage;
use tempfile::tempdir;
use tokio::sync::broadcast;

/// Stub facade that records `resume_run` calls and returns a handle that
/// completes immediately (so `spawn_forward_task` winds down cleanly).
struct RecoveryStubFacade {
    resume_calls: Mutex<Vec<RunId>>,
}

impl RecoveryStubFacade {
    fn new() -> Self {
        Self {
            resume_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl EngineFacade for RecoveryStubFacade {
    async fn start_run(
        &self,
        _run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        Err(surge_orchestrator::engine::EngineError::Internal(
            "stub: start_run not used in recovery test".into(),
        ))
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        self.resume_calls.lock().unwrap().push(run_id.clone());

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
        Ok(RunHandle {
            run_id,
            events: rx,
            completion,
        })
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

#[tokio::test(flavor = "multi_thread")]
async fn recover_resumes_live_worktree_and_fails_lost_worktree() {
    let tmp = tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let worktrees_root = tmp.path().join("worktrees");

    // Run A — worktree present → must be resumed.
    let run_a = RunId::new();
    let _wa = storage.create_run(run_a.clone(), "/proj", None).await.unwrap();
    std::fs::create_dir_all(worktrees_root.join(run_a.to_string())).unwrap();

    // Run B — worktree absent → must be marked Failed.
    let run_b = RunId::new();
    let _wb = storage.create_run(run_b.clone(), "/proj", None).await.unwrap();

    let stub = Arc::new(RecoveryStubFacade::new());
    let facade: Arc<dyn EngineFacade> = stub.clone();
    let admission = Arc::new(surge_daemon::admission::AdmissionController::new(8, 16));
    let broadcast = Arc::new(surge_daemon::broadcast::BroadcastRegistry::new());
    // A notifier with no channels: flag_stuck (unused here) would map
    // ChannelNotConfigured → Ok anyway.
    let notifier: Arc<dyn surge_notify::NotifyDeliverer> =
        Arc::new(surge_notify::MultiplexingNotifier::new());

    let now_ms = 1_700_000_000_000;
    let outcome = surge_daemon::recovery::recover_on_startup(
        &storage,
        &facade,
        &admission,
        &broadcast,
        &notifier,
        worktrees_root,
        now_ms,
    )
    .await;

    // Run A resumed exactly once.
    let resumed = stub.resume_calls.lock().unwrap().clone();
    assert_eq!(resumed, vec![run_a.clone()], "only run A should resume");

    // Run B marked Failed in the registry.
    let b = storage.get_run(&run_b).await.unwrap().unwrap();
    assert_eq!(b.status, RunStatus::Failed, "lost-worktree run → Failed");

    assert_eq!(outcome.resumed, 1);
    assert_eq!(outcome.failed_worktree, 1);
    assert_eq!(outcome.errors, 0);
}

/// Idempotency: a second recovery pass after the first must be a no-op
/// for runs already brought to terminal/failed — the worktree-lost run is
/// now `Failed` (terminal) and no longer a candidate.
#[tokio::test(flavor = "multi_thread")]
async fn second_recovery_pass_does_not_refail_terminal_run() {
    let tmp = tempdir().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    let worktrees_root = tmp.path().join("worktrees");

    let run_b = RunId::new();
    let _wb = storage.create_run(run_b.clone(), "/proj", None).await.unwrap();

    let stub = Arc::new(RecoveryStubFacade::new());
    let facade: Arc<dyn EngineFacade> = stub.clone();
    let admission = Arc::new(surge_daemon::admission::AdmissionController::new(8, 16));
    let broadcast = Arc::new(surge_daemon::broadcast::BroadcastRegistry::new());
    let notifier: Arc<dyn surge_notify::NotifyDeliverer> =
        Arc::new(surge_notify::MultiplexingNotifier::new());
    let now_ms = 1_700_000_000_000;

    let first = surge_daemon::recovery::recover_on_startup(
        &storage, &facade, &admission, &broadcast, &notifier,
        worktrees_root.clone(), now_ms,
    )
    .await;
    assert_eq!(first.failed_worktree, 1);

    // Second pass: run_b is now Failed (terminal) → not a candidate.
    let second = surge_daemon::recovery::recover_on_startup(
        &storage, &facade, &admission, &broadcast, &notifier,
        worktrees_root, now_ms,
    )
    .await;
    assert_eq!(second.failed_worktree, 0, "Failed run must not be re-processed");
    assert_eq!(second.resumed, 0);
    assert_eq!(second.skipped, 0, "terminal runs are filtered out entirely");
}
