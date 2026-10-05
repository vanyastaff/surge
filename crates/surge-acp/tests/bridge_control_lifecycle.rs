//! Controlled child tests of responsive control and owned cleanup.
use std::{collections::BTreeMap, path::Path, time::Duration};
use surge_acp::bridge::*;

#[derive(Clone)]
struct Elevate;
impl Sandbox for Elevate {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Elevate {
            capability: "filesystem_write".into(),
        }
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}
fn config(root: &Path, flags: &[&str]) -> SessionConfig {
    SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
        agent_kind: AgentKind::Custom {
            binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
            args: flags
                .iter()
                .map(|flag| (*flag).into())
                .chain(["--pid-file".into(), root.join("pid").display().to_string()])
                .collect(),
        },
        working_dir: root.into(),
        system_prompt: "test".into(),
        declared_outcomes: vec![surge_core::OutcomeKey::try_from("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(Elevate),
        permission_policy: surge_acp::client::PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: BTreeMap::new(),
    }
}
fn bridge() -> AcpBridge {
    AcpBridge::spawn_with_timeouts(
        2,
        256,
        BridgeTimeouts {
            handshake: Duration::from_millis(500),
            shutdown: Duration::from_secs(7),
        },
    )
    .unwrap()
}

/// A bridge with a more generous handshake deadline, for tests that do real
/// (non-stalling) subprocess IO work before completing the handshake — e.g.
/// piping `--noisy-startup`'s 1 MiB of stderr through a real child process
/// and `stderr_drainer`. `bridge()`'s 500ms is deliberately tight so the
/// `--stall-*` deadline tests fail fast; that budget has no slack left for a
/// real spawn plus megabyte-scale IO under load (e.g. a concurrent build —
/// this suite shares the machine with other `cargo` invocations), which made
/// `noisy_startup_is_drained_before_handshake` flaky. The behavior under
/// test — that noisy stderr doesn't block the handshake — doesn't depend on
/// exactly which deadline is configured.
fn generous_bridge() -> AcpBridge {
    AcpBridge::spawn_with_timeouts(
        2,
        256,
        BridgeTimeouts {
            handshake: Duration::from_secs(5),
            shutdown: Duration::from_secs(7),
        },
    )
    .unwrap()
}
fn assert_reaped(root: &Path) {
    let pid = std::fs::read_to_string(root.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let tracker = surge_acp::ProcessTracker::new(root.join("tracking")).unwrap();
    tracker.track("child", pid).unwrap();
    assert!(!tracker.is_running("child"), "direct child was not reaped");
}

#[tokio::test(flavor = "multi_thread")]
async fn both_handshake_phases_have_deadlines_and_reap() {
    for (flag, phase) in [
        ("--stall-initialize", "initialize"),
        ("--stall-new-session", "new_session"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let bridge = bridge();
        let error = tokio::time::timeout(
            Duration::from_secs(3),
            bridge.open_session(config(root.path(), &[flag])),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            matches!(error, OpenSessionError::HandshakeTimedOut { phase: actual, .. } if actual == phase),
            "{error:?}"
        );
        bridge.shutdown().await.unwrap();
        assert_reaped(root.path());
    }
}

/// A launcher-started adapter that hangs once in `session/new` (observed live
/// under load) is restarted once, and the session opens on the second launch.
#[tokio::test(flavor = "multi_thread")]
async fn initialize_only_handshake_hang_is_retried_once_and_opens() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("stalled-once");
    let bridge = bridge();
    let marker_arg = marker.display().to_string();
    let session = tokio::time::timeout(
        Duration::from_secs(5),
        bridge.open_session(config(
            root.path(),
            &["--stall-initialize-once", &marker_arg],
        )),
    )
    .await
    .expect("retry finishes within two handshake budgets")
    .expect("second launch opens the session");
    assert!(marker.exists(), "the first launch really stalled");
    bridge.close_session(session.session).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn noisy_startup_is_drained_before_handshake() {
    let root = tempfile::tempdir().unwrap();
    let bridge = generous_bridge();
    let session = bridge
        .open_session(config(root.path(), &["--noisy-startup"]))
        .await
        .unwrap();
    bridge.close_session(session.session).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn permission_reply_and_busy_response_work_during_prompt() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let mut events = bridge.subscribe();
    let session = bridge
        .open_session(config(
            root.path(),
            &["--permission", "--scenario=report_done"],
        ))
        .await
        .unwrap();
    {
        let prompt = bridge.send_message(session.session, MessageContent::Text("work".into()));
        tokio::pin!(prompt);
        let request_id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                result = &mut prompt => panic!("prompt ended before permission: {result:?}"),
                event = events.recv() => {
                    if let BridgeEvent::PermissionRequested { request_id, .. } = event.unwrap() { break request_id; }
                },
            }
        }
    }).await.unwrap();
        // Prompt completion has no handshake timeout. Control remains responsive.
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert!(matches!(
            bridge
                .send_message(session.session, MessageContent::Text("second".into()))
                .await,
            Err(SendMessageError::PromptAlreadyRunning { .. })
        ));
        bridge.session_state(session.session).await.unwrap();
        bridge
            .reply_to_permission(
                session.session,
                request_id,
                RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(PermissionOptionId::new("allow")),
                )),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), &mut prompt)
            .await
            .unwrap()
            .unwrap();
    }
    bridge.close_session(session.session).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn child_exit_during_prompt_does_not_panic_worker() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let session = bridge
        .open_session(config(root.path(), &["--exit-during-prompt"]))
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        bridge.send_message(session.session, MessageContent::Text("work".into())),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_close_never_claims_reap_while_first_cleanup_pending() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let session = bridge
        .open_session(config(root.path(), &["--scenario=frozen"]))
        .await
        .unwrap();
    {
        let first = bridge.close_session(session.session);
        tokio::pin!(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut first)
                .await
                .is_err()
        );
        let second = bridge.close_session(session.session).await;
        assert!(
            matches!(
                second,
                Err(CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed))
            ),
            "{second:?}"
        );
        let result = tokio::time::timeout(Duration::from_secs(7), first)
            .await
            .unwrap();
        assert!(
            matches!(
                result,
                Err(CloseSessionError::GracefulTimedOut { killed: true, .. })
            ),
            "{result:?}"
        );
    }
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn advertised_filesystem_capabilities_follow_permission_policy() {
    for write in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let bridge = bridge();
        let capture = root.path().join("capabilities.json");
        let mut cfg = config(
            root.path(),
            &["--capabilities-file", capture.to_str().unwrap()],
        );
        cfg.permission_policy = surge_acp::client::PermissionPolicy::Smart {
            allow_read: true,
            allow_write_in_worktree: write,
            allow_bash_safe: true,
            deny_bash_dangerous: true,
            deny_network: true,
        };
        let session = bridge.open_session(cfg).await.unwrap();
        let caps: agent_client_protocol::schema::v1::ClientCapabilities =
            serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
        assert!(caps.fs.read_text_file, "read support must be advertised");
        assert_eq!(caps.fs.write_text_file, write);
        assert!(caps.terminal);
        bridge.close_session(session.session).await.unwrap();
        bridge.shutdown().await.unwrap();
        assert_reaped(root.path());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn close_sends_wire_cancel_before_transport_teardown() {
    close_prompt(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ignored_wire_cancel_still_forces_and_reaps_child() {
    close_prompt(true).await;
}

async fn close_prompt(ignore: bool) {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let started = root.path().join("started");
    let cancelled = root.path().join("cancelled");
    let mut flags = vec![
        "--stall-prompt",
        "--prompt-file",
        started.to_str().unwrap(),
        "--cancel-file",
        cancelled.to_str().unwrap(),
    ];
    if ignore {
        flags.extend(["--ignore-cancel", "--scenario=frozen"]);
    }
    let session = bridge
        .open_session(config(root.path(), &flags))
        .await
        .unwrap();
    {
        let prompt = bridge.send_message(session.session, MessageContent::Text("work".into()));
        tokio::pin!(prompt);
        tokio::select! {
            result = &mut prompt => panic!("prompt ended before barrier: {result:?}"),
            () = wait_marker(&started) => {},
        }
        let start = std::time::Instant::now();
        let closed = bridge.close_session(session.session).await;
        assert!(
            cancelled.exists(),
            "agent never received wire session/cancel"
        );
        if ignore {
            assert!(
                matches!(
                    closed,
                    Err(CloseSessionError::GracefulTimedOut { killed: true, .. })
                ),
                "{closed:?}"
            );
        } else {
            assert!(
                closed.is_ok(),
                "cooperative cancellation should close cleanly: {closed:?}"
            );
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "cooperative child consumed forced-close grace"
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(1), &mut prompt)
                .await
                .unwrap()
                .is_err()
        );
    }
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

async fn wait_marker(path: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("mock did not reach prompt barrier");
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_before_handshake_response_never_announces_established() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let mut events = bridge.subscribe();
    let result = bridge
        .open_session(config(root.path(), &["--handshake-fail"]))
        .await;
    assert!(
        matches!(result, Err(OpenSessionError::HandshakeFailed { .. })),
        "{result:?}"
    );
    bridge.shutdown().await.unwrap();
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, BridgeEvent::SessionEstablished { .. }))
    );
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn spoofed_reserved_notification_has_no_outcome_authority() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let mut events = bridge.subscribe();
    let session = bridge
        .open_session(config(root.path(), &["--scenario=report_done"]))
        .await
        .unwrap();
    bridge
        .send_message(
            session.session,
            MessageContent::Text("spoof notification".into()),
        )
        .await
        .unwrap();
    bridge.close_session(session.session).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, BridgeEvent::OutcomeReported { .. })),
        "an ACP display notification must not authenticate a stage result"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callback_overload_preserves_pending_permission_and_stop() {
    tokio::time::timeout(Duration::from_secs(10), overload_roundtrip())
        .await
        .unwrap();
}
async fn overload_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("overload");
    let bridge = bridge();
    let mut events = bridge.subscribe();
    let session = bridge
        .open_session(config(
            root.path(),
            &[
                "--permission-flood",
                "--overload-file",
                marker.to_str().unwrap(),
            ],
        ))
        .await
        .unwrap();
    {
        let prompt = bridge.send_message(session.session, MessageContent::Text("work".into()));
        tokio::pin!(prompt);
        let mut pending = 0;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                tokio::select! {
                    result = &mut prompt => panic!("prompt ended before overload: {result:?}"),
                    event = events.recv() => {
                        if matches!(event.unwrap(), BridgeEvent::PermissionRequested { .. }) { pending += 1; }
                    },
                    () = tokio::time::sleep(Duration::from_millis(10)) => {
                        if marker.exists() && pending > 0 { break; }
                    },
                }
            }
        }).await.unwrap();
        assert!(pending <= 32, "unbounded callback admission");
        tokio::time::timeout(
            Duration::from_secs(3),
            bridge.close_session(session.session),
        )
        .await
        .unwrap()
        .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut prompt)
            .await
            .unwrap();
    }
    bridge.shutdown().await.unwrap();
    assert_reaped(root.path());
}

