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
    /// Graph extracted from the `PipelineMaterialized` event.
    pub graph: surge_core::graph::Graph,
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
/// 2. Read all events from seq 1 onwards to rebuild memory and find the graph.
/// 3. Return the graph, memory, and cursor (snapshot's cursor or graph.start).
pub async fn replay(
    reader: &surge_persistence::runs::reader::RunReader,
) -> Result<ReplayedState, EngineError> {
    // Load latest snapshot (if any).
    let snap = reader
        .latest_snapshot_at_or_before(EventSeq(u64::MAX))
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    let snap_cursor: Option<Cursor> = match snap {
        Some((_seq, blob)) => {
            let snapshot = EngineSnapshot::deserialize(&blob)
                .map_err(|e| EngineError::Internal(format!("snapshot deserialize: {e}")))?;
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
    let max_seq = reader
        .current_seq()
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    let all_events = reader
        .read_events(EventSeq(1)..EventSeq(max_seq.as_u64().saturating_add(1)))
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;

    // Find the graph from PipelineMaterialized.
    let graph = all_events
        .iter()
        .find_map(|e| match &e.payload.payload {
            EventPayload::PipelineMaterialized { graph, .. } => Some((**graph).clone()),
            _ => None,
        })
        .ok_or_else(|| EngineError::Internal("no PipelineMaterialized event in log".into()))?;

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
        already_terminal,
        run_config: persisted_run_config,
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
}
