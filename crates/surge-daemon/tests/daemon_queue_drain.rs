//! Integration test for the queue-drain task.
//!
//! Setup: `run_server` with `max_active=1` and a stub facade whose
//! `start_run` returns a quickly-terminating `RunHandle`.
//!
//! Procedure:
//! 1. Send `StartRun` for run 1 over raw IPC → expect `StartRunOk`.
//! 2. Send `StartRun` for run 2 over raw IPC → expect `StartRunQueued`.
//! 3. The handle the stub returned for run 1 emits `Terminal` and its
//!    completion future resolves immediately, so `spawn_forward_task`
//!    calls `admission.notify_completed`.
//! 4. The drain task wakes on `wait_changed`, pops run 2, and calls
//!    `facade.start_run` again.
//! 5. Assert `stub.start_count == 2` within a short timeout.
//!
//! We use raw frame I/O (rather than `DaemonEngineFacade::start_run`)
//! because the facade pipelines a `Subscribe` immediately after the
//! StartRun reply, and that `Subscribe` fails for a queued run (the
//! run is not yet registered with `BroadcastRegistry` until admission).
//! That failure mode is documented as a known limitation of this PR;
//! the drain-task production behaviour is what we want to exercise.

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

fn make_minimal_graph() -> surge_core::graph::Graph {
    surge_core::graph::Graph {
        schema_version: surge_core::graph::SCHEMA_VERSION,
        metadata: surge_core::graph::GraphMetadata {
            name: "queue-drain-test".into(),
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

/// Stub `EngineFacade` whose `start_run` increments a counter and
/// returns a `RunHandle`. The FIRST call's handle holds its events
/// channel open until `release_first` fires — that lets the test
/// queue a second `StartRun` while run 1 is still active. Subsequent
/// calls return a handle that completes immediately.
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
            // First run: hold events open until the test notifies
            // `release_first`. Then emit Terminal and drop the sender,
            // which lets `spawn_forward_task` finish and call
            // `admission.notify_completed` — waking the drain task.
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
            // Subsequent runs: complete instantly.
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
async fn queued_run_admitted_after_completion() {
    let temp = TempDir::new().unwrap();
    let socket = unique_socket_path(&temp, "queue_drain");
    let cfg = ServerConfig {
        max_active: 1,
        // Plenty of queue capacity — this test exercises the drain
        // path, not the rejection path.
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

    // Wait for the listener to come up before connecting.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let name = local_socket_name_from_path(&socket).expect("socket name");
    let stream = LocalSocketStream::connect(name)
        .await
        .expect("connect to daemon");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // --- StartRun #1 ---
    let run1 = RunId::new();
    let req1 = DaemonRequest::StartRun {
        request_id: 1,
        run_id: run1,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: Box::new(EngineRunConfig::default()),
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

    // --- StartRun #2 — admission cap reached, should queue ---
    let run2 = RunId::new();
    let req2 = DaemonRequest::StartRun {
        request_id: 2,
        run_id: run2,
        graph: Box::new(make_minimal_graph()),
        worktree_path: temp.path().to_path_buf(),
        run_config: Box::new(EngineRunConfig::default()),
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

    // Release run 1's terminal event. `spawn_forward_task` will call
    // `admission.notify_completed`, the drain task wakes via
    // `wait_changed`, and run 2 gets admitted via `facade.start_run`.
    stub.release_first();

    // The stub's run-1 handle terminates immediately, so
    // `spawn_forward_task` calls `admission.notify_completed` very
    // quickly. The drain task should then pop run 2 and call
    // `facade.start_run` for it. Poll the counter with a generous
    // timeout to avoid CI flakes.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline && stub.start_count() < 2 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        stub.start_count(),
        2,
        "drain task should have admitted the queued run within 2s"
    );

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
