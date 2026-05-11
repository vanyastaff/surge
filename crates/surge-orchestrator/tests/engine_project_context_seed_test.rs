mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, RunEvent};
use surge_core::run_state::RunMemory;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::config::ProjectContextSeed;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
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
            name: "project-context-seeding".into(),
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
async fn start_run_seeds_project_context_artifact() {
    let storage_dir = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(worktree.path().to_path_buf()))
        as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());
    let seed = ProjectContextSeed::new(
        worktree.path().join("project.md"),
        "# Stable project context\n".to_string(),
    );

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            terminal_graph(),
            worktree.path().to_path_buf(),
            EngineRunConfig {
                project_context: Some(seed.clone()),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap();
    let _ = handle.await_completion().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader.read_events(EventSeq(1)..EventSeq(64)).await.unwrap();
    let mut memory = RunMemory::default();
    let mut saw_seed = false;

    for event in &events {
        let payload = event.payload.payload.clone();
        if let EventPayload::ArtifactProduced {
            node,
            artifact,
            path,
            name,
        } = &payload
        {
            if name == "project_context" {
                assert_eq!(node.as_ref(), "project_context_seed");
                assert_eq!(*artifact, seed.hash);
                assert_eq!(std::fs::read(path).unwrap(), seed.content.as_bytes());
                saw_seed = true;
            }
        }
        memory.apply_event(&RunEvent {
            run_id,
            seq: event.seq.as_u64(),
            timestamp: chrono::Utc::now(),
            payload,
        });
    }

    assert!(
        saw_seed,
        "project_context ArtifactProduced event is missing"
    );
    let artifact = memory.artifacts.get("project_context").unwrap();
    assert_eq!(artifact.hash, seed.hash);
    assert_eq!(artifact.produced_by.as_ref(), "project_context_seed");
}
