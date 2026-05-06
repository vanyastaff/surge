//! Integration test: close_session against a stuck (frozen) mock returns
//! GracefulTimedOut and emits SessionEnded::Timeout.
//!
//! The test has a hard 30-second outer timeout so that any regression in the
//! kill_tx mechanism causes a fast, clear failure rather than hanging CI.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, CloseSessionError, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

// Ignored: M5.1 bridge-worker reply-routing limitation surfaces here as
// a hang. The internal tokio::time::timeout(30s) does not unwind cleanly
// because close_session_impl + the kill_tx mechanism deadlocks against
// the worker. Re-enable (drop #[ignore]) once M5.1 lands; until then
// CI runs this only via `cargo nextest run --run-ignored=ignored-only`
// in the dedicated step.
#[ignore = "M5.1 hang: bridge worker reply routing — re-enable when M5.1 lands"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_against_stuck_mock_times_out() {
    // Hard outer timeout — if close_session_impl + the kill_tx mechanism is
    // broken, this fires in 30s instead of hanging CI for hours.
    timeout(Duration::from_secs(30), inner_test())
        .await
        .expect("test exceeded 30s — kill_tx mechanism is likely broken");
}

async fn inner_test() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    // The `frozen` scenario processes the ACP handshake normally but the binary
    // never exits on stdin EOF — it blocks forever. close_session_impl hits its
    // 5s grace timeout, sends kill_tx to subprocess_waiter, which force-kills
    // the child and emits SessionEnded::Timeout. The result is
    // GracefulTimedOut { killed: true }.
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
    // send kill_tx → subprocess_waiter kills the child → GracefulTimedOut { killed: true }.
    let close_result = bridge.close_session(sid).await;
    assert!(
        matches!(
            close_result,
            Err(CloseSessionError::GracefulTimedOut { killed: true, .. })
        ),
        "got {close_result:?}"
    );

    // SessionEnded::Timeout should have been emitted (by subprocess_waiter or
    // close_session_impl's fallback path).
    let mut saw_timeout = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                if matches!(reason, SessionEndReason::Timeout { .. }) {
                    saw_timeout = true;
                    break;
                }
            },
            _ => continue,
        }
    }
    assert!(saw_timeout);

    bridge.shutdown().await.unwrap();
}
