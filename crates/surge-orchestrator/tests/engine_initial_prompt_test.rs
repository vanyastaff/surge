//! Task 29 — `ArtifactSource::InitialPrompt` seeding in `Engine::start_run`.
//!
//! Asserts that when `EngineRunConfig.initial_prompt` is non-empty, the
//! engine:
//!   * sets `RunStarted.initial_prompt` to the supplied value (instead of
//!     the legacy hardcoded empty string),
//!   * writes the prompt body to `<worktree>/.surge/user_prompt.txt`,
//!   * emits an `ArtifactProduced` event with `name = "user_prompt"`, the
//!     correct content hash, and the synthetic `start_node` producer node,
//!   * and that folding the resulting event log produces a `RunMemory`
//!     whose `artifacts["user_prompt"]` resolves to the verbatim prompt
//!     through the standard binding path used by bootstrap profiles.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::EventPayload;
use surge_core::run_state::{RunMemory, RunState, fold};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};

fn minimal_terminal_graph() -> Graph {
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
            name: "initial-prompt-seeding".into(),
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
async fn start_run_seeds_user_prompt_artifact_from_initial_prompt() {
    let prompt = "fix the broken cart-total bug";

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        initial_prompt: prompt.to_string(),
        ..EngineRunConfig::default()
    };

    let handle = engine
        .start_run(
            run_id,
            minimal_terminal_graph(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await
        .expect("start_run");

    let outcome = handle.await_completion().await.unwrap();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed, got {other:?}"),
    }

    // The prompt body must be on disk at the canonical relative path so
    // both `ArtifactSource::InitialPrompt` and `ArtifactSource::RunArtifact
    // { name: "user_prompt" }` resolve through `read_artifact_text`.
    let prompt_file = dir.path().join(".surge").join("user_prompt.txt");
    let on_disk = tokio::fs::read_to_string(&prompt_file)
        .await
        .expect("user_prompt.txt was not written by start_run");
    assert_eq!(on_disk, prompt);

    // Read every event and locate the seeded ArtifactProduced. Range covers
    // the first few seqs — RunStarted, PipelineMaterialized, ArtifactProduced,
    // plus whatever the run task itself emitted to terminate.
    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");
    assert!(!events.is_empty(), "expected at least RunStarted");

    let mut saw_run_started_with_prompt = false;
    let mut saw_initial_prompt_artifact = false;
    let expected_hash = ContentHash::compute(prompt.as_bytes());
    let producer = NodeKey::try_from("start_node").unwrap();

    for ev in &events {
        match &ev.payload.payload {
            EventPayload::RunStarted { initial_prompt, .. } => {
                assert_eq!(
                    initial_prompt, prompt,
                    "RunStarted.initial_prompt must reflect EngineRunConfig.initial_prompt",
                );
                saw_run_started_with_prompt = true;
            },
            EventPayload::ArtifactProduced {
                node,
                artifact,
                path,
                name,
            } if name == "user_prompt" => {
                assert_eq!(node, &producer, "synthetic producer node mismatch");
                assert_eq!(artifact, &expected_hash, "content hash mismatch");
                assert_eq!(
                    path.to_string_lossy().replace('\\', "/"),
                    ".surge/user_prompt.txt",
                    "path must be the relative .surge/user_prompt.txt",
                );
                saw_initial_prompt_artifact = true;
            },
            _ => {},
        }
    }

    assert!(saw_run_started_with_prompt);
    assert!(
        saw_initial_prompt_artifact,
        "ArtifactProduced{{name = user_prompt}} must be emitted right after PipelineMaterialized",
    );

    // Folding the event log must reconstruct a Pipeline (or terminal-after-pipeline)
    // RunMemory whose artifacts map carries the user prompt — this is the
    // exact path bootstrap agent stages will take when resolving
    // `ArtifactSource::InitialPrompt`.
    let run_events: Vec<_> = events
        .iter()
        .map(|re| surge_core::run_event::RunEvent {
            run_id,
            seq: re.seq.0,
            timestamp: chrono::Utc::now(),
            payload: re.payload.payload.clone(),
        })
        .collect();
    let folded = fold(&run_events).expect("event log folds cleanly");
    let memory: RunMemory = match folded {
        RunState::Pipeline { memory, .. } => memory,
        RunState::Terminal { .. } => {
            // The mock bridge may already have driven the Terminal node to
            // completion; in that case re-fold up to (but not including) the
            // terminal-transition events to inspect mid-run memory.
            let mut acc = RunMemory::default();
            for ev in &run_events {
                acc.apply_event(ev);
            }
            acc
        },
        other => panic!("unexpected fold output: {other:?}"),
    };

    let aref = memory
        .artifacts
        .get("user_prompt")
        .expect("RunMemory.artifacts must carry the seeded user_prompt entry");
    assert_eq!(aref.name, "user_prompt");
    assert_eq!(aref.hash, expected_hash);
    assert_eq!(aref.produced_by, producer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_run_with_empty_prompt_skips_seeding() {
    // Backward-compat guard: legacy callers (every existing test in the
    // workspace) leave `initial_prompt` empty and must not see a synthetic
    // user_prompt artifact, an extra event, or a `.surge/user_prompt.txt`
    // file written into their worktree.
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            minimal_terminal_graph(),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let _ = handle.await_completion().await.unwrap();

    let prompt_file = dir.path().join(".surge").join("user_prompt.txt");
    assert!(
        !prompt_file.exists(),
        "empty initial_prompt must not write a user_prompt artifact file",
    );

    let reader = storage.open_run_reader(run_id).await.expect("reader");
    let events = reader
        .read_events(EventSeq(0)..EventSeq(64))
        .await
        .expect("read_events");
    let seeded_artifact_present = events.iter().any(|re| {
        matches!(
            &re.payload.payload,
            EventPayload::ArtifactProduced { name, .. } if name == "user_prompt"
        )
    });
    assert!(
        !seeded_artifact_present,
        "no ArtifactProduced(user_prompt) should appear when initial_prompt is empty",
    );
}
