//! Integration test: AcpBridge::shutdown() while sessions are open emits
//! a SessionEnded for each open session.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, SessionConfig, SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_emits_session_ended_for_each_open_session() {
    tokio::time::timeout(Duration::from_secs(60), inner_test())
        .await
        .expect("test exceeded 60s — shutdown likely deadlocked");
}

async fn inner_test() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    #[cfg(windows)]
    let mut jobs = Vec::new();
    let mut sids: Vec<SessionId> = Vec::with_capacity(2);
    for index in 0..2 {
        let cfg = SessionConfig {
            effect_fence: None,
            writer_id: surge_core::id::ExecutionWriterId::new(),
            invocation: surge_core::id::StageInvocationId::new(),
            runtime: "fixture".into(),
            opening: Default::default(),
            config_selections: Vec::new(),
            stage_mcp: None,
            agent_kind: AgentKind::Custom {
                binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
                args: vec![
                    "--scenario".into(),
                    "echo".into(),
                    "--pid-file".into(),
                    wt.path().join(format!("pid-{index}")).display().to_string(),
                ],
            },
            working_dir: wt.path().to_path_buf(),
            system_prompt: "x".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
            env: Default::default(),
        };
        let opened = bridge.open_session(cfg).await.unwrap();
        #[cfg(windows)]
        {
            let container = opened
                .execution_writer
                .as_ref()
                .unwrap()
                .container()
                .unwrap();
            let identity = container.windows_job().unwrap();
            assert!(identity.name().starts_with("Global\\SurgeMcp-"));
            let job = surge_process::windows_job::Job::open(identity.name())
                .unwrap()
                .unwrap();
            assert!(job.active_processes().unwrap() >= 1);
            jobs.push(job);
        }
        sids.push(opened.session);
    }

    bridge.shutdown().await.unwrap();
    #[cfg(windows)]
    for job in jobs {
        assert_eq!(job.active_processes().unwrap(), 0);
    }

    // Accept any bridge-initiated end reason: ForcedClose (from close_all_sessions
    // explicit emit) OR Timeout (from subprocess_waiter racing the kill_tx). Both
    // indicate the bridge terminated the session.
    let mut ended = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && ended.len() < 2 {
        if let Ok(Ok(BridgeEvent::SessionEnded { session, reason })) =
            timeout(Duration::from_millis(100), events.recv()).await
        {
            // Accept ForcedClose or Timeout (both bridge-initiated). Reject
            // AgentCrashed and Normal — those would indicate the test is observing
            // some other path than bridge-initiated shutdown.
            assert!(
                matches!(
                    reason,
                    SessionEndReason::ForcedClose | SessionEndReason::Timeout { .. }
                ),
                "expected bridge-initiated reason, got {reason:?}"
            );
            ended.insert(session);
        }
    }
    assert_eq!(ended, sids.into_iter().collect());
    let tracker = surge_acp::ProcessTracker::new(wt.path().join("tracking")).unwrap();
    for index in 0..2 {
        let pid = std::fs::read_to_string(wt.path().join(format!("pid-{index}")))
            .unwrap()
            .parse()
            .unwrap();
        let name = format!("child-{index}");
        tracker.track(&name, pid).unwrap();
        assert!(
            !tracker.is_running(&name),
            "direct child {index} remains alive"
        );
    }
}
