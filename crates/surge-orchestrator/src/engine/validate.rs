//! Pre-execution graph validation: rejects M6+ features and structural
//! errors (start node missing, edges referencing unknown nodes).

use crate::engine::error::EngineError;
use surge_core::graph::Graph;
use surge_core::node::NodeKind;

/// Validate the graph for M5 execution. Returns Ok(()) if it can run.
pub fn validate_for_m5(graph: &Graph) -> Result<(), EngineError> {
    if !graph.nodes.contains_key(&graph.start) {
        return Err(EngineError::GraphInvalid(format!(
            "start node '{}' not present in nodes",
            graph.start
        )));
    }

    for (key, node) in &graph.nodes {
        match node.kind() {
            NodeKind::Loop | NodeKind::Subgraph => {
                return Err(EngineError::UnsupportedNodeKind { kind: node.kind() });
            }
            _ => {}
        }
        if &node.id != key {
            return Err(EngineError::GraphInvalid(format!(
                "node id {} differs from map key {}",
                node.id, key
            )));
        }
    }

    for edge in &graph.edges {
        if !graph.nodes.contains_key(&edge.from.node) {
            return Err(EngineError::GraphInvalid(format!(
                "edge {} references unknown source node {}",
                edge.id, edge.from.node
            )));
        }
        if !graph.nodes.contains_key(&edge.to) {
            return Err(EngineError::GraphInvalid(format!(
                "edge {} references unknown target node {}",
                edge.id, edge.to
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::NodeKey;
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::BTreeMap;

    fn graph_with_one_terminal(start: &str) -> Graph {
        let key = NodeKey::try_from(start).unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            key.clone(),
            Node {
                id: key.clone(),
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
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: key,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    #[test]
    fn minimal_terminal_graph_is_valid() {
        let g = graph_with_one_terminal("end");
        assert!(validate_for_m5(&g).is_ok());
    }

    #[test]
    fn missing_start_node_rejected() {
        let mut g = graph_with_one_terminal("end");
        g.start = NodeKey::try_from("nonexistent").unwrap();
        let err = validate_for_m5(&g).unwrap_err();
        match err {
            EngineError::GraphInvalid(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected GraphInvalid, got {other:?}"),
        }
    }

    #[test]
    fn loop_node_rejected_as_unsupported() {
        use surge_core::loop_config::{ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode};
        use surge_core::keys::SubgraphKey;

        let key = NodeKey::try_from("loop1").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            key.clone(),
            Node {
                id: key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Loop(LoopConfig {
                    iterates_over: IterableSource::Static(vec![]),
                    body: SubgraphKey::try_from("body").unwrap(),
                    iteration_var_name: "item".into(),
                    exit_condition: ExitCondition::AllItems,
                    on_iteration_failure: FailurePolicy::default(),
                    parallelism: ParallelismMode::default(),
                    gate_after_each: false,
                }),
            },
        );
        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: key,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };
        let err = validate_for_m5(&g).unwrap_err();
        assert!(matches!(
            err,
            EngineError::UnsupportedNodeKind { kind: NodeKind::Loop }
        ));
    }
}
