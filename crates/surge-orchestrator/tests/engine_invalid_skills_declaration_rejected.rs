//! Ticket 18: a broken `custom_fields["skills"]` declaration on an Agent
//! node must reject the run through the real `Engine::start_run` entry
//! point — before any bridge call, before any per-run storage/event log is
//! created, and therefore before a worktree would ever be touched.
//!
//! Before Ticket 18, `surge_core::validation::validate` (which owns the
//! `InvalidSkillsDeclaration` rule, History 15 / R09.1: "a broken skills
//! declaration must fail the run before a worktree is created, not lazily
//! the first time that node's stage runs") had zero non-test callers on the
//! production run-start path — `Engine::start_run` called only the
//! narrower `validate_for_m6`, which never ran this rule. This test goes
//! through `Engine::start_run` itself, not `surge_core::validate` directly,
//! so it cannot pass by accident the way a unit test on the validator alone
//! could.

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
use surge_persistence::runs::Storage;

/// A single-node graph whose Agent node declares `skills` with a malformed
/// entry (missing the required `name` field) — the same shape
/// `AgentConfig::declared_skills_malformed_entry_is_typed_error` (in
/// `surge-core`) uses to prove `AgentConfig::declared_skills()` rejects it.
fn graph_with_broken_skills_declaration() -> Graph {
    let agent_key = NodeKey::try_from("implement").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();

    let mut broken_entry = toml::map::Map::new();
    broken_entry.insert(
        "provider".to_string(),
        toml::Value::String("project_dir".to_string()),
    );
    // `name` is required by `surge_core::skill::SkillRef` and deliberately
    // omitted here.
    let mut custom_fields = BTreeMap::new();
    custom_fields.insert(
        "skills".to_string(),
        toml::Value::Array(vec![toml::Value::Table(broken_entry)]),
    );

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
                profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields,
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
        metadata: GraphMetadata::new("broken-skills-harness", chrono::Utc::now()),
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
async fn broken_skills_declaration_rejects_before_any_run_or_bridge_state_exists() {
    let dir = tempfile::tempdir().unwrap();
    // This run must never progress far enough to write memory — routed at a
    // throwaway store rather than the developer's real `~/.surge/memory.db`
    // as a defensive measure even though `validate_for_m6` runs before
    // `EngineRunConfig.memory_store_path` is ever resolved.
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");

    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        memory_store_path: Some(store_path),
        ..EngineRunConfig::default()
    };

    let result = engine
        .start_run(
            run_id,
            graph_with_broken_skills_declaration(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await;

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!(
            "start_run should reject a broken `skills` declaration but returned Ok — \
             surge_core::validate's InvalidSkillsDeclaration rule did not fire on the \
             production start_run path"
        ),
    };
    match &err {
        EngineError::GraphInvalid(msg) => {
            assert!(
                msg.contains("skills"),
                "error should name the broken skills declaration, got: {msg}"
            );
            assert!(
                msg.contains("implement"),
                "error should name the offending node, got: {msg}"
            );
        },
        other => panic!("expected GraphInvalid, got {other:?}"),
    }

    // The rejection must happen before the run touches anything: no bridge
    // call (no session ever opens), and no per-run storage/event log is
    // created for `run_id` — i.e. before whatever would come next (a
    // worktree) could ever be reached.
    assert!(
        mock.recorded_calls.lock().await.is_empty(),
        "a graph-invalid run must never touch the bridge — the node never started"
    );
    assert!(
        storage.open_run_reader(run_id).await.is_err(),
        "a rejected run must never create per-run storage — proves rejection happens \
         before any run/worktree state exists, not merely before the agent stage runs"
    );
}
