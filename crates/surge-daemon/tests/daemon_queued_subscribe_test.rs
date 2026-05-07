//! Integration test for Task 3.1 — queued-run auto-resubscribe on
//! admission.
//!
//! Verifies that a `Subscribe { run_id }` issued while the run is
//! still in the FIFO admission queue (not yet admitted) **does not**
//! return `RunNotActive`. Instead the daemon retains the subscription
//! and forwards per-run events as soon as the drain task admits the
//! run — without the client having to issue a fresh `Subscribe`.
//!
//! Setup mirrors `daemon_queue_drain.rs`: `max_active = 1`, stub
//! facade with a controllable first-run hold so we can observe the
//! queued window.
//!
//! Procedure:
//! 1. `StartRun #1` → expect `StartRunOk` (admitted).
//! 2. `StartRun #2` → expect `StartRunQueued` (position 0).
//! 3. `Subscribe(run2)` while run 2 is still queued → expect
//!    `SubscribeOk` (NOT `RunNotActive`).
//! 4. Release run 1 — drain task wakes and admits run 2.
//! 5. Receive a per-run event for run 2 on the same connection's
//!    inbound frame stream (no re-subscribe). Specifically, the stub
//!    emits `Terminal` for the second run; we assert that arrives as
//!    a `DaemonEvent::PerRun { run_id: run2, event: Terminal(_) }`.
//! 6. Shut down cleanly.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_orchestrator::engine::ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, InboundServerFrame, local_socket_name_from_path,
    read_inbound_server_frame, write_frame,
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
            name: "queued-subscribe-test".into(),
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

/// Mirrors `CountingStubFacade` from `daemon_queue_drain.rs`: the
/// FIRST `start_run` returns a handle that holds its events channel
/// open until `release_first()` fires. Subsequent calls return a
/// handle that completes immediately, which lets us observe the
/// queued→admitted transition for run 2.
struct CountingStubFacade {
    start_count: AtomicUsize,
    release_first: Arc<tokio::sync::Notify>,
}

