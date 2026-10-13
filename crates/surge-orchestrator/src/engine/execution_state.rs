//! Execution state at seq N (#100): the one place that answers "for this run,
//! at seq N (or the latest seq), where does execution stand?"
//!
//! Fresh start, resume, fork and the CLI scrubber all consume the
//! [`ExecutionState`] [`load`] returns. It reads through the existing
//! `RunReader` seam and never reads past N: only events `1..=N` and only the
//! latest checkpoint at or before N.
//!
//! # Rules
//! - **Deterministic.** Memory is folded with `RunMemory::apply_event` over
//!   the whole prefix, through the one shared event conversion
//!   (`ReadEvent::to_run_event`), which never reads the clock: a corrupt
//!   timestamp is a typed error.
//! - **Not a report.** `run_state::fold` and `RunReport` are not used here
//!   (ADR-0017): a terminal transition must not discard memory.
//! - **Graph in force** ([`graph_in_force`]) is the **first**
//!   `PipelineMaterialized` at or before N, then the latest
//!   `GraphRevisionAccepted` at or below the checkpoint's applied-revision
//!   seq. A bootstrap run later appends its generated child flow as a second
//!   materialization; "last materialization wins" made a resumed planning run
//!   look for its own gate in the child's graph. Fork edits rewrite that same
//!   first event.
//! - **Explicit cursor source.** Without a checkpoint the cursor is the graph
//!   start (attempt 1) and [`CursorSource::GraphStart`] says so.
//! - **Consumed routes need their checkpoint.** A committed stage route with
//!   no matching checkpoint at its route seq (or a contradicting one) refuses:
//!   falling back to the graph start would re-execute committed effects.
//! - A terminal conflict is **not** an error here: it is reported in
//!   [`ExecutionState::terminal_verdict`] and each caller applies its policy.

use std::collections::HashMap;

use surge_core::content_hash::ContentHash;
use surge_core::graph::Graph;
use surge_core::keys::EdgeKey;
use surge_core::roadmap_patch::{ActivePickupPolicy, RoadmapPatchId, RoadmapPatchTarget};
use surge_core::run_event::{EventPayload, RunConfig};
use surge_core::run_state::{Cursor, RunMemory};
use surge_core::run_verdict::TerminalVerdict;
use surge_persistence::runs::reader::{ReadEvent, RunReader};
use surge_persistence::runs::seq::EventSeq;

use crate::engine::error::EngineError;
use crate::engine::frames::Frame;
use crate::engine::snapshot::EngineSnapshot;

/// Which prefix to rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// The latest persisted seq.
    Latest,
    /// Exactly seq N (1-based, at most the latest seq).
    AtSeq(u64),
}

/// Where the cursor came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorSource {
    /// The latest checkpoint at or before the position, written at this seq.
    Checkpoint {
        /// The checkpoint's seq.
        seq: u64,
    },
    /// No checkpoint at or before the position: execution starts at the
    /// graph's start node, attempt 1, and re-enters the first stage.
    GraphStart,
}

/// A graph revision accepted at or before the position but not yet applied
/// to the graph in force; the run task picks it up at its next safe point.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingGraphRevision {
    /// The revision's seq.
    pub seq: u64,
    /// The roadmap patch it carries.
    pub patch_id: RoadmapPatchId,
    /// What the patch targets.
    pub target: RoadmapPatchTarget,
    /// Identity of the graph it revises.
    pub previous_graph_hash: ContentHash,
    /// The revised graph.
    pub graph: Graph,
    /// The revised graph's identity.
    pub graph_hash: ContentHash,
    /// When an active run may pick it up.
    pub active_pickup: ActivePickupPolicy,
}

