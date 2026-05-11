//! Engine error taxonomy.

use std::path::PathBuf;
use surge_core::id::RunId;
use surge_core::node::NodeKind;
use thiserror::Error;

/// Errors that can be returned by `Engine` methods.
#[derive(Debug, Error)]
pub enum EngineError {
    /// A run with this ID is already executing in this process.
    #[error("run is already active in this process: {0}")]
    RunAlreadyActive(RunId),

    /// The submitted graph failed structural validation.
    #[error("graph validation failed: {0}")]
    GraphInvalid(String),

    /// The graph contains a node kind not supported until M6+.
    #[error("graph contains M6+ feature ({kind:?}); not supported in M5")]
    UnsupportedNodeKind {
        /// The unsupported `NodeKind` variant.
        kind: NodeKind,
    },

    /// The provided worktree path does not exist on disk.
    #[error("worktree path does not exist: {0}")]
    WorktreeMissing(PathBuf),

    /// A persistence-layer operation failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An ACP bridge operation failed.
    #[error("bridge error: {0}")]
    Bridge(String),

    /// No active run found for the given `RunId`.
    #[error("run not found: {0}")]
    RunNotFound(RunId),

    /// The selected engine facade does not support the requested operation.
    #[error("engine operation is not supported by this facade: {operation}")]
    OperationNotSupported {
        /// Stable operation name.
        operation: &'static str,
    },

    /// The daemon has no per-run channel registered for this `RunId`.
    /// Returned by daemon paths like `Subscribe`. This does NOT
    /// distinguish "the run terminated already" from "the run was
    /// never hosted here" or "the run is queued and not yet
    /// admitted" — all three present as the same condition on the
    /// wire. Useful so callers (e.g., the CLI's `watch` command) can
    /// branch into a fallback path (such as disk-replay) instead of
    /// propagating an opaque `Internal` error.
    #[error("run not currently active in daemon: {0}")]
    RunNotActive(RunId),

    /// The daemon's `AdmissionController` rejected a `StartRun`
    /// because both the active set and the FIFO queue are at their
    /// configured caps. Carries the operator-facing message produced
    /// by the daemon (e.g. `"queue is full (8/8)"`) so callers can
    /// surface it verbatim.
    ///
    /// Distinct from `Internal` so callers can back off and retry the
    /// `start_run` call without parsing strings.
    #[error("daemon queue full: {0}")]
    QueueFull(String),

    /// An unexpected internal condition occurred.
    #[error("internal engine error: {0}")]
    Internal(String),

    /// A Loop node references a body subgraph that is not in `graph.subgraphs`.
    #[error("loop body reference {0} not found in graph.subgraphs")]
    LoopBodyMissing(surge_core::keys::SubgraphKey),

    /// A Subgraph node references an inner subgraph that is not in `graph.subgraphs`.
    #[error("subgraph reference {0} not found in graph.subgraphs")]
    SubgraphMissing(surge_core::keys::SubgraphKey),

    /// A pure-`Forward` cycle was detected during pre-execution validation.
    /// Cycles are permitted iff at least one edge in the cycle has
    /// `EdgeKind::Backtrack` (deliberate iteration, e.g. bootstrap edit
    /// loops); a `Forward`-only cycle is a livelock and the engine refuses
    /// to start such a run. The `nodes` vector lists the nodes that form
    /// the cycle, in traversal order, with the entry node repeated at the
    /// end for readability (e.g. `[a, b, a]` for `a → b → a`).
    #[error("forward-only cycle detected: {}", format_node_cycle(.nodes))]
    ForwardCycleDetected {
        /// Nodes forming the offending cycle, in traversal order.
        nodes: Vec<surge_core::keys::NodeKey>,
    },

    /// The materialized graph carries a `[metadata.archetype]` block whose
    /// declared archetype does not match the detected topology — e.g., a
    /// `multi-milestone` archetype without a `Loop` over a `roadmap.milestones`
    /// iterable. Surfaced by the post-Flow-Generator validator (Task 11).
    #[error("archetype mismatch: declared {declared}, detected {detected}")]
    ArchetypeMismatch {
        /// Archetype name from `[metadata.archetype]` (kebab-case).
        declared: String,
        /// Human-readable description of what the topology actually looks like.
        detected: String,
    },

    /// A non-bootstrap pipeline graph is missing the `[metadata.archetype]`
    /// block where one is required (e.g., when running the post-Flow-Generator
    /// validator on a freshly materialized graph). Reserved for callers that
    /// want to enforce archetype declarations beyond the bootstrap path; the
    /// post-Flow-Generator hook itself only enforces consistency when the
    /// block is present.
    #[error("archetype block missing: {0}")]
    ArchetypeMissing(String),
}

fn format_node_cycle(nodes: &[surge_core::keys::NodeKey]) -> String {
    nodes
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::node::NodeKind;

    #[test]
    fn display_messages() {
        assert_eq!(
            EngineError::WorktreeMissing(PathBuf::from("/missing")).to_string(),
            "worktree path does not exist: /missing"
        );
        assert!(
            EngineError::UnsupportedNodeKind {
                kind: NodeKind::Loop
            }
            .to_string()
            .contains("Loop")
        );
    }
}
