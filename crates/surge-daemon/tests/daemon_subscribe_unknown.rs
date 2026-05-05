//! `DaemonEngineFacade::subscribe_to_run` returns
//! [`EngineError::RunNotActive`] (not a generic `Internal`) when the
//! daemon has no per-run channel registered for the requested id.
//!
//! This is the typed error the M7 polish #3 PR introduces so that the
//! CLI's `watch --daemon` path can fall back to disk-replay instead of
//! propagating an opaque error to the user. Without the typed variant,
//! the fallback would have to string-match on the error message.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_core::id::RunId;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::EngineError;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::EngineFacade;
use tempfile::TempDir;
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
/// listener accepts a connection (or the deadline elapses). Macos
/// CI runners have shown the listener taking >200ms to bind in some
/// cases; a fixed sleep is flaky.
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

struct StubFacade;

#[async_trait::async_trait]
impl EngineFacade for StubFacade {
    async fn start_run(
        &self,
        _run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: surge_orchestrator::engine::EngineRunConfig,
    ) -> Result<surge_orchestrator::engine::handle::RunHandle, EngineError> {
        Err(EngineError::Internal("stub: start_run".into()))
    }

    async fn resume_run(
        &self,
        _run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<surge_orchestrator::engine::handle::RunHandle, EngineError> {
        Err(EngineError::Internal("stub: resume_run".into()))
    }

    async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Ok(())
    }

    async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn list_runs(
        &self,
    ) -> Result<Vec<surge_orchestrator::engine::handle::RunSummary>, EngineError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn subscribe_unknown_run_returns_run_not_active() {
    let temp = TempDir::new().unwrap();
    let socket = unique_socket_path(&temp, "subscribe_unknown");
    let cfg = ServerConfig {
        max_active: 4,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let facade: Arc<dyn EngineFacade> = Arc::new(StubFacade);

    let server_handle = tokio::spawn({
        let facade = facade.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, facade, shutdown).await }
    });

    // Wait for the listener with retry — macOS CI runners have shown
    // 200ms+ tails between spawn and `bind` returning successfully.
    let client = connect_with_retry(socket, Duration::from_secs(3))
        .await
        .expect("connect");
    let unknown = RunId::new();
    let err = client
        .subscribe_to_run(unknown)
        .await
        .expect_err("subscribe to unknown run should error");
    match err {
        EngineError::RunNotActive(id) => assert_eq!(id, unknown),
        other => panic!("expected RunNotActive({unknown}), got {other:?}"),
    }

    shutdown.cancel();
    let join_result = tokio::time::timeout(Duration::from_secs(2), server_handle)
        .await
        .expect("server did not shut down within 2s after cancellation");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
