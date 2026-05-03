//! Unit test: agent stage opens, sends, closes the session.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::keys::{NodeKey, ProfileKey};
use surge_orchestrator::engine::stage::agent::{execute_agent_stage, AgentStageParams};
use surge_persistence::runs::storage::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_opens_and_closes_session() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage
        .create_run(run_id, dir.path(), None)
        .await
        .unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let agent_cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: NodeLimits::default(),
        hooks: vec![],
        custom_fields: BTreeMap::new(),
    };

    let node = NodeKey::try_from("plan_1").unwrap();
    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &agent_cfg,
        bridge: &bridge,
        writer: &writer,
        worktree_path: dir.path(),
    })
    .await
    .unwrap();

    assert_eq!(result.as_ref(), "done");

    // Allow the async Subscribe recording task to complete.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let calls = mock.recorded_calls.lock().await;
    let kinds: Vec<&'static str> = calls
        .iter()
        .map(|c| match c {
            fixtures::mock_bridge::RecordedCall::OpenSession => "open",
            fixtures::mock_bridge::RecordedCall::SendMessage { .. } => "send",
            fixtures::mock_bridge::RecordedCall::CloseSession(_) => "close",
            fixtures::mock_bridge::RecordedCall::ReplyToTool { .. } => "reply",
            fixtures::mock_bridge::RecordedCall::SessionState { .. } => "state",
            fixtures::mock_bridge::RecordedCall::Subscribe => "subscribe",
        })
        .collect();
    assert!(kinds.contains(&"open"), "expected open call, got: {kinds:?}");
    assert!(kinds.contains(&"send"), "expected send call, got: {kinds:?}");
    assert!(kinds.contains(&"close"), "expected close call, got: {kinds:?}");
}
