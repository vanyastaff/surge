//! Integration test: agent subprocess crash surfaces as
//! BridgeEvent::SessionEnded::AgentCrashed within 2 seconds.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

// Ignored: hangs under cargo-nextest waiting for the SessionEnded::Crashed
// event (passes under cargo test on main). Cause is nextest
// process-isolation specific — investigation tracked separately.
#[ignore = "nextest hang — see investigation task; passes under cargo test"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_after_n_tool_calls_surfaces_within_2s() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            // crash_after=0: mock crashes on the first prompt
            // (count > N is satisfied at count=1, N=0).
            args: vec!["--scenario".into(), "crash_after=0".into()],
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

    // Drain SessionEstablished
    let _ = timeout(Duration::from_secs(2), events.recv()).await;

    // Trigger a prompt — mock crashes on first prompt.
    bridge
        .send_message(sid, MessageContent::Text("crash now".into()))
        .await
        .ok();

    let crash_start = Instant::now();
    let mut saw_crash = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => match reason
            {
                SessionEndReason::AgentCrashed { exit_code, .. } => {
                    assert_eq!(exit_code, Some(137));
                    saw_crash = true;
                    let elapsed = crash_start.elapsed();
                    assert!(
                        elapsed <= Duration::from_secs(2),
                        "crash detection took {elapsed:?}, expected ≤2s"
                    );
                    break;
                },
                other => panic!("expected AgentCrashed, got {other:?}"),
            },
            _ => continue,
        }
    }
    assert!(saw_crash, "did not observe AgentCrashed within deadline");

    bridge.shutdown().await.unwrap();
}
