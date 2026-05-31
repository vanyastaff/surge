//! `surge engine fork` end-to-end.
//!
//! Fork is pure event copying — no agent runtime is needed — so the parent run
//! is seeded directly through the persistence layer, then the real `surge`
//! binary forks it and we assert the child run is created with the inherited
//! prefix.

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
            name: "fork-cli-test".into(),
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

/// Seed a parent run with two events (`RunStarted`, `PipelineMaterialized`)
/// under `home` (the `SURGE_HOME` the binary will read). Returns its id.
async fn seed_parent(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let parent = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(parent, &worktree, None).await.unwrap();

    let graph = minimal_graph();
    let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
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
        ])
        .await
        .unwrap();
    // Release the writer + registry handles before the binary opens the same
    // SURGE_HOME in a separate process.
    drop(writer);
    drop(storage);
    parent
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_fork_creates_child_from_seeded_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = seed_parent(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["engine", "fork", &parent.to_string(), "--seq", "2"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("events copied: 2"), "stdout was: {stdout}");

    // Parse the child run id from stdout and read it back from storage to prove
    // the run was actually persisted — not just that the banner was printed.
    let child_line = stdout
        .lines()
        .find(|l| l.contains("new run:"))
        .unwrap_or_else(|| panic!("stdout must report the new run id; was: {stdout}"));
    let child_id = child_line
        .split_whitespace()
        .last()
        .expect("new run line has an id");
    let child: RunId = child_id
        .parse()
        .unwrap_or_else(|e| panic!("new run id '{child_id}' must parse: {e}"));

    let storage = Storage::open(tmp.path()).await.unwrap();
    let reader = storage
        .open_run_reader(child)
        .await
        .expect("forked child run must be persisted on disk");
    let seq = reader.current_seq().await.unwrap();
    assert_eq!(
        seq.as_u64(),
        2,
        "child run must hold the 2 inherited events from the parent prefix"
    );
}

#[test]
fn engine_fork_nonexistent_run_fails_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    // Syntactically valid ULID, but there is no such run on disk.
    let fake = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["engine", "fork", fake, "--seq", "1"])
        .assert()
        .failure();
}
