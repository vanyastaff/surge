//! Actual agent-stage launch records complete resolved input hashes, not ACP echo limits.
mod fixtures;

use std::{collections::BTreeMap, sync::Arc, time::Duration};
use surge_acp::bridge::{error::SendMessageError, facade::BridgeFacade};
use surge_core::agent_config::{AgentConfig, ArtifactSource, Binding, PromptOverride, TemplateVar};
use surge_core::keys::NodeKey;
use surge_core::run_state::{ArtifactRef, RunMemory};
use surge_core::{ContentHash, EventPayload, RunId};
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use surge_orchestrator::engine::stage::{StageError, StageResult};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::{EventSeq, Storage};

struct UnusedDispatcher;
#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(&self, _: &ToolDispatchContext<'_>, _: &ToolCall) -> ToolResultPayload {
        ToolResultPayload::Unsupported {
            message: "not used by this launch test".into(),
        }
    }
}

fn config() -> AgentConfig {
    toml::from_str("profile = 'mock@1.0'").unwrap()
}
fn binding(name: &str, content: &str) -> Binding {
    Binding {
        target: TemplateVar(name.into()),
        source: ArtifactSource::Static {
            content: content.into(),
        },
        optional: false,
    }
}

async fn launch(
    config: &AgentConfig,
    file: Option<&str>,
    registry: bool,
) -> (StageResult, Vec<EventPayload>, bool) {
    let directory = tempfile::tempdir().unwrap();
    if let Some(content) = file {
        std::fs::write(directory.path().join("context.md"), content).unwrap();
    }
    let storage = Storage::open(directory.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage
        .create_run(run, directory.path(), None)
        .await
        .unwrap();
    let artifacts = surge_persistence::artifacts::ArtifactStore::new(directory.path().join("runs"));
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    // Stop after the real launch boundary without driving a native provider.
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "controlled stop".into(),
    })
    .await;
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let node = NodeKey::try_from("worker").unwrap();
    let mut memory = RunMemory::default();
    memory.artifacts.insert(
        "context".into(),
        ArtifactRef {
            hash: ContentHash::compute(b"artifact catalog metadata is not resolved content"),
            path: "context.md".into(),
            name: "context".into(),
            produced_by: node.clone(),
            produced_at_seq: 1,
        },
    );
    let resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hooks = HookExecutor::new();
    let result = execute_agent_stage(AgentStageParams {
        frames: &[],
        cancel: tokio_util::sync::CancellationToken::new(),
        node: &node,
        steers: Vec::new(),
        agent_config: config,
        bound_skills: &[],
        declared_outcomes: &[],
        bridge: &bridge,
        writer: &writer,
        artifact_store: &artifacts,
        worktree_path: directory.path(),
        tool_dispatcher: &dispatcher,
        run_memory: &memory,
        run_id: run,
        tool_resolutions: &resolutions,
        human_input_timeout: Duration::from_secs(1),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        tool_call_loop_guard: Default::default(),
        output_spill: Default::default(),
        profile_registry: registry.then(|| Arc::new(ProfileRegistry::new(DiskProfileSet::empty()))),
        agent_registry: None,
        hook_executor: &hooks,
        pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
        active_task_id: None,
    })
    .await;
    let events = writer
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload.payload)
        .collect();
    let opened = mock
        .recorded_calls
        .lock()
        .await
        .iter()
        .any(|call| matches!(call, fixtures::mock_bridge::RecordedCall::OpenSession));
    writer.close().await.unwrap();
    (result, events, opened)
}

fn recorded_inputs(events: &[EventPayload]) -> &BTreeMap<String, ContentHash> {
    let recorded: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            EventPayload::StageInputsResolved { node, bindings } => {
                assert_eq!(node.as_str(), "worker");
                Some((index, bindings))
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        recorded.len(),
        1,
        "one resolution record per actual stage attempt"
    );
    let session = events
        .iter()
        .position(|event| matches!(event, EventPayload::SessionOpened { .. }))
        .unwrap();
    assert!(
        recorded[0].0 < session,
        "resolution must be durable before session opens"
    );
    recorded[0].1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_launch_records_full_static_and_file_values_before_session() {
    let mut config = config();
    let large = "λ complete static input ".repeat(100);
    config.bindings = (0..9)
        .map(|index| binding(&format!("input_{index}"), "short"))
        .collect();
    config.bindings.push(binding("large", &large));
    config.bindings.push(Binding {
        target: TemplateVar("file".into()),
        source: ArtifactSource::RunArtifact {
            name: "context".into(),
        },
        optional: false,
    });
    let first_file = "Full file content beyond the bridge echo cap. ".repeat(100);
    let (result, events, opened) = launch(&config, Some(&first_file), false).await;
    assert!(matches!(result, Err(StageError::RateLimited { .. })));
    assert!(opened);
    let inputs = recorded_inputs(&events);
    assert_eq!(inputs.len(), 11, "do not use the eight-entry ACP echo");
    assert_eq!(inputs["large"], ContentHash::compute(large.as_bytes()));
    assert_eq!(inputs["file"], ContentHash::compute(first_file.as_bytes()));
    let changed = format!("{first_file}changed bytes");
    let (_, next, _) = launch(&config, Some(&changed), false).await;
    let next_inputs = recorded_inputs(&next);
    assert_ne!(inputs["file"], next_inputs["file"]);
    assert_eq!(
        next_inputs["file"],
        ContentHash::compute(changed.as_bytes())
    );
    assert_eq!(inputs["large"], next_inputs["large"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_successful_resolution_is_recorded_explicitly() {
    let (result, events, opened) = launch(&config(), None, false).await;
    assert!(matches!(result, Err(StageError::RateLimited { .. })));
    assert!(opened);
    assert!(recorded_inputs(&events).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolution_validation_and_prompt_failures_record_no_inputs_or_session() {
    let mut missing = config();
    missing.bindings.push(Binding {
        target: TemplateVar("file".into()),
        source: ArtifactSource::RunArtifact {
            name: "context".into(),
        },
        optional: false,
    });
    let mut invalid_prompt = config();
    invalid_prompt.prompt_overrides = Some(PromptOverride {
        system: Some("{{#if}}".into()),
        append_system: None,
    });
    let mut required = config();
    required.profile = "implementer@1.0".try_into().unwrap();
    for (config, registry) in [
        (&missing, false),
        (&invalid_prompt, false),
        (&required, true),
    ] {
        let (result, events, opened) = launch(config, None, registry).await;
        assert!(matches!(result, Err(StageError::Internal(_))), "{result:?}");
        assert!(!opened);
        assert!(!events.iter().any(|event| matches!(
            event,
            EventPayload::StageInputsResolved { .. } | EventPayload::SessionOpened { .. }
        )));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_targets_are_rejected_before_recording_or_launch_without_registry() {
    let mut config = config();
    config.bindings = vec![
        binding("duplicate", "first"),
        binding("duplicate", "second"),
    ];
    let (result, events, opened) = launch(&config, None, false).await;
    assert!(
        matches!(result, Err(StageError::Internal(ref error)) if error.contains("duplicate")),
        "{result:?}"
    );
    assert!(!opened);
    assert!(events.is_empty());
}
