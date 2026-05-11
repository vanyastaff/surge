//! `dispatch::ListRuns` surfaces queued runs (held by the daemon's
//! `pending_starts` map after the admission queue rejected them) in
//! the response with `RunStatus::Awaiting` — alongside the engine's
//! list of currently-active runs.
//!
//! Setup: `run_server` with `max_active=1` and a stub facade whose
//! `start_run` holds run 1's events channel open so run 2 must
//! queue.
//!
//! Procedure:
//! 1. `StartRun` for run 1 over raw IPC → expect `StartRunOk`
//!    (admitted).
//! 2. `StartRun` for run 2 → expect `StartRunQueued`. The daemon
//!    has stashed run 2 in `pending_starts`.
//! 3. `ListRuns` → must contain a `RunSummary` for run 2 with
//!    `RunStatus::Awaiting`. The stub facade's `list_runs` returns
//!    an empty slice (we cannot construct `RunSummary` directly here
//!    because the type is `#[non_exhaustive]`), so the queued entry
//!    can only come from the daemon synthesising it from
//!    `pending_starts`.
//!
//! Raw frame I/O is used (rather than `DaemonEngineFacade`) because
//! the facade pipelines a `Subscribe` immediately after `StartRun`,
//! and `Subscribe` for a queued run currently fails with
//! `RunNotActive` (queued runs aren't registered with
//! `BroadcastRegistry` until admission). That's a known limitation
//! tracked separately; this test exercises the daemon's wire-level
//! `ListRuns` behaviour directly.

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
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome, RunStatus};
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

/// Repeatedly attempt to connect to the daemon socket until the
/// listener accepts a connection (or the deadline elapses). Macos CI
/// runners have shown the listener taking >200ms to bind in some
/// cases; a fixed sleep is flaky. Mirrors the helper in
/// `daemon_subscribe_unknown.rs` but returns the raw
/// `LocalSocketStream` because this test bypasses the
/// `DaemonEngineFacade` (it pipelines a `Subscribe` after every
/// `StartRun`, which fails for queued runs by design).
async fn connect_stream_with_retry(
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
            name: "list-queued-test".into(),
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

/// Stub `EngineFacade` whose `start_run` keeps run 1's events
/// channel open until `release_first` fires — so the test can
/// observe ListRuns while run 1 is active and run 2 is queued.
/// `list_runs` returns an empty slice; `RunSummary` is
/// `#[non_exhaustive]` so we cannot construct one in this crate, and
/// we don't need to: the test asserts on the queued entry, which the
/// daemon must synthesise from `pending_starts`.
struct ListQueuedStubFacade {
    start_count: AtomicUsize,
    release_first: Arc<tokio::sync::Notify>,
}

impl ListQueuedStubFacade {
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
impl EngineFacade for ListQueuedStubFacade {
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
async fn list_runs_includes_queued_run_as_awaiting() {
    let temp = TempDir::new().unwrap();
    // Keep the prefix short — macOS caps `sockaddr_un.sun_path` at 104
    // chars and CI temp paths eat most of the budget.
    let socket = unique_socket_path(&temp, "ls_q");
    let cfg = ServerConfig {
        max_active: 1,
        max_queue: 8,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let stub = Arc::new(ListQueuedStubFacade::new());
    let stub_for_facade: Arc<dyn EngineFacade> = stub.clone();

    let server_handle = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { run_server(cfg, stub_for_facade, shutdown).await }
    });

    let stream = connect_stream_with_retry(socket, Duration::from_secs(3))
        .await
        .expect("connect to daemon");
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // --- StartRun #1 — admitted ---
    let run1 = RunId::new();
    write_frame(
        &mut write_half,
        &DaemonRequest::StartRun {
            request_id: 1,
            run_id: run1,
            graph: Box::new(make_minimal_graph()),
            worktree_path: temp.path().to_path_buf(),
            run_config: Box::new(EngineRunConfig::default()),
        },
    )
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

    // --- StartRun #2 — admission cap reached, queued ---
    let run2 = RunId::new();
    write_frame(
        &mut write_half,
        &DaemonRequest::StartRun {
            request_id: 2,
            run_id: run2,
            graph: Box::new(make_minimal_graph()),
            worktree_path: temp.path().to_path_buf(),
            run_config: Box::new(EngineRunConfig::default()),
        },
    )
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

    // --- ListRuns — must surface the queued run as `Awaiting`.
    // The stub facade's `list_runs` returns an empty slice, so
    // active-run rendering is out of scope for this test; we assert
    // only that the queued entry appears in the merged response.
    write_frame(&mut write_half, &DaemonRequest::ListRuns { request_id: 3 })
        .await
        .expect("write ListRuns");
    write_half.flush().await.expect("flush ListRuns");
    let resp3 = read_response_frame(&mut reader)
        .await
        .expect("read ListRuns")
        .expect("frame ListRuns");
    let runs = match resp3 {
        DaemonResponse::ListRunsOk { request_id, runs } => {
            assert_eq!(request_id, 3);
            runs
        },
        other => panic!("expected ListRunsOk, got {other:?}"),
    };

    // The stub's facade `list_runs` returns nothing, so any entry in
    // the response must have come from the daemon's `pending_starts`
    // synthesis. We expect exactly one — for run 2.
    assert_eq!(
        runs.len(),
        1,
        "expected exactly the queued run in ListRuns response, got {runs:?}",
    );
    let queued = &runs[0];
    assert_eq!(queued.run_id, run2);
    assert_eq!(queued.status, RunStatus::Awaiting);
    assert!(
        queued.last_event_seq.is_none(),
        "queued run has no persisted events yet, got {:?}",
        queued.last_event_seq,
    );

    // Release run 1 so spawned forwarder tasks can wind down before
    // the runtime is dropped (avoids dangling-task warnings).
    stub.release_first();

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
