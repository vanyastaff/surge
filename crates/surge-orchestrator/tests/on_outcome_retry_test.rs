//! Integration test: `on_outcome` hooks gate `OutcomeReported`. A rejecting
//! hook drops the agent's outcome attempt and lets it pick another one until
//! `AgentLimits::max_retries` is exhausted, after which the stage fails.

#![allow(clippy::too_many_lines)]

mod fixtures;

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
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("pass").unwrap(),
        summary: "first try".into(),
        artifacts_produced: vec![],
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("fixes_needed").unwrap(),
        summary: "fallback".into(),
        artifacts_produced: vec![],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg(vec![outcome_reject_hook("deny-pass", "pass")], 3);
    let node = NodeKey::try_from("agent_1").unwrap();
    let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
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
        profile_registry: None,
        hook_executor: &hook_executor,
    })
    .await
    .expect("stage should complete with the retry outcome");

    pump.await.unwrap();
    assert_eq!(result.as_str(), "fixes_needed");

    // Drop the writer so the reader can take its lock.
    drop(writer);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_on_outcome_hook_rejects_and_retries() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile_with_on_outcome_hook(profiles_dir.path());
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("pass").unwrap(),
        summary: "profile hook rejects this".into(),
        artifacts_produced: vec![],
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("fixes_needed").unwrap(),
        summary: "fallback".into(),
        artifacts_produced: vec![],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg_with_profile("validator@1.0", vec![], 3);
    let node = NodeKey::try_from("agent_1").unwrap();
    let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
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
        profile_registry: Some(registry),
        hook_executor: &hook_executor,
    })
    .await
    .expect("stage should complete after profile hook rejection retry");

    pump.await.unwrap();
    assert_eq!(result.as_str(), "fixes_needed");

    drop(writer);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_budget_exhausted_emits_stage_failed() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

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
        })
        .await;
    }

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    // max_retries = 1: first rejection allowed, second tips us over budget.
    let cfg = agent_cfg(vec![outcome_reject_hook("deny-pass", "pass")], 1);
    let node = NodeKey::try_from("agent_1").unwrap();
    let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
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
        profile_registry: None,
        hook_executor: &hook_executor,
    })
    .await;

    pump.await.unwrap();

    match result {
        Err(StageError::AgentCrashed(reason)) => {
            assert!(
                reason.contains("rejection budget exhausted"),
                "unexpected reason: {reason}"
            );
        },
        other => panic!("expected AgentCrashed after retries exhausted, got {other:?}"),
    }

    drop(writer);

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap();

    let mut rejection_count = 0;
    let mut saw_stage_failed = false;
    for ev in events {
        match ev.payload.payload {
            EventPayload::OutcomeRejectedByHook { .. } => rejection_count += 1,
            EventPayload::StageFailed {
                reason,
                retry_available,
                ..
            } => {
                assert!(reason.contains("rejection budget exhausted"));
                assert!(!retry_available);
                saw_stage_failed = true;
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
    assert!(
        saw_stage_failed,
        "missing StageFailed event after exhaustion"
    );
}