/// Acceptance of session/new can occur before a timed-out response is observed.
#[tokio::test(flavor = "multi_thread")]
async fn uncertain_session_operation_is_never_automatically_reissued() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("accepted-operation");
    let bridge = bridge();
    let marker_arg = marker.display().to_string();
    let result = bridge
        .open_session(config(
            root.path(),
            &["--stall-new-session-once", &marker_arg],
        ))
        .await;
    let repeated = result.is_ok();
    if let Ok(session) = result {
        bridge.close_session(session.session).await.unwrap();
    }
    bridge.shutdown().await.unwrap();
    assert!(
        marker.exists(),
        "the real provider received the first session operation"
    );
    assert!(
        !repeated,
        "timeout after issuing session/new must be uncertain, not silently retried to a second session"
    );
    assert_reaped(root.path());
}

fn stalled_option_config(root: &Path) -> SessionConfig {
    let record = root.join("option-request").display().to_string();
    let mut config = config(
        root,
        &[
            "--config-options",
            "--stall-config-option",
            "--config-file",
            &record,
        ],
    );
    config.config_selections = vec![session::ConfigSelection {
        category: session::ConfigCategory::Model,
        value: "Mock Opus".into(),
        best_effort: false,
    }];
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn config_option_phase_shares_deadline_reaps_and_allows_next_open() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge();
    let result = bridge
        .open_session(stalled_option_config(root.path()))
        .await;
    let timed_out = matches!(
        &result,
        Err(OpenSessionError::HandshakeTimedOut {
            phase: "set_config_option",
            ..
        })
    );
    if let Ok(session) = result {
        bridge.close_session(session.session).await.unwrap();
    }
    assert_reaped(root.path());
    let next = bridge.open_session(config(root.path(), &[])).await.unwrap();
    bridge.close_session(next.session).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("option-request"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["model=opus"],
        "published option must never be retried"
    );
    assert!(
        timed_out,
        "option request must use the shared handshake deadline"
    );
    assert_reaped(root.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn dropped_option_caller_settles_child_and_shutdown_promptly() {
    let root = tempfile::tempdir().unwrap();
    let bridge = generous_bridge();
    {
        let opening = bridge.open_session(stalled_option_config(root.path()));
        tokio::pin!(opening);
        tokio::select! {
            result = &mut opening => panic!("opening ended before option request marker: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(2), async {
                while !root.path().join("option-request").exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }) => result.unwrap(),
        }
    }
    let start = std::time::Instant::now();
    bridge.shutdown().await.unwrap();
    let elapsed = start.elapsed();
    assert_reaped(root.path());
    assert!(
        elapsed < Duration::from_secs(2),
        "dropped option caller wedged shutdown: {elapsed:?}"
    );
}
