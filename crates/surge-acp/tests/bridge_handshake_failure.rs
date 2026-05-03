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
    // The mock honors MOCK_ACP_HANDSHAKE_FAIL=1 by exiting before handshake.
    // SAFETY: tokio multi-thread tests share env; this test runs alone.
    unsafe {
        std::env::set_var("MOCK_ACP_HANDSHAKE_FAIL", "1");
    }

    let bridge = AcpBridge::with_defaults().unwrap();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec![] },
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

    unsafe {
        std::env::remove_var("MOCK_ACP_HANDSHAKE_FAIL");
    }
    bridge.shutdown().await.unwrap();
}