/// Where execution stands at one position. See the module doc for the rules.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionState {
    /// The resolved position N.
    pub position: u64,
    /// The cursor to execute from.
    pub cursor: Cursor,
    /// Where [`Self::cursor`] came from.
    pub cursor_source: CursorSource,
    /// The frame stack captured with the checkpoint (empty without one).
    pub frames: Vec<Frame>,
    /// Edge traversal counts outside loop frames.
    pub root_traversal_counts: HashMap<EdgeKey, u32>,
    /// Run memory folded over events `1..=N`.
    pub memory: RunMemory,
    /// The exact prefix folded into [`Self::memory`]: always [`Self::position`].
    pub memory_applied_seq: u64,
    /// The graph in force at N.
    pub graph: Graph,
    /// The latest graph-revision seq applied to [`Self::graph`].
    pub applied_graph_revision_seq: u64,
    /// Revisions accepted after the applied seq and at or before N, in seq
    /// order.
    pub pending_graph_revisions: Vec<PendingGraphRevision>,
    /// The persisted run config from the first `RunStarted`; `None` for logs
    /// that predate it.
    pub run_config: Option<RunConfig>,
    /// The terminal verdict over events `1..=N` (#95).
    pub terminal_verdict: TerminalVerdict,
}

/// Why the execution state could not be rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionStateError {
    /// Reading the run's log or checkpoints failed.
    #[error("storage error: {0}")]
    Storage(String),
    /// Position 0 names no prefix.
    #[error("position 0 names no prefix; sequences start at 1")]
    PositionZero,
    /// The position is beyond the latest seq.
    #[error("position {position} is beyond the latest seq {latest}")]
    PositionBeyond {
        /// The requested position.
        position: u64,
        /// The latest seq.
        latest: u64,
    },
    /// The log is empty.
    #[error("the run's log is empty")]
    EmptyLog,
    /// No graph-bearing event at or before the position.
    #[error("no graph-bearing event (PipelineMaterialized) at or before seq {0}")]
    NoGraph(u64),
    /// The checkpoint does not decode.
    #[error("checkpoint at seq {seq} does not decode: {reason}")]
    CheckpointDecode {
        /// The checkpoint's seq.
        seq: u64,
        /// Why.
        reason: String,
    },
    /// The checkpoint's envelope seq contradicts its row.
    #[error("checkpoint row at seq {seq} carries envelope seq {envelope}")]
    CheckpointSeqMismatch {
        /// The row's seq.
        seq: u64,
        /// The envelope's seq.
        envelope: u64,
    },
    /// A committed stage route has no checkpoint at its route seq.
    #[error("consumed stage route at seq {route_seq} lacks its durable execution checkpoint")]
    ConsumedRouteWithoutCheckpoint {
        /// The route's seq.
        route_seq: u64,
    },
    /// A committed stage route contradicts its checkpoint.
    #[error("consumed stage route at seq {route_seq} contradicts its execution checkpoint")]
    ConsumedRouteContradicted {
        /// The route's seq.
        route_seq: u64,
    },
    /// An event's recorded timestamp is not a valid instant.
    #[error("event {seq} has a corrupt timestamp ({raw_ms} ms)")]
    CorruptTimestamp {
        /// The event's seq.
        seq: u64,
        /// The raw milliseconds.
        raw_ms: i64,
    },
}

impl From<surge_persistence::runs::CorruptTimestamp> for ExecutionStateError {
    fn from(error: surge_persistence::runs::CorruptTimestamp) -> Self {
        Self::CorruptTimestamp {
            seq: error.seq,
            raw_ms: error.raw_ms,
        }
    }
}

impl From<ExecutionStateError> for EngineError {
    fn from(error: ExecutionStateError) -> Self {
        match error {
            ExecutionStateError::Storage(message) => Self::Storage(message),
            other => Self::Internal(other.to_string()),
        }
    }
}

fn storage(error: impl std::fmt::Display) -> ExecutionStateError {
    ExecutionStateError::Storage(error.to_string())
}

