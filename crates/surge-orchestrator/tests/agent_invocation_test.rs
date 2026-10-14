//! Standalone agent invocation (#99), driven through `MockBridge`: scripted
//! agent behaviour in, the interface's outcome or typed failure out, plus the
//! bridge's observed open/send/close calls.

mod fixtures;
#[path = "agent_invocation/settlement_lifecycle.rs"]
mod settlement_lifecycle;
use fixtures::runtime_home as runtime_home_fixture;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use fixtures::mock_bridge::{MockBridge, RecordedCall};
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::id::SessionId;
use surge_core::keys::OutcomeKey;
use surge_orchestrator::agent_invocation::{
    DEFAULT_MAX_CONTRACT_REJECTIONS, InvocationFailure, InvocationOutcome, InvocationRequest,
    invoke,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use tokio_util::sync::CancellationToken;

const DESCRIPTION: &str = include_str!("fixtures/artifacts/valid/description.md");

fn profile_toml(id: &str, agent: &str, extra: &str) -> String {
    format!(
        r#"schema_version = 1

[role]
id = "{id}"
version = "1.0.0"
display_name = "Invoker"
category = "agents"
description = "Standalone invocation fixture."
when_to_use = "Tests."

[runtime]
recommended_model = "fixture"
agent_id = "{agent}"

[sandbox]
mode = "read-only"

[[outcomes]]
id = "done"
description = "Description written."
edge_kind_hint = "forward"
required_artifacts = ["description.md"]

[[outcomes.produced_artifacts]]
path = "description.md"
contract = {{ kind = "description", schema_version = 1 }}

[[outcomes]]
id = "skipped"
description = "Nothing to write."
edge_kind_hint = "forward"

[[bindings.expected]]
name = "topic"
source = {{ source = "any" }}

{extra}

[prompt]
system = "Write the description of {{{{topic}}}}."
"#
    )
}

struct Fixture {
    _profiles_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    registry: ProfileRegistry,
    mock: Arc<MockBridge>,
    bridge: Arc<dyn BridgeFacade>,
    session: SessionId,
    cancel: CancellationToken,
}

impl Fixture {
    fn new() -> Self {
        let profiles_dir = tempfile::tempdir().unwrap();
        for (name, agent, extra) in [
            ("invoker", "mock", ""),
            ("unknown-agent", "no-such-agent", ""),
            ("strict-agent", "strict-provider", ""),
            ("custom-agent", "my-provider", ""),
            (
                "hooked",
                "mock",
                "[[hooks.entries]]\nid = \"check\"\ntrigger = \"on_outcome\"\ncommand = \"true\"\non_failure = \"reject\"\n",
            ),
        ] {
            std::fs::write(
                profiles_dir.path().join(format!("{name}-1.0.toml")),
                profile_toml(name, agent, extra),
            )
            .unwrap();
        }
        let registry = ProfileRegistry::new(DiskProfileSet::scan(profiles_dir.path()).unwrap());
        let mock = Arc::new(MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        Self {
            _profiles_dir: profiles_dir,
            work: tempfile::tempdir().unwrap(),
            registry,
            mock,
            bridge,
            session: SessionId::new(),
            cancel: CancellationToken::new(),
        }
    }

    fn request<'a>(
        &'a self,
        profile: &'a str,
        agents: Option<&'a surge_acp::Registry>,
    ) -> InvocationRequest<'a> {
        InvocationRequest {
            profile,
            bindings: vec![("topic".into(), "the app".into())],
            working_dir: self.work.path(),
            deadline: Duration::from_secs(10),
            cancel: &self.cancel,
            recorder: None,
            profiles: &self.registry,
            agents,
            max_contract_rejections: DEFAULT_MAX_CONTRACT_REJECTIONS,
            seed_agent_settings: true,
        }
    }

    /// Run `request` while `script` plays the agent's side once the
    /// invocation has subscribed and sent its prompt.
    async fn run<F>(
        &self,
        request: InvocationRequest<'_>,
        script: impl FnOnce(SessionId) -> F,
    ) -> Result<InvocationOutcome, InvocationFailure>
    where
        F: std::future::Future<Output = ()>,
    {
        self.mock.pin_next_session_id(self.session).await;
        let agent = async {
            self.wait_for_sends(1).await;
            script(self.session).await;
        };
        let (result, ()) = tokio::join!(invoke(&self.bridge, request), agent);
        result
    }

    async fn wait_for_sends(&self, count: usize) {
        for _ in 0..500 {
            if self.sends().await >= count {
                return;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    async fn sends(&self) -> usize {
        self.mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, RecordedCall::SendMessage { .. }))
            .count()
    }

    async fn closes(&self) -> usize {
        self.mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, RecordedCall::CloseSession(_)))
            .count()
    }

    async fn emit(&self, event: BridgeEvent) {
        self.mock.enqueue_event(event).await;
        self.mock.pump_scripted_events().await;
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.work.path().join(name), content).unwrap();
    }
}

