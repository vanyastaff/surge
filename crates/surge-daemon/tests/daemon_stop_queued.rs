//! `StopRun` on a run that's still in the admission queue cancels
//! it: removes it from the queue and from `pending_starts` so the
//! drain task never admits it.
//!
//! Without this fix, today's `dispatch::StopRun` delegates straight
//! to `facade.stop_run`, which doesn't know about queued runs (the
//! engine never saw them). The drain task would then admit the
//! queued run as soon as a slot frees, ignoring the user's
//! cancellation.
//!
//! Procedure (raw IPC, mirroring `daemon_queue_drain.rs`):
//!   1. `StartRun` for run 1 → `StartRunOk`. Stub holds run 1 open
//!      via a Notify so its slot stays occupied.
//!   2. `StartRun` for run 2 → `StartRunQueued{position: 0}` (cap is 1).
//!   3. `StopRun` for run 2 → `StopRunOk`.
//!   4. Release run 1's stub. Drain task wakes; with the fix in
//!      place, the queue is empty so nothing happens.
//!   5. Wait briefly. Assert `stub.start_count() == 1` (run 2 was
//!      never admitted).

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
    DaemonRequest, DaemonResponse, local_socket_name_from_path, read_response_frame, write_frame,
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

/// Connect to the daemon's local socket with retry — macOS CI has
/// shown >200ms tails between spawn and the listener accepting (see
/// `daemon_subscribe_unknown.rs` for the same pattern).
async fn connect_with_retry(
    socket: PathBuf,
    timeout: Duration,
) -> std::io::Result<LocalSocketStream> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let name = local_socket_name_from_path(&socket).expect("socket name");
        match LocalSocketStream::connect(name).await {
            Ok(s) => return Ok(s),
            Err(e) if std::time::Instant::now() >= deadline => return Err(e),
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
        }
    }
}

fn make_minimal_graph() -> surge_core::graph::Graph {
    surge_core::graph::Graph {
        schema_version: surge_core::graph::SCHEMA_VERSION,
        metadata: surge_core::graph::GraphMetadata {
            name: "stop-queued-test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: NodeKey::try_from("placeholder").unwrap(),
        nodes: BTreeMap::new(),
        edges: Vec::new(),
        subgraphs: BTreeMap::new(),
    }
}

/// Stub facade — same shape as `daemon_queue_drain.rs`'s
/// `CountingStubFacade`. Mirrored locally because integration test
/// targets are independent compilation units and don't share helpers.
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

    fn start_count(&self) -> usize {
        self.start_count.load(Ordering::SeqCst)
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
            // First run: hold events open until the test fires
            // `release_first`. Then emit Terminal and drop the sender
            // so `spawn_forward_task` finishes and the admission slot
            // frees, waking the drain task.
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
            // Defensive: this branch should NEVER fire in the
            // happy-path of this test (run 2 must be cancelled before
            // it ever admits). If it does fire, the assertion below
            // catches it — but a quickly-completing handle keeps the
            // test from hanging on a real bug.
            let _ = tx.send(EngineRunEvent::Terminal {
                outcome: RunOutcome::Completed {
                    terminal: NodeKey::try_from("end").unwrap(),
                },
            });
            drop(tx);
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
        // With the fix in place, dispatch::StopRun handles a queued
        // run id directly and never calls into the facade — so this
        // path is unreachable in the happy case. Returning Ok keeps
        // the failure mode focused on the actual contract under test
        // (the final start_count assertion catches a regression
        // because the drain task would re-admit run 2 if StopRun
        // hadn't removed it from the queue).
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
async fn stop_run_cancels_queued_run() {
    let temp = TempDir::new().unwrap();
    // Short prefix — macOS caps `sockaddr_un.sun_path` at 104 chars.
    let socket = unique_socket_path(&temp, "stop_q");
    let cfg = ServerConfig {
        max_active: 1,
        max_queue: 8,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let stub = Arc::new(CountingStubFacade::new());
    let stub_for_facade: Arc<dyn EngineFacade> = stub.clone();

    let server_handle = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { run_server(cfg, stub_for_facade, shutdown).await }
    });

    let stream = connect_with_retry(socket, Duration::from_secs(3))
        .await
        .expect("connect to daemon");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // --- StartRun #1 — admitted, calls facade.start_run ---
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
    let resp1 = read_response_frame(&mut reader)
        .await
        .expect("read #1")
        .expect("frame #1");
    match resp1 {
        DaemonResponse::StartRunOk { request_id, run_id } => {
            assert_eq!(request_id, 1);
            assert_eq!(run_id, run1);
        },
        other => panic!("expected StartRunOk for run1, got {other:?}"),
    }

    // --- StartRun #2 — admission cap reached (max_active=1), queued ---
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
    let resp2 = read_response_frame(&mut reader)
        .await
        .expect("read #2")
        .expect("frame #2");
    match resp2 {
        DaemonResponse::StartRunQueued {
            request_id,
            run_id,
            position,
        } => {
            assert_eq!(request_id, 2);
            assert_eq!(run_id, run2);
            assert_eq!(position, 0);
        },
        other => panic!("expected StartRunQueued for run2, got {other:?}"),
    }

    // --- StopRun #2 — should cancel the queued run without ever
    //     touching the facade.
    let stop_req = DaemonRequest::StopRun {
        request_id: 3,
        run_id: run2,
        reason: "test cancel".into(),
    };
    write_frame(&mut write_half, &stop_req)
        .await
        .expect("write StopRun #2");
    write_half.flush().await.expect("flush stop");
    let stop_resp = read_response_frame(&mut reader)
        .await
        .expect("read stop")
        .expect("frame stop");
    match stop_resp {
        DaemonResponse::StopRunOk { request_id } => {
            assert_eq!(request_id, 3);
        },
        other => panic!("expected StopRunOk for run2, got {other:?}"),
    }

    // Release run 1 so its slot frees and the drain task wakes. If
    // run 2 had stayed in the queue, the drain task would now admit
    // it and call `facade.start_run` a second time. With the fix,
    // start_count must stay at 1.
    stub.release_first();

    // Generous window for the drain task to do anything wrong.
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        stub.start_count(),
        1,
        "drain task must NOT admit a cancelled queued run"
    );

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