impl CountingStubFacade {
    fn new() -> Self {
        Self {
            start_count: AtomicUsize::new(0),
            release_first: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn release_first(&self) {
        self.release_first.notify_one();
    }
}

#[async_trait::async_trait]
impl EngineFacade for CountingStubFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, surge_orchestrator::engine::EngineError> {
        let prev = self.start_count.fetch_add(1, Ordering::SeqCst);
        let is_first = prev == 0;

        let (tx, rx) = broadcast::channel(8);
        let release = self.release_first.clone();
        if is_first {
            tokio::spawn(async move {
                release.notified().await;
                let _ = tx.send(EngineRunEvent::Terminal {
                    outcome: RunOutcome::Completed {
                        terminal: NodeKey::try_from("end").unwrap(),
                    },
                });
                drop(tx);
            });
        } else {
            // Subsequent runs: send Terminal after a small delay so
            // the queued-subscribe forwarder has time to attach to
            // the per-run publisher before any event is pushed. The
            // pre-fix code path would have lost events here too;
            // the post-fix path doesn't need the delay because
            // `subscribe_eventual` resolves before any publisher
            // send happens. The delay is small but documents the
            // production scenario where engine events trickle in.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = tx.send(EngineRunEvent::Terminal {
                    outcome: RunOutcome::Completed {
                        terminal: NodeKey::try_from("end").unwrap(),
                    },
                });
                drop(tx);
            });
        }

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
async fn subscribe_to_queued_run_streams_after_admission() {
    let temp = TempDir::new().unwrap();
    let socket = unique_socket_path(&temp, "queue_sub");
    let cfg = ServerConfig {
        max_active: 1,
        max_queue: 4,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let stub = Arc::new(CountingStubFacade::new());
    let stub_for_facade: Arc<dyn EngineFacade> = stub.clone();

    let server_handle = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { run_server(cfg, stub_for_facade, shutdown).await }
    });

    // Wait for listener to come up.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let name = local_socket_name_from_path(&socket).expect("socket name");
    let stream = LocalSocketStream::connect(name)
        .await
        .expect("connect to daemon");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // --- StartRun #1 (admitted) ---
    let run1 = RunId::new();
    let req1 = DaemonRequest::StartRun {
        request_id: 1,
        run_id: run1,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: EngineRunConfig::default(),
    };
    write_frame(&mut write_half, &req1)
        .await
        .expect("write StartRun #1");
    write_half.flush().await.expect("flush #1");
    let resp1 = read_inbound_server_frame(&mut reader)
        .await
        .expect("read #1")
        .expect("frame #1");
    let InboundServerFrame::Response(DaemonResponse::StartRunOk { run_id: got1, .. }) = resp1
    else {
        panic!("expected StartRunOk for run1, got {resp1:?}");
    };
    assert_eq!(got1, run1);

    // --- StartRun #2 (queued) ---
    let run2 = RunId::new();
    let req2 = DaemonRequest::StartRun {
        request_id: 2,
        run_id: run2,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: EngineRunConfig::default(),
    };
    write_frame(&mut write_half, &req2)
        .await
        .expect("write StartRun #2");
    write_half.flush().await.expect("flush #2");
    let resp2 = read_inbound_server_frame(&mut reader)
        .await
        .expect("read #2")
        .expect("frame #2");
    let InboundServerFrame::Response(DaemonResponse::StartRunQueued { run_id: got2, .. }) = resp2
    else {
        panic!("expected StartRunQueued for run2, got {resp2:?}");
    };
    assert_eq!(got2, run2);

    // --- Subscribe(run2) while run2 is still queued ---
    // Pre-fix this would have returned Error{RunNotActive}. Post-fix
    // the daemon waits for admission and forwards per-run events.
    let sub_req = DaemonRequest::Subscribe {
        request_id: 3,
        run_id: run2,
    };
    write_frame(&mut write_half, &sub_req)
        .await
        .expect("write Subscribe(run2)");
    write_half.flush().await.expect("flush Subscribe");
    let sub_resp = read_inbound_server_frame(&mut reader)
        .await
        .expect("read Subscribe resp")
        .expect("frame Subscribe resp");
    match sub_resp {
        InboundServerFrame::Response(DaemonResponse::SubscribeOk { request_id }) => {
            assert_eq!(request_id, 3);
        },
        InboundServerFrame::Response(DaemonResponse::Error { code, message, .. }) => {
            panic!("Subscribe on queued run should succeed; got error {code:?}: {message}");
        },
        other => panic!("expected SubscribeOk, got {other:?}"),
    }

    // --- Release run 1; drain task admits run 2 ---
    stub.release_first();

    // --- Read frames until we see a per-run event for run 2 ---
    // Timeout generously; macOS CI runners can be slow.
    let mut saw_run2_event = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let next_frame = tokio::time::timeout(
            Duration::from_millis(500),
            read_inbound_server_frame(&mut reader),
        )
        .await;
        match next_frame {
            Ok(Ok(Some(InboundServerFrame::Event(DaemonEvent::PerRun { run_id, event }))))
                if run_id == run2 =>
            {
                saw_run2_event = true;
                // We expect the Terminal event from the
                // delayed-finish path of the stub for run 2.
                assert!(
                    matches!(event, EngineRunEvent::Terminal { .. }),
                    "first per-run event for run2 should be Terminal; got {event:?}"
                );
                break;
            },
            Ok(Ok(Some(_))) => continue, // unrelated frame; keep reading
            Ok(Ok(None)) => break,       // EOF — daemon closed
            Ok(Err(_)) | Err(_) => continue,
        }
    }
    assert!(
        saw_run2_event,
        "did not receive any per-run event for queued-then-admitted run within deadline"
    );

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
