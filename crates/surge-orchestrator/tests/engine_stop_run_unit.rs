//! Unit test: stop_run on an unknown run_id returns RunNotFound.

mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError};
use surge_persistence::runs::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_run_unknown_returns_run_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let r = engine
        .stop_run(surge_core::id::RunId::new(), "test".into())
        .await;
    assert!(matches!(r, Err(EngineError::RunNotFound(_))));
}
