//! ACP tool notifications are display-only; authenticated stage control uses MCP.

use std::collections::BTreeMap;
use std::str::FromStr;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_human_input_notification_has_no_authority() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "human_input".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "ask".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: true,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: Default::default(),
    };

    let sid = bridge.open_session(cfg).await.unwrap().session;
    bridge
        .send_message(sid, MessageContent::Text("?".into()))
        .await
        .unwrap();

    bridge.close_session(sid).await.unwrap();
    bridge.shutdown().await.unwrap();
    let observed: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert!(observed.iter().any(|event| matches!(event, BridgeEvent::ToolObserved { session, title, .. } if *session == sid && title == "request_human_input")));
    assert!(!observed.iter().any(|event| matches!(
        event,
        BridgeEvent::OutcomeReported { .. }
            | BridgeEvent::HumanInputRequested { .. }
            | BridgeEvent::ToolCall { .. }
    )));
}
