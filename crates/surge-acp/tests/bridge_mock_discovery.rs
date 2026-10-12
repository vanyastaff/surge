//! Mock executable lookup is exercised in isolated processes, without changing
//! the test runner's environment. Native executable suffixes matter on Windows.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use surge_acp::bridge::{AcpBridge, AgentKind, AlwaysAllowSandbox, SessionConfig};
use surge_acp::client::PermissionPolicy;
use tempfile::TempDir;

#[test]
fn mock_executable_discovery_uses_native_paths() {
    let root = TempDir::new().unwrap();
    let debug = root.path().join("debug");
    std::fs::create_dir(&debug).unwrap();
    let binary = debug.join(format!("mock_acp_agent{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_mock_acp_agent"), &binary).unwrap();
    for explicit in [true, false] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--ignored",
                "--exact",
                "isolated_mock_discovery_child",
                "--nocapture",
            ])
            .env("SURGE_MOCK_DISCOVERY_CHILD", "1")
            .env("CARGO_TARGET_DIR", root.path());
        if explicit {
            child.env("CARGO_BIN_EXE_mock_acp_agent", &binary);
        } else {
            child.env_remove("CARGO_BIN_EXE_mock_acp_agent");
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "explicit={explicit}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "subprocess-only helper exercised by mock_executable_discovery_uses_native_paths"]
async fn isolated_mock_discovery_child() {
    assert!(
        std::env::var_os("SURGE_MOCK_DISCOVERY_CHILD").is_some(),
        "subprocess marker missing"
    );
    tokio::time::timeout(Duration::from_secs(20), async {
        let root = TempDir::new().unwrap();
        let bridge = AcpBridge::with_defaults().unwrap();
        let opened = bridge
            .open_session(SessionConfig {
                effect_fence: None,
                writer_id: surge_core::id::ExecutionWriterId::new(),
                invocation: surge_core::id::StageInvocationId::new(),
                runtime: "fixture".into(),
                opening: Default::default(),
                config_selections: Vec::new(),
                stage_mcp: None,
                agent_kind: AgentKind::Mock {
                    args: vec!["--scenario".into(), "echo".into()],
                },
                working_dir: root.path().to_path_buf(),
                system_prompt: "x".into(),
                declared_outcomes: vec!["done".parse().unwrap()],
                allows_escalation: false,
                tools: vec![],
                sandbox: Box::new(AlwaysAllowSandbox),
                permission_policy: PermissionPolicy::default(),
                bindings: BTreeMap::new(),
                env: Default::default(),
            })
            .await
            .unwrap();
        bridge.close_session(opened.session).await.unwrap();
        bridge.shutdown().await.unwrap();
    })
    .await
    .expect("isolated mock lookup exceeded 20 seconds");
}
