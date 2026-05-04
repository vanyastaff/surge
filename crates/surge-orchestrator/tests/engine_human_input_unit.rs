//! Unit test: resolve_human_input returns RunNotFound for unknown run.

mod fixtures;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_human_input_returns_run_not_found_for_unknown_run() {
    use std::sync::Arc;
    use surge_acp::bridge::facade::BridgeFacade;
    use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
    use surge_orchestrator::engine::{Engine, EngineConfig, EngineError};
    use surge_persistence::runs::Storage;

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let unknown = surge_core::id::RunId::new();
    let result = engine
        .resolve_human_input(unknown, None, serde_json::json!({"outcome": "x"}))
        .await;
    assert!(matches!(result, Err(EngineError::RunNotFound(_))));
}
