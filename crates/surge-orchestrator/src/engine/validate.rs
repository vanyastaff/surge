//! Pre-execution graph validation for M6: allows Loop and Subgraph nodes,
//! rejects `gate_after_each: true` (M7) and multi-edge fanout from the same
//! `(node, outcome)` port (M8+), and validates subgraph references.

use crate::engine::error::EngineError;
use surge_core::graph::Graph;

/// Validate the graph for M6 execution. Allows Loop and Subgraph nodes
/// (M5 rejected them). Rejects multi-edge fanout (M8+) and
/// `gate_after_each: true` (M7).
pub fn validate_for_m6(graph: &Graph) -> Result<(), EngineError> {
    if !graph.nodes.contains_key(&graph.start) {
        return Err(EngineError::GraphInvalid(format!(
            "start node '{}' not present in nodes",
            graph.start
        )));
    }

    // Per-node validation.
    for (key, node) in &graph.nodes {
        if &node.id != key {
            return Err(EngineError::GraphInvalid(format!(
                "node id {} differs from map key {}",
                node.id, key
            )));
        }

        // gate_after_each rejection (deferred to M7).
        if let surge_core::node::NodeConfig::Loop(cfg) = &node.config {
            if cfg.gate_after_each {
                return Err(EngineError::GraphInvalid(format!(
                    "node {key}: gate_after_each = true is not supported in M6 \
                    (deferred to M7 alongside daemon's broadcast registry); \
                    rewrite as an explicit HumanGate node inside the body subgraph"
                )));
            }
            // Loop body subgraph must exist.
            if !graph.subgraphs.contains_key(&cfg.body) {
                return Err(EngineError::LoopBodyMissing(cfg.body.clone()));
            }
        }

        // Subgraph reference must exist.
        if let surge_core::node::NodeConfig::Subgraph(cfg) = &node.config {
            if !graph.subgraphs.contains_key(&cfg.inner) {
                return Err(EngineError::SubgraphMissing(cfg.inner.clone()));
            }
        }
    }

    // Edge validation — outer graph.
    let mut seen_ports: std::collections::HashSet<(
        surge_core::keys::NodeKey,
        surge_core::keys::OutcomeKey,
    )> = std::collections::HashSet::new();
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
        if !seen_ports.insert((edge.from.node.clone(), edge.from.outcome.clone())) {
            return Err(EngineError::GraphInvalid(format!(
                "multiple edges from ({}, {}) — parallel fanout is M8+ scope (NodeKind::Parallel)",
                edge.from.node, edge.from.outcome
            )));
        }
    }

    // Recursively validate inner subgraphs.
    for (key, sg) in &graph.subgraphs {
        if !sg.nodes.contains_key(&sg.start) {
            return Err(EngineError::GraphInvalid(format!(
                "subgraph '{key}' start '{}' not in subgraph nodes",
                sg.start
            )));
        }
        // Inner-subgraph edge port-uniqueness.
        let mut inner_ports: std::collections::HashSet<(
            surge_core::keys::NodeKey,
            surge_core::keys::OutcomeKey,
        )> = std::collections::HashSet::new();
        for edge in &sg.edges {
            if !inner_ports.insert((edge.from.node.clone(), edge.from.outcome.clone())) {
                return Err(EngineError::GraphInvalid(format!(
                    "subgraph '{key}': multiple edges from ({}, {}) — M8+",
                    edge.from.node, edge.from.outcome
                )));
            }
        }
        // Per-node validation inside subgraphs (gate_after_each etc).
        for (node_key, node) in &sg.nodes {
            if let surge_core::node::NodeConfig::Loop(cfg) = &node.config {
                if cfg.gate_after_each {
                    return Err(EngineError::GraphInvalid(format!(
                        "subgraph '{key}' node {node_key}: gate_after_each = true is not supported in M6 (M7)"
                    )));
                }
                if !graph.subgraphs.contains_key(&cfg.body) {
                    return Err(EngineError::LoopBodyMissing(cfg.body.clone()));
                }
            }
            if let surge_core::node::NodeConfig::Subgraph(cfg) = &node.config {
                if !graph.subgraphs.contains_key(&cfg.inner) {
                    return Err(EngineError::SubgraphMissing(cfg.inner.clone()));
                }
            }
        }
    }

    Ok(())
}

// Back-compat alias for any internal caller still using the M5 name.
#[allow(dead_code)]
pub use validate_for_m6 as validate_for_m5;