fn reported(session: SessionId, outcome: &str, artifacts: &[&str]) -> BridgeEvent {
    BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::try_from(outcome).unwrap(),
        summary: "did it".into(),
        artifacts_produced: artifacts.iter().map(|path| (*path).to_owned()).collect(),
        verification_report: None,
    }
}

#[tokio::test]
async fn unconfirmed_cleanup_never_accepts_a_successful_body() {
    let fx = &Fixture::new();
    *fx.mock.next_close_error.lock().await = Some(
        surge_acp::bridge::error::CloseSessionError::SessionNotFound {
            session: fx.session,
        },
    );
    let result = fx
        .run(fx.request("invoker@1.0", None), |session| async move {
            fx.emit(reported(session, "skipped", &[])).await;
        })
        .await;
    assert!(
        result.is_err(),
        "unconfirmed cleanup accepted an outcome: {result:?}"
    );
}

#[tokio::test]
async fn unconfirmed_cleanup_makes_a_retryable_body_terminal() {
    let fx = &Fixture::new();
    *fx.mock.next_close_error.lock().await = Some(
        surge_acp::bridge::error::CloseSessionError::SessionNotFound {
            session: fx.session,
        },
    );
    let failure = fx
        .run(fx.request("invoker@1.0", None), |session| async move {
            fx.emit(reported(session, "undeclared", &[])).await;
        })
        .await
        .unwrap_err();
    assert!(
        !failure.is_retryable(),
        "cleanup uncertainty must forbid another writer: {failure:?}"
    );
    let InvocationFailure::Settlement(settlement) = &failure else {
        panic!("{failure:?}")
    };
    assert!(matches!(
        settlement.original(),
        Some(InvocationFailure::UndeclaredOutcome(_))
    ));
    assert!(std::error::Error::source(settlement).is_some());
    assert!(failure.to_string().contains("undeclared outcome"));
    assert_eq!(settlement.failures().len(), 1);
}

#[tokio::test]
async fn confirmed_forced_reap_preserves_the_body_result() {
    for outcome in ["skipped", "undeclared"] {
        let fx = &Fixture::new();
        *fx.mock.next_close_error.lock().await = Some(
            surge_acp::bridge::error::CloseSessionError::GracefulTimedOut {
                session: fx.session,
                killed: true,
            },
        );
        let result = fx
            .run(fx.request("invoker@1.0", None), |session| async move {
                fx.emit(reported(session, outcome, &[])).await;
            })
            .await;
        if outcome == "skipped" {
            assert_eq!(result.unwrap().outcome.as_str(), "skipped");
        } else {
            assert!(matches!(
                result,
                Err(InvocationFailure::UndeclaredOutcome(_))
            ));
        }
        assert_eq!(fx.closes().await, 1);
    }
}

#[tokio::test]
async fn cancellation_retains_its_cause_but_uncertain_cleanup_is_terminal() {
    for unconfirmed in [false, true] {
        let fx = &Fixture::new();
        if unconfirmed {
            *fx.mock.next_close_error.lock().await = Some(
                surge_acp::bridge::error::CloseSessionError::GracefulTimedOut {
                    session: fx.session,
                    killed: false,
                },
            );
        }
        let failure = fx
            .run(fx.request("invoker@1.0", None), |_| async {
                fx.cancel.cancel();
            })
            .await
            .unwrap_err();
        if unconfirmed {
            let InvocationFailure::Settlement(ref cleanup) = failure else {
                panic!("{failure:?}")
            };
            assert_eq!(cleanup.original(), Some(&InvocationFailure::Cancelled));
        } else {
            assert_eq!(failure, InvocationFailure::Cancelled);
        }
        assert!(!failure.is_retryable());
        assert_eq!(fx.closes().await, 1);
    }
}

