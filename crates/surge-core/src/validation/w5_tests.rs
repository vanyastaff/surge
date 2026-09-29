use super::*;
use crate::agent_config::AgentConfig;
use crate::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use crate::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use crate::node::{LedgerEffect, Node, NodeConfig, OutcomeDecl, Position};
use crate::terminal_config::{TerminalConfig, TerminalKind};
use std::collections::{BTreeMap, HashMap};

/// Resolver double: profiles always exist, but runtime is only known
/// for names present in `runtimes` — everything else is *unknown*
/// (`None`), matching a real resolver's behavior for an unresolved
/// profile.
struct RuntimeResolver {
    runtimes: HashMap<&'static str, &'static str>,
}

impl ReferenceResolver for RuntimeResolver {
    fn profile_exists(&self, _: &str) -> bool {
        true
    }
    fn template_exists(&self, _: &str) -> bool {
        true
    }
    fn named_agent_exists(&self, _: &str) -> bool {
        true
    }
    fn profile_runtime(&self, name: &str) -> Option<String> {
        self.runtimes.get(name).map(|runtime| (*runtime).to_owned())
    }
}

pub(super) fn agent_node(key: &str, profile: &str, effects: &[LedgerEffect]) -> Node {
    Node {
        id: NodeKey::try_from(key).unwrap(),
        position: Position::default(),
        declared_outcomes: effects
            .iter()
            .enumerate()
            .map(|(i, effect)| OutcomeDecl {
                id: OutcomeKey::try_from(format!("o{i}")).unwrap(),
                description: "ok".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: *effect,
            })
            .collect(),
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from(profile).unwrap(),
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

pub(super) fn success_terminal(key: &str) -> Node {
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

pub(super) fn edge(id: &str, from: &str, from_outcome: &str, to: &str) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: NodeKey::try_from(from).unwrap(),
            outcome: OutcomeKey::try_from(from_outcome).unwrap(),
        },
        to: NodeKey::try_from(to).unwrap(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }
}

pub(super) fn graph(start: &str, nodes: Vec<Node>, edges: Vec<Edge>) -> Graph {
    let mut map = BTreeMap::new();
    for node in nodes {
        map.insert(node.id.clone(), node);
    }
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "w5".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: NodeKey::try_from(start).unwrap(),
        nodes: map,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

fn w5_warnings(graph: &Graph, resolver: &dyn ReferenceResolver) -> Vec<(NodeKey, NodeKey, String)> {
    let mut out = Vec::new();
    warning_w5_same_runtime_verification(graph, resolver, &mut out);
    out.into_iter()
        .filter_map(|f| match f.kind {
            ValidationErrorKind::SameRuntimeVerification {
                implementer,
                verifier,
                runtime,
            } => Some((implementer, verifier, runtime)),
            _ => None,
        })
        .collect()
}

#[test]
fn a1_different_runtimes_no_warning() {
    let resolver = RuntimeResolver {
        runtimes: HashMap::from([
            ("implementer@1.0", "claude-code"),
            ("verifier@1.0", "codex"),
        ]),
    };
    let g = graph(
        "impl_1",
        vec![
            agent_node(
                "impl_1",
                "implementer@1.0",
                &[LedgerEffect::ReadyForVerification],
            ),
            agent_node("verify_1", "verifier@1.0", &[LedgerEffect::Verified]),
        ],
        vec![edge("e", "impl_1", "o0", "verify_1")],
    );
    assert!(w5_warnings(&g, &resolver).is_empty());
}

#[test]
fn a2_same_runtime_warns_naming_both_nodes_and_runtime() {
    let resolver = RuntimeResolver {
        runtimes: HashMap::from([
            ("implementer@1.0", "claude-code"),
            ("verifier@1.0", "claude-code"),
        ]),
    };
    let g = graph(
        "impl_1",
        vec![
            agent_node(
                "impl_1",
                "implementer@1.0",
                &[LedgerEffect::ReadyForVerification],
            ),
            agent_node("verify_1", "verifier@1.0", &[LedgerEffect::Verified]),
        ],
        vec![edge("e", "impl_1", "o0", "verify_1")],
    );
    assert_eq!(
        w5_warnings(&g, &resolver),
        vec![(
            NodeKey::try_from("impl_1").unwrap(),
            NodeKey::try_from("verify_1").unwrap(),
            "claude-code".to_owned(),
        )]
    );
}

#[test]
fn a3_unresolved_verifier_profile_does_not_warn() {
    // implementer resolves; verifier is unknown to the resolver — must
    // not be treated as "same runtime as anything".
    let resolver = RuntimeResolver {
        runtimes: HashMap::from([("implementer@1.0", "claude-code")]),
    };
    let g = graph(
        "impl_1",
        vec![
            agent_node(
                "impl_1",
                "implementer@1.0",
                &[LedgerEffect::ReadyForVerification],
            ),
            agent_node("verify_1", "verifier@1.0", &[LedgerEffect::Verified]),
        ],
        vec![edge("e", "impl_1", "o0", "verify_1")],
    );
    assert!(w5_warnings(&g, &resolver).is_empty());
}

#[test]
fn a4_unresolved_implementer_profile_does_not_warn() {
    let resolver = RuntimeResolver {
        runtimes: HashMap::from([("verifier@1.0", "claude-code")]),
    };
    let g = graph(
        "impl_1",
        vec![
            agent_node(
                "impl_1",
                "implementer@1.0",
                &[LedgerEffect::ReadyForVerification],
            ),
            agent_node("verify_1", "verifier@1.0", &[LedgerEffect::Verified]),
        ],
        vec![edge("e", "impl_1", "o0", "verify_1")],
    );
    assert!(w5_warnings(&g, &resolver).is_empty());
}

#[test]
fn a5_no_op_resolver_never_warns() {
    let g = graph(
        "impl_1",
        vec![
            agent_node(
                "impl_1",
                "implementer@1.0",
                &[LedgerEffect::ReadyForVerification],
            ),
            agent_node("verify_1", "verifier@1.0", &[LedgerEffect::Verified]),
        ],
        vec![edge("e", "impl_1", "o0", "verify_1")],
    );
    assert!(
        w5_warnings(&g, &NoOpResolver).is_empty(),
        "NoOpResolver's default profile_runtime is None; unknown must never warn"
    );
}

#[test]
fn a6_self_verifying_node_warns_against_itself() {
    // One node declares both ready_for_verification and verified —
    // authorable today, no rule forbids it. It is the same runtime by
    // construction, so it must warn even with no downstream edge at all.
    let resolver = RuntimeResolver {
        runtimes: HashMap::from([("implementer@1.0", "claude-code")]),
    };
    let g = graph(
        "impl_1",
        vec![agent_node(
            "impl_1",
            "implementer@1.0",
            &[LedgerEffect::ReadyForVerification, LedgerEffect::Verified],
        )],
        vec![],
    );
    assert_eq!(
        w5_warnings(&g, &resolver),
        vec![(
            NodeKey::try_from("impl_1").unwrap(),
            NodeKey::try_from("impl_1").unwrap(),
            "claude-code".to_owned(),
        )]
    );
}

#[test]
fn fires_through_public_validate_with_resolver_entry_point() {
    let resolver = RuntimeResolver {
        runtimes: HashMap::from([
            ("implementer@1.0", "claude-code"),
            ("verifier@1.0", "claude-code"),
        ]),
    };
    let g = graph(
        "impl_1",
        vec![
            agent_node(
                "impl_1",
                "implementer@1.0",
                &[LedgerEffect::ReadyForVerification],
            ),
            agent_node("verify_1", "verifier@1.0", &[LedgerEffect::Verified]),
            success_terminal("end"),
        ],
        vec![
            edge("e1", "impl_1", "o0", "verify_1"),
            edge("e2", "verify_1", "o0", "end"),
        ],
    );
    let warnings = validate_with_resolver(&g, &resolver)
        .expect("a Warning-severity finding must not turn this into Err");
    assert!(
        warnings.iter().any(|f| matches!(
            &f.kind,
            ValidationErrorKind::SameRuntimeVerification { runtime, .. }
                if runtime == "claude-code"
        )),
        "expected SameRuntimeVerification via validate_with_resolver, got {warnings:?}"
    );
}
