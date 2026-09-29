use super::*;
use crate::agent_config::AgentConfig;
use crate::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
use crate::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey, SubgraphKey};
use crate::loop_config::{
    ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
};
use crate::node::{LedgerEffect, Node, NodeConfig, OutcomeDecl, Position};
use crate::terminal_config::{TerminalConfig, TerminalKind};
use std::collections::BTreeMap;

fn agent_node(key: &str, effect: LedgerEffect) -> Node {
    Node {
        id: NodeKey::try_from(key).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: OutcomeKey::try_from("pass").unwrap(),
            description: "ok".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: effect,
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: Default::default(),
            hooks: vec![],
            custom_fields: Default::default(),
        }),
    }
}

fn success_terminal(key: &str) -> Node {
    Node {
        id: NodeKey::try_from(key).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn edge(id: &str, from: &str, to: &str) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: NodeKey::try_from(from).unwrap(),
            outcome: OutcomeKey::try_from("pass").unwrap(),
        },
        to: NodeKey::try_from(to).unwrap(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }
}

fn graph(
    start: &str,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    subgraphs: BTreeMap<SubgraphKey, Subgraph>,
) -> Graph {
    let mut map = BTreeMap::new();
    for node in nodes {
        map.insert(node.id.clone(), node);
    }
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "w4".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: NodeKey::try_from(start).unwrap(),
        nodes: map,
        edges,
        subgraphs,
    }
}

fn w4_warnings(graph: &Graph) -> Vec<NodeKey> {
    let mut out = Vec::new();
    warning_w4_unverified_success(graph, &mut out);
    out.into_iter()
        .filter_map(|f| match f.kind {
            ValidationErrorKind::UnverifiedSuccessPath { terminal } => Some(terminal),
            _ => None,
        })
        .collect()
}

#[test]
fn warns_when_success_has_no_verifier() {
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", LedgerEffect::None),
            success_terminal("end"),
        ],
        vec![edge("e", "impl_1", "end")],
        BTreeMap::new(),
    );
    assert_eq!(w4_warnings(&g), vec![NodeKey::try_from("end").unwrap()]);
}

#[test]
fn no_warning_when_verifier_gates_success() {
    // impl_1 (none) → verify_1 (verified) → end
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", LedgerEffect::None),
            agent_node("verify_1", LedgerEffect::Verified),
            success_terminal("end"),
        ],
        vec![
            edge("e1", "impl_1", "verify_1"),
            edge("e2", "verify_1", "end"),
        ],
        BTreeMap::new(),
    );
    assert!(w4_warnings(&g).is_empty());
}

#[test]
fn warns_when_a_bypass_path_skips_the_verifier() {
    // impl_1 → verify_1 → end, but impl_1 also → end directly (bypass).
    let g = graph(
        "impl_1",
        vec![
            agent_node("impl_1", LedgerEffect::None),
            agent_node("verify_1", LedgerEffect::Verified),
            success_terminal("end"),
        ],
        vec![
            edge("e1", "impl_1", "verify_1"),
            edge("e2", "verify_1", "end"),
            edge("e3", "impl_1", "end"),
        ],
        BTreeMap::new(),
    );
    assert_eq!(w4_warnings(&g), vec![NodeKey::try_from("end").unwrap()]);
}

#[test]
fn loop_body_verifier_counts_as_a_gate() {
    // loop_1 (body = task_body containing a verified node) → end.
    let mut subgraphs = BTreeMap::new();
    let body_key = SubgraphKey::try_from("task_body").unwrap();
    let mut body_nodes = BTreeMap::new();
    let verify = agent_node("verify_in_task", LedgerEffect::Verified);
    body_nodes.insert(verify.id.clone(), verify);
    let body_term = success_terminal("body_end");
    body_nodes.insert(body_term.id.clone(), body_term);
    subgraphs.insert(
        body_key.clone(),
        Subgraph {
            start: NodeKey::try_from("verify_in_task").unwrap(),
            nodes: body_nodes,
            edges: vec![],
        },
    );

    let loop_node = Node {
        id: NodeKey::try_from("loop_1").unwrap(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: OutcomeKey::try_from("pass").unwrap(),
            description: "done".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: LedgerEffect::None,
        }],
        config: NodeConfig::Loop(LoopConfig {
            iterates_over: IterableSource::Static(vec![toml::Value::Integer(1)]),
            body: body_key,
            iteration_var_name: "item".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Abort,
            parallelism: ParallelismMode::Sequential,
            gate_after_each: false,
        }),
    };
    let g = graph(
        "loop_1",
        vec![loop_node, success_terminal("end")],
        vec![edge("e", "loop_1", "end")],
        subgraphs,
    );
    assert!(
        w4_warnings(&g).is_empty(),
        "a loop whose body verifies should gate the outer success terminal"
    );
}
