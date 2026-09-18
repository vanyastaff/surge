//! The profile registry is **per run**, scoped by
//! `EngineRunConfig::project_layer` — one engine serving two repositories
//! never lets one repository's `.surge/profiles/` reach the other's run.
//!
//! Acceptance (autonomous-task-orchestration spec): "Given a
//! `.surge/profiles/x-1.0.toml` in repo and none in home, then the run
//! resolves `x@1` with `Provenance::Project`; with the same name in home,
//! project wins; two daemons/two repos never see each other's profiles."
//! The registry-level half lives in `profile_loader::registry` unit tests;
//! this file proves the *engine* honours it end to end:
//!
//! 1. A run whose layer carries `x-1.0.toml` starts, and its agent stage
//!    resolves `x@1.0` through the run-scoped registry — the project
//!    profile's `prompt.system` is what the mock bridge receives. That is
//!    the proof `RunTaskParams.profile_registry` is the per-run instance,
//!    not the engine-wide one (which cannot resolve `x` at all).
//! 2. A second run on the same engine, whose layer lacks the profile, is
//!    rejected at `start_run` with `ProfileNotFound` before any bridge or
//!    storage state exists.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::ProjectLayer;
use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError, EngineRunConfig, RunOutcome};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;

const PROJECT_PROMPT: &str = "PROJECT-ONLY prompt from .surge/profiles/x-1.0.toml";

/// Write `<repo>/.surge/profiles/x-1.0.toml` — a profile no home or
/// bundled lane knows, routed to the in-process mock runtime.
fn write_project_profile(repo: &std::path::Path) -> ProjectLayer {
    let layer = ProjectLayer::for_project(repo);
    std::fs::create_dir_all(layer.profiles_dir()).unwrap();
    std::fs::write(
        layer.profiles_dir().join("x-1.0.toml"),
        format!(
            r#"
schema_version = 1

[role]
id = "x"
version = "1.0.0"
display_name = "Project X"
category = "agents"
description = "repo-authored"
when_to_use = "only in this repo"

[runtime]
agent_id = "mock"
recommended_model = "test-model"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "{PROJECT_PROMPT}"
"#
        ),
    )
    .unwrap();
    layer
}

