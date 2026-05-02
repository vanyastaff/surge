use criterion::{criterion_group, criterion_main, Criterion};
use std::collections::BTreeMap;
use surge_core::{
    edge::{Edge, EdgeKind, EdgePolicy, PortRef},
    graph::{Graph, GraphMetadata, Subgraph, SCHEMA_VERSION},
    keys::{EdgeKey, NodeKey, OutcomeKey, SubgraphKey},
    node::{Node, NodeConfig, OutcomeDecl, Position},
    notify_config::{NotifyChannel, NotifyConfig, NotifyFailureAction, NotifySeverity, NotifyTemplate},
    terminal_config::{TerminalConfig, TerminalKind},
    validate,
};

fn make_filler_node(id: NodeKey, done: &OutcomeKey) -> Node {
    Node {
        id: id.clone(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: done.clone(),
            description: "Forward".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }],
        config: NodeConfig::Notify(NotifyConfig {
            channel: NotifyChannel::Desktop,
            template: NotifyTemplate {
                severity: NotifySeverity::Info,
                title: "f".into(),
                body: "f".into(),
                artifacts: vec![],
            },
            on_failure: NotifyFailureAction::Continue,
        }),
    }
}

fn build_n_node_graph(n: usize) -> Graph {
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();
    let done = OutcomeKey::try_from("done").unwrap();

    for i in 0..n {
        let key = NodeKey::try_from(format!("n{i}").as_str()).unwrap();
        let is_last = i == n - 1;
        let node = if is_last {
            Node {
                id: key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            }
        } else {
            make_filler_node(key.clone(), &done)
        };
        nodes.insert(key.clone(), node);
        if !is_last {
            let next = NodeKey::try_from(format!("n{}", i + 1).as_str()).unwrap();
            edges.push(Edge {
                id: EdgeKey::try_from(format!("e{i}").as_str()).unwrap(),
                from: PortRef { node: key, outcome: done.clone() },
                to: next,
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            });
        }
    }

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "bench".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: NodeKey::try_from("n0").unwrap(),
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

fn validate_50_nodes(c: &mut Criterion) {
    let g = build_n_node_graph(50);
    c.bench_function("validate_50_node_graph", |b| {
        b.iter(|| validate(criterion::black_box(&g)).unwrap())
    });
}

fn validate_100_nodes_with_subgraphs(c: &mut Criterion) {
    let mut g = build_n_node_graph(100);
    let done = OutcomeKey::try_from("done").unwrap();
    for s in 0..5 {
        let sk = SubgraphKey::try_from(format!("s{s}").as_str()).unwrap();
        let mut sub_nodes = BTreeMap::new();
        let start = NodeKey::try_from(format!("sn{s}_0").as_str()).unwrap();
        sub_nodes.insert(start.clone(), Node {
            id: start.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        });
        let _ = &done; // suppress unused warning
        g.subgraphs.insert(sk, Subgraph {
            start,
            nodes: sub_nodes,
            edges: vec![],
        });
    }
    c.bench_function("validate_pathological_100_nodes_5_subgraphs", |b| {
        b.iter(|| {
            let _ = validate(criterion::black_box(&g));
        })
    });
}

criterion_group!(benches, validate_50_nodes, validate_100_nodes_with_subgraphs);
criterion_main!(benches);
