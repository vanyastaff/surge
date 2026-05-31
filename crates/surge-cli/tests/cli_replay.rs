//! `surge engine replay --format json` end-to-end.
//!
//! Replay is a pure fold over the event log, so the run is seeded directly
//! through the persistence layer (no agent runtime), then the real `surge`
//! binary folds it and emits the enriched JSON view.

use std::collections::BTreeMap;
use std::path::Path;

use surge_core::approvals::ApprovalPolicy;
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
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

/// Seed a completed run: RunStarted, PipelineMaterialized, StageEntered(end),
/// StageCompleted(end), RunCompleted (5 events).
async fn seed_completed_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();

    let graph = minimal_graph();
    let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
    let end = NodeKey::try_from("end").unwrap();
    let done = OutcomeKey::try_from("done").unwrap();
    let config = RunConfig {
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
            VersionedEventPayload::new(EventPayload::StageEntered {
                node: end.clone(),
                attempt: 1,
            }),
            VersionedEventPayload::new(EventPayload::StageCompleted {
                node: end.clone(),
                outcome: done,
            }),
            VersionedEventPayload::new(EventPayload::RunCompleted { terminal_node: end }),
        ])
        .await
        .unwrap();
    drop(writer);
    drop(storage);
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_replay_json_emits_enriched_view() {
    let tmp = tempfile::tempdir().unwrap();
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

    assert_eq!(json["events_folded"], 5);
    assert_eq!(json["terminal"], true);
    assert_eq!(json["view"]["terminal"], "completed");

    let nodes = json["view"]["nodes"].as_array().expect("nodes array");
    let end = nodes
        .iter()
        .find(|n| n["node"] == "end")
        .expect("end node present");
    assert_eq!(end["status"], "completed");
    assert_eq!(end["last_outcome"], "done");
}
