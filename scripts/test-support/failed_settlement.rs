// Shared actual-Engine producer for historical settlement regressions.
// Included only inside #[cfg(test)] modules; never supplies a synthetic SQL error.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_core::{RunId, graph::Graph};
use surge_orchestrator::engine::handle::RunSettlementFailure;
use surge_orchestrator::engine::test_support::{FeedProbe, FeedProbeKind};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_orchestrator::engine::{
    Engine, EngineConfig, EngineRunConfig, EngineRunEvent, RunOutcome,
};
use surge_persistence::runs::{EventSeq, Storage};

const DEADLINE: Duration = Duration::from_secs(15);

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(
        &self,
        _context: &ToolDispatchContext<'_>,
        _call: &ToolCall,
    ) -> ToolResultPayload {
        panic!("terminal-only historical regression must not dispatch tools");
    }
}

pub async fn produce(home: &Path, worktree: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
    let engine = Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let run = RunId::new();
    let probe = engine
        .install_feed_probe_for_test(run, FeedProbeKind::BeforeCheckout)
        .unwrap();
    let graph: Graph = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/flow_terminal_only.toml"
    )))
    .unwrap();
    let handle = engine
        .start_run(
            run,
            graph,
            worktree.to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let FeedProbe::BeforeCheckout { mut gate, mut pool } = probe.await.unwrap().unwrap() else {
        panic!("producer requires the pre-checkout probe");
    };
    tokio::time::timeout(DEADLINE, gate.entered())
        .await
        .unwrap()
        .unwrap();
    let prefix = tokio::time::timeout(DEADLINE, pool.wait_sealed())
        .await
        .unwrap()
        .unwrap();
    assert!(
        prefix.0 > 0,
        "actual writer must seal a nonempty terminal journal"
    );
    let (pool, slots) = tokio::task::spawn_blocking(move || {
        pool.check_available_zero_timeout().unwrap();
        let slots = pool.exhaust_all_slots().unwrap();
        assert!(slots.capacity() > 0);
        assert_eq!(slots.count(), slots.capacity());
        pool.verify_exhausted_zero_timeout().unwrap();
        (pool, slots)
    })
    .await
    .unwrap();
    gate.release();
    let mut events = handle.events;
    let outcome = tokio::time::timeout(DEADLINE, handle.completion)
        .await
        .unwrap()
        .unwrap();
    let stream = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        drop(slots);
        pool.check_available_zero_timeout().unwrap();
        drop(pool);
    })
    .await
    .unwrap();
    drop(gate);
    drop(events);
    drop(engine);
    drop(storage);
    Arc::try_unwrap(bridge)
        .ok()
        .expect("all engine bridge owners released")
        .shutdown()
        .await
        .unwrap();
    verify_live_outcome(&outcome, &stream);
    verify_reopened_journal(home, run, prefix).await;
    println!(
        "actual producer controls passed: run={run}, sealed_prefix={}, actual ForwarderRead after writer seal; checkout restored; valid terminal journal reopened",
        prefix.0
    );
    run
}

fn verify_live_outcome(outcome: &RunOutcome, stream: &[EngineRunEvent]) {
    assert!(
        matches!(
            outcome,
            RunOutcome::SettlementFailed { original, failures }
                if matches!(original.as_ref(), RunOutcome::Completed { .. })
                    && failures.as_slice() == [RunSettlementFailure::ForwarderRead]
        ),
        "actual producer returned {outcome:?}"
    );
    assert!(
        stream
            .iter()
            .any(|event| matches!(event, EngineRunEvent::StreamError { .. }))
    );
    assert!(
        !stream
            .iter()
            .any(|event| matches!(event, EngineRunEvent::Terminal { .. }))
    );
}

async fn verify_reopened_journal(home: &Path, run: RunId, prefix: EventSeq) {
    let storage = Storage::open(home).await.unwrap();
    let reader = storage.open_run_reader(run).await.unwrap();
    let events = reader.read_run_events().await.unwrap();
    drop(reader);
    drop(storage);
    assert_eq!(
        events.last().unwrap().seq,
        prefix.0,
        "journal must end at the actual seal"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload,
                surge_core::run_event::EventPayload::RunCompleted { .. }
            ))
            .count(),
        1,
        "terminal journal is produced by the actual Engine exactly once"
    );
    assert!(matches!(
        surge_core::run_state::fold(&events).unwrap(),
        surge_core::RunState::Terminal { .. }
    ));
}
