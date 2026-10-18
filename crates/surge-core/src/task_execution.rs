//! Read-only task, stage and provider history projected from journal envelopes.
//!
//! These identities identify observations, never grant runtime authority. No
//! projected completion, split, or session closure substitutes for verification.
mod compile;

use crate::{
    NodeKey, RunEvent, RunId, SessionId,
    id::StageInvocationId,
    roadmap::{RoadmapStatus, RoadmapTaskId},
};
use serde::{Deserialize, Serialize};

/// Stable identity anchored to the first recorded evidence of an occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOccurrenceId {
    /// Journal run identity.
    pub run: RunId,
    /// First evidence sequence, preserved when discovery precedes execution.
    pub seq: u64,
    /// Item ordinal when one event records several children.
    pub ordinal: u32,
}

/// One recorded iteration, with identity independent of its display index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IterationOccurrence {
    /// Loop entry event sequence.
    pub seq: u64,
    /// Actual loop node.
    pub loop_id: NodeKey,
    /// Recorded iteration index.
    pub index: u32,
    /// Recorded item, including milestone/task context when available.
    pub item: toml::Value,
    /// Outer iteration identities, in order.
    pub parents: Vec<u64>,
    /// Completion record, when observed.
    pub completed: Option<(u64, String)>,
}

/// Recorded occurrence path or explicitly unavailable attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskScope {
    /// A proven loop path; an empty path is the recorded root scope.
    Recorded(Vec<u64>),
    /// Inner anchors recorded after a gap; the omitted outer prefix is unknown.
    Partial { after: u64, observed: Vec<u64> },
    /// The journal does not establish a unique occurrence owner.
    Unassigned,
}
impl TaskScope {
    /// Recorded path, without converting unknown attribution into a root scope.
    #[must_use]
    pub fn recorded(&self) -> Option<&[u64]> {
        match self {
            Self::Recorded(path) => Some(path),
            Self::Partial { .. } | Self::Unassigned => None,
        }
    }
    fn observed(&self) -> Option<&[u64]> {
        match self {
            Self::Recorded(path) | Self::Partial { observed: path, .. } => Some(path),
            Self::Unassigned => None,
        }
    }
    fn with_path(&self, path: Vec<u64>) -> Self {
        match self {
            Self::Recorded(_) => Self::Recorded(path),
            Self::Partial { after, .. } => Self::Partial {
                after: *after,
                observed: path,
            },
            Self::Unassigned => Self::Unassigned,
        }
    }
}

/// A task occurrence, distinct from another task with the same roadmap ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskExecution {
    /// Stable first-evidence identity.
    pub id: TaskOccurrenceId,
    /// Recorded roadmap identifier.
    pub task_id: RoadmapTaskId,
    /// Recorded title.
    pub title: String,
    /// Description recorded with an item or split.
    pub description: Option<String>,
    /// Criteria recorded with an item or split.
    pub acceptance_criteria: Vec<String>,
    /// Recorded loop occurrence path (not guessed from task identifiers).
    pub scope: TaskScope,
    /// Real nested task parent; split replacements are siblings.
    pub parent: Option<TaskOccurrenceId>,
    /// Discovery lineage identifier, which alone does not establish nesting.
    pub discovered_from: Option<RoadmapTaskId>,
    /// Exact originating occurrence when unambiguous in the recorded scope.
    pub split_origin: Option<TaskOccurrenceId>,
    /// Discovery event sequence.
    pub discovered: Option<u64>,
    /// Committed TaskSplit ordering fact; distinct from route consumption.
    pub split: Option<u64>,
    /// First iteration-start record; absent for undispatched discoveries.
    pub started: Option<u64>,
    /// Iteration completion record.
    pub finished: Option<(u64, String)>,
    /// First journal gap after which an unfinished iteration may have changed.
    pub continuity_unknown: Option<u64>,
    /// A recorded TaskSplit replaced this task.
    pub replaced: bool,
    /// Most recent scoped ledger state, without promoting lifecycle success.
    pub status: RoadmapStatus,
    /// Current revision-bound verification claim, using the shared claim gate.
    pub verified: bool,
}

/// A stage occurrence is identified by its actual StageEntered sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageExecution {
    /// StageEntered journal sequence.
    pub entered: u64,
    /// Actual graph node.
    pub node: NodeKey,
    /// Display attempt counter, never an identity.
    pub attempt: u32,
    /// Task owner, when recorded scope establishes it.
    pub task: Option<TaskOccurrenceId>,
    /// Full iteration path, including milestone wiring.
    pub scope: TaskScope,
    /// Stage lifecycle result, without implying settlement or verification.
    pub result: Option<(u64, String)>,
    /// First journal gap after which an unfinished stage may have changed.
    pub continuity_unknown: Option<u64>,
    /// Recorded failure diagnostic.
    pub failure: Option<String>,
    /// Most recent agent summary.
    pub summary: Option<String>,
    /// Provider connections actually opened for this occurrence.
    pub sessions: Vec<SessionExecution>,
    /// Host accepted outcome markers.
    pub accepted: Vec<AcceptedOutcome>,
    /// Names of resolved inputs, without exposing their content.
    pub inputs: Option<Vec<String>>,
    /// Names of skills actually bound.
    pub skills: Vec<String>,
    /// Whether an observed human-input request remains unanswered.
    pub waiting: bool,
    /// Quota-stage identity recorded against this exact StageEntered sequence.
    pub quota_invocation: Option<StageInvocationId>,
}

