//! Integration test: `pre_tool_use` rejects a dispatched call; `post_tool_use`
//! warns but does not block. Verifies Task 1.2 wiring against a `MockBridge`
//! and the production `HookExecutor` (real shell).

#![allow(clippy::too_many_lines)]

mod fixtures;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::{BridgeEvent, ToolCallMeta, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::sandbox::SandboxDecision;
use surge_core::agent_config::AgentConfig;
use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
use surge_core::id::SessionId;
use surge_core::keys::{NodeKey, OutcomeKey, ProfileKey};
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload as EngineResultPayload,
};
use surge_persistence::runs::Storage;
use tokio::sync::Mutex;

use fixtures::mock_bridge::{MockBridge, RecordedCall};

/// Records every dispatch attempt so the test can assert
/// `pre_tool_use` rejection actually skipped the dispatcher.
struct RecordingDispatcher {
    calls: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl ToolDispatcher for RecordingDispatcher {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> EngineResultPayload {
        self.calls.lock().await.push(call.tool.clone());
        EngineResultPayload::Ok {
            content: serde_json::json!({"echo": call.tool}),
        }
    }
}

fn agent_cfg_with_hooks(hooks: Vec<Hook>) -> AgentConfig {
    AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks,
        custom_fields: Default::default(),
    }
}

fn shell_hook(id: &str, trigger: HookTrigger, command: &str, on_failure: HookFailureMode) -> Hook {
    Hook {
        id: id.into(),
        trigger,
        matcher: MatcherSpec::default(),
        command: command.into(),
        on_failure,
        timeout_seconds: Some(5),
        inherit: HookInheritance::Extend,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_tool_use_reject_skips_dispatcher_and_replies_error() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    // Pin the session id and queue: ToolCall, then OutcomeReported to end the loop.
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::ToolCall {
        session: session_id,
        call_id: "call-1".into(),
        tool: "echo".into(),
        args_redacted_json: r#"{"text":"hi"}"#.into(),
        sandbox_decision: SandboxDecision::Allow,
        meta: ToolCallMeta {
            mcp_id: None,
            injected: false,
        },
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatched: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(RecordingDispatcher {
        calls: dispatched.clone(),
    });

    // Reject hook: `exit 1` is portable across cmd.exe and POSIX shells.
    let cfg = agent_cfg_with_hooks(vec![shell_hook(
        "deny-echo",
        HookTrigger::PreToolUse,
        "exit 1",
        HookFailureMode::Reject,
    )]);
    let memory = surge_core::run_state::RunMemory::default();
    let node = NodeKey::try_from("agent_1").unwrap();
    let tool_resolutions = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
        declared_outcomes: &[],
        bridge: &bridge,
        writer: &writer,
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
    .expect("agent stage should still complete after pre-hook reject");

    pump.await.unwrap();
    assert_eq!(result.as_ref(), "done");

    // Dispatcher must NOT have been invoked.
    let dispatched = dispatched.lock().await;
    assert!(
        dispatched.is_empty(),
        "pre_tool_use Reject must skip dispatcher; got {dispatched:?}"
    );
    drop(dispatched);

    // Bridge must have received a ToolResultPayload::Error reply for the call.
    let calls = mock.recorded_calls.lock().await;
    let mut saw_error = false;
    for call in calls.iter() {
        if let RecordedCall::ReplyToTool {
            call_id, payload, ..
        } = call
            && call_id == "call-1"
            && matches!(payload, ToolResultPayload::Error { .. })
        {
            saw_error = true;
        }
    }
    assert!(
        saw_error,
        "expected a ToolResultPayload::Error reply for call-1, got {calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_tool_use_warn_does_not_block_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::ToolCall {
        session: session_id,
        call_id: "call-2".into(),
        tool: "echo".into(),
        args_redacted_json: r#"{"text":"hi"}"#.into(),
        sandbox_decision: SandboxDecision::Allow,
        meta: ToolCallMeta {
            mcp_id: None,
            injected: false,
        },
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatched: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(RecordingDispatcher {
        calls: dispatched.clone(),
    });

    let cfg = agent_cfg_with_hooks(vec![shell_hook(
        "noisy-post",
        HookTrigger::PostToolUse,
        "exit 1",
        HookFailureMode::Warn,
    )]);
    let memory = surge_core::run_state::RunMemory::default();
    let node = NodeKey::try_from("agent_1").unwrap();
    let tool_resolutions = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
        declared_outcomes: &[],
        bridge: &bridge,
        writer: &writer,
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
    .expect("agent stage should complete normally");

    pump.await.unwrap();
    assert_eq!(result.as_ref(), "done");

    // Dispatcher MUST have run despite the failing post-hook.
    let dispatched = dispatched.lock().await;
    assert_eq!(dispatched.as_slice(), &["echo".to_owned()]);
}
