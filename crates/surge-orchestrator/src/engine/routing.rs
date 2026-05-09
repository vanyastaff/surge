//! Edge selection given (current node, outcome).

use surge_core::edge::{Edge, ExceededAction};
use surge_core::graph::Graph;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use thiserror::Error;

/// Errors that can occur when determining the next node after a stage outcome.
#[derive(Debug, Error, PartialEq)]
pub enum RoutingError {
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

/// Find the next node after `current` produces `outcome`.
///
/// M5 expects a unique edge per `(from_node, outcome)` pair. Multiple matches
/// indicate parallel fan-out, which is M6 scope — surfaced as `MultipleMatches`.
pub fn next_node_after(
    graph: &Graph,
    current: &NodeKey,
    outcome: &OutcomeKey,
) -> Result<NodeKey, RoutingError> {
    let matches: Vec<&Edge> = graph
        .edges
        .iter()
        .filter(|e| &e.from.node == current && &e.from.outcome == outcome)
        .collect();
    match matches.as_slice() {
        [] => Err(RoutingError::NoMatchingEdge {
            from: current.clone(),
            outcome: outcome.clone(),
        }),
        [edge] => Ok(edge.to.clone()),
        _ => Err(RoutingError::MultipleMatches {
            from: current.clone(),
            outcome: outcome.clone(),
        }),
    }
}

/// Edge selection with traversal-counter enforcement. Mutates the
/// counter map; returns `Err(RoutingError::ExceededTraversal)` when
/// the policy threshold is breached.
///
/// `frames` decides which counter scope to use: a top `Frame::Loop`
/// uses its own `traversal_counts` (so loop body edges are isolated
/// per loop), otherwise the `root_counts` map is used.
///
/// `frames` also decides which edge set to search — body subgraph
/// edges if inside a frame, outer graph edges otherwise.
#[allow(clippy::implicit_hasher)] // root_counts must match LoopFrame::traversal_counts (RandomState)
pub fn next_node_after_with_counters(
    graph: &Graph,
    current: &NodeKey,
    outcome: &OutcomeKey,
    frames: &mut [crate::engine::frames::Frame],
    root_counts: &mut std::collections::HashMap<EdgeKey, u32>,
) -> Result<NodeKey, RoutingError> {
    let edges = active_edge_set(graph, frames);

    let edge = edges
        .iter()
        .find(|e| &e.from.node == current && &e.from.outcome == outcome)
        .ok_or_else(|| RoutingError::NoMatchingEdge {
            from: current.clone(),
            outcome: outcome.clone(),
        })?;

    // Clone edge metadata before dropping the immutable borrow on `edges`/`frames`,
    // so that we can take a mutable borrow on `frames` (via `top_loop_mut`) next.
    let edge_id = edge.id.clone();
    let edge_to = edge.to.clone();
    let max_traversals = edge.policy.max_traversals;
    let on_max_exceeded = edge.policy.on_max_exceeded;

    let counts = match crate::engine::frames::top_loop_mut(frames) {
        Some(lf) => &mut lf.traversal_counts,
        None => root_counts,
    };
    let count = counts.entry(edge_id.clone()).or_insert(0);
    *count += 1;

    // No let-chains (workspace MSRV is 1.85; let-chains stable in 1.88).
    if let Some(max) = max_traversals {
        if *count > max {
            return Err(RoutingError::ExceededTraversal {
                edge: edge_id,
                count: *count,
                max,
                action: on_max_exceeded,
            });
        }
    }

    Ok(edge_to)
}

/// Find the outgoing edge target for `(node, outcome)`. If no edge
/// matches, error out. Used at frame-push time to compute `return_to`
/// for Loop/Subgraph frames — they need to know where to advance the
/// outer cursor when the inner subgraph terminates.
pub fn edge_target_after_outcome_or_default(
    graph: &Graph,
    node: &NodeKey,
    outcome: &OutcomeKey,
) -> Result<NodeKey, RoutingError> {
    edge_target_in_edges(&graph.edges, node, outcome)
}

/// Like [`edge_target_after_outcome_or_default`], but resolves against the
/// currently active graph/subgraph edge set. Used when pushing nested Loop or
/// Subgraph frames so their `return_to` points to the correct graph scope.
pub fn edge_target_after_outcome_in_active_graph(
    graph: &Graph,
    node: &NodeKey,
    outcome: &OutcomeKey,
    frames: &[crate::engine::frames::Frame],
) -> Result<NodeKey, RoutingError> {
    edge_target_in_edges(active_edge_set(graph, frames), node, outcome)
}

fn edge_target_in_edges(
    edges: &[Edge],
    node: &NodeKey,
    outcome: &OutcomeKey,
) -> Result<NodeKey, RoutingError> {
    edges
        .iter()
        .find(|e| &e.from.node == node && &e.from.outcome == outcome)
        .map(|e| e.to.clone())
        .ok_or_else(|| RoutingError::NoMatchingEdge {
            from: node.clone(),
            outcome: outcome.clone(),
        })
}

fn active_edge_set<'a>(graph: &'a Graph, frames: &[crate::engine::frames::Frame]) -> &'a [Edge] {
    use crate::engine::frames::Frame;
    match frames.last() {
        None => &graph.edges,
        Some(Frame::Loop(lf)) => graph
            .subgraphs
            .get(&lf.config.body)
            .map_or(&[] as &[Edge], |sg| sg.edges.as_slice()),
        Some(Frame::Subgraph(sf)) => graph
            .subgraphs
            .get(&sf.inner_subgraph)
            .map_or(&[] as &[Edge], |sg| sg.edges.as_slice()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::EdgeKey;

    fn graph_with_edges(edges: Vec<Edge>) -> Graph {
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: NodeKey::try_from("start").unwrap(),
            nodes: BTreeMap::new(),
            edges,
            subgraphs: BTreeMap::new(),
        }
    }

    fn edge(id: &str, from_node: &str, from_outcome: &str, to: &str) -> Edge {
        Edge {
            id: EdgeKey::try_from(id).unwrap(),
            from: PortRef {
                node: NodeKey::try_from(from_node).unwrap(),
                outcome: OutcomeKey::try_from(from_outcome).unwrap(),
            },
            to: NodeKey::try_from(to).unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }
    }

    #[test]
    fn unique_match_returns_target() {
        let g = graph_with_edges(vec![edge("e1", "a", "done", "b")]);
        let next = next_node_after(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
        )
        .unwrap();
        assert_eq!(next, NodeKey::try_from("b").unwrap());
    }

    #[test]
    fn no_match_returns_error() {
        let g = graph_with_edges(vec![edge("e1", "a", "done", "b")]);
        let result = next_node_after(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("retry").unwrap(),
        );
        assert!(matches!(result, Err(RoutingError::NoMatchingEdge { .. })));
    }

    #[test]
    fn multiple_matches_returns_error() {
        let g = graph_with_edges(vec![
            edge("e1", "a", "done", "b"),
            edge("e2", "a", "done", "c"),
        ]);
        let result = next_node_after(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
        );
        assert!(matches!(result, Err(RoutingError::MultipleMatches { .. })));
    }

    // --- next_node_after_with_counters tests ---

    fn edge_with_max(
        id: &str,
        from_node: &str,
        from_outcome: &str,
        to: &str,
        max: Option<u32>,
    ) -> Edge {
        let mut e = edge(id, from_node, from_outcome, to);
        e.policy.max_traversals = max;
        e
    }

    fn edge_with_max_and_fail(
        id: &str,
        from_node: &str,
        from_outcome: &str,
        to: &str,
        max: u32,
    ) -> Edge {
        let mut e = edge(id, from_node, from_outcome, to);
        e.policy.max_traversals = Some(max);
        e.policy.on_max_exceeded = ExceededAction::Fail;
        e
    }

    #[test]
    fn traversal_counter_increments_on_each_call() {
        let g = graph_with_edges(vec![edge_with_max("e1", "a", "done", "b", Some(2))]);
        let mut frames: Vec<crate::engine::frames::Frame> = vec![];
        let mut counts: std::collections::HashMap<EdgeKey, u32> = std::collections::HashMap::new();

        let result = next_node_after_with_counters(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
            &mut frames,
            &mut counts,
        );
        assert_eq!(result.unwrap(), NodeKey::try_from("b").unwrap());
        assert_eq!(
            counts.get(&EdgeKey::try_from("e1").unwrap()).copied(),
            Some(1)
        );
    }

    #[test]
    fn max_traversals_exceeded_with_escalate_returns_error() {
        let g = graph_with_edges(vec![edge_with_max("e1", "a", "done", "b", Some(1))]);
        let mut frames: Vec<crate::engine::frames::Frame> = vec![];
        let mut counts: std::collections::HashMap<EdgeKey, u32> = std::collections::HashMap::new();

        // First call: ok (count = 1, max = 1, count > max is FALSE).
        let _ = next_node_after_with_counters(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
            &mut frames,
            &mut counts,
        );
        // Second call: count goes to 2, > max (1), exceeded.
        let result = next_node_after_with_counters(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
            &mut frames,
            &mut counts,
        );
        match result {
            Err(RoutingError::ExceededTraversal {
                action: ExceededAction::Escalate,
                ..
            }) => {},
            other => panic!("expected ExceededTraversal::Escalate, got {other:?}"),
        }
    }

    #[test]
    fn max_traversals_exceeded_with_fail_returns_error() {
        let g = graph_with_edges(vec![edge_with_max_and_fail("e1", "a", "done", "b", 1)]);
        let mut frames: Vec<crate::engine::frames::Frame> = vec![];
        let mut counts: std::collections::HashMap<EdgeKey, u32> = std::collections::HashMap::new();

        let _ = next_node_after_with_counters(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
            &mut frames,
            &mut counts,
        );
        let result = next_node_after_with_counters(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
            &mut frames,
            &mut counts,
        );
        match result {
            Err(RoutingError::ExceededTraversal {
                action: ExceededAction::Fail,
                ..
            }) => {},
            other => panic!("expected ExceededTraversal::Fail, got {other:?}"),
        }
    }
}