/// One Agent node on `x@1.0` wired to a success terminal on `done`.
fn graph_using_project_profile() -> Graph {
    let agent_key = NodeKey::try_from("implement").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent_key.clone(),
        Node {
            id: agent_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "stage completed".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("x@1.0").unwrap(),
                // No override: the stage must fall back to the resolved
                // profile's `prompt.system`, which only the project lane has.
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields: BTreeMap::new(),
            }),
        },
    );
    nodes.insert(
        end_key.clone(),
        Node {
            id: end_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("project-layer-scoping", chrono::Utc::now()),
        start: agent_key.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("e_done").unwrap(),
            from: PortRef {
                node: agent_key,
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end_key,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_engine_two_repos_each_run_sees_only_its_own_project_profiles() {
    // Two repositories: A carries `.surge/profiles/x-1.0.toml`, B has no
    // `.surge/` at all. Both run on one engine whose registry has an
    // empty home lane — exactly a daemon's shape.
    let repo_a = tempfile::tempdir().unwrap();
    let repo_b = tempfile::tempdir().unwrap();
    let layer_a = write_project_profile(repo_a.path());
    let layer_b = ProjectLayer::for_project(repo_b.path());

    // Worktrees are deliberately *not* the repositories (daemon-managed
    // runs execute in `<worktrees_root>/<run_id>`): the lane must come
    // from the layer on the run config, never from the worktree.
    let worktree_a = tempfile::tempdir().unwrap();
    let worktree_b = tempfile::tempdir().unwrap();
    let storage_dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(worktree_a.path().to_path_buf()))
        as Arc<dyn ToolDispatcher>;
    let engine_wide = Arc::new(ProfileRegistry::new(DiskProfileSet::empty()));
    let engine = Engine::new(
        bridge,
        storage.clone(),
        dispatcher,
        EngineConfig {
            profile_registry: Some(engine_wide.clone()),
            ..EngineConfig::default()
        },
    );

    // Sanity: the engine-wide registry cannot resolve `x` — so anything
    // that does resolve it below went through a run-scoped registry.
    let x_ref = surge_core::profile::keyref::parse_key_ref("x@1.0").unwrap();
    assert!(matches!(
        engine_wide.resolve(&x_ref),
        Err(surge_core::error::SurgeError::ProfileNotFound(_))
    ));

    // ── Run B (no project profile) is rejected before anything exists. ──
    let run_b = RunId::new();
    let rejected = engine
        .start_run(
            run_b,
            graph_using_project_profile(),
            worktree_b.path().to_path_buf(),
            EngineRunConfig {
                project_layer: Some(layer_b),
                ..EngineRunConfig::default()
            },
        )
        .await;
    match rejected {
        Err(EngineError::GraphInvalid(message)) => {
            assert!(message.contains('x'), "should name the profile: {message}");
        },
        Err(other) => panic!("run B must be rejected with GraphInvalid, got {other:?}"),
        Ok(_) => panic!("run B must be rejected: its layer has no x@1.0"),
    }
    assert!(mock.recorded_calls.lock().await.is_empty());
    assert!(storage.open_run_reader(run_b).await.is_err());

    // ── Run A (project profile present) starts and its agent stage sees
    //    the project prompt. ──
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.pump_after_subscribe(1).await;
    });

    let run_a = RunId::new();
    let handle = engine
        .start_run(
            run_a,
            graph_using_project_profile(),
            worktree_a.path().to_path_buf(),
            EngineRunConfig {
                project_layer: Some(layer_a),
                ..EngineRunConfig::default()
            },
        )
        .await
        .expect("run A resolves x@1.0 from its own .surge/profiles");

    let outcome = handle.await_completion().await.unwrap();
    pump.await.unwrap();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed, got {other:?}"),
    }
    let prompt = mock
        .last_prompt()
        .await
        .expect("the agent stage must have sent a prompt");
    assert!(
        prompt.contains(PROJECT_PROMPT),
        "the project profile's prompt.system must reach the agent: {prompt}"
    );

    // The run-scoped registry left the engine-wide one untouched.
    assert!(matches!(
        engine_wide.resolve(&x_ref),
        Err(surge_core::error::SurgeError::ProfileNotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_lane_that_fails_to_load_is_a_typed_start_run_error() {
    let repo = tempfile::tempdir().unwrap();
    let layer = ProjectLayer::for_project(repo.path());
    std::fs::create_dir_all(layer.profiles_dir()).unwrap();
    // Parses as a profile but its prompt template is broken — the one
    // per-file failure the scan does not absorb.
    std::fs::write(
        layer.profiles_dir().join("broken-1.0.toml"),
        r#"
schema_version = 1

[role]
id = "broken"
version = "1.0.0"
display_name = "Broken"
category = "agents"
description = "broken template"
when_to_use = "never"

[runtime]
agent_id = "mock"
recommended_model = "test-model"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "{{unclosed"
"#,
    )
    .unwrap();

    let worktree = tempfile::tempdir().unwrap();
    let storage_dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(worktree.path().to_path_buf()))
        as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(
        bridge,
        storage.clone(),
        dispatcher,
        EngineConfig {
            profile_registry: Some(Arc::new(ProfileRegistry::new(DiskProfileSet::empty()))),
            ..EngineConfig::default()
        },
    );

    let result = engine
        .start_run(
            RunId::new(),
            graph_using_project_profile(),
            worktree.path().to_path_buf(),
            EngineRunConfig {
                project_layer: Some(layer.clone()),
                ..EngineRunConfig::default()
            },
        )
        .await;
    match result {
        Err(EngineError::ProjectLayer { profiles_dir, .. }) => {
            assert_eq!(profiles_dir, layer.profiles_dir());
        },
        Err(other) => panic!("expected EngineError::ProjectLayer, got {other:?}"),
        Ok(_) => panic!("a broken project profile must fail start_run"),
    }
}

/// A crash (no terminal event in the log) followed by `resume_run` keeps
/// the project lane: the layer is re-derived from the worktree, which is
/// what the daemon IPC server and the CLI seed it from at start. Two
/// runtimes stand in for two daemon processes.
#[test]
fn resumed_run_still_resolves_its_project_profile_from_the_worktree() {
    // The repository *is* the worktree here, as it is for the IPC path.
    let repo = tempfile::tempdir().unwrap();
    let layer = write_project_profile(repo.path());
    let storage_dir = tempfile::tempdir().unwrap();
    let run_id = RunId::new();

    // ── Process 1: start, let the agent stage open its session, crash. ──
    let first = tokio::runtime::Runtime::new().unwrap();
    first.block_on(async {
        let storage = Storage::open(storage_dir.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(repo.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(
            bridge,
            storage,
            dispatcher,
            EngineConfig {
                profile_registry: Some(Arc::new(ProfileRegistry::new(DiskProfileSet::empty()))),
                ..EngineConfig::default()
            },
        );
        // Session opens; no outcome is ever delivered, so the run is
        // mid-stage when the runtime is torn down.
        mock.pin_next_session_id(SessionId::new()).await;
        let _handle = engine
            .start_run(
                run_id,
                graph_using_project_profile(),
                repo.path().to_path_buf(),
                EngineRunConfig {
                    project_layer: Some(layer.clone()),
                    ..EngineRunConfig::default()
                },
            )
            .await
            .expect("start_run under the project layer");
        let opened =
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if mock.recorded_calls.lock().await.iter().any(|call| {
                        matches!(call, fixtures::mock_bridge::RecordedCall::OpenSession)
                    }) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;
        assert!(opened.is_ok(), "the agent stage never opened a session");
    });
    first.shutdown_timeout(std::time::Duration::from_secs(5));

    // ── Process 2: resume with only the worktree path. ──
    let second = tokio::runtime::Runtime::new().unwrap();
    second.block_on(async {
        let storage = Storage::open(storage_dir.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(repo.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Engine::new(
            bridge,
            storage,
            dispatcher,
            EngineConfig {
                profile_registry: Some(Arc::new(ProfileRegistry::new(DiskProfileSet::empty()))),
                ..EngineConfig::default()
            },
        );
        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::try_from("done").unwrap(),
            summary: "ok".into(),
            artifacts_produced: vec![],
        })
        .await;
        let mock_for_pump = mock.clone();
        let pump = tokio::spawn(async move {
            mock_for_pump.pump_after_subscribe(1).await;
        });

        let handle = engine
            .resume_run(run_id, repo.path().to_path_buf())
            .await
            .expect("resume_run");
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            handle.await_completion(),
        )
        .await
        .expect("resumed run timed out")
        .unwrap();
        pump.await.unwrap();
        match outcome {
            RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
            other => panic!("expected Completed after resume, got {other:?}"),
        }
        let prompt = mock
            .last_prompt()
            .await
            .expect("the resumed agent stage must have sent a prompt");
        assert!(
            prompt.contains(PROJECT_PROMPT),
            "the project profile must still resolve after resume: {prompt}"
        );
    });
}