#[tokio::test]
async fn a_bridge_worker_failure_is_typed_and_terminal() {
    use surge_orchestrator::agent_invocation::{
        InvocationCleanupFailure, SessionSettlementFailureKind,
    };
    let fx = &Fixture::new();
    *fx.mock.next_close_error.lock().await =
        Some(surge_acp::bridge::error::CloseSessionError::Bridge(
            surge_acp::bridge::error::BridgeError::WorkerDead,
        ));
    let failure = fx
        .run(fx.request("invoker@1.0", None), |session| async move {
            fx.emit(reported(session, "skipped", &[])).await;
        })
        .await
        .unwrap_err();
    assert!(!failure.is_retryable());
    let InvocationFailure::Settlement(cleanup) = failure else {
        panic!("{failure:?}")
    };
    assert!(
        matches!(cleanup.failures(), [InvocationCleanupFailure::Session {
        kind: SessionSettlementFailureKind::Bridge, session, ..
    }] if *session == fx.session)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_evidence_is_recorded_only_after_confirmed_cleanup() {
    use surge_core::{RunId, keys::NodeKey, run_event::EventPayload};
    use surge_orchestrator::agent_invocation::InvocationRecorder;
    use surge_persistence::runs::{EventSeq, Storage};

    for unconfirmed in [false, true] {
        let fx = &Fixture::new();
        fx.write("description.md", DESCRIPTION);
        let home = runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let reader = storage.open_run_reader(run).await.unwrap();
        let before = reader.current_seq().await.unwrap();
        let artifacts = surge_persistence::artifacts::ArtifactStore::new(home.path().join("runs"));
        let node = NodeKey::try_from("invocation").unwrap();
        let mut request = fx.request("invoker@1.0", None);
        request.recorder = Some(InvocationRecorder {
            writer: &writer,
            node: &node,
            artifact_store: &artifacts,
            run_id: run,
        });
        if unconfirmed {
            *fx.mock.next_close_error.lock().await = Some(
                surge_acp::bridge::error::CloseSessionError::SessionNotFound {
                    session: fx.session,
                },
            );
        }
        let result = fx
            .run(request, |session| async move {
                fx.emit(reported(session, "done", &["description.md"]))
                    .await;
            })
            .await;
        let after = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(before.as_u64() + 1)..EventSeq(after.as_u64() + 1))
            .await
            .unwrap();
        let accepted_events = events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload.payload,
                    EventPayload::ArtifactProduced { .. } | EventPayload::OutcomeReported { .. }
                )
            })
            .count();
        if unconfirmed {
            assert!(matches!(result, Err(InvocationFailure::Settlement(_))));
            assert_eq!(accepted_events, 0);
        } else {
            assert_eq!(
                result.unwrap().artifact("description.md").unwrap().bytes,
                DESCRIPTION.as_bytes()
            );
            assert_eq!(accepted_events, 2);
        }
        drop((reader, artifacts));
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }
}

#[tokio::test]
async fn a_declared_outcome_returns_its_validated_artifacts_and_closes_once() {
    let fx = &Fixture::new();
    fx.write("description.md", DESCRIPTION);
    let outcome = fx
        .run(fx.request("invoker@1.0", None), |session| async move {
            fx.emit(reported(session, "done", &["description.md"]))
                .await;
        })
        .await
        .unwrap();
    assert_eq!(outcome.outcome.as_str(), "done");
    let artifact = outcome.artifact("description.md").unwrap();
    assert_eq!(artifact.bytes, DESCRIPTION.as_bytes());
    assert_eq!(fx.closes().await, 1);
    assert!(
        fx.mock
            .last_prompt()
            .await
            .unwrap()
            .contains("Write the description of the app.")
    );
}

