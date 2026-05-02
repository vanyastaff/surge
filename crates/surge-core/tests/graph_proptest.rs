//! Property-based tests for graph types and validation.

use proptest::prelude::*;
use std::collections::BTreeMap;
use surge_core::{
    edge::{Edge, EdgeKind, EdgePolicy, PortRef},
    graph::{Graph, GraphMetadata, SCHEMA_VERSION},
    keys::{EdgeKey, NodeKey, OutcomeKey},
    node::{Node, NodeConfig, OutcomeDecl, Position},
    notify_config::{NotifyChannel, NotifyConfig, NotifySeverity, NotifyTemplate},
    terminal_config::{TerminalConfig, TerminalKind},
    validate,
};

fn arb_node_key() -> impl Strategy<Value = NodeKey> {
    "[a-z][a-z0-9_]{0,15}".prop_map(|s| NodeKey::try_from(s.as_str()).unwrap())
}

fn arb_outcome_key() -> impl Strategy<Value = OutcomeKey> {
    "[a-z][a-z0-9_]{0,15}".prop_map(|s| OutcomeKey::try_from(s.as_str()).unwrap())
}

#[allow(dead_code)]
fn arb_edge_key() -> impl Strategy<Value = EdgeKey> {
    "[a-z][a-z0-9_]{0,15}".prop_map(|s| EdgeKey::try_from(s.as_str()).unwrap())
}

fn notify_filler() -> NodeConfig {
    NodeConfig::Notify(NotifyConfig {
        channel: NotifyChannel::Desktop,
        template: NotifyTemplate {
            severity: NotifySeverity::Info,
            title: "filler".into(),
            body: "filler".into(),
            artifacts: vec![],
        },
        on_failure: Default::default(),
    })
}

/// Generates a valid linear graph: start → node1 → ... → nodeN → terminal.
fn arb_linear_graph(min_inner: usize, max_inner: usize) -> impl Strategy<Value = Graph> {
    (min_inner..=max_inner).prop_flat_map(|n_inner| {
        prop::collection::vec(arb_node_key(), n_inner + 2)
            .prop_filter("unique node keys", |keys| {
                let set: std::collections::HashSet<_> = keys.iter().collect();
                set.len() == keys.len()
            })
            .prop_map(build_linear_graph)
    })
}

fn build_linear_graph(keys: Vec<NodeKey>) -> Graph {
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();
    let done_outcome = OutcomeKey::try_from("done").unwrap();

    for (i, k) in keys.iter().enumerate() {
        let is_last = i == keys.len() - 1;
        let config = if is_last {
            NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            })
        } else {
            notify_filler()
        };
        let outcomes = if is_last {
            vec![]
        } else {
            vec![OutcomeDecl {
                id: done_outcome.clone(),
                description: "Forward".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
            }]
        };
        nodes.insert(k.clone(), Node {
            id: k.clone(),
            position: Position::default(),
            declared_outcomes: outcomes,
            config,
        });
        if !is_last {
            let next = &keys[i + 1];
            edges.push(Edge {
                id: EdgeKey::try_from(format!("e_{i}").as_str()).unwrap(),
                from: PortRef { node: k.clone(), outcome: done_outcome.clone() },
                to: next.clone(),
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            });
        }
    }
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "proptest".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: keys[0].clone(),
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn valid_linear_graphs_pass_validation(g in arb_linear_graph(1, 8)) {
        let result = validate(&g);
        prop_assert!(result.is_ok(), "expected valid graph to pass: {:?}", result);
    }

    #[test]
    fn graphs_with_missing_start_fail(mut g in arb_linear_graph(2, 5)) {
        g.start = NodeKey::try_from("nonexistent").unwrap();
        let result = validate(&g);
        prop_assert!(result.is_err());
    }

    #[test]
    fn toml_roundtrip_preserves_graph(g in arb_linear_graph(1, 5)) {
        let toml_s = toml::to_string(&g).unwrap();
        let parsed: Graph = toml::from_str(&toml_s).unwrap();
        prop_assert_eq!(g, parsed);
    }
}

// Silence unused import warning for arb_outcome_key since it's exported but unused
// in the current property set (kept for future extensions).
#[allow(dead_code)]
fn _ensure_arb_outcome_key_compiles() -> impl Strategy<Value = OutcomeKey> {
    arb_outcome_key()
}
