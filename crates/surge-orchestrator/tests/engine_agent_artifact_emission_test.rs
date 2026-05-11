//! Task 30 — emit `EventPayload::ArtifactProduced` for each path declared in
//! `BridgeEvent::OutcomeReported.artifacts_produced` BEFORE the
//! `OutcomeReported` event is appended.
//!
//! The standard fold rule then populates `RunMemory.artifacts` and
//! `RunMemory.artifacts_by_node` deterministically, so downstream stages can
//! bind to artifacts produced by an earlier agent stage without any special-
//! casing in `bindings.rs`.

mod fixtures;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::AgentConfig;
use surge_core::content_hash::ContentHash;
use surge_core::id::SessionId;
use surge_core::keys::{NodeKey, OutcomeKey, ProfileKey};
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::{EventSeq, Storage};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outcome_reported_emits_artifact_produced_for_each_declared_path() {
    let dir = tempfile::tempdir().unwrap();
    let spec_body = b"# Spec\nproduced by the agent.\n";
    let design_body = b"# Design\nalso produced.\n";
    tokio::fs::write(dir.path().join("spec.md"), spec_body)
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("design.md"), design_body)
        .await
        .unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec!["spec.md".into(), "design.md".into()],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg();
    let node = NodeKey::try_from("spec_author").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
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
    .unwrap();
    pump.await.unwrap();
    assert_eq!(result.as_ref(), "done");

    // Read back the event log for the run and assert the two
    // ArtifactProduced events appear in declaration order BEFORE the
    // OutcomeReported event, and that each carries the correct content
    // hash for the file the test wrote into the worktree.
    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");

    let expected_spec_hash = ContentHash::compute(spec_body);
    let expected_design_hash = ContentHash::compute(design_body);

    let mut artifact_indices: Vec<(usize, String)> = Vec::new();
    let mut outcome_index: Option<usize> = None;
    for (i, ev) in events.iter().enumerate() {
        match &ev.payload.payload {
            EventPayload::ArtifactProduced {
                node: producer,
                artifact,
                path,
                name,
            } => {
                assert_eq!(producer, &node, "producer node mismatch");
                match name.as_str() {
                    "spec" => {
                        assert_eq!(artifact, &expected_spec_hash);
                        assert_eq!(
                            path.file_name().unwrap(),
                            expected_spec_hash.to_hex().as_str()
                        );
                        assert_eq!(std::fs::read(path).unwrap(), spec_body);
                    },
                    "design" => {
                        assert_eq!(artifact, &expected_design_hash);
                        assert_eq!(
                            path.file_name().unwrap(),
                            expected_design_hash.to_hex().as_str()
                        );
                        assert_eq!(std::fs::read(path).unwrap(), design_body);
                    },
                    other => panic!("unexpected ArtifactProduced.name: {other}"),
                }
                artifact_indices.push((i, name.clone()));
            },
            EventPayload::OutcomeReported { .. } if outcome_index.is_none() => {
                outcome_index = Some(i);
            },
            _ => {},
        }
    }

    assert_eq!(
        artifact_indices.len(),
        2,
        "expected one ArtifactProduced event per declared path, got {artifact_indices:?}",
    );
    assert_eq!(
        artifact_indices[0].1, "spec",
        "spec.md must be emitted first"
    );
    assert_eq!(
        artifact_indices[1].1, "design",
        "design.md must be emitted second"
    );
    let outcome_idx = outcome_index.expect("OutcomeReported must be persisted");
    assert!(
        artifact_indices.iter().all(|(i, _)| *i < outcome_idx),
        "all ArtifactProduced events must precede OutcomeReported (got artifacts {artifact_indices:?}, outcome at {outcome_idx})",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_artifact_path_logs_warning_and_skips_event() {
    // The agent declares a path that does not exist on disk. The stage must
    // log a warning and continue: no ArtifactProduced event for the missing
    // path, but the existing path still emits one and OutcomeReported is
    // still appended.
    let dir = tempfile::tempdir().unwrap();
    let real_body = b"# Real\nthis one exists.\n";
    tokio::fs::write(dir.path().join("real.md"), real_body)
        .await
        .unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "partial".into(),
        artifacts_produced: vec!["real.md".into(), "ghost.md".into()],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg();
    let node = NodeKey::try_from("spec_author").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
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
    .unwrap();
    pump.await.unwrap();
    assert_eq!(result.as_ref(), "done");

    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");

    let mut artifact_names: Vec<String> = Vec::new();
    let mut saw_outcome = false;
    for ev in &events {
        match &ev.payload.payload {
            EventPayload::ArtifactProduced { name, .. } => artifact_names.push(name.clone()),
            EventPayload::OutcomeReported { .. } => saw_outcome = true,
            _ => {},
        }
    }
    assert_eq!(
        artifact_names,
        vec!["real".to_string()],
        "only the existing path should produce an ArtifactProduced event",
    );
    assert!(
        saw_outcome,
        "OutcomeReported must still be appended even when one declared artifact path is missing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compatible_artifacts_with_the_same_stem_get_distinct_names() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("spec.toml"),
        br#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["first check passes"] },
]
"#,
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir.path().join("spec.md"),
        b"# Spec\n\n## Goal\nShip it.\n\n## Subtasks\n- one\n\n## Acceptance Criteria\n- [ ] passes\n",
    )
    .await
    .unwrap();
    tokio::fs::create_dir_all(dir.path().join("docs"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("docs").join("spec.md"), b"docs spec")
        .await
        .unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "colliding artifacts".into(),
        artifacts_produced: vec!["spec.toml".into(), "spec.md".into(), "docs/spec.md".into()],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg();
    let node = NodeKey::try_from("spec_author").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
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
    .unwrap();
    pump.await.unwrap();

    assert_eq!(result.as_ref(), "done");

    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");
    let artifact_names: Vec<String> = events
        .iter()
        .filter_map(|ev| match &ev.payload.payload {
            EventPayload::ArtifactProduced { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        artifact_names,
        vec![
            "spec_toml".to_string(),
            "spec_md".to_string(),
            "docs_spec_md".to_string()
        ],
        "colliding stems should be disambiguated without dropping valid artifacts",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn artifact_paths_that_escape_worktree_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("worktree");
    tokio::fs::create_dir_all(&worktree).await.unwrap();
    let real_body = b"# Real\ninside worktree.\n";
    let secret_body = b"# Secret\noutside worktree.\n";
    let absolute_secret = dir.path().join("absolute-secret.md");
    tokio::fs::write(worktree.join("real.md"), real_body)
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("secret.md"), secret_body)
        .await
        .unwrap();
    tokio::fs::write(&absolute_secret, secret_body)
        .await
        .unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, &worktree, None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "partial".into(),
        artifacts_produced: vec![
            "real.md".into(),
            "../secret.md".into(),
            absolute_secret.to_string_lossy().into_owned(),
        ],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
    let memory = surge_core::run_state::RunMemory::default();
    let cfg = agent_cfg();
    let node = NodeKey::try_from("spec_author").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hook_executor = HookExecutor::new();
    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &cfg,
        declared_outcomes: &[],
        bridge: &bridge,
        writer: &writer,
        artifact_store: &artifact_store,
        worktree_path: &worktree,
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
    .unwrap();
    pump.await.unwrap();
    assert_eq!(result.as_ref(), "done");

    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");

    let artifact_names: Vec<String> = events
        .iter()
        .filter_map(|ev| match &ev.payload.payload {
            EventPayload::ArtifactProduced { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let saw_outcome = events
        .iter()
        .any(|ev| matches!(ev.payload.payload, EventPayload::OutcomeReported { .. }));

    assert_eq!(
        artifact_names,
        vec!["real".to_string()],
        "only worktree-relative artifact paths should be persisted",
    );
    assert!(
        saw_outcome,
        "OutcomeReported must still be appended when unsafe artifact paths are skipped"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_artifact_contract_rejects_invalid_adr_without_shell_hook() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join("docs").join("adr"))
        .await
        .unwrap();
    tokio::fs::write(
        dir.path()
            .join("docs")
            .join("adr")
            .join("0001-contracts.md"),
        b"# ADR 0001\n\n## Status\nDrafted without the required frontmatter.\n",
    )
    .await
    .unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.path().join("runs"));
    let registry = Arc::new(ProfileRegistry::new(DiskProfileSet::empty()));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("drafted").unwrap(),
        summary: "invalid adr".into(),
        artifacts_produced: vec!["docs/adr/0001-contracts.md".into()],
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("no_decision_needed").unwrap(),
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
    let cfg = agent_cfg_with_profile("architect@1.0");
    let node = NodeKey::try_from("architect").unwrap();
    let tool_resolutions =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
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
    .expect("stage should retry after invalid ADR contract rejection");
    pump.await.unwrap();
    assert_eq!(result.as_ref(), "no_decision_needed");

    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");

    let mut saw_contract_rejection = false;
    let mut outcome_names = Vec::new();
    let artifact_count = events
        .iter()
        .filter(|ev| matches!(ev.payload.payload, EventPayload::ArtifactProduced { .. }))
        .count();
    for ev in events {
        match ev.payload.payload {
            EventPayload::OutcomeRejectedByHook {
                outcome, hook_id, ..
            } => {
                assert_eq!(outcome.as_ref(), "drafted");
                assert_eq!(hook_id, "profile-artifact-contract:adr");
                saw_contract_rejection = true;
            },
            EventPayload::OutcomeReported { outcome, .. } => {
                outcome_names.push(outcome.to_string());
            },
            _ => {},
        }
    }

    assert!(
        saw_contract_rejection,
        "invalid ADR should be rejected by the profile artifact contract"
    );
    assert_eq!(
        outcome_names,
        vec!["no_decision_needed".to_string()],
        "the rejected drafted outcome must not be persisted"
    );
    assert_eq!(
        artifact_count, 0,
        "invalid artifacts must not be emitted before contract validation passes"
    );
}
