//! The child process contains the baseline detached task after observing RED.
//! Runtime teardown is cleanup only: the ownership assertion samples beforehand.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use surge_core::{RunId, graph::Graph};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::storage::reader_pool_witness::ReaderPoolWitness;

#[path = "../../../tests/fixtures/mock_bridge.rs"]
mod mock_bridge;
pub(crate) mod runtime_home {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

const CHILD_ENV: &str = "SURGE_FORWARDER_POOL_ORACLE_CHILD";
const TEST_NAME: &str = "ownership_tests::completed_run_releases_actual_reader_pool_owners";
const RELEASE_DEADLINE: Duration = Duration::from_secs(3);

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        _call: &ToolCall,
    ) -> ToolResultPayload {
        panic!("terminal-only graph must not dispatch tools");
    }
}

#[test]
fn completed_run_releases_actual_reader_pool_owners() {
    if std::env::var_os(CHILD_ENV).is_some() {
        run_child();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, "1")
        .spawn()
        .expect("start isolated ownership oracle");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().expect("observe oracle process") {
            assert!(
                status.success(),
                "isolated pool ownership oracle failed: {status}"
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate timed-out oracle process");
            child.wait().expect("settle timed-out oracle process");
            panic!("isolated pool ownership oracle exceeded 60 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_child() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let observed = runtime.block_on(observe_completed_run(home.path(), worktree.path()));
    // Cancels the baseline infinite forwarder and waits for actual blocking SQL.
    // This cannot turn the earlier retained-manager observation into a pass.
    drop(runtime);
    let cleanup = home.close();
    let (count, retained, notified) = observed;
    assert!(
        count > 0,
        "oracle must observe actual engine reader pool managers"
    );
    assert_eq!(
        retained, 0,
        "completed engine run retained {retained}/{count} actual reader pool managers after every legitimate owner was dropped"
    );
    assert!(notified, "every actual manager must report destruction");
    cleanup.expect("all child runtime owners closed before fixture cleanup");
}

async fn observe_completed_run(home: &Path, worktree: &Path) -> (usize, usize, bool) {
    let storage = Storage::open(home).await.unwrap();
    verify_stream_owns_real_manager(&storage, worktree).await;
    let engine = Engine::new(
        Arc::new(mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let tap = engine.subscribe_tap();
    let graph: Graph = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/flow_terminal_only.toml"
    )))
    .unwrap();
    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            graph,
            worktree.to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let outcome = handle.await_completion().await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let mut witnesses = storage.take_reader_pool_witnesses(run_id).unwrap();
    drop(tap);
    drop(engine);
    drop(storage);
    let notified = wait_for_release(&mut witnesses).await;
    let retained = witnesses
        .iter()
        .filter(|witness| witness.is_retained())
        .count();
    (witnesses.len(), retained, notified)
}

async fn verify_stream_owns_real_manager(storage: &Arc<Storage>, worktree: &Path) {
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, worktree, None).await.unwrap();
    writer.close().await.unwrap();
    // Discard the closed writer's observations; only the explicit reader follows.
    drop(storage.take_reader_pool_witnesses(run_id).unwrap());
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let stream = reader.subscribe_events();
    let mut witnesses = storage.take_reader_pool_witnesses(run_id).unwrap();
    drop(reader);
    assert_eq!(witnesses.len(), 1);
    assert!(
        witnesses[0].is_retained(),
        "unpolled public stream must retain actual manager"
    );
    drop(stream);
    assert!(
        wait_for_release(&mut witnesses).await,
        "dropping the final stream must destroy its manager"
    );
    assert!(!witnesses[0].is_retained());
}

async fn wait_for_release(witnesses: &mut [ReaderPoolWitness]) -> bool {
    tokio::time::timeout(RELEASE_DEADLINE, async {
        for witness in witnesses {
            if witness.released().await.is_err() {
                return false;
            }
        }
        true
    })
    .await
    .unwrap_or(false)
}

fn terminal_graph() -> Graph {
    toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/flow_terminal_only.toml"
    )))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_start_closes_writer_before_returning_original_error() {
    use surge_orchestrator::engine::config::RunSeedArtifact;
    let home = runtime_home::FixtureHome::new().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    std::fs::write(worktree.path().join("blocked"), "ordinary file").unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let engine = Engine::new(
        Arc::new(mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let run = RunId::new();
    let config = EngineRunConfig {
        seed_artifacts: vec![
            RunSeedArtifact::new("seed", "blocked/seed.md", "body", "seed").unwrap(),
        ],
        ..EngineRunConfig::default()
    };
    let error = match engine
        .start_run(run, terminal_graph(), worktree.path().into(), config)
        .await
    {
        Ok(_) => panic!("startup must reject its actual non-directory parent"),
        Err(error) => error,
    };
    assert!(!matches!(
        error,
        surge_orchestrator::engine::EngineError::OperationAndWriterClose { .. }
    ));
    assert!(
        error.to_string().contains("seed"),
        "original error retained: {error}"
    );
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut witnesses = storage.take_reader_pool_witnesses(run).unwrap();
    drop(engine);
    drop(storage);
    assert!(wait_for_release(&mut witnesses).await);
    assert!(witnesses.iter().all(|witness| !witness.is_retained()));
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_resume_closes_writer_before_returning_replay_error() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    storage
        .create_run(run, home.path(), None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let engine = Engine::new(
        Arc::new(mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    assert!(engine.resume_run(run, home.path().into()).await.is_err());
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut witnesses = storage.take_reader_pool_witnesses(run).unwrap();
    drop(engine);
    drop(storage);
    assert!(wait_for_release(&mut witnesses).await);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_resume_replays_full_prefix_and_releases_reader_owners() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let engine = Engine::new(
        Arc::new(mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let run = RunId::new();
    let mut first_tap = engine.subscribe_tap();
    let original = engine
        .start_run(
            run,
            terminal_graph(),
            worktree.path().into(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap()
        .await_completion()
        .await
        .unwrap();
    assert!(matches!(original, RunOutcome::Completed { .. }));
    let mut first = Vec::new();
    while let Ok(event) = first_tap.try_recv() {
        first.push(event.event.seq);
    }
    drop(first_tap);
    let mut resumed_tap = engine.subscribe_tap();
    let resumed = engine
        .resume_run(run, worktree.path().into())
        .await
        .unwrap()
        .await_completion()
        .await
        .unwrap();
    assert_eq!(resumed, original);
    let mut second = Vec::new();
    while let Ok(event) = resumed_tap.try_recv() {
        second.push(event.event.seq);
    }
    assert_eq!(first.first(), Some(&surge_persistence::runs::EventSeq(1)));
    assert_eq!(second, first);
    drop(resumed_tap);
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut witnesses = storage.take_reader_pool_witnesses(run).unwrap();
    drop(engine);
    drop(storage);
    assert!(wait_for_release(&mut witnesses).await);
    assert!(witnesses.iter().all(|witness| !witness.is_retained()));
    home.close().unwrap();
}
