//! v0.2 M1 — end-to-end live budget enforcement.
//!
//! Drives a one-agent graph through the full engine run loop with a mock
//! bridge that reports a large `TokenUsage` snapshot before completing the
//! stage. With a tiny token budget and the default `Abort` policy, the engine
//! must emit `BudgetExceeded` and abort the run at the stage boundary — proving
//! the `enforce_budget` hook fires against the freshly-folded cumulative cost.

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
/// `TokenUsage` snapshot and then the session's declared outcome — so the run
/// accrues real cost in its event log before the stage boundary.
struct BudgetMockBridge {
    tx: broadcast::Sender<BridgeEvent>,
    outcomes: Mutex<HashMap<SessionId, OutcomeKey>>,
    prompt_tokens: u32,
    output_tokens: u32,
}

impl BudgetMockBridge {
    fn new(prompt_tokens: u32, output_tokens: u32) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            outcomes: Mutex::new(HashMap::new()),
            prompt_tokens,
            output_tokens,
        }
    }
}

#[async_trait]
impl BridgeFacade for BudgetMockBridge {
    async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        let session = SessionId::new();
        let outcome = config
            .declared_outcomes
            .first()
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is valid"));
        self.outcomes.lock().await.insert(session, outcome);
        Ok(session)
    }

    async fn send_message(
        &self,
        session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        // Report usage first so `TokensConsumed` is appended before the stage
        // completes; the engine folds it into cumulative cost at the boundary.
        let _ = self.tx.send(BridgeEvent::TokenUsage {
            session,
            prompt_tokens: self.prompt_tokens,
            output_tokens: self.output_tokens,
            cache_hits: 0,
            model: "mock-model".into(),
        });
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
        });
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
        _response: agent_client_protocol::RequestPermissionResponse,
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
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    // 15_000 tokens reported by the stage, against a 1_000-token budget.
    let bridge = Arc::new(BudgetMockBridge::new(10_000, 5_000)) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
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
        .start_run(run_id, one_agent_graph(), dir.path().to_path_buf(), run_config)
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
        }
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
    assert!(saw_exceeded, "expected a BudgetExceeded(Tokens) event in the log");

    let completed = events
        .iter()
        .any(|ev| matches!(ev.payload.payload, EventPayload::RunCompleted { .. }));
    assert!(!completed, "run must not complete after a budget breach");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_completes_within_budget() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    // 150 tokens reported, well under a 10_000-token budget.
    let bridge = Arc::new(BudgetMockBridge::new(100, 50)) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
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
        .start_run(run_id, one_agent_graph(), dir.path().to_path_buf(), run_config)
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
