use super::*;
use crate::engine::tools::{ToolCall, ToolDispatchContext, ToolResultPayload};
use std::time::Duration;
use surge_acp::bridge::acp_bridge::AcpBridge;
mod runtime_home {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

struct UnusedDispatcher;
#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        _call: &ToolCall,
    ) -> ToolResultPayload {
        panic!("unpolled completion must not dispatch a tool");
    }
}

struct ReleaseGate(Option<tokio::sync::oneshot::Sender<()>>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[test]
fn aborted_public_completion_before_first_inner_poll_releases_real_owners() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let observed = runtime.block_on(abort_before_first_poll(home.path(), worktree.path()));
    drop(runtime);
    let cleanup = home.close();
    let (cancelled, no_terminal, observed_managers, released, retained) = observed;
    assert!(
        cancelled,
        "the actual public completion must return cancelled/UNKNOWN"
    );
    assert!(
        no_terminal,
        "an unpolled execution cannot publish a confirmed terminal result"
    );
    assert!(
        observed_managers > 0,
        "actual engine reader managers must be observed"
    );
    assert!(
        released,
        "actual managers must be destroyed before runtime teardown"
    );
    assert_eq!(
        retained, 0,
        "saved pre-teardown Weak observation must show no retained manager"
    );
    cleanup.unwrap();
}

async fn abort_before_first_poll(home: &Path, worktree: &Path) -> (bool, bool, usize, bool, usize) {
    let storage = surge_persistence::runs::Storage::open(home).await.unwrap();
    let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
    let engine = Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let release = ReleaseGate(Some(release));
    *engine.completion_poll_gate.lock().unwrap() = Some(CompletionPollGate {
        entered,
        release: released,
    });
    let graph: Graph = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/flow_terminal_only.toml"
    )))
    .unwrap();
    let run = RunId::new();
    let handle = engine
        .start_run(
            run,
            graph,
            worktree.to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    entry.await.unwrap();
    let RunHandle {
        completion,
        mut events,
        ..
    } = handle;
    completion.abort();
    let cancelled = completion.await.unwrap_err().is_cancelled();
    drop(release);
    let mut no_terminal = true;
    while let Ok(event) = events.try_recv() {
        no_terminal &= !matches!(event, super::super::handle::EngineRunEvent::Terminal { .. });
    }
    drop(events);
    let mut witnesses = storage.take_reader_pool_witnesses(run).unwrap();
    drop(engine);
    drop(storage);
    let released = tokio::time::timeout(Duration::from_secs(3), async {
        for witness in &mut witnesses {
            witness.released().await.unwrap();
        }
    })
    .await
    .is_ok();
    let retained = witnesses
        .iter()
        .filter(|witness| witness.is_retained())
        .count();
    Arc::try_unwrap(bridge)
        .ok()
        .expect("all actual completion bridge owners dropped")
        .shutdown()
        .await
        .unwrap();
    (cancelled, no_terminal, witnesses.len(), released, retained)
}

#[test]
fn aborted_public_completion_during_sql_keeps_original_worker_lease() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let observed = runtime.block_on(abort_public_completion_during_sql(
        home.path(),
        worktree.path(),
    ));
    drop(runtime);
    let cleanup = home.close();
    let (cancelled, busy, no_terminal, released, retained) = observed;
    assert!(cancelled, "public completion must remain cancelled/UNKNOWN");
    assert!(
        busy,
        "actual blocked worker must retain original writer exclusion"
    );
    assert!(
        no_terminal,
        "blocked forwarder cannot publish confirmed terminal completion"
    );
    assert!(
        released,
        "real reader managers must end before runtime teardown"
    );
    assert_eq!(
        retained, 0,
        "post-release saved Weak observation must be empty"
    );
    cleanup.unwrap();
}

