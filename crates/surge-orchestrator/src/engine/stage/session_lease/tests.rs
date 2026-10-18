//! Lease interface tests (#101): scripted bridge, temporary run writer, real
//! stage endpoint. Assertions use only what the lease documents: its result,
//! the recorded bridge calls and the event log.

use super::*;

#[path = "../../../../tests/fixtures/mock_bridge.rs"]
#[allow(dead_code)]
pub(crate) mod mock_bridge;

use std::collections::BTreeMap;
use std::time::Duration;

use mock_bridge::{MockBridge, RecordedCall};
use surge_acp::bridge::error::{
    BridgeError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
};
use surge_acp::bridge::event::ToolResultPayload;
use surge_acp::bridge::sandbox::AlwaysAllowSandbox;
use surge_acp::bridge::session::{AgentKind, SessionState};
use surge_acp::client::PermissionPolicy;
use surge_core::id::RunId;
use surge_persistence::runs::{EventSeq, Storage};

/// How the scripted bridge deviates from `MockBridge`.
#[derive(Clone, Copy, Default)]
struct Script {
    /// `open_session` fails.
    open_fails: bool,
    /// The provider reports a writer other than the durable intent.
    foreign_writer: bool,
    /// `send_message` never returns.
    send_hangs: bool,
}

struct ScriptedBridge {
    inner: Arc<MockBridge>,
    script: Script,
}

#[async_trait::async_trait]
impl BridgeFacade for ScriptedBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        self.inner.legacy_stage_event_adapter()
    }

    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        if self.script.open_fails {
            return Err(OpenSessionError::Cancelled);
        }
        let mut opened = self.inner.open_session(config).await?;
        if self.script.foreign_writer {
            opened.execution_writer = Some(
                surge_core::execution_recovery::process::ExecutionWriterObservation::new(
                    surge_core::id::ExecutionWriterId::new(),
                    None,
                )
                .unwrap(),
            );
        }
        Ok(opened)
    }

    async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let sent = self.inner.send_message(session, content).await;
        if self.script.send_hangs {
            std::future::pending::<()>().await;
        }
        sent
    }

    async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError> {
        self.inner.session_state(session).await
    }

    async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
        self.inner.close_session(session).await
    }

    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.inner.reply_to_tool(session, call_id, payload).await
    }

    async fn reply_to_permission(
        &self,
        session: SessionId,
        request_id: String,
        response: agent_client_protocol::schema::v1::RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        self.inner
            .reply_to_permission(session, request_id, response)
            .await
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.inner.subscribe()
    }
}

struct Fixture {
    dir: crate::runtime_home_fixture::FixtureHome,
    storage: Arc<Storage>,
    run: RunId,
    writer: RunWriter,
    mock: Arc<MockBridge>,
    bridge: Arc<dyn BridgeFacade>,
}

impl Fixture {
    async fn new() -> Self {
        Self::scripted(Script::default()).await
    }

    async fn scripted(script: Script) -> Self {
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, dir.path(), None).await.unwrap();
        let mock = Arc::new(MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = Arc::new(ScriptedBridge {
            inner: Arc::clone(&mock),
            script,
        });
        Self {
            dir,
            storage,
            run,
            writer,
            mock,
            bridge,
        }
    }

