//! Task 19 — bootstrap driver smoke test with scripted agents and approvals.

mod fixtures;

use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::id::{RunId, SessionId};
use surge_core::keys::OutcomeKey;
use surge_orchestrator::bootstrap_driver::run_bootstrap_in_worktree;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;

async fn wait_for_subscribe_count(mock: &fixtures::mock_bridge::MockBridge, expected: usize) {
    for _ in 0..100 {
        let count = mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, fixtures::mock_bridge::RecordedCall::Subscribe))
            .count();
        if count >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {expected} bridge subscribers");
}

async fn approve_next_gate(
    engine: &Engine,
    run_id: RunId,
    tap: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = tap.recv().await.unwrap();
            if event.run_id != run_id {
                continue;
            }
            if let surge_core::EventPayload::HumanInputRequested { node, call_id, .. } =
                event.event.payload.payload
            {
                engine
                    .resolve_requested_input(
                        run_id,
                        node,
                        call_id,
                        serde_json::json!({"outcome":"approve","comment":"ok"}),
                    )
                    .await
                    .unwrap();
                return;
            }
        }
    })
    .await
    .unwrap();
}

async fn report_agent_outcome(
    mock: &fixtures::mock_bridge::MockBridge,
    session: SessionId,
    outcome: &str,
    artifacts: &[&str],
) {
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::try_from(outcome).unwrap(),
        summary: "scripted".into(),
        artifacts_produced: artifacts.iter().map(|a| (*a).into()).collect(),

        verification_report: None,
    })
    .await;
    mock.pump_scripted_events().await;
}

/// Write a trivial always-succeeding executable that ignores its arguments,
/// for use as `SURGE_BIN`.
///
/// `description-author@1.0` and `roadmap-planner@1.0` each declare a real
/// `on_outcome` hook (`{surge} artifact validate --kind ...`, `on_failure =
/// "reject"`) that `ProcessSpawner` shells out to. With no `SURGE_BIN`,
/// `resolve_surge_bin` falls back to `current_exe()` — this *test* binary —
/// so the hook would invoke the test harness itself with `artifact validate
/// ...` as libtest arguments, which it rejects, driving every outcome into
/// an endless reject/retry loop the scripted mock bridge can never satisfy.
/// This test exercises the bootstrap driver's graph plumbing, not the
/// hook's shell-validation path (already covered by
/// `crates/surge-orchestrator/src/engine/hooks/mod.rs`'s own tests), so a
/// stand-in that always succeeds is the right fixture here.
fn write_noop_surge_bin(dir: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let path = dir.join("surge-noop.bat");
        std::fs::write(&path, "@exit /b 0\r\n").unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("surge-noop.sh");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_bootstrap_materializes_followup_graph() {
    let bin_dir = tempfile::tempdir().unwrap();
    let noop_bin = write_noop_surge_bin(bin_dir.path());
    // SAFETY: this is the only test in this binary and it runs before any
    // other code reads the environment, so there is no data race.
    unsafe {
        std::env::set_var("SURGE_BIN", &noop_bin);
    }
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    // `flow_generator` in the bundled bootstrap flow binds `profile_catalog`
    // as a required `run_artifact`, which `Engine::start_run` only seeds when
    // a `ProfileRegistry` is wired (see `engine_profile_catalog_seed_test.rs`).
    // Without it, the third stage never opens a session and this test hangs
    // waiting for a subscriber that never appears.
    let profile_registry = Arc::new(ProfileRegistry::new(DiskProfileSet::empty()));
    let engine = Arc::new(Engine::new(
        bridge,
        storage,
        dispatcher,
        EngineConfig {
            profile_registry: Some(profile_registry),
            ..EngineConfig::default()
        },
    ));

    let mut gate_tap = engine.subscribe_tap();
    let run_id = RunId::new();
    let description_session = SessionId::new();
    let roadmap_session = SessionId::new();
    let flow_session = SessionId::new();
    mock.pin_session_ids(vec![description_session, roadmap_session, flow_session])
        .await;

    let driver_engine = engine.clone();
    let worktree = dir.path().to_path_buf();
    let driver = tokio::spawn(async move {
        run_bootstrap_in_worktree(
            driver_engine.as_ref(),
            "build an adaptive bootstrap flow".into(),
            run_id,
            worktree,
            None,
            Some(store_path),
        )
        .await
    });

    // Contents must satisfy each profile's `produced_artifacts` contract
    // (`validate_profile_artifact_contracts` in `engine/stage/agent.rs`) —
    // reuse the repo's own canonical valid fixtures rather than inventing
    // under-specified content that the contract would reject.
    wait_for_subscribe_count(&mock, 1).await;
    tokio::fs::write(
        dir.path().join("description.md"),
        include_str!("fixtures/artifacts/valid/description.md"),
    )
    .await
    .unwrap();
    report_agent_outcome(&mock, description_session, "drafted", &["description.md"]).await;
    approve_next_gate(engine.as_ref(), run_id, &mut gate_tap).await;

    // `roadmap-planner@1.0` declares `required_artifacts = ["roadmap.toml",
    // "roadmap.md"]` — both must be written and reported.
    wait_for_subscribe_count(&mock, 2).await;
    tokio::fs::write(
        dir.path().join("roadmap.toml"),
        include_str!("fixtures/artifacts/valid/roadmap.toml"),
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir.path().join("roadmap.md"),
        include_str!("fixtures/artifacts/valid/roadmap.md"),
    )
    .await
    .unwrap();
    report_agent_outcome(
        &mock,
        roadmap_session,
        "drafted",
        &["roadmap.toml", "roadmap.md"],
    )
    .await;
    approve_next_gate(engine.as_ref(), run_id, &mut gate_tap).await;

    wait_for_subscribe_count(&mock, 3).await;
    tokio::fs::write(
        dir.path().join("flow.toml"),
        include_str!("fixtures/golden_multi_milestone_flow.toml"),
    )
    .await
    .unwrap();
    report_agent_outcome(&mock, flow_session, "drafted", &["flow.toml"]).await;
    approve_next_gate(engine.as_ref(), run_id, &mut gate_tap).await;

    let materialized = driver.await.unwrap().expect("bootstrap driver succeeds");
    assert_eq!(materialized.bootstrap_run_id, run_id);
    assert_eq!(
        materialized.materialized_graph.metadata.name,
        "golden_multi_milestone"
    );
    assert_eq!(
        materialized
            .artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect::<Vec<_>>(),
        vec!["description", "roadmap", "flow"]
    );
}
