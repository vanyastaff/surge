//! Integration test: `on_outcome` hooks gate `OutcomeReported`. A rejecting
//! hook drops the agent's outcome attempt and lets it pick another one until
//! `AgentLimits::max_retries` is exhausted, after which the stage fails.

#![allow(clippy::too_many_lines)]

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
use surge_core::id::SessionId;
use surge_core::keys::{NodeKey, OutcomeKey, ProfileKey};
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::StageError;
use surge_orchestrator::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload as EngineResultPayload,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

use fixtures::mock_bridge::MockBridge;

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> EngineResultPayload {
        EngineResultPayload::Unsupported {
            message: format!("unused: {}", call.tool),
        }
    }
}

fn agent_cfg(hooks: Vec<Hook>, max_retries: u32) -> AgentConfig {
    agent_cfg_with_profile("implementer@1.0", hooks, max_retries)
}

fn agent_cfg_with_profile(profile: &str, hooks: Vec<Hook>, max_retries: u32) -> AgentConfig {
    AgentConfig {
        profile: ProfileKey::try_from(profile).unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: NodeLimits {
            max_retries,
            ..Default::default()
        },
        hooks,
        custom_fields: Default::default(),
    }
}

fn outcome_reject_hook(id: &str, target_outcome: &str) -> Hook {
    Hook {
        id: id.into(),
        trigger: HookTrigger::OnOutcome,
        matcher: MatcherSpec {
            outcome: Some(OutcomeKey::try_from(target_outcome).unwrap()),
            ..Default::default()
        },
        command: "exit 1".into(),
        on_failure: HookFailureMode::Reject,
        timeout_seconds: Some(5),
        inherit: HookInheritance::Extend,
    }
}