/// Distinct recorded connection, authority and provider invocation identities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionExecution {
    /// SessionOpened sequence.
    pub opened: u64,
    /// Bridge connection handle from SessionOpened.
    pub connection: SessionId,
    /// Actual provider metadata; legacy absence remains unknown.
    pub provider: Option<crate::execution_recovery::OpenedSession>,
    /// MCP authority from the matching establishment request.
    pub authority: Option<crate::stage_tool::StageToolContext>,
    /// Logical invocation explicitly recorded by establishment.
    pub invocation: Option<StageInvocationId>,
    /// Quota opening operation, when explicitly present.
    pub handoff: Option<crate::id::WorkItemOperationId>,
    /// Recorded profile.
    pub profile: String,
    /// Recorded runtime; profile identity is not runtime identity.
    pub runtime: Option<String>,
    /// SessionClosed fact only; absent closure remains unknown.
    pub closed: Option<(u64, crate::run_event::SessionDisposition)>,
    /// Exact ExecutionWriterClosed record; absent means closure is unconfirmed.
    pub writer_closed: Option<u64>,
    /// Exact best-effort group cleanup record; never confirmed writer closure.
    pub group_stopped: Option<u64>,
}

/// Acceptance and exact route consumption of the same invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceptedOutcome {
    /// Actual StageOutcomeCommitted sequence.
    pub seq: u64,
    /// Validated recorded acceptance marker.
    pub commit: crate::execution_recovery::commit::StageOutcomeCommit,
    /// Unambiguous matching route sequence; withdrawn after conflicting history.
    pub routed: Option<u64>,
    /// Exact-ID route records retained even when their certainty is withdrawn.
    pub route_records: Vec<u64>,
    /// Contradictory acceptance or route markers were recorded for this invocation.
    pub conflicting: bool,
}

/// A coverage limitation; absence of evidence remains visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionCaveat {
    /// Slice begins after sequence one or contains a sequence gap.
    PartialJournal { after: u64, before: u64 },
    /// The log lacks modern provider/authority metadata.
    LegacySession { seq: u64 },
    /// A record cannot be assigned uniquely without guessing.
    Unassigned { seq: u64, fact: String },
    /// Route consumption conflicts with or lacks its exact acceptance marker.
    ConflictingRoute { seq: u64 },
    /// Contradictory acceptance/route history withdraws invocation route certainty.
    ConflictingInvocation {
        seq: u64,
        invocation: StageInvocationId,
    },
}

impl std::fmt::Display for ExecutionCaveat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PartialJournal { after, before } => write!(
                formatter,
                "Journal coverage is incomplete between events {after} and {before}."
            ),
            Self::LegacySession { seq } => write!(
                formatter,
                "Event {seq} omits provider or tool-authority identity; attribution remains unknown."
            ),
            Self::Unassigned { seq, fact } => {
                write!(formatter, "Event {seq} cannot be assigned: {fact}.")
            },
            Self::ConflictingInvocation { seq, invocation } => write!(
                formatter,
                "Event {seq} conflicts with invocation {invocation}; route certainty is withdrawn."
            ),
            Self::ConflictingRoute { seq } => write!(
                formatter,
                "Event {seq} has no unique matching unconsumed acceptance marker."
            ),
        }
    }
}

/// Invalid journal envelope; callers must present unavailability explicitly.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskExecutionError {
    #[error(transparent)]
    Fold(#[from] crate::run_state::FoldError),
    /// Every supplied envelope must belong to the requested run.
    #[error("event {seq} belongs to another run")]
    ForeignRun { seq: u64 },
    /// Sequences must be nonzero and strictly increasing.
    #[error("invalid event sequence {seq} after {previous}")]
    Sequence { seq: u64, previous: u64 },
}

/// Shared terminal-preserving historical execution projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskExecutionView {
    /// Exact journal run.
    pub run: RunId,
    /// Tasks in observed execution/split order; no scheduler decisions implied.
    pub tasks: Vec<TaskExecution>,
    /// All stage occurrences, retaining retries after terminal events.
    pub stages: Vec<StageExecution>,
    /// Actual loop iterations in journal order.
    pub iterations: Vec<IterationOccurrence>,
    /// Coverage gaps and attribution uncertainties.
    pub caveats: Vec<ExecutionCaveat>,
    /// Terminal event, when observed; never evidence of verified task completion.
    pub terminal: Option<(u64, String)>,
    /// Shared recovery history; absent legacy data is explicitly not loaded.
    #[serde(default)]
    pub recovery: crate::recovery_observation::RecoveryObservation,
}
impl TaskExecutionView {
    /// Compile only recorded envelopes, without I/O, randomness or live state.
    ///
    /// # Errors
    /// Returns an error for foreign runs or zero, duplicate, or descending sequences.
    pub fn compile(run: RunId, events: &[RunEvent]) -> Result<Self, TaskExecutionError> {
        compile::compile(run, events)
    }
}

#[cfg(test)]
mod tests;
