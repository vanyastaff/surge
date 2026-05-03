//! Integration test: DenyListSandbox removes denied tools from the agent's
//! visible tool list, observable via BridgeEvent::SessionEstablished::tools_visible.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use serde_json::json;
use surge_acp::bridge::{
    AcpBridge, AgentKind, BridgeEvent, DenyListSandbox, SessionConfig, ToolCategory, ToolDef,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denied_tool_does_not_appear_in_visible_list() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let tools = vec![
        ToolDef::new("read_file", "read", ToolCategory::Builtin, json!({})),
        ToolDef::new(
            "shell_exec",
            "shell",
            ToolCategory::Mcp("ops".into()),
            json!({}),
        ),
        ToolDef::new("write_file", "write", ToolCategory::Builtin, json!({})),
    ];
    let sandbox = DenyListSandbox::deny_tools(["shell_exec"]);

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "echo".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools,
        sandbox: Box::new(sandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let _sid = bridge.open_session(cfg).await.unwrap();

    let ev = timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    match ev {
        BridgeEvent::SessionEstablished { tools_visible, .. } => {
            assert!(tools_visible.contains(&"read_file".into()));
            assert!(tools_visible.contains(&"write_file".into()));
            assert!(tools_visible.contains(&"report_stage_outcome".into()));
            assert!(
                !tools_visible.contains(&"shell_exec".into()),
                "shell_exec should be filtered out by sandbox"
            );
        },
        other => panic!("expected SessionEstablished, got {other:?}"),
    }

    bridge.shutdown().await.unwrap();
}