async fn abort_public_completion_during_sql(
    home: &Path,
    worktree: &Path,
) -> (bool, bool, bool, bool, usize) {
    use crate::engine::test_support::{FeedProbe, FeedProbeKind};
    let storage = surge_persistence::runs::Storage::open(home).await.unwrap();
    let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
    let engine = Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let run = RunId::new();
    let probe = engine
        .install_feed_probe_for_test(run, FeedProbeKind::AfterCheckout)
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
    let FeedProbe::AfterCheckout { mut gate } = probe.await.unwrap().unwrap() else {
        panic!("expected only a channel gate, never a fixture-owned pool or writer lease");
    };
    gate.entered().await.unwrap();
    let RunHandle {
        completion,
        mut events,
        ..
    } = handle;
    completion.abort();
    let cancelled = completion.await.unwrap_err().is_cancelled();
    let busy = match storage.open_run_writer(run).await {
        Err(surge_persistence::runs::OpenError::WriterAlreadyHeld { .. }) => true,
        Ok(unexpected) => {
            unexpected.close().await.unwrap();
            false
        },
        Err(_) => false,
    };
    gate.release();
    let mut no_terminal = true;
    while let Ok(event) = events.try_recv() {
        no_terminal &= !matches!(event, super::super::handle::EngineRunEvent::Terminal { .. });
    }
    drop(events);
    let mut witnesses = storage.take_reader_pool_witnesses(run).unwrap();
    drop(engine);
    drop(storage);
    let released = tokio::time::timeout(Duration::from_secs(3), async {
        for witness in &mut witnesses {
            witness.released().await.unwrap();
        }
    })
    .await
    .is_ok();
    let retained = witnesses
        .iter()
        .filter(|witness| witness.is_retained())
        .count();
    Arc::try_unwrap(bridge)
        .ok()
        .expect("cancelled completion released actual bridge")
        .shutdown()
        .await
        .unwrap();
    (cancelled, busy, no_terminal, released, retained)
}

struct RetirementObservation {
    completed: bool,
    stop_found_registration: bool,
    replacement_cancelled: bool,
}

#[test]
fn settled_completion_preserves_replacement_registration() {
    let observed = observe_registration_retirement(true);
    assert!(observed.completed, "the original real run must complete");
    assert!(
        observed.stop_found_registration,
        "old completion must not remove the replacement registration"
    );
    assert!(
        observed.replacement_cancelled,
        "public stop must reach the replacement's actual cancellation token"
    );
}

#[test]
fn settled_completion_retires_its_own_registration() {
    let observed = observe_registration_retirement(false);
    assert!(observed.completed, "the original real run must complete");
    assert!(
        !observed.stop_found_registration,
        "normal joined completion must retire its own registration"
    );
}

fn observe_registration_retirement(replace: bool) -> RetirementObservation {
    let home = runtime_home::FixtureHome::new().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let observed = runtime.block_on(registration_retirement(
        home.path(),
        worktree.path(),
        replace,
    ));
    drop(runtime);
    home.close().unwrap();
    observed
}

async fn registration_retirement(
    home: &Path,
    worktree: &Path,
    replace: bool,
) -> RetirementObservation {
    let storage = surge_persistence::runs::Storage::open(home).await.unwrap();
    let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
    let engine = Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        EngineConfig::default(),
    );
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let release = ReleaseGate(Some(release));
    *engine.completion_retirement_gate.lock().unwrap() = Some(CompletionPollGate {
        entered,
        release: released,
    });
    let graph: Graph = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/flow_terminal_only.toml"
    )))
    .unwrap();
    let run = RunId::new();
    let handle = engine
        .start_run(
            run,
            graph,
            worktree.to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    entry.await.unwrap();
    // This probes the actual registration primitive, not terminal-run resume admission.
    // Acquiring a real replacement writer proves the old writer/feed lease has ended.
    let replacement = if replace {
        let writer = storage.open_run_writer(run).await.unwrap();
        Some((writer, engine.register_active_run(run).await))
    } else {
        None
    };
    drop(release);
    let RunHandle {
        completion, events, ..
    } = handle;
    let completed = matches!(
        completion.await.unwrap(),
        crate::engine::RunOutcome::Completed { .. }
    );
    let stop_found_registration = match engine.stop_run(run, "replacement probe".into()).await {
        Ok(()) => true,
        Err(EngineError::RunNotFound(missing)) if missing == run => false,
        Err(error) => panic!("unexpected public stop error: {error}"),
    };
    let replacement_cancelled = if let Some((writer, registration)) = replacement {
        let cancelled = registration.cancel.is_cancelled();
        writer.close().await.unwrap();
        drop(registration);
        cancelled
    } else {
        false
    };
    drop(events);
    drop(engine);
    drop(storage);
    Arc::try_unwrap(bridge)
        .ok()
        .expect("joined run released actual bridge")
        .shutdown()
        .await
        .unwrap();
    RetirementObservation {
        completed,
        stop_found_registration,
        replacement_cancelled,
    }
}
