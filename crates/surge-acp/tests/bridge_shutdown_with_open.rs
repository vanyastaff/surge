//! Integration test: AcpBridge::shutdown() while sessions are open emits
//! a SessionEnded for each open session.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, SessionConfig, SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_emits_session_ended_for_each_open_session() {
    tokio::time::timeout(Duration::from_secs(60), inner_test())
        .await
        .expect("test exceeded 60s — shutdown likely deadlocked");
}

async fn inner_test() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let mut sids: Vec<SessionId> = Vec::with_capacity(2);
    for _ in 0..2 {
        let cfg = SessionConfig {
            agent_kind: AgentKind::Mock {
                args: vec!["--scenario".into(), "echo".into()],
            },
            working_dir: wt.path().to_path_buf(),
            system_prompt: "x".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        };
        sids.push(bridge.open_session(cfg).await.unwrap());
    }

    bridge.shutdown().await.unwrap();

    // Accept any bridge-initiated end reason: ForcedClose (from close_all_sessions
    // explicit emit) OR Timeout (from subprocess_waiter racing the kill_tx). Both
    // indicate the bridge terminated the session.
    let mut ended = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && ended.len() < 2 {
        if let Ok(Ok(BridgeEvent::SessionEnded { session, reason })) =
            timeout(Duration::from_millis(100), events.recv()).await
        {
            // Accept ForcedClose or Timeout (both bridge-initiated). Reject
            // AgentCrashed and Normal — those would indicate the test is observing
            // some other path than bridge-initiated shutdown.
            assert!(
                matches!(
                    reason,
                    SessionEndReason::ForcedClose | SessionEndReason::Timeout { .. }
                ),
                "expected bridge-initiated reason, got {reason:?}"
            );
            ended.insert(session);
        }
    }
    assert_eq!(ended.len(), 2);
}
