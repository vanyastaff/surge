//! Graceful shutdown: cancelling the shutdown token causes the
//! server loop to exit cleanly within ~500ms (no in-flight runs).

use std::sync::Arc;
use std::time::Duration;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::facade::EngineFacade;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct StubFacade;

#[async_trait::async_trait]
impl EngineFacade for StubFacade {
    async fn start_run(
        &self,
        _: surge_core::id::RunId,
        _: surge_core::graph::Graph,
        _: std::path::PathBuf,
        _: surge_orchestrator::engine::EngineRunConfig,
    ) -> Result<
        surge_orchestrator::engine::handle::RunHandle,
        surge_orchestrator::engine::EngineError,
    > {
        Err(surge_orchestrator::engine::EngineError::Internal(
            "stub".into(),
        ))
    }
    async fn resume_run(
        &self,
        _: surge_core::id::RunId,
        _: std::path::PathBuf,
    ) -> Result<
        surge_orchestrator::engine::handle::RunHandle,
        surge_orchestrator::engine::EngineError,
    > {
        Err(surge_orchestrator::engine::EngineError::Internal(
            "stub".into(),
        ))
    }
    async fn stop_run(
        &self,
        _: surge_core::id::RunId,
        _: String,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }
    async fn resolve_human_input(
        &self,
        _: surge_core::id::RunId,
        _: Option<String>,
        _: serde_json::Value,
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
async fn shutdown_token_exits_within_500ms() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("shutdown.sock");
    let cfg = ServerConfig {
        max_active: 4,
        socket_path: socket,
    };
    let shutdown = CancellationToken::new();
    let facade: Arc<dyn EngineFacade> = Arc::new(StubFacade);
    let handle = tokio::spawn({
        let facade = facade.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, facade, shutdown).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    shutdown.cancel();
    let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
    assert!(
        res.is_ok(),
        "server failed to exit within 500ms after cancel"
    );
}