/// A binding the bridge could not carry (a whole ticket, a roadmap) still
/// reaches the agent through the prompt; the session opens anyway.
#[tokio::test]
async fn an_oversized_binding_reaches_the_prompt_and_the_session_still_opens() {
    let fx = &Fixture::new();
    fx.write("description.md", DESCRIPTION);
    let long = "a long topic ".repeat(40);
    let mut request = fx.request("invoker@1.0", None);
    request.bindings = vec![("topic".into(), long.clone())];
    let outcome = fx
        .run(request, |session| async move {
            fx.emit(reported(session, "done", &["description.md"]))
                .await;
        })
        .await
        .unwrap();
    assert_eq!(outcome.outcome.as_str(), "done");
    assert!(fx.mock.last_prompt().await.unwrap().contains(long.trim()));
}

/// (reported outcome, reported artifacts, file to write, expected failure).
type MistakeCase = (
    &'static str,
    Vec<&'static str>,
    Option<(&'static str, &'static str)>,
    fn(&InvocationFailure) -> bool,
);

#[tokio::test]
async fn agent_mistakes_are_typed_and_retryable() {
    let cases: Vec<MistakeCase> = vec![
        (
            "not_declared",
            vec![],
            None,
            |f| matches!(f, InvocationFailure::UndeclaredOutcome(o) if o == "not_declared"),
        ),
        ("done", vec!["description.md"], None, |f| {
            matches!(f, InvocationFailure::ArtifactContract(_))
        }),
        ("skipped", vec!["absent.md"], None, |f| {
            matches!(f, InvocationFailure::ArtifactMissing(_))
        }),
        (
            "skipped",
            vec!["../escaped.md"],
            Some(("../escaped.md", "x")),
            |f| matches!(f, InvocationFailure::ArtifactOutsideWorkingDir(_)),
        ),
    ];
    for (outcome, artifacts, file, expected) in cases {
        let fx = &Fixture::new();
        if let Some((name, content)) = file {
            std::fs::write(fx.work.path().join(name), content).unwrap();
        }
        let mut request = fx.request("invoker@1.0", None);
        request.max_contract_rejections = 0;
        let failure = fx
            .run(request, |session| {
                let artifacts = artifacts.clone();
                async move {
                    fx.emit(reported(session, outcome, &artifacts)).await;
                }
            })
            .await
            .unwrap_err();
        assert!(expected(&failure), "{failure}");
        assert!(failure.is_retryable(), "{failure}");
        assert_eq!(fx.closes().await, 1, "{failure}");
    }
}

#[tokio::test]
async fn a_contract_rejection_is_corrected_in_the_same_session() {
    let fx = &Fixture::new();
    fx.write("description.md", "not a description");
    let outcome = fx
        .run(fx.request("invoker@1.0", None), |session| async move {
            fx.emit(reported(session, "done", &["description.md"]))
                .await;
            // The rejection's retry turn arrives as a second send.
            fx.wait_for_sends(2).await;
            assert!(
                fx.mock
                    .last_prompt()
                    .await
                    .unwrap()
                    .contains("rejected by validation")
            );
            fx.write("description.md", DESCRIPTION);
            fx.emit(reported(session, "done", &["description.md"]))
                .await;
        })
        .await
        .unwrap();
    assert_eq!(outcome.outcome.as_str(), "done");
    assert_eq!(fx.sends().await, 2);
    assert_eq!(fx.closes().await, 1);
}

#[tokio::test]
async fn session_failures_close_exactly_once() {
    for (event, expected) in [
        (
            SessionEndReason::AgentCrashed {
                exit_code: Some(1),
                stderr_tail: String::new(),
            },
            "crash",
        ),
        (SessionEndReason::Normal, "ended"),
    ] {
        let fx = &Fixture::new();
        let failure = fx
            .run(fx.request("invoker@1.0", None), |session| async move {
                fx.emit(BridgeEvent::SessionEnded {
                    session,
                    reason: event,
                })
                .await;
            })
            .await
            .unwrap_err();
        match expected {
            "crash" => assert!(
                matches!(failure, InvocationFailure::AgentCrashed(_)),
                "{failure}"
            ),
            _ => assert!(
                matches!(failure, InvocationFailure::SessionEnded(_)),
                "{failure}"
            ),
        }
        assert!(failure.is_retryable());
        assert_eq!(fx.closes().await, 1);
    }
}

