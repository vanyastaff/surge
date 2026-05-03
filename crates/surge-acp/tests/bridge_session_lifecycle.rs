//! Integration test: open session → send text → receive echo → close.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_send_close_round_trip() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().expect("spawn bridge");
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "you are a mock".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.expect("open session");

    // Expect SessionEstablished as first event.
    let ev = timeout(Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
    match ev {
        BridgeEvent::SessionEstablished { session, agent, .. } => {
            assert_eq!(session, sid);
            assert_eq!(agent, "mock");
        }
        other => panic!("expected SessionEstablished, got {other:?}"),
    }

    bridge.send_message(sid.clone(), MessageContent::Text("hello".into())).await.unwrap();

    // Drain until AgentMessage is observed (spec §9.2 requires this assertion).
    let agent_msg_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_agent_msg = false;
    while tokio::time::Instant::now() < agent_msg_deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::AgentMessage { session, chunk, .. })) if session == sid => {
                assert!(!chunk.is_empty(), "agent message chunk should be non-empty");
                saw_agent_msg = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(saw_agent_msg, "did not observe BridgeEvent::AgentMessage from echo scenario");

    bridge.close_session(sid.clone()).await.expect("close session");

    // Drain events until SessionEnded.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_end = false;
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                assert!(
                    matches!(reason, SessionEndReason::Normal),
                    "expected Normal close, got {reason:?} (regression of close_session_impl io_task_handle.abort()?)"
                );
                saw_end = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(saw_end, "did not observe SessionEnded for {sid}");

    bridge.shutdown().await.unwrap();
}
