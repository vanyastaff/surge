//! Reconstruct in-memory engine state from snapshot + post-snapshot events.

use crate::engine::error::EngineError;
use crate::engine::handle::RunOutcome;
use crate::engine::snapshot::EngineSnapshot;
use surge_core::run_event::EventPayload;
use surge_core::run_state::{Cursor, RunMemory};
use surge_persistence::runs::seq::EventSeq;

/// In-memory engine state reconstructed from storage.
pub struct ReplayedState {
    /// Cursor to resume from (from snapshot, or `graph.start` if none).
    pub cursor: Cursor,
    /// Run memory rebuilt by replaying all events from seq 1 onwards.
    pub memory: RunMemory,
    /// Latest graph extracted from `PipelineMaterialized` plus any accepted
    /// graph revision events.
    pub graph: surge_core::graph::Graph,
    /// Latest graph revision sequence known to have been applied to the
    /// active graph at a persisted stage boundary.
    pub applied_graph_revision_seq: u64,
    /// Set when the event log already contains a terminal event
    /// (`RunCompleted`, `RunFailed`, or `RunAborted`). When `Some`, the
    /// caller should return this outcome immediately without re-executing.
    pub already_terminal: Option<RunOutcome>,
    /// The persisted `RunConfig` from the `RunStarted` event, if present.
    /// `None` for old runs whose event log predates the `mcp_servers` field.
    /// Used by `Engine::resume_run` to reconstruct the per-run MCP registry.
    pub run_config: Option<surge_core::run_event::RunConfig>,
}

/// Replay the event log for a run, returning reconstructed in-memory state.
///
/// Steps:
/// 1. Load the latest snapshot (if any) to obtain a base cursor.
/// 2. Read all events from seq 1 onwards to rebuild memory and find the latest graph.
/// 3. Return the graph, memory, and cursor (snapshot's cursor or graph.start).
pub async fn replay(
    reader: &surge_persistence::runs::reader::RunReader,
) -> Result<ReplayedState, EngineError> {
    let max_seq = reader
        .current_seq()
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    // Load latest snapshot (if any).
    let snap = reader
        .latest_snapshot_at_or_before(max_seq)
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    let mut applied_graph_revision_seq = 0;
    let snap_cursor: Option<Cursor> = match snap {
        Some((_seq, blob)) => {
            let snapshot = EngineSnapshot::deserialize(&blob)
                .map_err(|e| EngineError::Internal(format!("snapshot deserialize: {e}")))?;
            applied_graph_revision_seq = snapshot.applied_graph_revision_seq;
            let cursor = snapshot
                .cursor
                .into_cursor()
                .map_err(|e| EngineError::Internal(format!("snapshot cursor: {e}")))?;
            Some(cursor)
        },
        None => None,
    };

    // Read all events from seq 1 onwards. We need ALL events for memory
    // reconstruction (artifacts, outcomes, costs).
    let all_events = reader
        .read_events(EventSeq(1)..EventSeq(max_seq.as_u64().saturating_add(1)))
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    // Find the latest graph from PipelineMaterialized plus graph revisions.
    let graph = latest_graph_from_events(&all_events, applied_graph_revision_seq)?;

    // Extract the persisted RunConfig from RunStarted (if any).
    let persisted_run_config = all_events.iter().find_map(|e| match &e.payload.payload {
        EventPayload::RunStarted { config, .. } => Some(config.clone()),
        _ => None,
    });

    // Rebuild memory from events.
    let mut memory = RunMemory::default();
    for ev in &all_events {
        use chrono::TimeZone;
        let timestamp = chrono::Utc
            .timestamp_millis_opt(ev.timestamp_ms)
            .single()
            .unwrap_or_else(chrono::Utc::now);
        let core_event = surge_core::run_event::RunEvent {
            run_id: *reader.run_id(),
            seq: ev.seq.as_u64(),
            timestamp,
            payload: ev.payload.payload.clone(),
        };
        memory.apply_event(&core_event);
    }

    // Detect whether the run already reached a terminal state.
    let already_terminal = all_events.iter().find_map(|e| match &e.payload.payload {
        EventPayload::RunCompleted { terminal_node } => Some(RunOutcome::Completed {
            terminal: terminal_node.clone(),
        }),
        EventPayload::RunFailed { error } => Some(RunOutcome::Failed {
            error: error.clone(),
        }),
        EventPayload::RunAborted { reason } => Some(RunOutcome::Aborted {
            reason: reason.clone(),
        }),
        _ => None,
    });

    // Cursor: snapshot's, or graph.start if no snapshot.
    let cursor = snap_cursor.unwrap_or_else(|| Cursor {
        node: graph.start.clone(),
        attempt: 1,
    });

    Ok(ReplayedState {
        cursor,
        memory,
        graph,
        applied_graph_revision_seq,
        already_terminal,
        run_config: persisted_run_config,
    })
}