#[tokio::test]
async fn deadline_and_cancellation_close_the_session() {
    let fx = &Fixture::new();
    let mut request = fx.request("invoker@1.0", None);
    request.deadline = Duration::from_millis(50);
    let failure = fx.run(request, |_| async {}).await.unwrap_err();
    assert!(
        matches!(failure, InvocationFailure::DeadlineElapsed(_)),
        "{failure}"
    );
    assert!(failure.is_retryable());
    assert_eq!(fx.closes().await, 1);

    let fx = &Fixture::new();
    let failure = fx
        .run(fx.request("invoker@1.0", None), |_| async {
            fx.cancel.cancel();
        })
        .await
        .unwrap_err();
    assert_eq!(failure, InvocationFailure::Cancelled);
    assert!(!failure.is_retryable());
    assert_eq!(fx.closes().await, 1);
}

#[tokio::test]
async fn a_lagged_stream_fails_fast_as_events_lost() {
    let fx = &Fixture::new();
    let mut request = fx.request("invoker@1.0", None);
    request.deadline = Duration::from_secs(60);
    let started = std::time::Instant::now();
    let failure = fx
        .run(request, |session| async move {
            // More than the broadcast buffer, without yielding: the waiting
            // receiver lags.
            for _ in 0..400 {
                fx.mock
                    .enqueue_event(BridgeEvent::AgentMessage {
                        session,
                        chunk: "noise".into(),
                        meta: None,
                    })
                    .await;
            }
            fx.mock.pump_scripted_events().await;
        })
        .await
        .unwrap_err();
    assert!(
        matches!(failure, InvocationFailure::EventsLost(_)),
        "{failure}"
    );
    assert!(failure.is_retryable());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "fails fast, not at the deadline"
    );
    assert_eq!(fx.closes().await, 1);
}

#[tokio::test]
async fn a_failed_send_closes_the_opened_session() {
    let fx = &Fixture::new();
    fx.mock
        .fail_next_send_message(
            surge_acp::bridge::SendMessageError::AgentAuthenticationFailed {
                details: "fixture".into(),
            },
        )
        .await;
    fx.mock.pin_next_session_id(fx.session).await;
    let failure = invoke(&fx.bridge, fx.request("invoker@1.0", None))
        .await
        .unwrap_err();
    assert!(matches!(failure, InvocationFailure::Send(_)), "{failure}");
    assert!(!failure.is_retryable());
    assert_eq!(fx.closes().await, 1);
}

#[tokio::test]
async fn configuration_failures_never_open_a_session() {
    let fx = &Fixture::new();
    let failure = invoke(&fx.bridge, fx.request("unknown-agent@1.0", None))
        .await
        .unwrap_err();
    let InvocationFailure::ProviderUnknown { agent_id, known } = &failure else {
        panic!("{failure}");
    };
    assert_eq!(agent_id, "no-such-agent");
    assert!(known.contains("claude"), "known ids are listed: {known}");

    let strict = registry_with(
        "strict-provider",
        BTreeMap::from([(
            "PROVIDER_TOKEN".to_string(),
            surge_core::config::AgentEnvValue::Inject {
                from: "SURGE_TEST_AGENT_INVOCATION_UNSET_TOKEN".into(),
                default: None,
                required: true,
            },
        )]),
    );
    let failure = invoke(&fx.bridge, fx.request("strict-agent@1.0", Some(&strict)))
        .await
        .unwrap_err();
    let InvocationFailure::ProviderEnvironment { reason, .. } = &failure else {
        panic!("{failure}");
    };
    assert!(
        reason.contains("SURGE_TEST_AGENT_INVOCATION_UNSET_TOKEN"),
        "{reason}"
    );

    let failure = invoke(&fx.bridge, fx.request("hooked@1.0", None))
        .await
        .unwrap_err();
    assert!(
        matches!(
            failure,
            InvocationFailure::Unsupported { what: "hooks", .. }
        ),
        "{failure}"
    );
    let failure = invoke(&fx.bridge, fx.request("missing@1.0", None))
        .await
        .unwrap_err();
    assert!(
        matches!(failure, InvocationFailure::ProfileUnresolved { .. }),
        "{failure}"
    );
    assert!(
        fx.mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .all(|call| !matches!(
                call,
                RecordedCall::OpenSession | RecordedCall::CloseSession(_)
            )),
        "configuration failures are detected before any session"
    );
}

