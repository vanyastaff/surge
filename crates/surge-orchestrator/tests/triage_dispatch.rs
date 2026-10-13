//! Unit tests for `triage::dispatch_triage` against `MockBridge`.

#[path = "fixtures/mod.rs"]
mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;

use chrono::Utc;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, SessionId};
use surge_intake::types::{Priority, TaskDetails, TaskId, TriageDecision};
use surge_orchestrator::triage::{TriageInput, TriageOptions, dispatch_triage};
use tempfile::TempDir;

fn task_details(id: &str) -> TaskDetails {
    TaskDetails {
        task_id: TaskId::try_new(id).unwrap(),
        source_id: "mock:t".into(),
        title: "Fix parser panic".into(),
        description: "Stack overflow on nested JSON".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: format!("https://x/{id}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    }
}

/// The profile registry triage resolves through: the bundled profiles,
/// optionally with a disk override of `triage-author` naming `agent`.
fn profiles(agent: Option<&str>) -> Arc<surge_orchestrator::profile_loader::ProfileRegistry> {
    use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
    let Some(agent) = agent else {
        return Arc::new(ProfileRegistry::new(DiskProfileSet::empty()));
    };
    let dir = tempfile::tempdir().unwrap().keep();
    let bundled = include_str!("../../surge-core/bundled/profiles/triage-author-1.0.toml");
    std::fs::write(
        dir.join("triage-author-1.0.toml"),
        bundled.replace(
            "agent_id = \"claude-code\"",
            &format!("agent_id = \"{agent}\""),
        ),
    )
    .unwrap();
    Arc::new(ProfileRegistry::new(DiskProfileSet::scan(&dir).unwrap()))
}

fn input() -> TriageInput {
    TriageInput {
        task: task_details("mock:t#1"),
        candidates: vec![],
        active_runs: vec![],
    }
}

/// Reusable drive task: sleep, find the scratch sub-dir, write the
/// supplied JSON (and optional summary), enqueue an OutcomeReported
/// matching `outcome_key`, then pump events.
async fn drive_one_attempt(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch_root: std::path::PathBuf,
    session: SessionId,
    outcome_key: &'static str,
    decision_json: &'static str,
    summary_md: &'static str,
) {
    // try_one_attempt calls bridge.subscribe() first thing; waiting for the
    // subscriber both guarantees the broadcast receiver exists before we pump
    // and that the dispatcher has progressed past scratch-dir creation.
    bridge.wait_for_subscribe_count(1).await;
    let scratch = std::fs::read_dir(&scratch_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .expect("dispatcher should have created scratch subdir")
        .path();

    std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
    if !summary_md.is_empty() {
        std::fs::write(scratch.join("inbox_summary.md"), summary_md).unwrap();
    }
    bridge
        .enqueue_event(BridgeEvent::OutcomeReported {
            session,
            outcome: OutcomeKey::from_str(outcome_key).unwrap(),
            summary: format!("agent picked {outcome_key}"),
            artifacts_produced: if summary_md.is_empty() {
                vec!["triage_decision.json".into()]
            } else {
                vec!["triage_decision.json".into(), "inbox_summary.md".into()]
            },

            verification_report: None,
        })
        .await;
    bridge.pump_scripted_events().await;
}

/// Drive multiple sequential attempts. The dispatcher reuses the same
/// scratch dir across retries (per spec), so we don't need to find a
/// new sub-dir each time — just overwrite `triage_decision.json` and
/// emit a fresh `OutcomeReported` per attempt.
async fn drive_n_attempts(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch_root: std::path::PathBuf,
    sessions: Vec<SessionId>,
    outcomes_and_artifacts: Vec<(&'static str, &'static str)>,
) {
    assert_eq!(sessions.len(), outcomes_and_artifacts.len());
    for (i, (outcome_key, decision_json)) in outcomes_and_artifacts.iter().enumerate() {
        // Each attempt re-subscribes (try_one_attempt calls bridge.subscribe()
        // at its start), so wait for the (i+1)-th subscriber before pumping
        // this attempt's outcome. This also guarantees the dispatcher has
        // (re)opened the session and the scratch sub-dir exists.
        bridge.wait_for_subscribe_count(i + 1).await;
        let scratch = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("dispatcher created scratch subdir")
            .path();
        std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
        bridge
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: sessions[i],
                outcome: OutcomeKey::from_str(outcome_key).unwrap(),
                summary: format!("attempt {} → {outcome_key}", i + 1),
                artifacts_produced: vec!["triage_decision.json".into()],

                verification_report: None,
            })
            .await;
        bridge.pump_scripted_events().await;
    }
}

#[tokio::test]
async fn enqueued_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = false;

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "enqueued",
        r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#,
        "## Fix parser panic\n\nStack overflow at depth 16.",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .expect("dispatch_triage should succeed");

    drive.await.unwrap();

    match result {
        TriageDecision::Enqueued { priority, .. } => {
            assert_eq!(priority, Priority::High);
        },
        other => panic!("expected Enqueued, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = false;

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "duplicate",
        r#"{"decision":"duplicate","duplicate_of":"mock:t#42","priority":"high","priority_reasoning":"same code path"}"#,
        "",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Duplicate { of, .. } => {
            assert_eq!(of.as_str(), "mock:t#42");
        },
        other => panic!("expected Duplicate, got {other:?}"),
    }
}

#[tokio::test]
async fn out_of_scope_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = false;

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "out_of_scope",
        r#"{"decision":"out_of_scope","priority":"low","priority_reasoning":"hiring task"}"#,
        "",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    assert!(matches!(result, TriageDecision::OutOfScope { .. }));
}

