//! Integration test: two parallel sessions with distinct declared_outcomes
//! each expose their exact declared enum; wire acceptance lives in the MCP Engine fixture.

use std::collections::BTreeMap;
use std::str::FromStr;

use surge_acp::bridge::{AgentKind, AlwaysAllowSandbox, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;

fn cfg_with(outcome: &str, wt: &std::path::Path) -> SessionConfig {
    SessionConfig {
        stage_mcp: None,
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
        env: Default::default(),
    }
}

#[test]
fn session_catalogs_have_exact_distinct_outcome_enums() {
    let wt = TempDir::new().unwrap();
    let first = cfg_with("done", wt.path()).stage_tools();
    let second = cfg_with("blocked", wt.path()).stage_tools();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(
        first[0].input_schema["properties"]["outcome"]["enum"],
        serde_json::json!(["done"])
    );
    assert_eq!(
        second[0].input_schema["properties"]["outcome"]["enum"],
        serde_json::json!(["blocked"])
    );
    assert!(
        first[0].input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("call_id"))
    );
}
