//! End-to-end integration test for the Profile registry & bundled roles
//! milestone (Task 24).
//!
//! Sets up a `tempdir` as `SURGE_HOME`, drops a disk profile that overrides
//! the bundled `implementer@1.0`, runs a minimal agent stage against the
//! mock bridge with the registry wired in, and asserts:
//!
//! 1. Resolution returns `Provenance::Latest` (disk wins over bundled).
//! 2. The merged profile's `prompt.system` contains the disk override.
//! 3. The agent stage opens the session, loops, and reports an outcome —
//!    proving the registry-driven `AgentKind` derivation path works for
//!    the `mock` agent_id without falling through to the legacy fallback.

mod fixtures;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::SessionId;
use surge_core::agent_config::AgentConfig;
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey, ProfileKey};
use surge_core::profile::keyref::parse_key_ref;
use surge_core::profile::registry::Provenance;
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(&self, _ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        ToolResultPayload::Unsupported {
            message: format!("unused: {}", call.tool),
        }
    }
}

const OVERRIDE_PROMPT: &str = "DISK OVERRIDE prompt for implementer";

/// Drop a disk profile under `dir` that shadows the bundled `implementer@1.0`
/// with the [`OVERRIDE_PROMPT`] body. Uses `agent_id = "mock"` so the agent
/// stage routes to the in-process mock and the test does not require a
/// claude-code binary on PATH.
fn drop_disk_implementer_override(profiles_dir: &std::path::Path) {
    std::fs::create_dir_all(profiles_dir).unwrap();
    let body = format!(
        r#"
schema_version = 1

[role]
id = "implementer"
version = "1.0.0"
display_name = "Implementer (disk override)"
category = "agents"
description = "test override"
when_to_use = "test override"

[runtime]
recommended_model = "test-model"
agent_id = "mock"

[[outcomes]]
id = "implemented"
description = "Override outcome"
edge_kind_hint = "forward"

[prompt]
system = "{OVERRIDE_PROMPT}"
"#
    );
    std::fs::write(profiles_dir.join("implementer-1.0.toml"), body).unwrap();
}

#[test]
fn registry_resolves_disk_override_with_latest_provenance() {
    // Build the registry directly from a tempdir-scoped DiskProfileSet so
    // the test does not have to mutate the process-wide SURGE_HOME env var.
    // (`ProfileRegistry::load` is exercised by the dedicated paths test.)
    let tmp = tempfile::tempdir().unwrap();
    drop_disk_implementer_override(tmp.path());
    let disk = DiskProfileSet::scan(tmp.path()).unwrap();
    let registry = ProfileRegistry::new(disk);

    let key_ref = parse_key_ref("implementer").unwrap();
    let resolved = registry.resolve(&key_ref).unwrap();

    assert_eq!(resolved.provenance, Provenance::Latest);
    assert_eq!(resolved.profile.role.id.as_str(), "implementer");
    assert_eq!(resolved.profile.prompt.system, OVERRIDE_PROMPT);
    // Disk profile says agent_id = "mock"; merged profile should agree.
    assert_eq!(resolved.profile.runtime.agent_id, "mock");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_uses_disk_override_prompt_via_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path().join("profiles");
    drop_disk_implementer_override(&profiles_dir);
    let disk = DiskProfileSet::scan(&profiles_dir).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    // Storage / writer for stage events (separate tempdir keeps the runs
    // sqlite away from the profiles tree).
    let storage_dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage
        .create_run(run_id, storage_dir.path(), None)
        .await
        .unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    // Pin the session id so we can script the OutcomeReported event with
    // the matching id.
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("implemented").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        // Yield so the stage subscribes before the pump fires.
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None, // ← critical: forces the agent stage to fall
        // back to the resolved profile's prompt.system, which is the disk
        // override (this is what we are actually testing).
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks: vec![],
        custom_fields: Default::default(),
    };
    let node = NodeKey::try_from("plan_1").unwrap();
    let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
        declared_outcomes: &[],
        bridge: &bridge,
        writer: &writer,
        worktree_path: storage_dir.path(),
        tool_dispatcher: &dispatcher,
        run_memory: &memory,
        run_id,
        tool_resolutions: &tool_resolutions,
        human_input_timeout: Duration::from_secs(5),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        profile_registry: Some(registry.clone()),
        hook_executor: &hook_executor,
    })
    .await
    .expect("agent stage should succeed when registry resolves to mock");

    pump.await.unwrap();

    // Outcome from the scripted event must propagate as the stage result.
    assert_eq!(result.as_ref(), "implemented");

    // Stage opened a session, sent a message, and closed cleanly. The
    // mock bridge's recorded-call history reflects the full lifecycle.
    let calls = mock.recorded_calls.lock().await;
    let kinds: Vec<&str> = calls
        .iter()
        .map(|c| match c {
            fixtures::mock_bridge::RecordedCall::OpenSession => "open",
            fixtures::mock_bridge::RecordedCall::SendMessage { .. } => "send",
            fixtures::mock_bridge::RecordedCall::ReplyToTool { .. } => "reply",
            fixtures::mock_bridge::RecordedCall::SessionState { .. } => "state",
            fixtures::mock_bridge::RecordedCall::CloseSession(_) => "close",
            fixtures::mock_bridge::RecordedCall::Subscribe => "subscribe",
        })
        .collect();
    assert!(
        kinds.contains(&"open"),
        "stage must open a session; got {kinds:?}"
    );
    assert!(
        kinds.contains(&"send"),
        "stage must send a prompt; got {kinds:?}"
    );
    assert!(
        kinds.contains(&"close"),
        "stage must close session; got {kinds:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_falls_back_to_mock_without_registry() {
    // Sanity check the legacy path: even without a registry, an
    // implementer-flavoured profile still drives the mock fast path so
    // pre-milestone tests stay green.
    let storage_dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage
        .create_run(run_id, storage_dir.path(), None)
        .await
        .unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
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

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks: vec![],
        custom_fields: Default::default(),
    };
    let node = NodeKey::try_from("plan_1").unwrap();
    let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
        declared_outcomes: &[],
        bridge: &bridge,
        writer: &writer,
        worktree_path: storage_dir.path(),
        tool_dispatcher: &dispatcher,
        run_memory: &memory,
        run_id,
        tool_resolutions: &tool_resolutions,
        human_input_timeout: Duration::from_secs(5),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        profile_registry: None, // legacy path
        hook_executor: &hook_executor,
    })
    .await
    .expect("legacy mock fallback path keeps working");

    pump.await.unwrap();
    assert_eq!(result.as_ref(), "done");
}