#[tokio::test]
async fn unclear_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = false;

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "unclear",
        r#"{"decision":"unclear","priority":"medium","question":"What does X mean here?"}"#,
        "",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(question.contains("What does X mean here"));
        },
        other => panic!("expected Unclear, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_json_then_recovers() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session_a = SessionId::new();
    let session_b = SessionId::new();
    bridge.pin_session_ids(vec![session_a, session_b]).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 3;
    opts.keep_scratch_on_failure = false;

    let drive = tokio::spawn(drive_n_attempts(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        vec![session_a, session_b],
        vec![
            ("enqueued", "{ this is not valid json"),
            (
                "enqueued",
                r#"{"decision":"enqueued","priority":"medium","priority_reasoning":"ok","summary":"fixed"}"#,
            ),
        ],
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    assert!(matches!(result, TriageDecision::Enqueued { .. }));
}

#[tokio::test]
async fn exhaust_retries_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s1 = SessionId::new();
    let s2 = SessionId::new();
    let s3 = SessionId::new();
    bridge.pin_session_ids(vec![s1, s2, s3]).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 3;
    opts.keep_scratch_on_failure = true;

    let drive = tokio::spawn(drive_n_attempts(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        vec![s1, s2, s3],
        vec![
            ("enqueued", "{ bad 1"),
            ("enqueued", "{ bad 2"),
            ("enqueued", "{ bad 3"),
        ],
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(
                question.contains("Triage failed after 3 attempts"),
                "expected diagnostic text in question, got: {question}"
            );
        },
        other => panic!("expected Unclear, got {other:?}"),
    }
}

#[tokio::test]
async fn timeout_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s = SessionId::new();
    bridge.pin_session_ids(vec![s]).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_millis(150);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = true;

    // No drive task — bridge never emits OutcomeReported, so the
    // dispatcher hits its tokio::time::timeout.
    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(
                question.contains("timeout"),
                "expected 'timeout' in diagnostic, got: {question}"
            );
        },
        other => panic!("expected Unclear, got {other:?}"),
    }
}

#[tokio::test]
async fn agent_crashed_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s = SessionId::new();
    bridge.pin_session_ids(vec![s]).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = true;

    let drive = {
        let bridge = Arc::clone(&bridge);
        tokio::spawn(async move {
            bridge.wait_for_subscribe_count(1).await;
            bridge
                .enqueue_event(BridgeEvent::SessionEnded {
                    session: s,
                    reason: SessionEndReason::AgentCrashed {
                        exit_code: Some(137),
                        stderr_tail: "killed".into(),
                    },
                })
                .await;
            bridge.pump_scripted_events().await;
        })
    };

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(
                question.contains("agent crashed") || question.contains("AgentCrashed"),
                "expected 'agent crashed' in diagnostic, got: {question}"
            );
        },
        other => panic!("expected Unclear, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unconfigured_agent_short_circuits_to_a_configuration_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(
        tmp.path().to_path_buf(),
        profiles(Some("no-such-agent")),
        None,
    );
    opts.attempt_timeout = Duration::from_secs(5);
    opts.max_attempts = 3;
    opts.keep_scratch_on_failure = false;

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();

    match &result {
        TriageDecision::Unclear { question } => {
            assert!(
                question.contains("not configured") && question.contains("no-such-agent"),
                "expected a configuration hint naming the agent, got: {question}"
            );
        },
        other => panic!("expected Unclear, got {other:?}"),
    }

    // Never retried, and no session was ever opened.
    let calls = bridge.recorded_calls.lock().await;
    assert!(calls.is_empty(), "no bridge calls expected, got {calls:?}");
}

