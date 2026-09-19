//! Integration test: a composed flow is recorded in the run's own log (T12).
//!
//! `install_composed_flow` writes and pins the artifact; the *run* that uses
//! it must carry `ComposedArtifactInstalled` in its startup prefix.
//! `EngineRunConfig::startup_events` is the seam the task scheduler uses for
//! exactly that, and this test proves the engine writes them into the log
//! before the first stage — so the event log alone is the authority for what
//! the run was handed.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::facade::BridgeFacade;
use surge_core::artifact_contract::{ArtifactKind, RelPath};
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};

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
            name: "compose-record".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
            when_to_use: None,
            autonomy: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_events_land_in_the_run_log_before_the_first_stage() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let composed = EventPayload::ComposedArtifactInstalled {
        kind: ArtifactKind::Flow,
        path: RelPath::new(".surge/flows/composed-review-1.0.toml").unwrap(),
        hash: ContentHash::compute(b"graph bytes"),
        gate_node: NodeKey::try_from("flow_generator").unwrap(),
    };
    let run_config = EngineRunConfig {
        startup_events: vec![VersionedEventPayload::new(composed)],
        ..EngineRunConfig::default()
    };

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            minimal_graph(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await
        .expect("start_run");
    let outcome = handle.await_completion().await.unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    // The event is in the log, before the first stage's events.
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap();
    let composed_index = events
        .iter()
        .position(|event| {
            matches!(
                event.payload.payload,
                EventPayload::ComposedArtifactInstalled { .. }
            )
        })
        .expect("ComposedArtifactInstalled must be in the run log");
    let first_stage_index = events
        .iter()
        .position(|event| matches!(event.payload.payload, EventPayload::StageEntered { .. }))
        .expect("the run entered a stage");
    assert!(
        composed_index < first_stage_index,
        "the composed artifact is part of the run's startup prefix"
    );
    if let EventPayload::ComposedArtifactInstalled { kind, path, .. } =
        &events[composed_index].payload.payload
    {
        assert_eq!(*kind, ArtifactKind::Flow);
        assert_eq!(path.as_str(), ".surge/flows/composed-review-1.0.toml");
    }
}
