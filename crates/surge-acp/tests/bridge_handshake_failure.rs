//! Integration test: MOCK_ACP_HANDSHAKE_FAIL=1 causes OpenSessionError::HandshakeFailed
//! or AgentSpawnFailed (depending on which side races wins).

use std::collections::BTreeMap;
use std::str::FromStr;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, OpenSessionError, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_failure_returns_open_session_error() {
    let wt = TempDir::new().unwrap();
    // Pass --handshake-fail as a CLI flag instead of mutating the process-global
    // MOCK_ACP_HANDSHAKE_FAIL env var, which is fragile under parallel test execution.
    let bridge = AcpBridge::with_defaults().unwrap();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--handshake-fail".into()],
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

    let err = bridge.open_session(cfg).await.unwrap_err();
    assert!(matches!(
        err,
        OpenSessionError::HandshakeFailed { .. } | OpenSessionError::AgentSpawnFailed { .. }
    ));

    bridge.shutdown().await.unwrap();
}
