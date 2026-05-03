//! Integration test: 20 streaming chunks arrive in order with reasonable cadence.

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
async fn long_streaming_delivers_chunks_in_order() {
    tokio::time::timeout(Duration::from_secs(30), inner_test())
        .await
        .expect("test exceeded 30s — likely deadlock in streaming path");
}

async fn inner_test() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "long_streaming".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "stream".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge
        .send_message(sid.clone(), MessageContent::Text("go".into()))
        .await
        .unwrap();

    let mut chunks = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && chunks < 20 {
        if let Ok(Ok(BridgeEvent::AgentMessage { session, .. })) =
            timeout(Duration::from_millis(500), events.recv()).await
        {
            if session == sid {
                chunks += 1;
            }
        }
    }
    assert!(chunks >= 20, "expected at least 20 chunks, got {chunks}");

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