    fn request(&self, open_failure: OpenFailure) -> LeaseRequest<'_> {
        let node = NodeKey::try_from("implement").unwrap();
        let (endpoint, _calls) = surge_mcp::stage::StageEndpoint::open(
            surge_core::stage_tool::StageToolContext {
                run: self.run,
                node: node.clone(),
                session: SessionId::new(),
                generation: surge_core::id::StageGenerationId::new(),
            },
            Vec::new(),
        )
        .unwrap();
        LeaseRequest {
            bridge: &self.bridge,
            writer: &self.writer,
            endpoint,
            config: SessionConfig {
                effect_fence: None,
                writer_id: surge_core::id::ExecutionWriterId::new(),
                invocation: surge_core::id::StageInvocationId::new(),
                runtime: "fixture".into(),
                opening: surge_core::execution_recovery::SessionOpening::default(),
                config_selections: Vec::new(),
                stage_mcp: None,
                agent_kind: AgentKind::Mock { args: vec![] },
                working_dir: self.dir.path().to_path_buf(),
                system_prompt: "sys".into(),
                declared_outcomes: vec!["done".parse().unwrap()],
                allows_escalation: false,
                tools: vec![],
                sandbox: Box::new(AlwaysAllowSandbox),
                permission_policy: PermissionPolicy::default(),
                bindings: BTreeMap::new(),
                env: BTreeMap::default(),
            },
            opened: SessionOpenedFacts {
                handoff: None,
                node,
                agent: "implementer@1.0".into(),
                agent_id: Some("mock".into()),
            },
            open_failure,
        }
    }

    async fn open(&self) -> OpenedLease<'_> {
        SessionLease::open(self.request(OpenFailure::Bridge))
            .await
            .expect("lease opens")
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

    async fn sends(&self) -> usize {
        self.mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, RecordedCall::SendMessage { .. }))
            .count()
    }

    /// The session lifecycle and usage events, in log order.
    async fn lifecycle(&self) -> Vec<String> {
        let reader = self.storage.open_run_reader(self.run).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(1)..EventSeq(last.as_u64() + 1))
            .await
            .unwrap();
        drop(reader);
        events
            .into_iter()
            .filter_map(|event| match event.payload.payload {
                EventPayload::SessionOpened { .. } => Some("opened".to_owned()),
                EventPayload::SessionClosed { disposition, .. } => {
                    Some(format!("closed {disposition:?}"))
                },
                EventPayload::ProviderUsageObserved { .. } => Some("usage".to_owned()),
                _ => None,
            })
            .collect()
    }

    async fn close(self) {
        let Self {
            dir,
            storage,
            writer,
            bridge,
            mock,
            ..
        } = self;
        writer.close().await.unwrap();
        drop((storage, bridge, mock));
        dir.close().unwrap();
    }
}

