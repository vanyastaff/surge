//! Integration test: an MCP server a stage explicitly selects
//! (`tool_overrides.mcp_add`) whose catalog cannot be built at session open
//! surfaces as a typed `EscalationRequested`, not only a WARN line, while the
//! stage itself still runs on the remaining tools.
//!
//! Driven through the real engine harness (`Engine::start_run` + `MockBridge`)
//! with an actual child process that never completes the MCP handshake inside
//! its startup deadline.
#![cfg(unix)]

mod fixtures;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, ToolOverride};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EscalationCause;
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

use fixtures::mock_bridge::MockBridge;

const SERVER: &str = "stalled";

/// One Agent node selecting `SERVER` via `mcp_add`, wired to a success
/// terminal on `"done"`.
fn agent_selecting_mcp_graph() -> Graph {
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
                profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: Some(ToolOverride {
                    mcp_add: vec![SERVER.into()],
                    ..ToolOverride::default()
                }),
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields: Default::default(),
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
        metadata: GraphMetadata::new("mcp-catalog-escalation", chrono::Utc::now()),
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

/// A child that starts but never answers `initialize`: it misses the
/// explicit 100 ms startup deadline while its `call_timeout` stays generous.
fn stalled_server() -> McpServerRef {
    McpServerRef::new(
        SERVER.into(),
        McpTransportConfig::stdio(
            PathBuf::from("/usr/bin/python3"),
            vec!["-c".into(), "import time; time.sleep(5)".into()],
            HashMap::new(),
        ),
        None,
        Duration::from_secs(60),
        false,
    )
    .with_startup_timeout(Some(Duration::from_millis(100)))
    // Allowed by the canonical spawn policy regardless of the run's mode.
    .with_sandbox(Some(SandboxMode::WorkspaceWrite))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_server_catalog_failure_escalates_and_stage_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
        verification_report: None,
    })
    .await;
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.pump_after_subscribe(1).await;
    });

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());
    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        mcp_servers: vec![stalled_server()],
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(
            run_id,
            agent_selecting_mcp_graph(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await
        .expect("start_run");
    let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
        .await
        .expect("run terminates")
        .unwrap();
    pump.await.unwrap();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("stage must still run without the server's tools, got {other:?}"),
    }

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let escalations = surge_persistence::runs::read_escalations(&reader)
        .await
        .unwrap();
    let catalog: Vec<_> = escalations
        .iter()
        .filter(|e| e.cause == EscalationCause::McpSelectedCatalogUnavailable)
        .collect();
    assert_eq!(
        catalog.len(),
        1,
        "exactly one typed catalog escalation, got {escalations:?}"
    );
    assert_eq!(
        catalog[0].node.as_ref().map(|n| n.as_str()),
        Some("implement")
    );
    assert!(
        catalog[0].reason.contains(&format!("'{SERVER}'"))
            && catalog[0]
                .reason
                .contains("did not complete MCP startup within 100ms"),
        "reason must name the server and the startup deadline: {}",
        catalog[0].reason
    );
}
