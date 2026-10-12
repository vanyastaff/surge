//! Integration test: agent subprocess crash surfaces as
//! BridgeEvent::SessionEnded::AgentCrashed within 2 seconds.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_after_n_tool_calls_surfaces_within_2s() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
        agent_kind: AgentKind::Mock {
            // crash_after=0: mock crashes on the first prompt
            // (count > N is satisfied at count=1, N=0).
            args: vec!["--scenario".into(), "crash_after=0".into()],
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

    let sid = bridge.open_session(cfg).await.unwrap().session;

    // Drain SessionEstablished
    let _ = timeout(Duration::from_secs(2), events.recv()).await;

    let crash_start = Instant::now();
    let (prompt, ended) = timeout(Duration::from_secs(2), async {
        tokio::join!(
            bridge.send_message(sid, MessageContent::Text("crash now".into())),
            async {
                loop {
                    if let BridgeEvent::SessionEnded { session, reason } =
                        events.recv().await.unwrap()
                    {
                        assert_eq!(session, sid);
                        break reason;
                    }
                }
            }
        )
    })
    .await
    .expect("prompt dispatch and crash detection exceeded 2s");
    assert!(prompt.is_err(), "crashed prompt unexpectedly succeeded");
    assert!(crash_start.elapsed() <= Duration::from_secs(2));
    match ended {
        SessionEndReason::AgentCrashed {
            exit_code,
            stderr_tail,
        } => {
            assert_eq!(exit_code, Some(137));
            assert!(
                stderr_tail.contains("crash_after=0 threshold reached at prompt #1; exiting 137"),
                "{stderr_tail}"
            );
        },
        other => panic!("expected AgentCrashed, got {other:?}"),
    }

    bridge.shutdown().await.unwrap();
}
