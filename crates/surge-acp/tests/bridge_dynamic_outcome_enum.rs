//! Integration test: two parallel sessions with distinct declared_outcomes
//! each see their own enum and accept their own outcome.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

fn cfg_with(outcome: &str, wt: &std::path::Path) -> SessionConfig {
    SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), format!("report_outcome={outcome}")],
        },
        working_dir: wt.to_path_buf(),
        system_prompt: "go".into(),
        declared_outcomes: vec![OutcomeKey::from_str(outcome).unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_sessions_use_distinct_outcome_enums() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let s1 = bridge.open_session(cfg_with("done", wt.path())).await.unwrap();
    let s2 = bridge.open_session(cfg_with("blocked", wt.path())).await.unwrap();

    bridge.send_message(s1.clone(), surge_acp::bridge::MessageContent::Text("go".into())).await.unwrap();
    bridge.send_message(s2.clone(), surge_acp::bridge::MessageContent::Text("go".into())).await.unwrap();

    let mut saw_done = false;
    let mut saw_blocked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !(saw_done && saw_blocked) {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::OutcomeReported { session, outcome, .. })) => {
                if session == s1 && outcome.as_str() == "done" { saw_done = true; }
                if session == s2 && outcome.as_str() == "blocked" { saw_blocked = true; }
            }
            _ => continue,
        }
    }
    assert!(saw_done && saw_blocked, "saw_done={saw_done} saw_blocked={saw_blocked}");

    bridge.close_session(s1).await.ok();
    bridge.close_session(s2).await.ok();
    bridge.shutdown().await.unwrap();
}
