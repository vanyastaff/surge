//! v0.2 M1 — end-to-end live budget enforcement.
//!
//! Drives a one-agent graph through the full engine run loop with a mock
//! bridge that reports a large provider usage observation before completing the
//! stage. With a tiny token budget and the default `Abort` policy, the engine
//! must emit `BudgetExceeded` and abort the run at the stage boundary — proving
//! the `enforce_budget` hook fires against the freshly-folded cumulative cost.

#[path = "fixtures/runtime_home.rs"]
mod runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::budget::{BudgetGuard, BudgetLimits, BudgetPolicy};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};
use tokio::sync::{Mutex, broadcast};

/// Mock bridge that, on each `send_message`, reports a fixed large
/// provider usage observation and then the session's declared outcome — so the run
/// accrues real cost in its event log before the stage boundary.
struct BudgetMockBridge {
    tx: broadcast::Sender<BridgeEvent>,
    outcomes: Mutex<HashMap<SessionId, OutcomeKey>>,
    prompt_tokens: u32,
    output_tokens: u32,
    flood_events: bool,
}

impl BudgetMockBridge {
    fn new(prompt_tokens: u32, output_tokens: u32) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            outcomes: Mutex::new(HashMap::new()),
            prompt_tokens,
            output_tokens,
            flood_events: false,
        }
    }
}

#[async_trait]
impl BridgeFacade for BudgetMockBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        true
    }
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        let session = SessionId::new();
        let outcome = config
            .declared_outcomes
            .first()
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is valid"));
        self.outcomes.lock().await.insert(session, outcome);
        use surge_core::execution_recovery::{
            OpenedSession, ProviderSessionDescriptor, ProviderSessionId, SessionOpenMode,
            SessionOpening, SessionRestoreCapabilities,
        };
        let (descriptor, mode) = match config.opening {
            SessionOpening::Continue(saved) => (saved, SessionOpenMode::Resume),
            SessionOpening::New => (
                ProviderSessionDescriptor::new(
                    ProviderSessionId::new(format!("budget-fixture-{session}")).unwrap(),
                    config.invocation,
                    config.runtime,
                    surge_core::ContentHash::compute(format!("{:?}", config.agent_kind).as_bytes()),
                    config.working_dir,
                    SessionRestoreCapabilities {
                        resume: true,
                        load: true,
                    },
                )
                .unwrap(),
                SessionOpenMode::New,
            ),
        };
        let mut opened = OpenedSession::new(session, descriptor, mode).unwrap();
        opened.execution_writer = Some(
            surge_core::execution_recovery::process::ExecutionWriterObservation::new(
                config.writer_id,
                None,
            )
            .unwrap(),
        );
        Ok(opened)
    }

    async fn send_message(
        &self,
        session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        // The provider's stage outcome arrives during its prompt, before the
        // authoritative final-response usage. Both must survive a prompt join.
        let outcome = self
            .outcomes
            .lock()
            .await
            .get(&session)
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is valid"));
        let _ = self.tx.send(BridgeEvent::OutcomeReported {
            session,
            outcome,
            summary: "budget mock outcome".into(),
            artifacts_produced: vec![],

            verification_report: None,
        });
        if self.flood_events {
            for _ in 0..1000 {
                let _ = self.tx.send(BridgeEvent::AgentMessage {
                    session,
                    chunk: "flood".into(),
                    meta: None,
                });
            }
        }
        // Accepted deltas, repeated cumulative evidence and context occupancy
        // must remain distinct in the durable journal and budget projection.
        let cumulative = surge_core::TokenUsageCounters {
            total_tokens: u64::from(self.prompt_tokens) + u64::from(self.output_tokens),
            input_tokens: u64::from(self.prompt_tokens),
            output_tokens: u64::from(self.output_tokens),
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
        };
        let zero = surge_core::TokenUsageCounters {
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
        };
        for event in [
            BridgeEvent::ProviderUsageObserved {
                session,
                usage: surge_core::TokenUsageObservation::Reported {
                    cumulative: cumulative.clone(),
                    delta: cumulative.clone(),
                },
            },
            BridgeEvent::ProviderUsageObserved {
                session,
                usage: surge_core::TokenUsageObservation::Reported {
                    cumulative,
                    delta: zero,
                },
            },
            BridgeEvent::ContextUsageObserved {
                session,
                usage: surge_core::ContextUsage {
                    used: 1_000_000,
                    size: 2_000_000,
                    cost: None,
                },
            },
        ] {
            let _ = self.tx.send(event);
        }
        Ok(())
    }

    async fn session_state(&self, _session: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }

    async fn close_session(&self, _session: SessionId) -> Result<(), CloseSessionError> {
        Ok(())
    }

    async fn reply_to_tool(
        &self,
        _session: SessionId,
        _call_id: String,
        _payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        Ok(())
    }

    async fn reply_to_permission(
        &self,
        _session: SessionId,
        _request_id: String,
        _response: agent_client_protocol::schema::v1::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.tx.subscribe()
    }
}

