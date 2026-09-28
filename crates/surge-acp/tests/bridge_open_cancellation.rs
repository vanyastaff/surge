//! A caller timeout must not wedge shutdown or Drop behind handshake.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use surge_acp::bridge::*;

#[test]
fn cancelled_open_settles_worker_and_child() {
    watchdog("cancelled_open_settles_worker_and_child", false);
}

#[test]
fn bare_drop_during_opening_is_bounded_and_reaps() {
    watchdog("bare_drop_during_opening_is_bounded_and_reaps", true);
}

fn watchdog(test: &str, bare: bool) {
    if let Some(root) = std::env::var_os("SURGE_OPEN_CANCEL_HELPER") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                if bare {
                    bare_drop(PathBuf::from(root)).await;
                } else {
                    helper(PathBuf::from(root)).await;
                }
            });
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("SURGE_OPEN_CANCEL_HELPER", root.path())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !result.is_some_and(|status| status.success())
        && let Ok(pid) = std::fs::read_to_string(root.path().join("pid"))
    {
        #[cfg(unix)]
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid.trim()])
            .status();
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", pid.trim()])
            .status();
    }
    assert!(
        result.is_some_and(|status| status.success()),
        "cancellation left worker or child unsettled"
    );
}

async fn helper(root: PathBuf) {
    let bridge = AcpBridge::with_defaults().unwrap();
    let config = config(&root);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), bridge.open_session(config))
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(2), bridge.shutdown())
        .await
        .expect("shutdown ignored opening cancellation")
        .unwrap();
    let pid: u32 = std::fs::read_to_string(root.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let tracker = surge_acp::ProcessTracker::new(root.join("tracking")).unwrap();
    tracker.track("child", pid).unwrap();
    assert!(
        !tracker.is_running("child"),
        "owned child remained alive after shutdown success"
    );
}

fn config(root: &std::path::Path) -> SessionConfig {
    SessionConfig {
        stage_mcp: None,
        agent_kind: AgentKind::Custom {
            binary: PathBuf::from(env!("CARGO_BIN_EXE_mock_acp_agent")),
            args: vec![
                "--stall-new-session".into(),
                "--pid-file".into(),
                root.join("pid").display().to_string(),
            ],
        },
        working_dir: root.into(),
        system_prompt: "test".into(),
        declared_outcomes: vec![surge_core::OutcomeKey::try_from("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: surge_acp::client::PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: BTreeMap::new(),
    }
}

async fn bare_drop(root: PathBuf) {
    let bridge = AcpBridge::with_defaults().unwrap();
    {
        let opening = bridge.open_session(config(&root));
        tokio::pin!(opening);
        tokio::select! {
            result = &mut opening => panic!("opening unexpectedly ended: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(2), async {
                while !root.join("pid").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
            }) => result.unwrap(),
        }
    }
    let start = Instant::now();
    drop(bridge); // No explicit shutdown; the dedicated owner must remain alive.
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "Drop blocked caller thread"
    );
    let pid = std::fs::read_to_string(root.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let tracker = surge_acp::ProcessTracker::new(root.join("tracking")).unwrap();
    tracker.track("child", pid).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while tracker.is_running("child") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bare Drop did not reap the opening child");
}