#[tokio::test]
async fn a_custom_registry_entry_is_launched_by_its_shape() {
    let fx = &Fixture::new();
    let custom = registry_with(
        "my-provider",
        BTreeMap::from([(
            "PROVIDER_BASE_URL".to_string(),
            surge_core::config::AgentEnvValue::Literal("https://example.invalid".into()),
        )]),
    );
    let outcome = fx
        .run(
            fx.request("custom-agent@1.0", Some(&custom)),
            |session| async move {
                fx.emit(reported(session, "skipped", &[])).await;
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome.outcome.as_str(), "skipped");
    assert_eq!(fx.closes().await, 1);
}

/// Settings files are seeded only where the request allows it: never into
/// a caller-owned checkout.
#[tokio::test]
async fn agent_settings_are_seeded_only_when_the_request_allows_it() {
    for seed in [false, true] {
        let fx = &Fixture::new();
        let custom = registry_with_settings(
            "my-provider",
            BTreeMap::new(),
            vec![surge_core::config::AgentSettingsFile::new(
                ".agent/settings.json",
                "{}",
            )],
        );
        let mut request = fx.request("custom-agent@1.0", Some(&custom));
        request.seed_agent_settings = seed;
        fx.run(request, |session| async move {
            fx.emit(reported(session, "skipped", &[])).await;
        })
        .await
        .unwrap();
        assert_eq!(
            fx.work.path().join(".agent/settings.json").exists(),
            seed,
            "seed_agent_settings = {seed}"
        );
    }
}

fn registry_with(
    id: &str,
    env: BTreeMap<String, surge_core::config::AgentEnvValue>,
) -> surge_acp::Registry {
    registry_with_settings(id, env, Vec::new())
}

fn registry_with_settings(
    id: &str,
    env: BTreeMap<String, surge_core::config::AgentEnvValue>,
    settings_files: Vec<surge_core::config::AgentSettingsFile>,
) -> surge_acp::Registry {
    let mut agents = HashMap::new();
    agents.insert(
        id.to_string(),
        surge_core::config::AgentConfig {
            command: "my-agent".into(),
            args: vec!["--acp".into()],
            transport: surge_core::config::Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env,
            capacity_route: None,
            settings_files,
        },
    );
    surge_acp::Registry::from_config(agents)
}

#[test]
fn fixture_profiles_parse() {
    let profile: surge_core::Profile =
        toml::from_str(&profile_toml("invoker", "mock", "")).unwrap();
    assert_eq!(profile.outcomes.len(), 2);
}

/// The `mock_acp_agent` binary, found from the workspace's `target/debug`.
fn mock_agent_path() -> std::path::PathBuf {
    let bin = if cfg!(windows) {
        "mock_acp_agent.exe"
    } else {
        "mock_acp_agent"
    };
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return std::path::PathBuf::from(target).join("debug").join(bin);
    }
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !dir.join("Cargo.lock").exists() && dir.pop() {}
    dir.join("target").join("debug").join(bin)
}

/// #127 live finding: a real agent reports its outcome by calling
/// `report_stage_outcome` on the stage MCP server. A standalone invocation
/// must give it that server, or the agent can only run into the deadline.
/// Real `AcpBridge` + `mock_acp_agent`, which calls the tool through the
/// stage MCP when the session offers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_agent_reports_its_outcome_through_the_stage_mcp_server() {
    let fx = Fixture::new();
    let mut agents = HashMap::new();
    agents.insert(
        "my-provider".to_string(),
        surge_core::config::AgentConfig {
            command: mock_agent_path().display().to_string(),
            args: vec!["--scenario".into(), "report_outcome=skipped".into()],
            transport: surge_core::config::Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: BTreeMap::new(),
            capacity_route: None,
            settings_files: vec![],
        },
    );
    let registry = surge_acp::Registry::from_config(agents);
    let bridge: Arc<dyn BridgeFacade> =
        Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap());
    let mut request = fx.request("custom-agent@1.0", Some(&registry));
    request.deadline = Duration::from_secs(30);
    let outcome = invoke(&bridge, request).await.unwrap();
    assert_eq!(outcome.outcome.as_str(), "skipped");
}
