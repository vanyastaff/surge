//! Integration test: report_stage_outcome surfaces as BridgeEvent::OutcomeReported,
//! NOT a generic ToolCall.

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
async fn report_stage_outcome_emits_outcome_reported_event() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "report_done".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "do thing".into(),
        declared_outcomes: vec![
            OutcomeKey::from_str("done").unwrap(),
            OutcomeKey::from_str("blocked").unwrap(),
        ],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge.send_message(sid.clone(), MessageContent::Text("go".into())).await.unwrap();

    let mut saw_outcome = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::OutcomeReported { outcome, .. })) => {
                assert_eq!(outcome.as_str(), "done");
                saw_outcome = true;
                break;
            }
            Ok(Ok(BridgeEvent::ToolCall { tool, .. })) if tool == "report_stage_outcome" => {
                panic!("report_stage_outcome should NOT surface as generic ToolCall");
            }
            _ => continue,
        }
    }
    assert!(saw_outcome);

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
