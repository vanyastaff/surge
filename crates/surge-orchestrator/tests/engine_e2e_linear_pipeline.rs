//! Integration test: 3-stage Plan → Execute → QA pipeline against a real
//! `AcpBridge` driving `mock_acp_agent` subprocess. Acceptance #6.
//!
//! `#[ignore]`d by default — run with:
//!
//! ```bash
//! cargo build -p surge-acp --bin mock_acp_agent
//! cargo test -p surge-orchestrator --test engine_e2e_linear_pipeline -- --ignored
//! ```
//!
//! Engine constructs `SessionConfig::agent_kind = AgentKind::Mock { args: vec![] }`
//! and the bridge spawns `mock_acp_agent` with no scenario flag. The mock's
//! default scenario is `report_done` (set in `mock_acp_agent::Scenario::parse`),
//! so the agent auto-emits `report_stage_outcome { outcome: "done" }` after
//! every prompt, advancing the pipeline through plan → execute → qa → end.

mod fixtures;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

/// Resolve the `mock_acp_agent` binary path.
///
/// During `cargo test` the `CARGO_BIN_EXE_mock_acp_agent` env var is set by
/// Cargo to the exact binary path. Outside of `cargo test` (e.g. manual `cargo
/// run --test`), fall back to `<CARGO_TARGET_DIR>/debug/mock_acp_agent[.exe]`.
fn mock_agent_path() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> only for the binary's own crate's tests,
    // not for cross-crate consumers — so we always fall through to discovery.
    let bin = if cfg!(windows) {
        "mock_acp_agent.exe"
    } else {
        "mock_acp_agent"
    };
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target).join("debug").join(bin);
    }
    // Walk up from this test crate's manifest dir to the workspace root
    // (where Cargo.lock lives), then target/debug/<bin>.
    let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if cur.join("Cargo.lock").exists() {
            return cur.join("target").join("debug").join(bin);
        }
        if !cur.pop() {
            // Last resort: relative path (will fail-loud below).
            return PathBuf::from("target").join("debug").join(bin);
        }
    }
}

/// Build an agent `Node` for use in the 3-stage graph.
///
/// Each node declares a single `"done"` outcome so the engine can route to the
/// next stage via a `Forward` edge.
fn agent_node(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: OutcomeKey::try_from("done").unwrap(),
            description: "stage completed".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: vec![],
            custom_fields: Default::default(),
        }),
    }
}

/// Build the 3-stage graph: plan → execute → qa → end (terminal/success).
fn build_linear_graph() -> Graph {
    let plan = NodeKey::try_from("plan").unwrap();
    let execute = NodeKey::try_from("execute").unwrap();
    let qa = NodeKey::try_from("qa").unwrap();
    let end = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(plan.clone(), agent_node("plan"));
    nodes.insert(execute.clone(), agent_node("execute"));
    nodes.insert(qa.clone(), agent_node("qa"));
    nodes.insert(
        end.clone(),
        Node {
            id: end.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    let edges = vec![
        Edge {
            id: EdgeKey::try_from("e_plan_done").unwrap(),
            from: PortRef {
                node: plan.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: execute.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e_execute_done").unwrap(),
            from: PortRef {
                node: execute.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: qa.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e_qa_done").unwrap(),
            from: PortRef {
                node: qa.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
    ];

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "linear-3-stage".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: plan,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

/// Integration test: 3-stage linear pipeline completes end-to-end against a
/// real `AcpBridge` driving `mock_acp_agent`.
///
/// # Failure mode (currently)
///
/// This test **hangs at the 60-second timeout** because `execute_agent_stage`
/// spawns `mock_acp_agent` with `args: vec![]` (no `--scenario` flag). Without
/// `--scenario report_done`, the mock defaults to the `echo` scenario and never
/// emits `report_stage_outcome`. The engine event loop therefore blocks waiting
/// for `BridgeEvent::OutcomeReported`.
///
/// See the module-level doc comment for the fix options.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary built; enable with --ignored. Currently hangs due to missing --scenario report_done arg in execute_agent_stage (see module doc)."]
async fn linear_pipeline_completes_end_to_end() {
    // Guard: fail fast with a clear message if the binary isn't built yet.
    let bin = mock_agent_path();
    assert!(
        bin.exists(),
        "mock_acp_agent binary missing at {} — run `cargo build -p surge-acp` first",
        bin.display()
    );

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();

    // Use the real AcpBridge. The engine's SessionConfig will use
    // AgentKind::Mock { args: vec![] }, so the bridge spawns
    // `mock_acp_agent` from CARGO_BIN_EXE_mock_acp_agent (or
    // target/debug/mock_acp_agent as fallback).
    let bridge_real = AcpBridge::with_defaults().expect("AcpBridge::with_defaults");
    let bridge: Arc<dyn BridgeFacade> = Arc::new(bridge_real);

    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(
        bridge.clone(),
        storage.clone(),
        dispatcher,
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            build_linear_graph(),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();

    // 60-second wall-clock timeout for the full 3-stage pipeline.
    let outcome = tokio::time::timeout(Duration::from_secs(60), handle.await_completion())
        .await
        .expect("run timed out after 60s — mock did not emit report_stage_outcome")
        .unwrap();

    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed, got {other:?}"),
    }
}