fn usage(events: &mut broadcast::Receiver<BridgeEvent>) -> VecDeque<BridgeEvent> {
    // Nothing is read ahead in these tests; the queue starts empty.
    let _ = events;
    VecDeque::new()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_success_closes_once_and_records_a_normal_close() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        mut lease,
        mut events,
        ..
    } = fixture.open().await;
    lease
        .start_turn(MessageContent::Text("work".into()))
        .unwrap();
    lease.join_turn().await.unwrap().unwrap();
    let mut deferred = usage(&mut events);
    let settled = lease
        .settle(
            Ok("done"),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    assert_eq!(settled.unwrap(), "done");
    assert_eq!(fixture.closes().await, 1);
    assert_eq!(fixture.lifecycle().await, ["opened", "closed Normal"]);
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_before_the_first_prompt_is_closed_and_recorded() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        lease, mut events, ..
    } = fixture.open().await;
    let mut deferred = usage(&mut events);
    let settled = lease
        .settle(
            Err::<&str, _>(StageError::RecoveryRequired(
                "quota opening confirmation failed".into(),
            )),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    assert!(
        matches!(&settled, Err(StageError::RecoveryRequired(reason)) if reason == "quota opening confirmation failed"),
        "a clean cleanup returns the body's error unchanged: {settled:?}"
    );
    assert_eq!(fixture.closes().await, 1);
    assert_eq!(fixture.sends().await, 0);
    assert_eq!(fixture.lifecycle().await, ["opened", "closed ForcedClose"]);
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreign_provider_writer_is_closed_without_being_recorded_as_opened() {
    let fixture = Fixture::scripted(Script {
        foreign_writer: true,
        ..Script::default()
    })
    .await;
    let opened = SessionLease::open(fixture.request(OpenFailure::Bridge)).await;
    let Err(StageError::Bridge(reason)) = opened.map(|_| ()) else {
        panic!("a writer identity mismatch must refuse the session");
    };
    assert!(reason.contains("writer identity mismatched"), "{reason}");
    assert!(reason.contains("session cleanup: Ok(())"), "{reason}");
    assert_eq!(fixture.closes().await, 1);
    assert_eq!(fixture.lifecycle().await, Vec::<String>::new());
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_failure_keeps_its_class_and_closes_no_session() {
    for (open_failure, recovery) in [
        (OpenFailure::Bridge, false),
        (OpenFailure::RecoveryRequired, true),
    ] {
        let fixture = Fixture::scripted(Script {
            open_fails: true,
            ..Script::default()
        })
        .await;
        let opened = SessionLease::open(fixture.request(open_failure))
            .await
            .map(|_| ());
        assert_eq!(
            matches!(opened, Err(StageError::RecoveryRequired(_))),
            recovery,
            "{opened:?}"
        );
        assert_eq!(
            matches!(opened, Err(StageError::Bridge(_))),
            !recovery,
            "{opened:?}"
        );
        assert_eq!(fixture.closes().await, 0);
        assert_eq!(fixture.lifecycle().await, Vec::<String>::new());
        fixture.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_forced_reap_keeps_a_success_and_records_a_forced_close() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        lease, mut events, ..
    } = fixture.open().await;
    *fixture.mock.next_close_error.lock().await = Some(CloseSessionError::GracefulTimedOut {
        session: lease.session(),
        killed: true,
    });
    let mut deferred = usage(&mut events);
    let settled = lease
        .settle(
            Ok("done"),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    assert_eq!(settled.unwrap(), "done");
    assert_eq!(fixture.closes().await, 1);
    assert_eq!(fixture.lifecycle().await, ["opened", "closed ForcedClose"]);
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unconfirmed_close_fails_any_result_and_leaves_the_open_unpaired() {
    let bodies: [fn() -> Result<&'static str, StageError>; 2] = [
        || Ok("done"),
        || {
            Err(StageError::RateLimited {
                runtime: None,
                retry_after: None,
                details: "429".into(),
            })
        },
    ];
    for body in bodies {
        for error in [
            CloseSessionError::GracefulTimedOut {
                session: SessionId::new(),
                killed: false,
            },
            CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed),
        ] {
            let fixture = Fixture::new().await;
            let OpenedLease {
                lease, mut events, ..
            } = fixture.open().await;
            *fixture.mock.next_close_error.lock().await = Some(error);
            let mut deferred = usage(&mut events);
            let settled = lease
                .settle(
                    body(),
                    UsageQueue {
                        events: &mut events,
                        deferred: &mut deferred,
                    },
                )
                .await;
            let Err(StageError::Bridge(reason)) = &settled else {
                panic!("an unconfirmed close must fail the stage: {settled:?}");
            };
            assert!(reason.contains("session cleanup failed"), "{reason}");
            assert!(reason.contains("stage endpoint: Ok(())"), "{reason}");
            assert_eq!(fixture.closes().await, 1);
            // No SessionClosed: it would confirm the writer gone.
            assert_eq!(fixture.lifecycle().await, ["opened"]);
            drop(events);
            fixture.close().await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_close_aborts_the_in_flight_turn_instead_of_waiting() {
    let fixture = Fixture::scripted(Script {
        send_hangs: true,
        ..Script::default()
    })
    .await;
    let OpenedLease {
        mut lease,
        mut events,
        ..
    } = fixture.open().await;
    lease
        .start_turn(MessageContent::Text("work".into()))
        .unwrap();
    *fixture.mock.next_close_error.lock().await =
        Some(CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed));
    let mut deferred = usage(&mut events);
    let settled = tokio::time::timeout(
        Duration::from_secs(10),
        lease.settle(
            Ok("done"),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        ),
    )
    .await
    .expect("settling must not wait on a turn whose session failed to close");
    assert!(matches!(settled, Err(StageError::Bridge(_))), "{settled:?}");
    assert_eq!(fixture.closes().await, 1);
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_provider_reported_end_is_the_recorded_disposition() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        mut lease,
        mut events,
        ..
    } = fixture.open().await;
    lease.note_session_ended(&SessionEndReason::AgentCrashed {
        exit_code: Some(1),
        stderr_tail: String::new(),
    });
    let mut deferred = usage(&mut events);
    let settled = lease
        .settle(
            Err::<&str, _>(StageError::AgentCrashed("ended".into())),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    assert!(matches!(settled, Err(StageError::AgentCrashed(_))));
    assert_eq!(fixture.lifecycle().await, ["opened", "closed AgentCrashed"]);
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turns_run_one_at_a_time() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        mut lease,
        mut events,
        ..
    } = fixture.open().await;
    assert!(lease.turn_joined(), "no turn before the first prompt");
    lease
        .start_turn(MessageContent::Text("first".into()))
        .unwrap();
    assert!(!lease.turn_joined());
    assert!(matches!(
        lease.start_turn(MessageContent::Text("overlap".into())),
        Err(StageError::Internal(_))
    ));
    lease.join_turn().await.unwrap().unwrap();
    let first = lease.turn_finished().clone();
    assert!(first.is_cancelled());
    lease
        .start_turn(MessageContent::Text("retry".into()))
        .unwrap();
    assert!(!lease.turn_joined());
    lease.join_turn().await.unwrap().unwrap();
    assert!(lease.turn_finished().is_cancelled());
    assert_eq!(fixture.sends().await, 2);
    let mut deferred = usage(&mut events);
    let _ = lease
        .settle(
            Ok("done"),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_usage_is_persisted_before_the_close_is_recorded() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        lease, mut events, ..
    } = fixture.open().await;
    let session = lease.session();
    fixture
        .mock
        .enqueue_event(BridgeEvent::ProviderUsageObserved {
            session,
            usage: surge_core::TokenUsageObservation::Unavailable {
                reason: surge_core::UsageUnavailableReason::Missing,
                reported: None,
            },
        })
        .await;
    fixture.mock.pump_scripted_events().await;
    let mut deferred = VecDeque::new();
    let settled = lease
        .settle(
            Ok("done"),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    assert_eq!(settled.unwrap(), "done");
    assert_eq!(
        fixture.lifecycle().await,
        ["opened", "usage", "closed Normal"]
    );
    drop(events);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_lease_schedules_a_close_and_records_nothing() {
    let fixture = Fixture::new().await;
    let OpenedLease { lease, events, .. } = fixture.open().await;
    drop(lease);
    drop(events);
    for _ in 0..200 {
        if fixture.closes().await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        fixture.closes().await,
        1,
        "the backstop must close the session"
    );
    assert_eq!(
        fixture.lifecycle().await,
        ["opened"],
        "an unsettled lease leaves SessionOpened unpaired"
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovery_stop_stays_recovery_when_cleanup_also_fails() {
    let fixture = Fixture::new().await;
    let OpenedLease {
        lease, mut events, ..
    } = fixture.open().await;
    *fixture.mock.next_close_error.lock().await =
        Some(CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed));
    let mut deferred = usage(&mut events);
    let settled = lease
        .settle(
            Err::<&str, _>(StageError::RecoveryRequired(
                "RunContinued: disk full".into(),
            )),
            UsageQueue {
                events: &mut events,
                deferred: &mut deferred,
            },
        )
        .await;
    let Err(StageError::RecoveryRequired(reason)) = &settled else {
        panic!("a cleanup failure must not downgrade a recovery stop: {settled:?}");
    };
    assert!(reason.starts_with("RunContinued: disk full"), "{reason}");
    assert!(reason.contains("session cleanup failed"), "{reason}");
    drop(events);
    fixture.close().await;
}
