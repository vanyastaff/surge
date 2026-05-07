//! Integration test for `DaemonRequest::SubscribeGlobal` and the
//! corresponding server-side fan-out of [`GlobalDaemonEvent`] frames.
//!
//! Setup: `run_server` with `max_active=4` and a stub facade whose
//! `start_run` returns a quickly-terminating `RunHandle` (mirrors the
//! `CountingStubFacade` pattern from `daemon_queue_drain.rs`).
//!
//! Procedure:
//! 1. Connect via `DaemonEngineFacade::connect`, call
//!    `subscribe_global()` to enable the wire-level forwarder for
//!    daemon-level events on this connection.
//! 2. Open a SECOND raw-IPC connection and send `StartRun` over it.
//!    We use a separate connection so the StartRun reply doesn't get
//!    multiplexed onto the global subscriber's stream — the
//!    `subscribe_global()` connection only needs to observe broadcast
//!    events, not request responses.
//! 3. Receive on the global receiver: assert `RunAccepted{run_id}`
//!    arrives, then later `RunFinished{run_id, outcome: Completed}`.
//! 4. Shut down cleanly.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::EngineRunConfig;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_orchestrator::engine::ipc::{
    DaemonRequest, DaemonResponse, GlobalDaemonEvent, local_socket_name_from_path,
    read_response_frame, write_frame,
};
use tempfile::TempDir;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

fn unique_socket_path(temp: &TempDir, prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    temp.path().join(format!("{prefix}_{pid}_{nanos}.sock"))
}

fn make_minimal_graph() -> surge_core::graph::Graph {
    surge_core::graph::Graph {
        schema_version: surge_core::graph::SCHEMA_VERSION,
        metadata: surge_core::graph::GraphMetadata {
            name: "subscribe-global-test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: NodeKey::try_from("placeholder").unwrap(),
        nodes: BTreeMap::new(),
        edges: Vec::new(),
        subgraphs: BTreeMap::new(),
    }
}

/// Repeatedly attempt to connect to the daemon socket until the
/// listener accepts a connection (or the deadline elapses). macOS CI
/// runners have shown >200ms tails between spawn and `bind` returning.
async fn connect_with_retry(
    socket: PathBuf,
    timeout: Duration,
) -> Result<DaemonEngineFacade, surge_orchestrator::engine::EngineError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match DaemonEngineFacade::connect(socket.clone()).await {
            Ok(c) => return Ok(c),
            Err(e) if std::time::Instant::now() >= deadline => return Err(e),
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
        }
    }
}

/// Stub `EngineFacade` whose `start_run` returns a `RunHandle` that
/// emits `Terminal` and the completion future resolves immediately.
/// This drives `spawn_forward_task` to publish `RunFinished` into the
/// broadcast registry — exactly what the global subscription should
/// observe.
struct InstantFinishStubFacade;

#[async_trait::async_trait]
impl EngineFacade for InstantFinishStubFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        let (tx, rx) = broadcast::channel(8);
        // Emit Terminal then drop the sender so spawn_forward_task's
        // recv loop sees Closed and proceeds to publish RunFinished.
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

    async fn resume_run(
        &self,
        _run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        Err(surge_orchestrator::engine::EngineError::Internal(
            "stub: resume_run not used in this test".into(),
        ))
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

#[tokio::test]
async fn subscribe_global_delivers_run_lifecycle_events() {
    let temp = TempDir::new().unwrap();
    // Keep the prefix short — macOS caps sockaddr_un.sun_path at 104 chars.
    let socket = unique_socket_path(&temp, "sub_glob");
    let cfg = ServerConfig {
        max_active: 4,
        max_queue: 16,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let stub: Arc<dyn EngineFacade> = Arc::new(InstantFinishStubFacade);

    let server_handle = tokio::spawn({
        let stub = stub.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, stub, shutdown).await }
    });

    // --- Subscriber connection (high-level facade) ---
    let client = connect_with_retry(socket.clone(), Duration::from_secs(3))
        .await
        .expect("connect global subscriber");
    let mut global_rx = client
        .subscribe_global()
        .await
        .expect("subscribe_global Ok");

    // --- Issuer connection (raw IPC) ---
    // Use a separate connection so the StartRun response goes back on
    // the issuer connection's wire and doesn't interleave with the
    // global subscriber's events.
    let name = local_socket_name_from_path(&socket).expect("socket name");
    let stream = LocalSocketStream::connect(name)
        .await
        .expect("connect issuer");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    let run_id = RunId::new();
    let req = DaemonRequest::StartRun {
        request_id: 1,
        run_id,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: EngineRunConfig::default(),
    };
    write_frame(&mut write_half, &req)
        .await
        .expect("write StartRun");
    write_half.flush().await.expect("flush StartRun");

    let resp = read_response_frame(&mut reader)
        .await
        .expect("read StartRun resp")
        .expect("frame StartRun resp");
    match resp {
        DaemonResponse::StartRunOk {
            request_id,
            run_id: got,
        } => {
            assert_eq!(request_id, 1);
            assert_eq!(got, run_id);
        },
        other => panic!("expected StartRunOk, got {other:?}"),
    }

    // --- Assert global events arrive ---
    // RunAccepted is published synchronously inside dispatch::StartRun
    // BEFORE facade.start_run is called; RunFinished is published by
    // spawn_forward_task once the handle's events stream closes and
    // the completion future resolves. Both should arrive within a
    // couple of seconds.
    let accepted = tokio::time::timeout(Duration::from_secs(2), global_rx.recv())
        .await
        .expect("RunAccepted not received within 2s")
        .expect("global channel unexpectedly closed before RunAccepted");
    match accepted {
        GlobalDaemonEvent::RunAccepted { run_id: got } => assert_eq!(got, run_id),
        other => panic!("expected RunAccepted, got {other:?}"),
    }

    let finished = tokio::time::timeout(Duration::from_secs(2), global_rx.recv())
        .await
        .expect("RunFinished not received within 2s")
        .expect("global channel unexpectedly closed before RunFinished");
    match finished {
        GlobalDaemonEvent::RunFinished {
            run_id: got,
            outcome,
        } => {
            assert_eq!(got, run_id);
            match outcome {
                RunOutcome::Completed { .. } => {},
                other => panic!("expected Completed outcome, got {other:?}"),
            }
        },
        other => panic!("expected RunFinished, got {other:?}"),
    }

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}

