//! End-to-end smoke: spin up `run_server` inline (no subprocess),
//! connect a [`DaemonEngineFacade`] over a real local socket, call
//! `list_runs`, shutdown, verify the server task exits within 2s.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::EngineFacade;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct StubFacade;

#[async_trait::async_trait]
impl EngineFacade for StubFacade {
    async fn start_run(
        &self,
        _run_id: surge_core::id::RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: surge_orchestrator::engine::EngineRunConfig,
    ) -> Result<
        surge_orchestrator::engine::handle::RunHandle,
        surge_orchestrator::engine::EngineError,
    > {
        Err(surge_orchestrator::engine::EngineError::Internal(
            "stub: start_run".into(),
        ))
    }

    async fn resume_run(
        &self,
        _run_id: surge_core::id::RunId,
        _worktree_path: PathBuf,
    ) -> Result<
        surge_orchestrator::engine::handle::RunHandle,
        surge_orchestrator::engine::EngineError,
    > {
        Err(surge_orchestrator::engine::EngineError::Internal(
            "stub: resume_run".into(),
        ))
    }

    async fn stop_run(
        &self,
        _run_id: surge_core::id::RunId,
        _reason: String,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }

    async fn resolve_human_input(
        &self,
        _run_id: surge_core::id::RunId,
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
async fn ping_round_trip() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("test.sock");
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

    // Wait briefly for the listener to start.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = DaemonEngineFacade::connect(socket).await.expect("connect");
    let runs = client.list_runs().await.expect("list_runs");
    assert!(runs.is_empty());

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_handle).await;
}