fn latest_graph_from_events(
    events: &[surge_persistence::runs::reader::ReadEvent],
    applied_graph_revision_seq: u64,
) -> Result<surge_core::graph::Graph, EngineError> {
    let mut selected = None;
    for event in events {
        match &event.payload.payload {
            EventPayload::PipelineMaterialized { graph, graph_hash } => {
                tracing::debug!(
                    target: "engine_replay",
                    seq = event.seq.as_u64(),
                    graph_hash = %graph_hash,
                    "replay_pipeline_graph_selected"
                );
                selected = Some((event.seq.as_u64(), (**graph).clone()));
            },
            EventPayload::GraphRevisionAccepted {
                graph, graph_hash, ..
            } if event.seq.as_u64() <= applied_graph_revision_seq => {
                tracing::debug!(
                    target: "engine_replay",
                    seq = event.seq.as_u64(),
                    graph_hash = %graph_hash,
                    "replay_graph_revision_selected"
                );
                selected = Some((event.seq.as_u64(), (**graph).clone()));
            },
            _ => {},
        }
    }
    selected.map(|(_, graph)| graph).ok_or_else(|| {
        EngineError::Internal(
            "no graph-bearing event (PipelineMaterialized or applied GraphRevisionAccepted) in log"
                .into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;
    use std::time::Duration;
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::content_hash::ContentHash;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::id::RunId;
    use surge_core::keys::NodeKey;
    use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::roadmap_patch::{ActivePickupPolicy, RoadmapPatchId, RoadmapPatchTarget};
    use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
    use surge_core::sandbox::SandboxMode;
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_persistence::runs::Storage;

    fn minimal_graph() -> Graph {
        minimal_graph_with_start("end")
    }

    fn minimal_graph_with_start(start: &str) -> Graph {
        let end = NodeKey::try_from(start).unwrap();
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
                name: "replay-test".into(),
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

    /// Persist a `RunStarted` event with non-empty `mcp_servers`, then call
    /// `replay` and assert that `run_config` comes back with the same servers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_returns_persisted_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = RunId::new();
        let worktree = dir.path().to_path_buf();

        let writer = storage
            .create_run(run_id, &worktree, None)
            .await
            .expect("create_run");

        let graph = minimal_graph();
        let graph_bytes = serde_json::to_vec(&graph).unwrap();
        let graph_hash = ContentHash::compute(&graph_bytes);

        let server = McpServerRef::new(
            "playwright".into(),
            McpTransportConfig::stdio(PathBuf::from("mcp-playwright"), vec![], HashMap::new()),
            Some(vec!["browser_navigate".into()]),
            Duration::from_secs(60),
            true,
        );

        let run_config = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![server],
        };

        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree.clone(),
                    initial_prompt: String::new(),
                    config: run_config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph.clone()),
                    graph_hash,
                }),
            ])
            .await
            .expect("append_events");

        let reader = storage
            .open_run_reader(run_id)
            .await
            .expect("open_run_reader");

        let replayed = replay(&reader).await.expect("replay");

        let rc = replayed
            .run_config
            .expect("run_config should be Some after RunStarted with mcp_servers");

        assert_eq!(
            rc.mcp_servers.len(),
            1,
            "expected 1 MCP server in replayed config"
        );
        assert_eq!(rc.mcp_servers[0].name, "playwright");
        assert_eq!(
            rc.mcp_servers[0].allowed_tools,
            Some(vec!["browser_navigate".into()])
        );
    }

    /// Old runs without `mcp_servers` in `RunConfig` must not cause errors;
    /// `run_config.mcp_servers` is empty after deserialization via `#[serde(default)]`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_run_config_mcp_servers_empty_when_not_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = RunId::new();
        let worktree = dir.path().to_path_buf();

        let writer = storage
            .create_run(run_id, &worktree, None)
            .await
            .expect("create_run");

        let graph = minimal_graph();
        let graph_bytes = serde_json::to_vec(&graph).unwrap();
        let graph_hash = ContentHash::compute(&graph_bytes);

        // Persist RunConfig with an empty mcp_servers list (mirrors pre-M7 runs).
        let run_config = RunConfig {
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
                    initial_prompt: String::new(),
                    config: run_config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(graph.clone()),
                    graph_hash,
                }),
            ])
            .await
            .expect("append_events");

        let reader = storage
            .open_run_reader(run_id)
            .await
            .expect("open_run_reader");

        let replayed = replay(&reader).await.expect("replay");

        // run_config is Some (event exists), but mcp_servers is empty.
        let rc = replayed.run_config.expect("run_config should be Some");
        assert!(
            rc.mcp_servers.is_empty(),
            "expected empty mcp_servers for run without MCP"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_uses_latest_graph_revision_with_snapshot_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = RunId::new();
        let worktree = dir.path().to_path_buf();

        let writer = storage
            .create_run(run_id, &worktree, None)
            .await
            .expect("create_run");

        let base_graph = minimal_graph();
        let amended_graph = minimal_graph_with_start("amend_001");
        let previous_graph_hash = ContentHash::compute(b"base-flow");
        let graph_hash = ContentHash::compute(b"amended-flow");
        let run_config = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
        };

        let seqs = writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree.clone(),
                    initial_prompt: String::new(),
                    config: run_config,
                }),
                VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                    graph: Box::new(base_graph),
                    graph_hash: previous_graph_hash,
                }),
                VersionedEventPayload::new(EventPayload::GraphRevisionAccepted {
                    patch_id: RoadmapPatchId::new("rpatch-replay").unwrap(),
                    target: RoadmapPatchTarget::ProjectRoadmap {
                        roadmap_path: ".ai-factory/ROADMAP.md".into(),
                    },
                    previous_graph_hash,
                    graph: Box::new(amended_graph),
                    graph_hash,
                    active_pickup: ActivePickupPolicy::Allowed,
                }),
            ])
            .await
            .expect("append_events");
        let graph_revision_seq = seqs[2];

        let snapshot_cursor = Cursor {
            node: NodeKey::try_from("amend_001").unwrap(),
            attempt: 1,
        };
        let mut snapshot = EngineSnapshot::new(
            &snapshot_cursor,
            graph_revision_seq.as_u64(),
            graph_revision_seq.as_u64(),
        );
        snapshot.applied_graph_revision_seq = graph_revision_seq.as_u64();
        writer
            .write_graph_snapshot(graph_revision_seq, serde_json::to_vec(&snapshot).unwrap())
            .await
            .expect("write snapshot");
        let (_, snapshot_blob) = writer
            .latest_snapshot_at_or_before(graph_revision_seq)
            .await
            .expect("read snapshot")
            .expect("snapshot row");
        let stored_snapshot =
            EngineSnapshot::deserialize(&snapshot_blob).expect("deserialize stored snapshot");
        assert_eq!(
            stored_snapshot.applied_graph_revision_seq,
            graph_revision_seq.as_u64()
        );

        let reader = storage
            .open_run_reader(run_id)
            .await
            .expect("open_run_reader");
        let replayed = replay(&reader).await.expect("replay");

        assert_eq!(replayed.graph.start, snapshot_cursor.node);
        assert_eq!(replayed.cursor, snapshot_cursor);
        let revision = replayed
            .memory
            .latest_graph_revision
            .expect("graph revision memory");
        assert_eq!(revision.graph_hash, graph_hash);
        assert_eq!(revision.previous_graph_hash, previous_graph_hash);
    }
}
