//! Graceful shutdown: cancelling the shutdown token causes the
//! server loop to exit cleanly within ~500ms (no in-flight runs).

use std::sync::Arc;
use std::time::Duration;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::facade::EngineFacade;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn unique_socket_path(temp: &TempDir, prefix: &str) -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    temp.path().join(format!("{prefix}_{pid}_{nanos}.sock"))
}

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
    let socket = unique_socket_path(&temp, "shutdown");
    let cfg = ServerConfig {
        max_active: 4,
        max_queue: 16,
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
    let join_result = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("server did not exit within 500ms after cancel");
    let server_result = join_result.expect("server task panicked");
    server_result.expect("server returned error");
}
