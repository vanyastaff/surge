//! Integration test: agent calling request_human_input surfaces as
//! BridgeEvent::HumanInputRequested, not a generic ToolCall, and bridge
//! does NOT auto-reply (per spec §5.3 — M5 will provide the reply API).

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_input_surfaces_as_distinct_event() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
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
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge
        .send_message(sid, MessageContent::Text("?".into()))
        .await
        .unwrap();

    let mut saw_human = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::HumanInputRequested {
                session, question, ..
            })) => {
                assert_eq!(session, sid);
                assert!(!question.is_empty());
                saw_human = true;
                break;
            },
            Ok(Ok(BridgeEvent::ToolCall { tool, .. })) if tool == "request_human_input" => {
                panic!("request_human_input should NOT surface as generic ToolCall");
            },
            _ => continue,
        }
    }
    assert!(saw_human);

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
