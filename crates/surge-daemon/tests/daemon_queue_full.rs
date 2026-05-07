//! Bounded admission queue: when both `max_active` and `max_queue` are
//! saturated, further `StartRun` requests are rejected with
//! `Error { code: QueueFull }` instead of growing the daemon's
//! pending-start map without bound.
//!
//! Setup: `run_server` with `max_active=1, max_queue=1` and a stub
//! facade whose first `start_run` holds the events channel open
//! until the test releases it. That keeps run 1 active and the queue
//! occupied (after run 2 lands), so the third `StartRun` must hit
//! `AdmissionDecision::QueueFull`.
//!
//! Procedure:
//! 1. `StartRun` for run 1 over raw IPC → expect `StartRunOk`.
//! 2. `StartRun` for run 2 → expect `StartRunQueued`.
//! 3. `StartRun` for run 3 → expect `Error { code: QueueFull }`.
//! 4. Release run 1 to let the drain task admit run 2 and the server
//!    shut down cleanly.
//!
//! Raw IPC (rather than `DaemonEngineFacade::start_run`) for the same
//! reason as `daemon_queue_drain.rs`: the facade pipelines a
//! `Subscribe` immediately after the StartRun reply, which fails for
//! a queued / rejected run that has no per-run channel registered.

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
    DaemonRequest, DaemonResponse, ErrorCode, local_socket_name_from_path, read_response_frame,
    write_frame,
};
use tempfile::TempDir;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// macOS caps `sockaddr_un.sun_path` at 104 chars. The system temp dir
/// on CI runners eats most of that, so we keep the prefix terse.
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
            name: "queue-full-test".into(),
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

/// Stub facade: first `start_run` holds the events channel open until
/// `release_first` fires; subsequent runs complete instantly. Used to
/// pin run 1 in the active set while we test what happens when the
/// queue fills.
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
            // First run: hold events open until released.
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
async fn third_start_run_at_saturation_returns_queue_full() {
    let temp = TempDir::new().unwrap();
    // Keep the prefix short — see comment on unique_socket_path.
    let socket = unique_socket_path(&temp, "q_full");
    let cfg = ServerConfig {
        max_active: 1,
        max_queue: 1,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let stub = Arc::new(CountingStubFacade::new());
    let stub_for_facade: Arc<dyn EngineFacade> = stub.clone();

    let server_handle = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { run_server(cfg, stub_for_facade, shutdown).await }
    });

    // Wait for the listener to come up before connecting.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let name = local_socket_name_from_path(&socket).expect("socket name");
    let stream = LocalSocketStream::connect(name)
        .await
        .expect("connect to daemon");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // --- StartRun #1 — admitted ---
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

    // --- StartRun #2 — queued ---
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

    // --- StartRun #3 — both caps hit, expect QueueFull ---
    let run3 = RunId::new();
    let req3 = DaemonRequest::StartRun {
        request_id: 3,
        run_id: run3,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: EngineRunConfig::default(),
    };
    write_frame(&mut write_half, &req3)
        .await
        .expect("write StartRun #3");
    write_half.flush().await.expect("flush #3");
    let resp3 = read_response_frame(&mut reader)
        .await
        .expect("read #3")
        .expect("frame #3");
    match resp3 {
        DaemonResponse::Error {
            request_id,
            code,
            message,
        } => {
            assert_eq!(request_id, 3);
            assert_eq!(code, ErrorCode::QueueFull);
            assert!(
                message.contains("queue is full"),
                "expected diagnostic message; got {message:?}"
            );
            assert!(
                message.contains("1/1"),
                "message should include queue_len/max_queue numbers; got {message:?}"
            );
        },
        other => panic!("expected Error{{QueueFull}} for run3, got {other:?}"),
    }

    // Now release run 1 so the drain task can admit run 2 and the
    // server shuts down cleanly. Without this, the held-open events
    // channel would keep the spawn_forward_task blocked past the
    // 2s timeout below.
    stub.release_first();

    // After release, run 2 will be admitted (start_count == 2).
    // The rejected run 3 must NOT result in another start_run call.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline && stub.start_count.load(Ordering::SeqCst) < 2 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        stub.start_count.load(Ordering::SeqCst),
        2,
        "run 2 must be admitted exactly once via the drain task; run 3 must NOT \
         have triggered a third start_run"
    );

    // Belt and braces: give the drain task a brief window to misbehave
    // (re-admit a phantom run 3) before checking again.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        stub.start_count.load(Ordering::SeqCst),
        2,
        "rejected run 3 must not appear in start_run history"
    );

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
