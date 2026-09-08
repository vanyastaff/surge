//! `Engine::start_run` seeds the resolved profile registry as the
//! `profile_catalog` run artifact when a `ProfileRegistry` is wired
//! (`profile_loader::render_profile_catalog`), and seeds nothing — while
//! still starting normally — when it isn't.
//!
//! Mirrors `engine_project_context_seed_test.rs`'s harness shape (same
//! `MockBridge`, `WorktreeToolDispatcher`, terminal-only graph, direct
//! event-log read after `await_completion`).

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
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
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
            name: "profile-catalog-seeding".into(),
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
async fn start_run_seeds_profile_catalog_artifact_when_registry_wired() {
    let storage_dir = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(worktree.path().to_path_buf()))
        as Arc<dyn ToolDispatcher>;
    // Bundled profiles only (no disk overlay) — enough to exercise the
    // catalog-rendering path without a `SURGE_HOME` fixture.
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
    let handle = engine
        .start_run(
            run_id,
            terminal_graph(),
            worktree.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let _ = handle.await_completion().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader.read_events(EventSeq(1)..EventSeq(64)).await.unwrap();
    let mut memory = RunMemory::default();
    let mut seeded_content: Option<String> = None;

    for event in &events {
        let payload = event.payload.payload.clone();
        if let EventPayload::ArtifactProduced {
            node, path, name, ..
        } = &payload
            && name == "profile_catalog"
        {
            assert_eq!(node.as_ref(), "profile_catalog_seed");
            seeded_content = Some(std::fs::read_to_string(worktree.path().join(path)).unwrap());
        }
        memory.apply_event(&RunEvent {
            run_id,
            seq: event.seq.as_u64(),
            timestamp: chrono::Utc::now(),
            payload,
        });
    }

    let content = seeded_content.expect("profile_catalog ArtifactProduced event is missing");
    // Proof the seeded artifact is the real rendered catalogue, not a stub:
    // it names bundled profiles and resolves their canonical runtime.
    assert!(
        content.contains("`implementer@1.0`"),
        "catalog should list implementer@1.0, got:\n{content}"
    );
    assert!(
        content.contains("claude-acp"),
        "catalog should show implementer@1.0's canonical runtime, got:\n{content}"
    );

    let artifact = memory.artifacts.get("profile_catalog").unwrap();
    assert_eq!(artifact.produced_by.as_ref(), "profile_catalog_seed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_run_seeds_nothing_and_still_starts_without_profile_registry() {
    let storage_dir = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let storage = Storage::open(storage_dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(worktree.path().to_path_buf()))
        as Arc<dyn ToolDispatcher>;
    // No `profile_registry` set — `EngineConfig::default()` leaves it `None`.
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            terminal_graph(),
            worktree.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let _ = handle.await_completion().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader.read_events(EventSeq(1)..EventSeq(64)).await.unwrap();

    let saw_catalog_seed = events.iter().any(|event| {
        matches!(
            &event.payload.payload,
            EventPayload::ArtifactProduced { name, .. } if name == "profile_catalog"
        )
    });
    assert!(
        !saw_catalog_seed,
        "no profile_registry was wired; start_run must not seed a profile_catalog artifact"
    );
}
