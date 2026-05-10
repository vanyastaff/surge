//! Task 20 — follow-up runs inherit bootstrap artifacts.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::approvals::ApprovalPolicy;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, RunConfig, RunEvent, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, Storage};

fn terminal_graph() -> Graph {
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
            name: "bootstrap-parent-artifacts".into(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_run_with_bootstrap_parent_seeds_parent_artifacts() {
    let storage_dir = tempfile::tempdir().unwrap();
    let parent_worktree = tempfile::tempdir().unwrap();
    let child_worktree = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let artifact_store = ArtifactStore::new(storage.home().join("runs"));

    let parent_run_id = RunId::new();
    let parent_writer = storage
        .create_run(parent_run_id, parent_worktree.path(), None)
        .await
        .unwrap();
    let parent_node = NodeKey::try_from("description_author").unwrap();
    let description = artifact_store
        .put(parent_run_id, "description", b"# Description\n")
        .await
        .unwrap();
    let roadmap = artifact_store
        .put(parent_run_id, "roadmap", b"# Roadmap\n")
        .await
        .unwrap();
    let flow = artifact_store
        .put(parent_run_id, "flow", b"schema_version = 1\n")
        .await
        .unwrap();

    parent_writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: parent_worktree.path().to_path_buf(),
                initial_prompt: "bootstrap this".into(),
                config: RunConfig {
                    sandbox_default: SandboxMode::WorkspaceWrite,
                    approval_default: ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: vec![],
                },
            }),
            VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: parent_node.clone(),
                artifact: description.hash,
                path: description.path,
                name: "description".into(),
            }),
            VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: parent_node.clone(),
                artifact: roadmap.hash,
                path: roadmap.path,
                name: "roadmap".into(),
            }),
            VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: parent_node,
                artifact: flow.hash,
                path: flow.path,
                name: "flow".into(),
            }),
        ])
        .await
        .unwrap();
    drop(parent_writer);

    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(
        child_worktree.path().to_path_buf(),
    )) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let child_run_id = RunId::new();
    let handle = engine
        .start_run(
            child_run_id,
            terminal_graph(),
            child_worktree.path().to_path_buf(),
            EngineRunConfig {
                bootstrap_parent: Some(parent_run_id),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap();
    let outcome = handle.await_completion().await.unwrap();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected completion, got {other:?}"),
    }

    let reader = storage.open_run_reader(child_run_id).await.unwrap();
    let events = reader.read_events(EventSeq(1)..EventSeq(64)).await.unwrap();
    let mut memory = RunMemory::default();
    let mut inherited = BTreeMap::new();

    for event in &events {
        let payload = event.payload.payload.clone();
        if let EventPayload::ArtifactProduced {
            node,
            artifact,
            path,
            name,
        } = &payload
        {
            if ["description", "roadmap", "flow"].contains(&name.as_str()) {
                assert_eq!(node.as_ref(), "bootstrap_parent");
                assert_eq!(path.file_name().unwrap(), artifact.to_hex().as_str());
                inherited.insert(name.clone(), std::fs::read(path).unwrap());
            }
        }
        memory.apply_event(&RunEvent {
            run_id: child_run_id,
            seq: event.seq.as_u64(),
            timestamp: chrono::Utc::now(),
            payload,
        });
    }

    assert_eq!(
        inherited.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["description", "flow", "roadmap"]
    );
    assert_eq!(inherited["description"], b"# Description\n");
    assert_eq!(inherited["roadmap"], b"# Roadmap\n");
    assert_eq!(inherited["flow"], b"schema_version = 1\n");
    assert!(memory.artifacts.contains_key("description"));
    assert!(memory.artifacts.contains_key("roadmap"));
    assert!(memory.artifacts.contains_key("flow"));
}
