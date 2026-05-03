//! Edge selection given (current node, outcome).

use surge_core::edge::Edge;
use surge_core::graph::Graph;
use surge_core::keys::{NodeKey, OutcomeKey};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum RoutingError {
    #[error("no edge from node {from} matches outcome {outcome}")]
    NoMatchingEdge { from: NodeKey, outcome: OutcomeKey },
    #[error("multiple edges from node {from} match outcome {outcome} (parallel fan-out — M6)")]
    MultipleMatches { from: NodeKey, outcome: OutcomeKey },
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

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::EdgeKey;
    use std::collections::BTreeMap;

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
}
