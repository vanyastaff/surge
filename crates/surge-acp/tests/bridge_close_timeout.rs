//! Integration test: close_session against a stuck (frozen) mock returns
//! GracefulTimedOut and emits SessionEnded::Timeout.
//!
//! The test has a hard 30-second outer timeout so that any regression in the
//! kill_tx mechanism causes a fast, clear failure rather than hanging CI.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use surge_acp::bridge::{
    AgentKind, AlwaysAllowSandbox, BridgeConfig, BridgeEvent, BridgeTimeouts, CloseSessionError,
    SessionConfig, SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

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
    let bridge = BridgeConfig::new()
        .timeouts(BridgeTimeouts {
            handshake: Duration::from_secs(10),
            shutdown: Duration::from_secs(15),
        })
        .spawn()
        .unwrap();
    let mut events = bridge.subscribe();

    // The `frozen` scenario processes the ACP handshake normally but the binary
    // never exits on stdin EOF — it blocks forever. close_session_impl hits its
    // 5s grace timeout, sends kill_tx to subprocess_waiter, which force-kills
    // the child and emits SessionEnded::Timeout. The result is
    // GracefulTimedOut { killed: true }.
    let cfg = SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
        agent_kind: AgentKind::Custom {
            binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
            args: vec![
                "--scenario".into(),
                "frozen".into(),
                "--pid-file".into(),
                wt.path().join("pid").display().to_string(),
            ],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: Default::default(),
    };

    let sid = bridge.open_session(cfg).await.unwrap().session;
    let _ = timeout(Duration::from_secs(2), events.recv()).await;

    // close_session will block ~5s waiting for the frozen child to exit, then
    // send kill_tx → subprocess_waiter kills the child → GracefulTimedOut { killed: true }.
    let close_start = Instant::now();
    let close_result = bridge.close_session(sid).await;
    assert!(close_start.elapsed() >= Duration::from_millis(5000));
    assert!(close_start.elapsed() < Duration::from_secs(15));
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
                if matches!(reason, SessionEndReason::Timeout { duration_ms: 5000 }) {
                    saw_timeout = true;
                    break;
                }
            },
            _ => continue,
        }
    }
    assert!(saw_timeout);

    bridge.shutdown().await.unwrap();
    let pid = std::fs::read_to_string(wt.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let tracker = surge_acp::ProcessTracker::new(wt.path().join("tracking")).unwrap();
    tracker.track("child", pid).unwrap();
    assert!(
        !tracker.is_running("child"),
        "frozen direct child remains alive"
    );
}
