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
            let snapshot: EngineSnapshot = serde_json::from_slice(&blob)
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
        EventPayload::RunCompleted { terminal_node } => {
            Some(RunOutcome::Completed { terminal: terminal_node.clone() })
        },
        EventPayload::RunFailed { error } => {
            Some(RunOutcome::Failed { error: error.clone() })
        },
        EventPayload::RunAborted { reason } => {
            Some(RunOutcome::Aborted { reason: reason.clone() })
        },
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
    })
}