/// `validate_for_m6` plus the `surge_core::ReferenceResolver` lookups for
/// profiles, templates, and named agents. Engine wiring picks this entry
/// point when a real registry is available; the terminal-only smoke path
/// can still use the no-resolver `validate_for_m6`.
///
/// # Errors
/// - All `validate_for_m6` errors.
/// - [`EngineError::GraphInvalid`] for every reference-resolution failure
///   reported by the supplied resolver, joined into a single message.
pub fn validate_for_m6_with_resolver(
    graph: &Graph,
    resolver: &dyn surge_core::ReferenceResolver,
) -> Result<(), EngineError> {
    validate_for_m6(graph)?;

    // Surge-core's reference checks layer on top of the structural pass —
    // surface only resolver-related diagnostics, since the rest are already
    // covered by validate_for_m6.
    if let Err(findings) = surge_core::validate_with_resolver(graph, resolver) {
        let resolver_failures: Vec<String> = findings
            .into_iter()
            .filter_map(|f| match f.kind {
                surge_core::ValidationErrorKind::ProfileNotFound { .. }
                | surge_core::ValidationErrorKind::TemplateNotFound { .. }
                | surge_core::ValidationErrorKind::NamedAgentNotFound { .. } => Some(f.message),
                _ => None,
            })
            .collect();
        if !resolver_failures.is_empty() {
            return Err(EngineError::GraphInvalid(format!(
                "reference resolution failed: {}",
                resolver_failures.join("; ")
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::{NodeKey, OutcomeKey};
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};

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
        assert!(validate_for_m6(&g).is_ok());
    }

    #[test]
    fn missing_start_node_rejected() {
        let mut g = graph_with_one_terminal("end");
        g.start = NodeKey::try_from("nonexistent").unwrap();
        let err = validate_for_m6(&g).unwrap_err();
        match err {
            EngineError::GraphInvalid(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected GraphInvalid, got {other:?}"),
        }
    }

    #[test]
    fn loop_node_no_longer_rejected() {
        use surge_core::graph::Subgraph;
        use surge_core::keys::SubgraphKey;
        use surge_core::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };

        let loop_key = NodeKey::try_from("loop_1").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("body_start").unwrap();

        let mut nodes = BTreeMap::new();
        nodes.insert(
            loop_key.clone(),
            Node {
                id: loop_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Loop(LoopConfig {
                    iterates_over: IterableSource::Static(vec![]),
                    body: body_key.clone(),
                    iteration_var_name: "item".into(),
                    exit_condition: ExitCondition::AllItems,
                    on_iteration_failure: FailurePolicy::Abort,
                    parallelism: ParallelismMode::Sequential,
                    gate_after_each: false,
                }),
            },
        );

        let mut body_nodes = BTreeMap::new();
        body_nodes.insert(
            body_start.clone(),
            Node {
                id: body_start.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            body_key,
            Subgraph {
                start: body_start,
                nodes: body_nodes,
                edges: vec![],
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
            start: loop_key,
            nodes,
            edges: vec![],
            subgraphs,
        };

        assert!(validate_for_m6(&g).is_ok(), "Loop nodes are allowed in M6");
    }

    #[test]
    fn gate_after_each_true_is_rejected() {
        use surge_core::graph::Subgraph;
        use surge_core::keys::SubgraphKey;
        use surge_core::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };

        let loop_key = NodeKey::try_from("loop_1").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("body_start").unwrap();

        let mut nodes = BTreeMap::new();
        nodes.insert(
            loop_key.clone(),
            Node {
                id: loop_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Loop(LoopConfig {
                    iterates_over: IterableSource::Static(vec![]),
                    body: body_key.clone(),
                    iteration_var_name: "item".into(),
                    exit_condition: ExitCondition::AllItems,
                    on_iteration_failure: FailurePolicy::Abort,
                    parallelism: ParallelismMode::Sequential,
                    gate_after_each: true,
                }),
            },
        );

        let mut body_nodes = BTreeMap::new();
        body_nodes.insert(
            body_start.clone(),
            Node {
                id: body_start.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            body_key,
            Subgraph {
                start: body_start,
                nodes: body_nodes,
                edges: vec![],
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
            start: loop_key,
            nodes,
            edges: vec![],
            subgraphs,
        };

        let err = validate_for_m6(&g).unwrap_err();
        let msg = match err {
            EngineError::GraphInvalid(s) => s,
            other => panic!("expected GraphInvalid, got {other:?}"),
        };
        assert!(
            msg.contains("gate_after_each"),
            "error mentions gate_after_each: {msg}"
        );
        assert!(msg.contains("M7"), "error mentions M7 pointer: {msg}");
    }

    #[test]
    fn multi_edge_same_port_rejected_with_m8_pointer() {
        use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
        use surge_core::keys::EdgeKey;

        let n_a = NodeKey::try_from("a").unwrap();
        let n_b = NodeKey::try_from("b").unwrap();
        let n_c = NodeKey::try_from("c").unwrap();
        let mut nodes = BTreeMap::new();
        for k in [&n_a, &n_b, &n_c] {
            nodes.insert(
                k.clone(),
                Node {
                    id: k.clone(),
                    position: Position::default(),
                    declared_outcomes: vec![],
                    config: NodeConfig::Terminal(TerminalConfig {
                        kind: TerminalKind::Success,
                        message: None,
                    }),
                },
            );
        }

        let port = PortRef {
            node: n_a.clone(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        let edges = vec![
            Edge {
                id: EdgeKey::try_from("e1").unwrap(),
                from: port.clone(),
                to: n_b,
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            },
            Edge {
                id: EdgeKey::try_from("e2").unwrap(),
                from: port,
                to: n_c,
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            },
        ];

        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: n_a,
            nodes,
            edges,
            subgraphs: BTreeMap::new(),
        };

        let err = validate_for_m6(&g).unwrap_err();
        let msg = match err {
            EngineError::GraphInvalid(s) => s,
            other => panic!("expected GraphInvalid, got {other:?}"),
        };
        assert!(
            msg.contains("multiple edges"),
            "error mentions multi-edge: {msg}"
        );
        assert!(
            msg.contains("M8") || msg.contains("NodeKind::Parallel"),
            "error mentions M8/Parallel: {msg}"
        );
    }
}