/// Single agent node `work` → `done` → terminal `end` (success).
fn one_agent_graph() -> Graph {
    let work = NodeKey::try_from("work").unwrap();
    let end = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        work.clone(),
        Node {
            id: work.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "stage completed".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: NodeLimits::default(),
                hooks: vec![],
                custom_fields: Default::default(),
            }),
        },
    );
    nodes.insert(
        end.clone(),
        Node {
            id: end.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    let edges = vec![Edge {
        id: EdgeKey::try_from("e_work_done").unwrap(),
        from: PortRef {
            node: work.clone(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        },
        to: end,
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }];

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "one-agent-budget".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: work,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_aborts_when_token_budget_exceeded() {
    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        // 15_000 tokens reported by the stage, against a 1_000-token budget.
        let bridge = Arc::new(BudgetMockBridge::new(10_000, 5_000)) as Arc<dyn BridgeFacade>;
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        let run_config = EngineRunConfig {
            budget: BudgetGuard {
                limits: BudgetLimits {
                    usd: None,
                    tokens: Some(1_000),
                    warn_threshold_pct: 80,
                },
                policy: BudgetPolicy::Abort,
            },
            ..EngineRunConfig::default()
        };

        let run_id = RunId::new();
        let handle = engine
            .start_run(
                run_id,
                one_agent_graph(),
                dir.path().to_path_buf(),
                run_config,
            )
            .await
            .unwrap();

        let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
            .await
            .expect("run hung > 30s")
            .expect("await_completion");

        match outcome {
            RunOutcome::Aborted { reason } => {
                assert!(
                    reason.contains("budget exceeded"),
                    "abort reason should cite the budget, got: {reason}"
                );
            },
            other => panic!("expected Aborted on budget breach, got {other:?}"),
        }

        drop(engine);

        // The event log must carry a BudgetExceeded record (Tokens dimension) and
        // must NOT reach the terminal `end` node (RunCompleted).
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(last.0 + 1))
            .await
            .unwrap();

        let saw_exceeded = events.iter().any(|ev| {
            matches!(
                &ev.payload.payload,
                EventPayload::BudgetExceeded {
                    dimension: surge_core::budget::BudgetDimension::Tokens,
                    ..
                }
            )
        });
        assert!(
            saw_exceeded,
            "expected a BudgetExceeded(Tokens) event in the log"
        );

        let completed = events
            .iter()
            .any(|ev| matches!(ev.payload.payload, EventPayload::RunCompleted { .. }));
        assert!(!completed, "run must not complete after a budget breach");
        let (input, output) = events
            .iter()
            .filter_map(|event| event.payload.payload.token_delta())
            .fold(
                (0_u64, 0_u64),
                |(input, output), (_, next_input, next_output, _)| {
                    (input + next_input, output + next_output)
                },
            );
        assert_eq!((input, output), (10_000, 5_000));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.payload.payload,
                    EventPayload::ProviderUsageObserved { .. }
                ))
                .count(),
            2
        );
        assert!(events.iter().any(|event| matches!(
            event.payload.payload,
            EventPayload::ContextUsageObserved { .. }
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload.payload, EventPayload::TokensConsumed { .. }))
        );
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_completes_within_budget() {
    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        // 150 tokens reported, well under a 10_000-token budget.
        let bridge = Arc::new(BudgetMockBridge::new(100, 50)) as Arc<dyn BridgeFacade>;
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        let run_config = EngineRunConfig {
            budget: BudgetGuard {
                limits: BudgetLimits {
                    usd: None,
                    tokens: Some(10_000),
                    warn_threshold_pct: 80,
                },
                policy: BudgetPolicy::Abort,
            },
            ..EngineRunConfig::default()
        };

        let run_id = RunId::new();
        let handle = engine
            .start_run(
                run_id,
                one_agent_graph(),
                dir.path().to_path_buf(),
                run_config,
            )
            .await
            .unwrap();

        let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
            .await
            .expect("run hung > 30s")
            .expect("await_completion");

        match outcome {
            RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
            other => panic!("expected Completed within budget, got {other:?}"),
        }
        drop(engine);
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warn_only_records_breach_but_completes() {
    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        // 15_000 tokens reported, against a 1_000-token budget under WarnOnly.
        let bridge = Arc::new(BudgetMockBridge::new(10_000, 5_000)) as Arc<dyn BridgeFacade>;
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        let run_config = EngineRunConfig {
            budget: BudgetGuard {
                limits: BudgetLimits {
                    usd: None,
                    tokens: Some(1_000),
                    warn_threshold_pct: 80,
                },
                policy: BudgetPolicy::WarnOnly,
            },
            ..EngineRunConfig::default()
        };

        let run_id = RunId::new();
        let handle = engine
            .start_run(
                run_id,
                one_agent_graph(),
                dir.path().to_path_buf(),
                run_config,
            )
            .await
            .unwrap();

        let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
            .await
            .expect("run hung > 30s")
            .expect("await_completion");

        // WarnOnly never stops the run — it completes.
        match outcome {
            RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
            other => panic!("expected Completed under WarnOnly, got {other:?}"),
        }
        drop(engine);

        // ...but the breach is still recorded for visibility (no abort).
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(last.0 + 1))
            .await
            .unwrap();
        let saw_exceeded = events
            .iter()
            .any(|ev| matches!(ev.payload.payload, EventPayload::BudgetExceeded { .. }));
        let saw_aborted = events
            .iter()
            .any(|ev| matches!(ev.payload.payload, EventPayload::RunAborted { .. }));
        assert!(saw_exceeded, "WarnOnly must still record the breach");
        assert!(!saw_aborted, "WarnOnly must not abort the run");
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lagged_usage_stream_fails_with_durable_unavailable_evidence() {
    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        let mut bridge = BudgetMockBridge::new(100, 50);
        bridge.flood_events = true;
        let (tx, _) = broadcast::channel(1);
        bridge.tx = tx;
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(
            Arc::new(bridge),
            storage.clone(),
            dispatcher,
            EngineConfig::default(),
        );
        let run = RunId::new();
        let handle = engine
            .start_run(
                run,
                one_agent_graph(),
                dir.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
            .await
            .expect("lag must fail promptly")
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Failed { .. }));
        drop(engine);
        let reader = storage.open_run_reader(run).await.unwrap();
        assert!(reader.cost_summary().await.unwrap().usage_unknown);
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(last.0 + 1))
            .await
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.payload.payload,
            EventPayload::ProviderUsageObserved {
                usage: surge_core::TokenUsageObservation::Unavailable {
                    reason: surge_core::UsageUnavailableReason::Missing,
                    reported: None,
                },
                ..
            }
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload.payload, EventPayload::RunCompleted { .. }))
        );
    }
    dir.close().unwrap();
}
