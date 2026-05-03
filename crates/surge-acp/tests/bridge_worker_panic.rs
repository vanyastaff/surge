//! Integration test: worker thread panic surfaces as BridgeError on next send.
//!
//! Uses the test-only `__test_panic_now` helper gated by `feature = "test-helpers"`.
//! Cargo.toml's `[[test]]` entry for this file specifies `required-features = ["test-helpers"]`.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeError, OpenSessionError, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::sleep;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_panic_surfaces_as_command_failure() {
    tokio::time::timeout(Duration::from_secs(30), inner_test())
        .await
        .expect("test exceeded 30s — worker panic surfacing path is broken");
}

async fn inner_test() {
    let bridge = AcpBridge::with_defaults().unwrap();

    bridge.__test_panic_now();
    sleep(Duration::from_millis(200)).await;

    let wt = TempDir::new().unwrap();
    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "echo".into()],
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
    assert!(
        matches!(
            err,
            OpenSessionError::Bridge(BridgeError::CommandSendFailed(_))
                | OpenSessionError::Bridge(BridgeError::ReplyDropped)
        ),
        "got {err:?}"
    );
}