/// Rebuild the execution state of `reader`'s run at `position`.
///
/// # Errors
/// A typed [`ExecutionStateError`]; see the module doc.
#[must_use = "the execution state is the input to fresh start, resume and fork"]
pub async fn load(
    reader: &RunReader,
    position: Position,
) -> Result<ExecutionState, ExecutionStateError> {
    let run_id = *reader.run_id();
    let n = resolve_position(reader, position).await?;
    let events = reader
        .read_events(EventSeq(1)..EventSeq(n).next())
        .await
        .map_err(storage)?;

    let checkpoint = reader
        .latest_snapshot_at_or_before(EventSeq(n))
        .await
        .map_err(storage)?;
    let mut applied_graph_revision_seq = 0;
    let mut frames = Vec::new();
    let mut root_traversal_counts = HashMap::new();
    let mut checkpoint_cursor = None;
    if let Some((seq, blob)) = checkpoint {
        let seq = seq.as_u64();
        let decode = |reason: String| ExecutionStateError::CheckpointDecode { seq, reason };
        let snapshot = EngineSnapshot::deserialize(&blob).map_err(|e| decode(e.to_string()))?;
        if snapshot.at_seq != seq {
            return Err(ExecutionStateError::CheckpointSeqMismatch {
                seq,
                envelope: snapshot.at_seq,
            });
        }
        applied_graph_revision_seq = snapshot.applied_graph_revision_seq;
        frames = snapshot
            .frames
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| decode(format!("frames: {error}")))?;
        for (edge, count) in snapshot.root_traversal_counts {
            let key = EdgeKey::try_from(edge.as_str())
                .map_err(|error| decode(format!("edge counter: {error}")))?;
            root_traversal_counts.insert(key, count);
        }
        let cursor = snapshot
            .cursor
            .into_cursor()
            .map_err(|error| decode(format!("cursor: {error}")))?;
        checkpoint_cursor = Some((seq, cursor));
    }

    let graph = graph_in_force(&events, applied_graph_revision_seq)
        .ok_or(ExecutionStateError::NoGraph(n))?;
    let memory = fold(run_id, &events)?;
    validate_consumed_route(
        reader,
        &events,
        &memory,
        checkpoint_cursor.as_ref().map(|(seq, _)| *seq),
    )
    .await?;

    let (cursor, cursor_source) = match checkpoint_cursor {
        Some((seq, cursor)) => (cursor, CursorSource::Checkpoint { seq }),
        None => (
            Cursor {
                node: graph.start.clone(),
                attempt: 1,
            },
            CursorSource::GraphStart,
        ),
    };
    let pending_graph_revisions = pending_revisions(&events, applied_graph_revision_seq);
    tracing::debug!(
        target: "engine::execution_state",
        %run_id,
        position = n,
        cursor = %cursor.node,
        ?cursor_source,
        applied_graph_revision_seq,
        pending_revisions = pending_graph_revisions.len(),
        "execution state loaded"
    );
    Ok(ExecutionState {
        position: n,
        cursor,
        cursor_source,
        frames,
        root_traversal_counts,
        memory,
        memory_applied_seq: n,
        graph,
        applied_graph_revision_seq,
        pending_graph_revisions,
        run_config: events
            .iter()
            .find_map(|event| match &event.payload.payload {
                EventPayload::RunStarted { config, .. } => Some(config.clone()),
                _ => None,
            }),
        terminal_verdict: ReadEvent::terminal_verdict(&events),
    })
}

/// Only the run memory at `position`: the fold, without checkpoint or route
/// validation (the MCP cleanup check needs memory alone).
///
/// # Errors
/// Storage, position or corrupt-timestamp errors.
#[must_use = "the folded memory is the result"]
pub async fn load_memory(
    reader: &RunReader,
    position: Position,
) -> Result<RunMemory, ExecutionStateError> {
    let n = resolve_position(reader, position).await?;
    let events = reader
        .read_events(EventSeq(1)..EventSeq(n).next())
        .await
        .map_err(storage)?;
    fold(*reader.run_id(), &events)
}

/// The graph in force over `events`: the first `PipelineMaterialized`, then
/// the latest `GraphRevisionAccepted` at or below `applied_revision_seq`. A
/// pure helper, so the scrubber can draw a log whose checkpoint is unusable.
#[must_use]
pub fn graph_in_force(events: &[ReadEvent], applied_revision_seq: u64) -> Option<Graph> {
    let mut selected: Option<&Graph> = None;
    for event in events {
        match &event.payload.payload {
            EventPayload::PipelineMaterialized { graph, graph_hash } if selected.is_none() => {
                tracing::debug!(target: "engine::execution_state", seq = event.seq.as_u64(),
                    %graph_hash, "graph in force: first materialization");
                selected = Some(graph);
            },
            EventPayload::GraphRevisionAccepted {
                graph, graph_hash, ..
            } if event.seq.as_u64() <= applied_revision_seq && selected.is_some() => {
                tracing::debug!(target: "engine::execution_state", seq = event.seq.as_u64(),
                    %graph_hash, "graph in force: applied revision");
                selected = Some(graph);
            },
            _ => {},
        }
    }
    selected.map(|graph| (*graph).clone())
}

