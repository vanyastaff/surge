//! M0 (Task 12 §3): `StageError::RateLimited` must actually reach the stage
//! boundary from a bridge-level `SendMessageError::RateLimited` — proved by
//! a test that scripts the bridge to fail that way, not assumed from reading
//! the `match` arm in `engine::stage::agent::execute_agent_stage`.

mod fixtures;

use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::error::SendMessageError;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::AgentConfig;
use surge_core::keys::{NodeKey, ProfileKey};
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::StageError;
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

fn agent_cfg() -> AgentConfig {
    agent_cfg_with_profile("implementer@1.0")
}

fn agent_cfg_with_profile(profile: &str) -> AgentConfig {
    AgentConfig {
        profile: ProfileKey::try_from(profile).unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks: vec![],
        custom_fields: Default::default(),
    }
}

/// Writes a minimal disk profile whose `[runtime] agent_id = "claude-code"`
/// — the field's own literal serde default, and the most common real value
/// — is itself only a registry *alias* for the canonical entry
/// `"claude-acp"` (`surge_acp::registry::REGISTRY_ID_ALIASES`). Used to prove
/// `StageError::RateLimited.account` normalizes through the same registry
/// the engine already uses for `AgentKind` derivation, rather than carrying
/// whatever raw spelling a profile happens to use.
fn drop_profile_with_claude_code_runtime(profiles_dir: &std::path::Path) {
    std::fs::create_dir_all(profiles_dir).unwrap();
    std::fs::write(
        profiles_dir.join("rate-limited-role-1.0.toml"),
        r#"
schema_version = 1

[role]
id = "rate-limited-role"
version = "1.0.0"
display_name = "Rate Limited Role"
category = "agents"
description = "Test profile targeting the claude-code runtime alias"
when_to_use = "Tests"

[runtime]
recommended_model = "test-model"
agent_id = "claude-code"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "test"
"#,
    )
    .unwrap();
}

/// A bridge-level `SendMessageError::RateLimited` from `send_message` must
/// surface as `StageError::RateLimited` at the stage boundary, carrying the
/// same `retry_after` — not the pre-M0 generic `StageError::Bridge(String)`
/// that stringified everything via `Display`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_maps_bridge_rate_limit_to_stage_rate_limited() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: Some(Duration::from_secs(30)),
        details: "429 Too Many Requests: Retry-After: 30".into(),
    })
    .await;

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg();
    let node = NodeKey::try_from("plan_1").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();
    let result = execute_agent_stage(AgentStageParams {
        steers: Vec::new(),
        node: &node,
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
        human_input_timeout: std::time::Duration::from_secs(5),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig::default(),
        output_spill: surge_core::spill_config::OutputSpillConfig::default(),
        // No profile registry wired (legacy path) — the account key must
        // come back `None`, never a fabricated placeholder (Task 12 §1.10).
        profile_registry: None,
        hook_executor: &hook_executor,
        pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
        active_task_id: None,
    })
    .await;

    let Err(StageError::RateLimited {
        account,
        retry_after,
        details,
    }) = result
    else {
        panic!("expected StageError::RateLimited, got: {result:?}");
    };
    assert_eq!(
        account, None,
        "legacy no-profile-registry path must report no account, not a placeholder"
    );
    assert_eq!(retry_after, Some(Duration::from_secs(30)));
    assert!(
        details.contains("429"),
        "details should preserve raw text, got: {details}"
    );
}

/// The branch every real entry point actually uses (`profile_registry:
/// Some(registry)`) must also map the bridge rate limit correctly — and
/// must report a *normalized* `account`, not the profile's raw
/// `runtime.agent_id` spelling. `None`-registry coverage above is the
/// legacy/test-only path; this is the one the M2 capacity ledger will read
/// from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_with_profile_registry_reports_normalized_account() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile_with_claude_code_runtime(profiles_dir.path());
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: Some(Duration::from_secs(30)),
        details: "429 Too Many Requests: Retry-After: 30".into(),
    })
    .await;

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg_with_profile("rate-limited-role@1.0");
    let node = NodeKey::try_from("plan_1").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();
    let result = execute_agent_stage(AgentStageParams {
        steers: Vec::new(),
        node: &node,
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
        human_input_timeout: std::time::Duration::from_secs(5),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig::default(),
        output_spill: surge_core::spill_config::OutputSpillConfig::default(),
        profile_registry: Some(registry),
        hook_executor: &hook_executor,
        pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
        active_task_id: None,
    })
    .await;

    let Err(StageError::RateLimited {
        account,
        retry_after,
        details,
    }) = result
    else {
        panic!("expected StageError::RateLimited, got: {result:?}");
    };
    assert_eq!(
        account.as_deref(),
        Some("claude-acp"),
        "the profile's raw agent_id \"claude-code\" is itself only a \
         registry alias for \"claude-acp\" — account must carry the \
         normalized id, not the raw profile spelling"
    );
    assert_eq!(retry_after, Some(Duration::from_secs(30)));
    assert!(
        details.contains("429"),
        "details should preserve raw text, got: {details}"
    );
}
