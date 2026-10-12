//! Integration test: open session → send text → receive echo → close.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeConfig, BridgeEvent, BridgeTimeouts,
    MessageContent, OpenSessionError, SessionConfig, SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_send_close_round_trip() {
    round_trip(AcpBridge::with_defaults().expect("spawn bridge")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builder_defaults_round_trip() {
    round_trip(BridgeConfig::new().spawn().unwrap()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builder_explicit_round_trip() {
    round_trip(
        BridgeConfig::new()
            .cmd_capacity(2)
            .event_capacity(16)
            .timeouts(BridgeTimeouts {
                handshake: Duration::from_secs(5),
                shutdown: Duration::from_secs(7),
            })
            .spawn()
            .unwrap(),
    )
    .await;
}

fn session_config(wt: &TempDir) -> SessionConfig {
    SessionConfig {
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
        working_dir: wt.path().to_path_buf(),
        system_prompt: "you are a mock".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: Default::default(),
    }
}

async fn round_trip(bridge: AcpBridge) {
    let wt = TempDir::new().unwrap();
    let mut events = bridge.subscribe();

    let cfg = session_config(&wt);

    let sid = bridge
        .open_session(cfg)
        .await
        .expect("open session")
        .session;

    // Expect SessionEstablished as first event.
    let ev = timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    match ev {
        BridgeEvent::SessionEstablished { session, agent, .. } => {
            assert_eq!(session, sid);
            assert_eq!(agent, "mock");
        },
        other => panic!("expected SessionEstablished, got {other:?}"),
    }

    bridge
        .send_message(sid, MessageContent::Text("hello".into()))
        .await
        .unwrap();

    // Drain until AgentMessage is observed (spec §9.2 requires this assertion).
    let agent_msg_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_agent_msg = false;
    while tokio::time::Instant::now() < agent_msg_deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::AgentMessage { session, chunk, .. })) if session == sid => {
                assert!(!chunk.is_empty(), "agent message chunk should be non-empty");
                saw_agent_msg = true;
                break;
            },
            _ => continue,
        }
    }
    assert!(
        saw_agent_msg,
        "did not observe BridgeEvent::AgentMessage from echo scenario"
    );

    bridge.close_session(sid).await.expect("close session");

    // Drain events until SessionEnded.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_end = false;
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                assert!(
                    matches!(reason, SessionEndReason::Normal),
                    "expected Normal close, got {reason:?} (regression of close_session_impl io_task_handle.abort()?)"
                );
                saw_end = true;
                break;
            },
            _ => continue,
        }
    }
    assert!(saw_end, "did not observe SessionEnded for {sid}");

    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builder_applies_handshake_deadline() {
    let wt = TempDir::new().unwrap();
    let bridge = BridgeConfig::new()
        .timeouts(BridgeTimeouts {
            handshake: Duration::from_millis(500),
            shutdown: Duration::from_secs(7),
        })
        .spawn()
        .unwrap();
    let mut config = session_config(&wt);
    config.agent_kind = AgentKind::Custom {
        binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
        args: vec![
            "--stall-new-session".into(),
            "--pid-file".into(),
            wt.path().join("pid").display().to_string(),
        ],
    };
    let error = timeout(Duration::from_secs(3), bridge.open_session(config))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(
            error,
            OpenSessionError::HandshakeTimedOut {
                phase: "new_session",
                ..
            }
        ),
        "{error:?}"
    );
    bridge.shutdown().await.unwrap();
    let pid = std::fs::read_to_string(wt.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let tracker = surge_acp::ProcessTracker::new(wt.path().join("tracking")).unwrap();
    tracker.track("child", pid).unwrap();
    assert!(!tracker.is_running("child"), "direct child was not reaped");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builder_event_capacity_reports_lag_without_blocking_commands() {
    let wt = TempDir::new().unwrap();
    let bridge = BridgeConfig::new().event_capacity(2).spawn().unwrap();
    let mut events = bridge.subscribe();
    let sid = bridge
        .open_session(session_config(&wt))
        .await
        .unwrap()
        .session;
    for _ in 0..4 {
        timeout(
            Duration::from_secs(3),
            bridge.send_message(sid, MessageContent::Text("hello".into())),
        )
        .await
        .unwrap()
        .unwrap();
    }
    bridge.close_session(sid).await.unwrap();
    assert!(matches!(
        events.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
    ));
    bridge.shutdown().await.unwrap();
}
