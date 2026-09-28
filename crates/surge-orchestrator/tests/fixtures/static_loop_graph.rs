use std::collections::BTreeMap;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, SubgraphKey};
use surge_core::loop_config::{
    ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};

pub fn build_static_loop_graph() -> Graph {
    // Nodes:
    //   loop_1 (Loop: 3 static items, body = "body_sg") -> on completion -> end
    //   end (Terminal::Success)
    //
    // Subgraph "body_sg":
    //   body_end (Terminal::Success)
    //
    // The Loop node routes its `completed` outcome to `end`.

    let loop_key = NodeKey::try_from("loop_1").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();
    let body_sg_key = SubgraphKey::try_from("body_sg").unwrap();
    let body_end_key = NodeKey::try_from("body_end").unwrap();
    let done_outcome = OutcomeKey::try_from("completed").unwrap();

    let loop_node = Node {
        id: loop_key.clone(),
        position: Position::default(),
        // Declared to match `edge_loop_to_end` below — `surge_core::validate`
        // rejects an edge whose source outcome the node never declared.
        declared_outcomes: vec![OutcomeDecl {
            id: done_outcome.clone(),
            description: "all iterations completed".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: Default::default(),
        }],
        config: NodeConfig::Loop(LoopConfig {
            iterates_over: IterableSource::Static(vec![
                toml::Value::Integer(1),
                toml::Value::Integer(2),
                toml::Value::Integer(3),
            ]),
            body: body_sg_key.clone(),
            iteration_var_name: "item".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Abort,
            parallelism: ParallelismMode::Sequential,
            gate_after_each: false,
        }),
    };

    let end_node = Node {
        id: end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: Some("loop done".into()),
        }),
    };

    let body_end_node = Node {
        id: body_end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    };

    let edge_loop_to_end = Edge {
        id: EdgeKey::try_from("e_loop_done").unwrap(),
        from: PortRef {
            node: loop_key.clone(),
            outcome: done_outcome,
        },
        to: end_key.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    };

    let mut nodes = BTreeMap::new();
    nodes.insert(loop_key.clone(), loop_node);
    nodes.insert(end_key, end_node);

    let mut body_nodes = BTreeMap::new();
    body_nodes.insert(body_end_key.clone(), body_end_node);

    let mut subgraphs = BTreeMap::new();
    subgraphs.insert(
        body_sg_key,
        Subgraph {
            start: body_end_key,
            nodes: body_nodes,
            edges: vec![],
        },
    );

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "static_loop_3".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: loop_key,
        nodes,
        edges: vec![edge_loop_to_end],
        subgraphs,
    }
}
