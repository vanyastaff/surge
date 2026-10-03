//! Pure declared-edge selection shared by execution and trusted recovery.
use crate::{
    edge::{Edge, EdgeKind, ExceededAction},
    keys::{EdgeKey, NodeKey, OutcomeKey},
};
use std::collections::HashMap;
use thiserror::Error;

/// Output of [`select_edge_with_counters`].
///
/// Bundles the chosen target node together with the matching edge's id and
/// kind so callers can emit `EventPayload::EdgeTraversed { kind, .. }` and
/// drive backtrack-aware bookkeeping (e.g. `RunMemory.node_visits`) without
/// re-scanning the graph for the same edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedEdge {
    /// Target node the engine cursor advances to next.
    pub target: NodeKey,
    /// Identifier of the edge that produced this routing decision.
    pub edge_id: EdgeKey,
    /// Edge kind selected by the routing pass — `Forward` for normal
    /// progression, `Backtrack` for `HumanGate` edit-loop re-entry,
    /// `Escalate` reserved for max-traversal escalation paths.
    pub kind: EdgeKind,
}

/// Errors that can occur when determining the next node after a stage outcome.
#[derive(Debug, Error, PartialEq)]
pub enum RoutingError {
    /// A durable counter cannot represent another traversal.
    #[error("edge {edge} traversal counter overflow")]
    CounterOverflow { edge: EdgeKey },
    /// The built-in escalation outcome could not be constructed.
    #[error("invalid built-in escalation outcome")]
    InvalidEscalationOutcome,
    /// No edge in the graph matches the `(from_node, outcome)` pair.
    #[error("no edge from node {from} matches outcome {outcome}")]
    NoMatchingEdge {
        /// Source node key.
        from: NodeKey,
        /// Outcome key that produced no match.
        outcome: OutcomeKey,
    },
    /// More than one edge matches — parallel fan-out requires M6.
    #[error("multiple edges from node {from} match outcome {outcome} (parallel fan-out — M6)")]
    MultipleMatches {
        /// Source node key.
        from: NodeKey,
        /// Outcome key that matched multiple edges.
        outcome: OutcomeKey,
    },
    /// Edge traversal limit (`EdgePolicy::max_traversals`) exceeded.
    /// `action` reports which branch the routing path follows next:
    /// `Escalate` (synthesise a `max_traversals_exceeded` outcome and
    /// re-route) or `Fail` (halt the run).
    #[error("edge {edge} max_traversals exceeded ({count}/{max}) — action: {action:?}")]
    ExceededTraversal {
        /// `EdgeKey` of the edge that exceeded the limit.
        edge: EdgeKey,
        /// Current traversal counter value (post-increment).
        count: u32,
        /// Configured maximum from `EdgePolicy::max_traversals`.
        max: u32,
        /// Action determined by `EdgePolicy::on_max_exceeded`.
        action: ExceededAction,
    },
}

/// Choose the first declared matching edge, incrementing before testing its limit.
/// The caller supplies the authoritative active graph scope and counter owner.
pub fn select_edge_with_counters(
    edges: &[Edge],
    current: &NodeKey,
    outcome: &OutcomeKey,
    counts: &mut HashMap<EdgeKey, u32>,
) -> Result<RoutedEdge, RoutingError> {
    let edge = edges
        .iter()
        .find(|edge| &edge.from.node == current && &edge.from.outcome == outcome)
        .ok_or_else(|| RoutingError::NoMatchingEdge {
            from: current.clone(),
            outcome: outcome.clone(),
        })?;
    let count = counts.entry(edge.id.clone()).or_insert(0);
    *count = count
        .checked_add(1)
        .ok_or_else(|| RoutingError::CounterOverflow {
            edge: edge.id.clone(),
        })?;
    if let Some(max) = edge.policy.max_traversals
        && *count > max
    {
        return Err(RoutingError::ExceededTraversal {
            edge: edge.id.clone(),
            count: *count,
            max,
            action: edge.policy.on_max_exceeded,
        });
    }
    Ok(RoutedEdge {
        target: edge.to.clone(),
        edge_id: edge.id.clone(),
        kind: edge.kind,
    })
}

/// Apply the execution rule including a declared max-traversal escalation route.
/// Escalation retains the failed original increment and increments the alternate.
pub fn resolve_stage_route(
    edges: &[Edge],
    current: &NodeKey,
    outcome: &OutcomeKey,
    counts: &mut HashMap<EdgeKey, u32>,
) -> Result<RoutedEdge, RoutingError> {
    match select_edge_with_counters(edges, current, outcome, counts) {
        Err(RoutingError::ExceededTraversal {
            action: ExceededAction::Escalate,
            ..
        }) => {
            let synthetic = OutcomeKey::try_from("max_traversals_exceeded")
                .map_err(|_| RoutingError::InvalidEscalationOutcome)?;
            select_edge_with_counters(edges, current, &synthetic, counts)
        },
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::{EdgePolicy, PortRef};

    fn edge(id: &str, outcome: &str, target: &str, max: Option<u32>) -> Edge {
        Edge {
            id: id.parse().unwrap(),
            from: PortRef {
                node: "gate".parse().unwrap(),
                outcome: outcome.parse().unwrap(),
            },
            to: target.parse().unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy {
                max_traversals: max,
                ..EdgePolicy::default()
            },
        }
    }

    #[test]
    fn declared_first_match_and_escalation_preserve_both_counter_increments() {
        let edges = vec![
            edge("edit_edge", "edit", "implement", Some(1)),
            edge("duplicate_edge", "edit", "unexpected", None),
            edge("escalate_edge", "max_traversals_exceeded", "operator", None),
        ];
        let mut counts = HashMap::new();
        let node = "gate".parse().unwrap();
        let outcome = "edit".parse().unwrap();
        let first = resolve_stage_route(&edges, &node, &outcome, &mut counts).unwrap();
        assert_eq!(first.target.as_str(), "implement");
        assert_eq!(counts.get(&edges[0].id), Some(&1));
        assert!(!counts.contains_key(&edges[1].id));
        let second = resolve_stage_route(&edges, &node, &outcome, &mut counts).unwrap();
        assert_eq!(second.target.as_str(), "operator");
        assert_eq!(second.edge_id.as_str(), "escalate_edge");
        assert_eq!(counts.get(&edges[0].id), Some(&2));
        assert_eq!(counts.get(&edges[2].id), Some(&1));
    }

    #[test]
    fn overflow_is_a_refusal_without_wrapping_durable_authority() {
        let edges = [edge("forward", "approve", "done", None)];
        let mut counts = HashMap::from([(edges[0].id.clone(), u32::MAX)]);
        assert!(matches!(
            resolve_stage_route(
                &edges,
                &"gate".parse().unwrap(),
                &"approve".parse().unwrap(),
                &mut counts
            ),
            Err(RoutingError::CounterOverflow { .. })
        ));
        assert_eq!(counts.get(&edges[0].id), Some(&u32::MAX));
    }
}
