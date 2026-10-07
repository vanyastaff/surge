//! Real storage failures must not publish half of a stage boundary.
mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::sync::Arc;
use surge_acp::bridge::BridgeEvent;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_stage_snapshot_rolls_back_routing_events() {
    let home = FixtureHome::new().unwrap();
    {
        let workspace = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let session = surge_core::SessionId::new();
        bridge.pin_next_session_id(session).await;
        bridge
            .enqueue_event(BridgeEvent::OutcomeReported {
                session,
                outcome: surge_core::OutcomeKey::try_from("done").unwrap(),
                summary: "controlled completed turn".into(),
                artifacts_produced: Vec::new(),
                verification_report: None,
            })
            .await;
        let engine = Engine::new(
            bridge.clone(),
            storage.clone(),
            Arc::new(WorktreeToolDispatcher::new(workspace.path().into())),
            EngineConfig::default(),
        );
        let run = surge_core::RunId::new();
        let graph: surge_core::Graph =
            toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
        let handle = engine
            .start_run(
                run,
                graph,
                workspace.path().into(),
                EngineRunConfig {
                    initial_prompt: "Exercise the accepted stage boundary".into(),
                    ..EngineRunConfig::default()
                },
            )
            .await
            .unwrap();
        bridge.wait_for_subscribe_count(1).await;
        let journal = home
            .path()
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite");
        let output = std::process::Command::new("python3").args([
        "-c", "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute(\"CREATE TRIGGER fixture_reject_snapshot BEFORE INSERT ON graph_snapshots BEGIN SELECT RAISE(ABORT, 'fixture snapshot failure'); END\"); c.commit()",
    ]).arg(journal).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        bridge.pump_after_subscribe(1).await;
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(8), handle.await_completion())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
        let events = storage
            .open_run_reader(run)
            .await
            .unwrap()
            .read_events(EventSeq(0)..EventSeq(100))
            .await
            .unwrap();
        assert!(
            !events.iter().any(|event| matches!(
                event.payload.payload(),
                EventPayload::EdgeTraversed { .. } | EventPayload::StageCompleted { .. }
            )),
            "routing without its durable post-route cursor must not commit"
        );
        let reader = storage.open_run_reader(run).await.unwrap();
        assert!(
            reader.list_snapshots().await.unwrap().is_empty(),
            "failed first route must not publish a cursor or traversal counters"
        );
    }
    home.close().unwrap();
}