fn drop_profile_with_on_outcome_hook(profiles_dir: &std::path::Path) {
    std::fs::create_dir_all(profiles_dir).unwrap();
    std::fs::write(
        profiles_dir.join("validator-1.0.toml"),
        r#"
schema_version = 1

[role]
id = "validator"
version = "1.0.0"
display_name = "Validator"
category = "agents"
description = "Test profile with an on_outcome validator hook"
when_to_use = "Tests"

[runtime]
recommended_model = "test-model"
agent_id = "mock"

[[outcomes]]
id = "pass"
description = "Rejected by profile hook"
edge_kind_hint = "forward"

[[outcomes]]
id = "fixes_needed"
description = "Accepted fallback"
edge_kind_hint = "forward"

[[hooks.entries]]
id = "profile-validator"
trigger = "on_outcome"
matcher = { outcome = "pass" }
command = "exit 1"
on_failure = "reject"
timeout_seconds = 5

[prompt]
system = "test"
"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_outcome_lets_agent_retry_with_different_outcome() {
    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

        let mock = Arc::new(MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();

        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::from_str("pass").unwrap(),
            summary: "first try".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::from_str("fixes_needed").unwrap(),
            summary: "fallback".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;

        let mock_for_pump = mock.clone();
        let pump = tokio::spawn(async move {
            mock_for_pump.pump_after_subscribe(1).await;
        });

        let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
        let memory = surge_core::run_state::RunMemory::default();
        let cfg = agent_cfg(vec![outcome_reject_hook("deny-pass", "pass")], 3);
        let node = NodeKey::try_from("agent_1").unwrap();
        let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let hook_executor = HookExecutor::new();

        let result = execute_agent_stage(AgentStageParams {
            quota_opening: None,
            quota_cycle: None,
            quota_owner: None,
            continuation: None,
            frames: &[],
            cancel: tokio_util::sync::CancellationToken::new(),
            steers: Vec::new(),
            node: &node,
            attempt: 1,
            agent_config: &cfg,
            bound_skills: &[],
            declared_outcomes: &[],
            bridge: &bridge,
            writer: &writer,
            artifact_store: &artifact_store,
            worktree_path: dir.path(),
            tool_dispatcher: &dispatcher,
            run_memory: &memory,
            run_id,
            tool_resolutions: &tool_resolutions,
            human_input_timeout: Duration::from_secs(5),
            mcp_registry: None,
            mcp_servers: Vec::new(),
            tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig::default(),
            output_spill: surge_core::spill_config::OutputSpillConfig::default(),
            profile_registry: None,
            agent_registry: None,
            hook_executor: &hook_executor,
            pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
            active_task_id: None,
        })
        .await
        .expect("stage should complete with the retry outcome");

        pump.await.unwrap();
        assert_eq!(result.as_str(), "fixes_needed");

        // Drop the writer so the reader can take its lock.
        writer.close().await.unwrap();

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(last.0 + 1))
            .await
            .unwrap();

        let mut saw_rejection = false;
        let mut saw_outcome = false;
        for ev in events {
            match ev.payload.payload {
                EventPayload::OutcomeRejectedByHook {
                    outcome, hook_id, ..
                } => {
                    assert_eq!(outcome.as_str(), "pass");
                    assert_eq!(hook_id, "deny-pass");
                    saw_rejection = true;
                },
                EventPayload::OutcomeReported { outcome, .. } => {
                    assert_eq!(outcome.as_str(), "fixes_needed");
                    saw_outcome = true;
                },
                _ => {},
            }
        }
        assert!(saw_rejection, "missing OutcomeRejectedByHook for 'pass'");
        assert!(saw_outcome, "missing OutcomeReported for 'fixes_needed'");
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_on_outcome_hook_rejects_and_retries() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile_with_on_outcome_hook(profiles_dir.path());
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

        let mock = Arc::new(MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();

        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::from_str("pass").unwrap(),
            summary: "profile hook rejects this".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::from_str("fixes_needed").unwrap(),
            summary: "fallback".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;

        let mock_for_pump = mock.clone();
        let pump = tokio::spawn(async move {
            mock_for_pump.pump_after_subscribe(1).await;
        });

        let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
        let memory = surge_core::run_state::RunMemory::default();
        let cfg = agent_cfg_with_profile("validator@1.0", vec![], 3);
        let node = NodeKey::try_from("agent_1").unwrap();
        let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let hook_executor = HookExecutor::new();

        let result = execute_agent_stage(AgentStageParams {
            quota_opening: None,
            quota_cycle: None,
            quota_owner: None,
            continuation: None,
            frames: &[],
            cancel: tokio_util::sync::CancellationToken::new(),
            steers: Vec::new(),
            node: &node,
            attempt: 1,
            agent_config: &cfg,
            bound_skills: &[],
            declared_outcomes: &[],
            bridge: &bridge,
            writer: &writer,
            artifact_store: &artifact_store,
            worktree_path: dir.path(),
            tool_dispatcher: &dispatcher,
            run_memory: &memory,
            run_id,
            tool_resolutions: &tool_resolutions,
            human_input_timeout: Duration::from_secs(5),
            mcp_registry: None,
            mcp_servers: Vec::new(),
            tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig::default(),
            output_spill: surge_core::spill_config::OutputSpillConfig::default(),
            profile_registry: Some(registry),
            agent_registry: None,
            hook_executor: &hook_executor,
            pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
            active_task_id: None,
        })
        .await
        .expect("stage should complete after profile hook rejection retry");

        pump.await.unwrap();
        assert_eq!(result.as_str(), "fixes_needed");

        writer.close().await.unwrap();

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(last.0 + 1))
            .await
            .unwrap();

        let saw_profile_rejection = events.into_iter().any(|ev| {
        matches!(
            ev.payload.payload,
            EventPayload::OutcomeRejectedByHook { ref hook_id, .. } if hook_id == "profile-validator"
        )
    });
        assert!(
            saw_profile_rejection,
            "profile on_outcome hook should reject the first outcome"
        );
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_budget_exhausted_returns_a_typed_error_and_writes_nothing_terminal() {
    let dir = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

        let mock = Arc::new(MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();

        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        for _ in 0..5 {
            mock.enqueue_event(BridgeEvent::OutcomeReported {
                session: session_id,
                outcome: OutcomeKey::from_str("pass").unwrap(),
                summary: "again".into(),
                artifacts_produced: vec![],

                verification_report: None,
            })
            .await;
        }

        let mock_for_pump = mock.clone();
        let pump = tokio::spawn(async move {
            mock_for_pump.pump_after_subscribe(1).await;
        });

        let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
        let memory = surge_core::run_state::RunMemory::default();
        // max_retries = 1: first rejection allowed, second tips us over budget.
        let cfg = agent_cfg(vec![outcome_reject_hook("deny-pass", "pass")], 1);
        let node = NodeKey::try_from("agent_1").unwrap();
        let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let hook_executor = HookExecutor::new();

        let result = execute_agent_stage(AgentStageParams {
            quota_opening: None,
            quota_cycle: None,
            quota_owner: None,
            continuation: None,
            frames: &[],
            cancel: tokio_util::sync::CancellationToken::new(),
            steers: Vec::new(),
            node: &node,
            attempt: 1,
            agent_config: &cfg,
            bound_skills: &[],
            declared_outcomes: &[],
            bridge: &bridge,
            writer: &writer,
            artifact_store: &artifact_store,
            worktree_path: dir.path(),
            tool_dispatcher: &dispatcher,
            run_memory: &memory,
            run_id,
            tool_resolutions: &tool_resolutions,
            human_input_timeout: Duration::from_secs(5),
            mcp_registry: None,
            mcp_servers: Vec::new(),
            tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig::default(),
            output_spill: surge_core::spill_config::OutputSpillConfig::default(),
            profile_registry: None,
            agent_registry: None,
            hook_executor: &hook_executor,
            pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
            active_task_id: None,
        })
        .await;

        pump.await.unwrap();

        match result {
            Err(StageError::OutcomeBudgetExhausted {
                hook_id, rejected, ..
            }) => {
                assert_eq!(hook_id, "deny-pass");
                assert_eq!(rejected, vec![OutcomeKey::from_str("pass").unwrap()]);
            },
            other => panic!("expected OutcomeBudgetExhausted, got {other:?}"),
        }

        writer.close().await.unwrap();

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(last.0 + 1))
            .await
            .unwrap();

        let mut rejection_count = 0;
        for ev in events {
            match ev.payload.payload {
                EventPayload::OutcomeRejectedByHook { .. } => rejection_count += 1,
                EventPayload::StageFailed { .. } => {
                    panic!("only the run task writes StageFailed; the stage returns the error");
                },
                EventPayload::OutcomeReported { .. } => {
                    panic!("OutcomeReported must not be persisted while hooks reject");
                },
                _ => {},
            }
        }
        assert_eq!(
            rejection_count, 2,
            "expected exactly max_retries+1 OutcomeRejectedByHook events"
        );
    }
    dir.close().unwrap();
}

// --- #94: terminal failure of an exhausted outcome-retry budget is owned by
// the run task. These drive a real run through `Engine::start_run`, so the
// whole path between the agent stage and stage-error resolution is covered.

mod exhaustion {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::id::RunId;
    use surge_core::keys::EdgeKey;
    use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};

    /// Liveness guard against a hang, not a latency assertion.
    const RUN_WAIT: Duration = Duration::from_secs(60);

    fn suppress_hook(dir: &Path, outcome: &str) -> Hook {
        let path = dir.join(format!("suppress-{outcome}.json"));
        std::fs::write(
            &path,
            format!(r#"{{"action":"suppress","outcome":"{outcome}"}}"#),
        )
        .unwrap();
        let command = if cfg!(target_os = "windows") {
            let literal = path.display().to_string().replace('\'', "''");
            format!("powershell -NoProfile -Command Get-Content -Raw -LiteralPath '{literal}'")
        } else {
            format!(r#"cat "{}""#, path.display())
        };
        Hook {
            id: "recover".into(),
            trigger: HookTrigger::OnError,
            matcher: MatcherSpec::default(),
            command,
            on_failure: HookFailureMode::Warn,
            timeout_seconds: Some(30),
            inherit: HookInheritance::Extend,
        }
    }

    fn decl(id: &str) -> OutcomeDecl {
        OutcomeDecl {
            id: OutcomeKey::try_from(id).unwrap(),
            description: format!("{id} outcome"),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: Default::default(),
        }
    }

    /// `agent_1` declares `done` and `retry_later`, both routed to `end`; an
    /// `on_outcome` hook rejects every `done`, with `max_retries = 1`.
    fn graph(on_error: Vec<Hook>) -> Graph {
        let agent = NodeKey::try_from("agent_1").unwrap();
        let end = NodeKey::try_from("end").unwrap();
        let mut hooks = vec![outcome_reject_hook("deny-done", "done")];
        hooks.extend(on_error);
        let mut nodes = BTreeMap::new();
        nodes.insert(
            agent.clone(),
            Node {
                id: agent.clone(),
                position: Position::default(),
                declared_outcomes: vec![decl("done"), decl("retry_later")],
                config: NodeConfig::Agent(agent_cfg(hooks, 1)),
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
        let edge = |id: &str, outcome: &str| Edge {
            id: EdgeKey::try_from(id).unwrap(),
            from: PortRef {
                node: agent.clone(),
                outcome: OutcomeKey::try_from(outcome).unwrap(),
            },
            to: end.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        };
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "outcome-budget-exhaustion".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: agent.clone(),
            nodes,
            edges: vec![edge("done_to_end", "done"), edge("later_to_end", "retry_later")],
            subgraphs: BTreeMap::new(),
        }
    }

    struct Exhausted {
        outcome: RunOutcome,
        events: Vec<EventPayload>,
        bridge_closes: usize,
    }

    /// Run `agent_1` until its `done` outcome is rejected past the budget.
    async fn exhaust(on_error: impl FnOnce(&Path) -> Vec<Hook>) -> Exhausted {
        let dir = FixtureHome::new().unwrap();
        let memory_dir = FixtureHome::new().unwrap();
        let result = {
            let storage = Storage::open(dir.path()).await.unwrap();
            let mock = Arc::new(MockBridge::new());
            let bridge: Arc<dyn BridgeFacade> = mock.clone();
            let engine = Engine::new(
                bridge,
                storage.clone(),
                Arc::new(UnusedDispatcher),
                EngineConfig::default(),
            );
            let session = SessionId::new();
            mock.pin_next_session_id(session).await;
            for summary in ["first", "second"] {
                mock.enqueue_event(BridgeEvent::OutcomeReported {
                    session,
                    outcome: OutcomeKey::from_str("done").unwrap(),
                    summary: summary.into(),
                    artifacts_produced: vec![],
                    verification_report: None,
                })
                .await;
            }
            let run_id = RunId::new();
            let handle = engine
                .start_run(
                    run_id,
                    graph(on_error(dir.path())),
                    dir.path().to_path_buf(),
                    EngineRunConfig {
                        memory_store_path: Some(memory_dir.path().join("memory.db")),
                        ..EngineRunConfig::default()
                    },
                )
                .await
                .expect("start_run");
            let pump_mock = mock.clone();
            let pump = tokio::spawn(async move { pump_mock.pump_after_subscribe(1).await });
            let outcome = tokio::time::timeout(RUN_WAIT, handle.await_completion())
                .await
                .expect("run timed out")
                .expect("run handle join");
            pump.await.unwrap();
            drop(engine);
            let bridge_closes = mock
                .recorded_calls
                .lock()
                .await
                .iter()
                .filter(|call| matches!(call, fixtures::mock_bridge::RecordedCall::CloseSession(_)))
                .count();
            let reader = storage.open_run_reader(run_id).await.unwrap();
            let last = reader.current_seq().await.unwrap();
            let events = reader
                .read_events(EventSeq(0)..EventSeq(last.0 + 1))
                .await
                .unwrap()
                .into_iter()
                .map(|event| event.payload.payload)
                .collect();
            Exhausted {
                outcome,
                events,
                bridge_closes,
            }
        };
        memory_dir.close().unwrap();
        dir.close().unwrap();
        result
    }

    fn count(events: &[EventPayload], matches: impl Fn(&EventPayload) -> bool) -> usize {
        events.iter().filter(|event| matches(event)).count()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exhausted_budget_fails_the_stage_once_and_closes_the_session_once() {
        let run = exhaust(|_| Vec::new()).await;
        assert!(
            matches!(run.outcome, RunOutcome::Failed { .. }),
            "got {:?}",
            run.outcome
        );
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::StageFailed { .. })),
            1,
            "one StageFailed per failed stage visit: {:#?}",
            run.events
        );
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::SessionClosed { .. })),
            1
        );
        assert_eq!(run.bridge_closes, 1, "the provider session is closed once");
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::OutcomeRejectedByHook { .. })),
            2
        );
        let reason = run
            .events
            .iter()
            .find_map(|e| match e {
                EventPayload::StageFailed { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            reason.contains("rejection budget exhausted") && reason.contains("deny-done"),
            "the reason names the exhausted budget and the last rejecting hook: {reason}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suppressing_exhaustion_into_another_outcome_routes_without_a_stage_failure() {
        let run = exhaust(|dir| vec![suppress_hook(dir, "retry_later")]).await;
        assert!(
            matches!(run.outcome, RunOutcome::Completed { .. }),
            "got {:?}",
            run.outcome
        );
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::StageFailed { .. })),
            0,
            "a suppressed failure never follows a written StageFailed: {:#?}",
            run.events
        );
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::OutcomeReported { outcome, .. } if outcome.as_str() == "retry_later")),
            1
        );
        assert_eq!(run.bridge_closes, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suppressing_exhaustion_into_a_rejected_outcome_is_refused() {
        let run = exhaust(|dir| vec![suppress_hook(dir, "done")]).await;
        assert!(
            matches!(run.outcome, RunOutcome::Failed { .. }),
            "an on_error hook cannot launder the rejected outcome: {:?}",
            run.outcome
        );
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::StageFailed { .. })),
            1
        );
        assert_eq!(
            count(&run.events, |e| matches!(e, EventPayload::OutcomeReported { .. })),
            0,
            "{:#?}",
            run.events
        );
    }
}
