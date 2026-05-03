//! Integration test: close_session against a stuck (frozen) mock returns
//! GracefulTimedOut and emits SessionEnded::Timeout.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, CloseSessionError, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_against_stuck_mock_times_out() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    // The `frozen` scenario (added in Task 10.1 fixup) processes prompts
    // normally but the binary never exits on stdin EOF — `std::future::pending()`
    // blocks forever. The bridge's close_session_impl will hit its 5s grace
    // timeout and emit SessionEnded::Timeout. M3 has no force-kill path, so
    // killed=false (per Task 8.3 limitation).
    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "frozen".into()],
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

    let sid = bridge.open_session(cfg).await.unwrap();
    let _ = timeout(Duration::from_secs(2), events.recv()).await;

    // close_session will block ~5s waiting for the frozen child to exit, then
    // give up and return GracefulTimedOut { killed: false }.
    let close_result = bridge.close_session(sid.clone()).await;
    assert!(matches!(
        close_result,
        Err(CloseSessionError::GracefulTimedOut { killed: false, .. })
    ), "got {close_result:?}");

    // SessionEnded::Timeout should follow.
    let mut saw_timeout = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                if matches!(reason, SessionEndReason::Timeout { .. }) {
                    saw_timeout = true;
                    break;
                }
            }
            _ => continue,
        }
    }
    assert!(saw_timeout);

    bridge.shutdown().await.unwrap();
}
