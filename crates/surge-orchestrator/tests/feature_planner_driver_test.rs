mod fixtures;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, RunId, SessionId};
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_orchestrator::feature_driver::{
    FeaturePlannerParams, FeaturePlannerResult, run_feature_planner,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::Storage;
use tokio::sync::Mutex;

struct NoopDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for NoopDispatcher {
    async fn dispatch(&self, _ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        ToolResultPayload::Unsupported {
            message: format!("unused: {}", call.tool),
        }
    }
}

const VALID_PATCH: &str = r#"schema_version = 1
id = "rpatch-driver"
rationale = "The requested follow-up belongs on the current roadmap."
status = "drafted"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "add_milestone"

[operations.milestone]
id = "m2"
title = "Driver feature"

[operations.insertion]
kind = "append_to_roadmap"
"#;

const TEST_FEATURE_PLANNER_PROFILE: &str = r#"schema_version = 1

[role]
id = "feature-planner"
version = "1.0.0"
display_name = "Feature Planner Test"
category = "agents"
description = "Test profile without shell hooks."
when_to_use = "Feature planner driver tests."

[runtime]
recommended_model = "test-model"
agent_id = "mock"

[sandbox]
mode = "workspace-write"

[[outcomes]]
id = "patched"
description = "Roadmap patch drafted."
edge_kind_hint = "forward"
required_artifacts = ["roadmap-patch.toml"]

[[outcomes.produced_artifacts]]
path = "roadmap-patch.toml"
contract = { kind = "roadmap-patch", schema_version = 1 }

[[outcomes]]
id = "out_of_scope"
description = "Out of scope."
edge_kind_hint = "escalate"

[prompt]
system = "Request: {{request}}\nRoadmap: {{roadmap}}"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feature_planner_driver_reuses_agent_stage_artifact_validation() {
    let storage_dir = tempfile::tempdir().unwrap();
    let profiles_dir = storage_dir.path().join("profiles");
    std::fs::create_dir_all(&profiles_dir).unwrap();
    std::fs::write(
        profiles_dir.join("feature-planner-1.0.toml"),
        TEST_FEATURE_PLANNER_PROFILE,
    )
    .unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage
        .create_run(run_id, storage_dir.path(), None)
        .await
        .unwrap();
    let artifact_store = ArtifactStore::new(storage_dir.path().join("runs"));
    std::fs::write(storage_dir.path().join("roadmap-patch.toml"), VALID_PATCH).unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("patched").unwrap(),
        summary: "patch drafted".into(),
        artifacts_produced: vec!["roadmap-patch.toml".into()],
    })
    .await;

    let mock_for_pump = mock.clone();
    let calls_for_pump = mock.recorded_calls.clone();
    let pump = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let saw_send = calls_for_pump.lock().await.iter().any(|call| {
                matches!(
                    call,
                    fixtures::mock_bridge::RecordedCall::SendMessage { .. }
                )
            });
            if saw_send || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(NoopDispatcher);
    let tool_resolutions = Arc::new(Mutex::new(HashMap::new()));
    let profile_registry = Arc::new(ProfileRegistry::new(
        DiskProfileSet::scan(&profiles_dir).unwrap(),
    ));
    let hook_executor = HookExecutor::new();

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_feature_planner(FeaturePlannerParams {
            request: "Add driver test".into(),
            roadmap: "## Milestones\n- m1".into(),
            bridge: &bridge,
            writer: &writer,
            artifact_store: &artifact_store,
            worktree_path: storage_dir.path(),
            tool_dispatcher: &dispatcher,
            run_memory: &surge_core::run_state::RunMemory::default(),
            run_id,
            tool_resolutions: &tool_resolutions,
            human_input_timeout: Duration::from_secs(5),
            mcp_registry: None,
            mcp_servers: Vec::new(),
            profile_registry,
            hook_executor: &hook_executor,
        }),
    )
    .await
    .expect("driver should not hang")
    .expect("driver succeeds");

    pump.await.unwrap();

    match result {
        FeaturePlannerResult::Patched { patch, patch_path } => {
            assert_eq!(patch.id.as_str(), "rpatch-driver");
            assert!(patch_path.ends_with("roadmap-patch.toml"));
        },
        other => panic!("expected Patched, got {other:?}"),
    }
}
