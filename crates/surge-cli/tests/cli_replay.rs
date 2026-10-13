//! `surge engine replay --format json` end-to-end.
//!
//! Replay is a pure fold over the event log, so the run is seeded directly
//! through the persistence layer (no agent runtime), then the real `surge`
//! binary folds it and emits the enriched JSON view.

mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}
use runtime_home_fixture::FixtureHome;

use std::collections::BTreeMap;
use std::path::Path;

use surge_core::approvals::ApprovalPolicy;
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::Storage;

fn minimal_graph() -> Graph {
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
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
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "replay-cli-test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

/// Seed a completed run the way the engine really emits it: the terminal node
/// gets no `StageCompleted`; `RunCompleted { terminal_node }` is the only signal
/// it finished. RunStarted, PipelineMaterialized, RunCompleted (3 events).
async fn seed_completed_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();

    let graph = minimal_graph();
    let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
    let end = NodeKey::try_from("end").unwrap();
    let config = RunConfig {
        bootstrap_edit_loop_cap: None,
        budget: Default::default(),
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![],
    };
    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree.clone(),
                initial_prompt: "seed".into(),
                config,
            }),
            VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash,
            }),
            VersionedEventPayload::new(EventPayload::RunCompleted { terminal_node: end }),
        ])
        .await
        .unwrap();
    writer.close().await.unwrap();
    drop(storage);
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_replay_json_emits_enriched_view() {
    let tmp = FixtureHome::new().unwrap();
    {
        let run = seed_completed_run(tmp.path()).await;

        let assert = assert_cmd::Command::cargo_bin("surge")
            .unwrap()
            .env("SURGE_HOME", tmp.path())
            .args(["engine", "replay", &run.to_string(), "--format", "json"])
            .assert()
            .success();
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout must be JSON: {e}\n{stdout}"));

        assert_eq!(json["events_folded"], 3);
        assert_eq!(json["terminal"], true);
        assert_eq!(json["view"]["terminal"], "completed");

        let nodes = json["view"]["nodes"].as_array().expect("nodes array");
        let end = nodes
            .iter()
            .find(|n| n["node"] == "end")
            .expect("end node present");
        // The terminal node is promoted to `completed` by RunCompleted even though
        // it never received a StageCompleted event.
        assert_eq!(end["status"], "completed");
    }
    tmp.close().unwrap();
}

/// A graph whose single terminal node is `node`.
fn graph_with_node(node: &str) -> Graph {
    let mut graph = minimal_graph();
    let key = NodeKey::try_from(node).unwrap();
    let mut end = graph.nodes.remove(&graph.start).unwrap();
    end.id = key.clone();
    graph.nodes = BTreeMap::from([(key.clone(), end)]);
    graph.start = key;
    graph
}

/// A bootstrap run appends its generated follow-up flow as a second
/// `PipelineMaterialized`; replay at any seq draws the run's own graph (#120).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_at_seq_of_a_bootstrap_run_draws_its_own_graph() {
    let tmp = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, tmp.path(), None).await.unwrap();
        let materialized = |graph: Graph| {
            let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
            VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash,
            })
        };
        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: tmp.path().to_path_buf(),
                    initial_prompt: "bootstrap".into(),
                    config: RunConfig {
                        bootstrap_edit_loop_cap: None,
                        budget: Default::default(),
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: vec![],
                    },
                }),
                materialized(graph_with_node("bootstrap_end")),
                materialized(graph_with_node("generated_flow_end")),
            ])
            .await
            .unwrap();
        writer.close().await.unwrap();
        drop(storage);

        let assert = assert_cmd::Command::cargo_bin("surge")
            .unwrap()
            .env("SURGE_HOME", tmp.path())
            .args([
                "engine",
                "replay",
                &run.to_string(),
                "--seq",
                "3",
                "--format",
                "json",
            ])
            .assert()
            .success();
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout must be JSON: {e}\n{stdout}"));
        let nodes: Vec<&str> = json["view"]["nodes"]
            .as_array()
            .expect("nodes array")
            .iter()
            .filter_map(|node| node["node"].as_str())
            .collect();
        assert_eq!(nodes, ["bootstrap_end"], "{stdout}");
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
        assert!(
            !stderr.contains("could not be rebuilt"),
            "no fallback: {stderr}"
        );
    }
    tmp.close().unwrap();
}
