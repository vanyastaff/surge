//! Integration test: 5 parallel sessions, all close cleanly, no deadlock.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn five_concurrent_sessions_complete_independently() {
    // Hard outer timeout — concurrent session shutdown shouldn't take more
    // than 60s. Anything longer indicates a deadlock regression.
    tokio::time::timeout(Duration::from_secs(60), inner_test()).await
        .expect("test exceeded 60s — likely a deadlock in concurrent session handling");
}

async fn inner_test() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let mut sids: Vec<SessionId> = Vec::with_capacity(5);
    for _ in 0..5 {
        let cfg = SessionConfig {
            agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
            working_dir: wt.path().to_path_buf(),
            system_prompt: "x".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        };
        let sid = bridge.open_session(cfg).await.expect("open session");
        sids.push(sid);
    }
    assert_eq!(sids.iter().collect::<HashSet<_>>().len(), 5, "session ids must be distinct");

    for sid in &sids {
        bridge.send_message(sid.clone(), MessageContent::Text("hi".into())).await.ok();
    }
    for sid in &sids {
        bridge.close_session(sid.clone()).await.ok();
    }

    // Drain events; expect 5 SessionEnded events.
    let mut ended = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && ended.len() < 5 {
        if let Ok(Ok(BridgeEvent::SessionEnded { session, .. })) =
            timeout(Duration::from_millis(200), events.recv()).await
        {
            ended.insert(session);
        }
    }
    assert_eq!(ended.len(), 5, "expected 5 SessionEnded; got {}", ended.len());

    bridge.shutdown().await.unwrap();
}
