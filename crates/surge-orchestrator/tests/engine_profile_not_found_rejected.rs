//! T3 — `ValidationErrorKind::ProfileNotFound` becomes a run-blocking Error
//! at `Engine::start_run` once a `profile_registry` is wired in. Before this
//! delivery (the W5 `SameRuntimeVerification` rule and its
//! `profile_loader::resolver::ProfileRegistry` adapter), `start_run` called
//! `validate_for_m6_with_resolver` whenever `EngineConfig::profile_registry`
//! was `Some`, but that entry point had zero production callers exercising
//! an *unresolvable* profile — so this is the first test proving the
//! rejection actually reaches `start_run`, not merely `surge_core::validate_with_resolver`
//! in isolation.
//!
//! Mirrors `engine_invalid_skills_declaration_rejected.rs`'s harness: same
//! `MockBridge`, same `WorktreeToolDispatcher`, same assertion shape (the
//! run never touches the bridge and never creates per-run storage).

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError, EngineRunConfig};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;

/// A single-node graph whose Agent node references a profile name that
/// cannot resolve under any registry — no such role id exists bundled or
/// on disk.
fn graph_with_unresolvable_profile() -> Graph {
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
                ledger_effect: surge_core::LedgerEffect::default(),
            }],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("definitely-not-a-real-profile@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields: BTreeMap::default(),
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
        metadata: GraphMetadata::new("unresolvable-profile-harness", chrono::Utc::now()),
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
async fn unresolvable_profile_rejects_before_any_run_or_bridge_state_exists() {
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");

    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    // A real `ProfileRegistry` (bundled profiles, no disk overlay) — the
    // production wiring `Engine::start_run` needs before `ProfileNotFound`
    // can fire at all; a `None` registry keeps the legacy resolver-free
    // path where this rule never runs.
    let profile_registry = Arc::new(ProfileRegistry::new(DiskProfileSet::empty()));
    let engine = Engine::new(
        bridge,
        storage.clone(),
        dispatcher,
        EngineConfig {
            profile_registry: Some(profile_registry),
            ..EngineConfig::default()
        },
    );

    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        memory_store_path: Some(store_path),
        ..EngineRunConfig::default()
    };

    let result = engine
        .start_run(
            run_id,
            graph_with_unresolvable_profile(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await;

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!(
            "start_run should reject a graph naming an unresolvable profile but returned \
             Ok — ValidationErrorKind::ProfileNotFound did not reach the production \
             start_run path"
        ),
    };
    match &err {
        EngineError::GraphInvalid(msg) => {
            assert!(
                msg.contains("definitely-not-a-real-profile"),
                "error should name the unresolvable profile, got: {msg}"
            );
        },
        other => panic!("expected GraphInvalid, got {other:?}"),
    }

    assert!(
        mock.recorded_calls.lock().await.is_empty(),
        "a graph-invalid run must never touch the bridge — the node never started"
    );
    assert!(
        storage.open_run_reader(run_id).await.is_err(),
        "a rejected run must never create per-run storage — proves rejection happens \
         before any run/worktree state exists"
    );
}