/// Graph revisions accepted after `applied_seq`, in seq order.
#[must_use]
pub(crate) fn pending_revisions(
    events: &[ReadEvent],
    applied_seq: u64,
) -> Vec<PendingGraphRevision> {
    events
        .iter()
        .filter(|event| event.seq.as_u64() > applied_seq)
        .filter_map(|event| match &event.payload.payload {
            EventPayload::GraphRevisionAccepted {
                patch_id,
                target,
                previous_graph_hash,
                graph,
                graph_hash,
                active_pickup,
            } => Some(PendingGraphRevision {
                seq: event.seq.as_u64(),
                patch_id: patch_id.clone(),
                target: target.clone(),
                previous_graph_hash: *previous_graph_hash,
                graph: (**graph).clone(),
                graph_hash: *graph_hash,
                active_pickup: *active_pickup,
            }),
            _ => None,
        })
        .collect()
}

/// Fold `events` into memory through the shared, clock-free conversion.
fn fold(
    run_id: surge_core::id::RunId,
    events: &[ReadEvent],
) -> Result<RunMemory, ExecutionStateError> {
    let mut memory = RunMemory::default();
    for event in events {
        memory.apply_event(&event.to_run_event(run_id)?);
    }
    Ok(memory)
}

async fn resolve_position(
    reader: &RunReader,
    position: Position,
) -> Result<u64, ExecutionStateError> {
    let latest = reader.current_seq().await.map_err(storage)?.as_u64();
    match position {
        Position::Latest if latest == 0 => Err(ExecutionStateError::EmptyLog),
        Position::Latest => Ok(latest),
        Position::AtSeq(0) => Err(ExecutionStateError::PositionZero),
        Position::AtSeq(n) if n > latest => Err(ExecutionStateError::PositionBeyond {
            position: n,
            latest,
        }),
        Position::AtSeq(n) => Ok(n),
    }
}

/// A consumed route requires its durable post-route execution checkpoint:
/// falling back to the graph start would re-execute committed effects.
async fn validate_consumed_route(
    reader: &RunReader,
    events: &[ReadEvent],
    memory: &RunMemory,
    checkpoint_seq: Option<u64>,
) -> Result<(), ExecutionStateError> {
    let Some(route_seq) = memory
        .committed_stage_outcomes
        .values()
        .filter_map(|record| record.routed_seq)
        .max()
    else {
        return Ok(());
    };
    if checkpoint_seq.is_none_or(|seq| seq < route_seq) {
        return Err(ExecutionStateError::ConsumedRouteWithoutCheckpoint { route_seq });
    }
    let routed_target = events.iter().find_map(|event| {
        if event.seq.as_u64().checked_add(2) != Some(route_seq) {
            return None;
        }
        match event.payload.payload() {
            EventPayload::EdgeTraversed { to, .. } => Some(to),
            _ => None,
        }
    });
    let contradicted = ExecutionStateError::ConsumedRouteContradicted { route_seq };
    let (seq, blob) = reader
        .latest_snapshot_at_or_before(EventSeq(route_seq))
        .await
        .map_err(storage)?
        .ok_or(ExecutionStateError::ConsumedRouteWithoutCheckpoint { route_seq })?;
    let snapshot = EngineSnapshot::deserialize(&blob).map_err(|_| contradicted.clone())?;
    let cursor = snapshot
        .cursor
        .into_cursor()
        .map_err(|_| contradicted.clone())?;
    if seq.as_u64() != route_seq
        || snapshot.at_seq != route_seq
        || snapshot.stage_boundary_seq != route_seq
        || routed_target.is_none_or(|target| cursor.node != *target)
    {
        return Err(contradicted);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