/// Regression test for the Copilot review on PR #30: a second
/// `SubscribeGlobal` on the same connection must not spawn a
/// concurrent forwarder. If it did, each `GlobalDaemonEvent` would
/// be forwarded twice on this connection's wire and every receiver
/// produced by `EventDispatcher.global.subscribe()` would observe
/// duplicate frames.
///
/// We can't directly inspect server-side task counts, so we assert
/// the observable invariant: after two `subscribe_global()` calls
/// followed by a `StartRun`, every receiver sees exactly ONE
/// `RunAccepted{run_id}` and exactly ONE `RunFinished{run_id}` —
/// never a third event for the same run.
#[tokio::test]
async fn subscribe_global_is_idempotent_within_a_connection() {
    let temp = TempDir::new().unwrap();
    let socket = unique_socket_path(&temp, "sub_glob_idem");
    let cfg = ServerConfig {
        max_active: 4,
        max_queue: 16,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let stub: Arc<dyn EngineFacade> = Arc::new(InstantFinishStubFacade);

    let server_handle = tokio::spawn({
        let stub = stub.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, stub, shutdown).await }
    });

    let client = connect_with_retry(socket.clone(), Duration::from_secs(3))
        .await
        .expect("connect global subscriber");

    // Two subscribes back-to-back. The second must be a no-op on the
    // server side; the receiver returned by the second call is fed
    // by the same local broadcast Sender.
    let mut rx_first = client
        .subscribe_global()
        .await
        .expect("first subscribe_global Ok");
    let mut rx_second = client
        .subscribe_global()
        .await
        .expect("second subscribe_global Ok");

    // Drive a run on a separate raw-IPC connection so the StartRun
    // response doesn't interleave on the subscriber connection.
    let name = local_socket_name_from_path(&socket).expect("socket name");
    let stream = LocalSocketStream::connect(name)
        .await
        .expect("connect issuer");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    let run_id = RunId::new();
    let req = DaemonRequest::StartRun {
        request_id: 1,
        run_id,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: EngineRunConfig::default(),
    };
    write_frame(&mut write_half, &req)
        .await
        .expect("write StartRun");
    write_half.flush().await.expect("flush StartRun");
    let resp = read_response_frame(&mut reader)
        .await
        .expect("read StartRun resp")
        .expect("frame StartRun resp");
    assert!(matches!(resp, DaemonResponse::StartRunOk { .. }));

    // Both receivers must observe exactly RunAccepted then RunFinished
    // for this run — no third frame. Drain on each independently.
    for (label, rx) in [("first", &mut rx_first), ("second", &mut rx_second)] {
        let accepted = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("{label}: RunAccepted not received within 2s"))
            .unwrap_or_else(|e| panic!("{label}: global channel closed early: {e:?}"));
        match accepted {
            GlobalDaemonEvent::RunAccepted { run_id: got } => assert_eq!(got, run_id),
            other => panic!("{label}: expected RunAccepted, got {other:?}"),
        }

        let finished = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("{label}: RunFinished not received within 2s"))
            .unwrap_or_else(|e| panic!("{label}: global channel closed early: {e:?}"));
        match finished {
            GlobalDaemonEvent::RunFinished {
                run_id: got,
                outcome: RunOutcome::Completed { .. },
            } => assert_eq!(got, run_id),
            other => panic!("{label}: expected RunFinished{{Completed}}, got {other:?}"),
        }

        // No further events should arrive for this run. A duplicate
        // forwarder would have re-emitted RunAccepted/RunFinished a
        // second time. 200ms is enough on tokio's current_thread.
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Err(_) => {}, // timeout — good
            Ok(Ok(unexpected)) => {
                panic!("{label}: unexpected extra event after RunFinished: {unexpected:?}")
            },
            Ok(Err(e)) => panic!("{label}: unexpected channel error: {e:?}"),
        }
    }

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
