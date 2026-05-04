//! Exercises the `MockBridge` fixture to verify it records calls correctly
//! and correctly pumps scripted events to subscribers.

mod fixtures;

use fixtures::mock_bridge::MockBridge;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::sandbox::AlwaysAllowSandbox;
use surge_acp::bridge::session::AgentKind;
use surge_acp::bridge::session::SessionConfig;
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};

fn minimal_session_config() -> SessionConfig {
    SessionConfig {
        agent_kind: AgentKind::Mock { args: vec![] },
        working_dir: PathBuf::from("/tmp/wt"),
        system_prompt: "sys".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    }
}

#[tokio::test]
async fn records_open_session() {
    let m = MockBridge::new();
    let _id = m.open_session(minimal_session_config()).await;
    let calls = m.recorded_calls.lock().await;
    assert!(
        matches!(calls[0], fixtures::mock_bridge::RecordedCall::OpenSession),
        "expected OpenSession in recorded calls"
    );
}

#[tokio::test]
async fn records_reply_to_tool() {
    let m = MockBridge::new();
    let session = SessionId::new();
    let _ = m
        .reply_to_tool(
            session,
            "call-1".into(),
            ToolResultPayload::Ok {
                result_json: "{}".into(),
            },
        )
        .await;
    let calls = m.recorded_calls.lock().await;
    match &calls[0] {
        fixtures::mock_bridge::RecordedCall::ReplyToTool { call_id, .. } => {
            assert_eq!(call_id, "call-1")
        },
        other => panic!("expected ReplyToTool, got {other:?}"),
    }
}

#[tokio::test]
async fn pumps_scripted_event() {
    let m = MockBridge::new();
    let mut rx = m.subscribe();
    let session = SessionId::new();
    let ev = BridgeEvent::SessionEnded {
        session,
        reason: SessionEndReason::Normal,
    };
    m.enqueue_event(ev).await;
    // Yield to let subscribe task land Subscribe call (best-effort).
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    m.pump_scripted_events().await;
    let received = rx.recv().await.unwrap();
    assert!(
        matches!(received, BridgeEvent::SessionEnded { .. }),
        "expected SessionEnded event"
    );
}
