//! Bootstrap operation verbs must not be mistaken for an ordinary run start.
use surge_orchestrator::engine::ipc::DaemonRequest;

#[test]
fn bootstrap_verbs_decode_with_request_identity() {
    let operation_id = serde_json::to_value(surge_core::RunId::new()).unwrap();
    for method in [
        "start_bootstrap",
        "bootstrap_status",
        "cancel_bootstrap",
        "retry_bootstrap",
    ] {
        let mut wire =
            serde_json::json!({"method": method, "request_id": 91, "operation_id": operation_id});
        if method == "start_bootstrap" {
            wire["intent"] = serde_json::json!({
                "version": 1, "project_path": std::env::temp_dir(), "prompt": " build an app\n",
                "budget": {"limits": {}, "policy": "abort"}
            });
        }
        if method == "retry_bootstrap" {
            wire["revision"] = serde_json::json!(3);
        }
        let request: DaemonRequest =
            serde_json::from_value(wire).expect("bootstrap contract decodes");
        assert_eq!(request.request_id(), 91);
    }
}

use std::{path::PathBuf, sync::Arc, time::Duration};
use surge_core::{RunId, bootstrap_operation::BootstrapIntent, budget::BudgetGuard};
use surge_orchestrator::engine::{
    EngineError, EngineRunConfig,
    daemon_facade::{BootstrapClientError, DaemonEngineFacade},
    facade::EngineFacade,
    handle::{RunHandle, RunSummary},
};

struct UnusedEngine;
#[async_trait::async_trait]
impl EngineFacade for UnusedEngine {
    async fn start_run(
        &self,
        _: RunId,
        _: surge_core::graph::Graph,
        _: PathBuf,
        _: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        panic!("disabled bootstrap must not start engine")
    }
    async fn resume_run(&self, _: RunId, _: PathBuf) -> Result<RunHandle, EngineError> {
        panic!("disabled bootstrap must not resume engine")
    }
    async fn stop_run(&self, _: RunId, _: String) -> Result<(), EngineError> {
        panic!("disabled bootstrap must not stop engine")
    }
    async fn resolve_human_input(
        &self,
        _: RunId,
        _: Option<String>,
        _: serde_json::Value,
    ) -> Result<(), EngineError> {
        panic!("disabled bootstrap must not resolve input")
    }
    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn synthetic_runs_only_adapter_refuses_bootstrap_operations() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("bootstrap.sock");
    let shutdown = tokio_util::sync::CancellationToken::new();
    let server = tokio::spawn(surge_daemon::run_synthetic_server(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 1,
            max_queue: 1,
        },
        Arc::new(UnusedEngine),
        shutdown.clone(),
    ));
    let client = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(client) = DaemonEngineFacade::connect(socket.clone()).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let intent = BootstrapIntent::new(
        root.path().to_path_buf(),
        "build".into(),
        BudgetGuard::default(),
    )
    .unwrap();
    let id = RunId::new();
    let results = tokio::time::timeout(Duration::from_secs(3), async {
        [
            client.start_bootstrap(id, intent).await,
            client.bootstrap_status(id).await,
            client.cancel_bootstrap(id).await,
            client.retry_bootstrap(id, 0).await,
        ]
    })
    .await
    .unwrap();
    for result in results {
        assert!(
            matches!(result, Err(BootstrapClientError::NotReady)),
            "{result:?}"
        );
    }
    assert!(client.list_runs().await.unwrap().is_empty());
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