/// Regression (#99): triage ran only on a Claude binary. It now runs on
/// whatever registry entry its profile names.
#[tokio::test]
async fn triage_runs_on_a_non_claude_registry_entry() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;
    let mut agents = std::collections::HashMap::new();
    agents.insert(
        "my-provider".to_string(),
        surge_core::config::AgentConfig {
            command: "my-agent".into(),
            args: vec!["--acp".into()],
            transport: surge_core::config::Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: Default::default(),
            capacity_route: None,
            settings_files: vec![],
        },
    );
    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(
        tmp.path().to_path_buf(),
        profiles(Some("my-provider")),
        Some(Arc::new(surge_acp::Registry::from_config(agents))),
    );
    opts.attempt_timeout = Duration::from_secs(5);
    opts.max_attempts = 1;
    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "out_of_scope",
        r#"{"decision":"out_of_scope","priority_reasoning":"belongs elsewhere"}"#,
        "",
    ));
    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();
    assert!(
        matches!(result, TriageDecision::OutOfScope { .. }),
        "{result:?}"
    );
}

/// Regression (#99): a failed `send_message` used to return before closing
/// the session it had opened.
#[tokio::test]
async fn a_failed_send_closes_the_triage_session() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    bridge
        .fail_next_send_message(
            surge_acp::bridge::SendMessageError::AgentAuthenticationFailed {
                details: "fixture".into(),
            },
        )
        .await;
    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts).await;
    assert!(
        matches!(
            result,
            Err(surge_orchestrator::triage::TriageError::Invocation(_))
        ),
        "{result:?}"
    );
    let closes = bridge
        .recorded_calls
        .lock()
        .await
        .iter()
        .filter(|call| matches!(call, fixtures::mock_bridge::RecordedCall::CloseSession(_)))
        .count();
    assert_eq!(closes, 1);
}

/// #119: the profile's `triage-decision` contract rejects an incomplete
/// decision when the outcome is reported, and the agent corrects it in the
/// same session — one attempt, one session, two prompts.
#[tokio::test]
async fn an_incomplete_decision_is_corrected_in_the_same_session() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let mut opts = TriageOptions::with_scratch_root(tmp.path().to_path_buf(), profiles(None), None);
    opts.attempt_timeout = Duration::from_secs(5);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = false;

    let sends = |bridge: &fixtures::mock_bridge::MockBridge| {
        let calls = bridge.recorded_calls.try_lock().map(|calls| {
            calls
                .iter()
                .filter(|call| {
                    matches!(
                        call,
                        fixtures::mock_bridge::RecordedCall::SendMessage { .. }
                    )
                })
                .count()
        });
        calls.unwrap_or(0)
    };
    let agent = {
        let bridge = Arc::clone(&bridge);
        let scratch_root = tmp.path().to_path_buf();
        tokio::spawn(async move {
            bridge.wait_for_subscribe_count(1).await;
            while sends(&bridge) < 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let scratch = std::fs::read_dir(&scratch_root)
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .unwrap()
                .path();
            let report = |bridge: Arc<fixtures::mock_bridge::MockBridge>| async move {
                bridge
                    .enqueue_event(BridgeEvent::OutcomeReported {
                        session,
                        outcome: OutcomeKey::from_str("duplicate").unwrap(),
                        summary: "duplicate".into(),
                        artifacts_produced: vec!["triage_decision.json".into()],
                        verification_report: None,
                    })
                    .await;
                bridge.pump_scripted_events().await;
            };
            // First report: a duplicate that names nothing.
            std::fs::write(
                scratch.join("triage_decision.json"),
                r#"{"decision":"duplicate"}"#,
            )
            .unwrap();
            report(Arc::clone(&bridge)).await;
            // The contract rejection comes back as a second prompt in the same session.
            while sends(&bridge) < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            std::fs::write(
                scratch.join("triage_decision.json"),
                r#"{"decision":"duplicate","duplicate_of":"mock:t#42"}"#,
            )
            .unwrap();
            report(Arc::clone(&bridge)).await;
        })
    };

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    agent.await.unwrap();

    match result {
        TriageDecision::Duplicate { of, .. } => assert_eq!(of.as_str(), "mock:t#42"),
        other => panic!("expected the corrected Duplicate, got {other:?}"),
    }
    let calls = bridge.recorded_calls.lock().await;
    let opened = calls
        .iter()
        .filter(|call| matches!(call, fixtures::mock_bridge::RecordedCall::OpenSession))
        .count();
    assert_eq!(opened, 1, "the correction happens in the same session");
    let prompts = calls
        .iter()
        .filter(|call| {
            matches!(
                call,
                fixtures::mock_bridge::RecordedCall::SendMessage { .. }
            )
        })
        .count();
    assert_eq!(prompts, 2);
    drop(calls);
    let retry = bridge.last_prompt().await.unwrap();
    assert!(
        retry.contains("duplicate_of"),
        "the retry names the missing field: {retry}"
    );
}
