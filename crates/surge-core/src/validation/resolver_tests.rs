use super::*;
use crate::agent_config::AgentConfig;
use crate::edge::EdgeKind;
use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use crate::keys::{NodeKey, OutcomeKey, ProfileKey};
use crate::node::{Node, NodeConfig, OutcomeDecl, Position};
use crate::terminal_config::{TerminalConfig, TerminalKind};
use std::collections::{BTreeMap, HashSet};

struct InMemoryResolver {
    profiles: HashSet<&'static str>,
}
impl ReferenceResolver for InMemoryResolver {
    fn profile_exists(&self, name: &str) -> bool {
        self.profiles.contains(name)
    }
    fn template_exists(&self, _: &str) -> bool {
        true
    }
    fn named_agent_exists(&self, _: &str) -> bool {
        true
    }
}

fn graph_with_agent(profile: &str) -> Graph {
    let agent_key = NodeKey::try_from("impl_1").unwrap();
    let term_key = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent_key.clone(),
        Node {
            id: agent_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: String::new(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
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
        },
    );
    nodes.insert(
        term_key.clone(),
        Node {
            id: term_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    let edge = crate::edge::Edge {
        id: crate::keys::EdgeKey::try_from("e_done").unwrap(),
        from: crate::edge::PortRef {
            node: agent_key.clone(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        },
        to: term_key,
        kind: EdgeKind::Forward,
        policy: crate::edge::EdgePolicy::default(),
    };
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "resolver-test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: agent_key,
        nodes,
        edges: vec![edge],
        subgraphs: BTreeMap::new(),
    }
}

#[test]
fn unknown_profile_is_reported_via_resolver() {
    let g = graph_with_agent("implementer@1.0");
    let resolver = InMemoryResolver {
        profiles: HashSet::new(),
    };
    let result = validate_with_resolver(&g, &resolver);
    let errs = result.expect_errors("unknown profile must error");
    let saw = errs.iter().any(|e| {
        matches!(
            &e.kind,
            ValidationErrorKind::ProfileNotFound { profile, .. } if profile == "implementer@1.0"
        )
    });
    assert!(saw, "expected ProfileNotFound, got {errs:?}");
}

#[test]
fn known_profile_passes_resolver_check() {
    let g = graph_with_agent("implementer@1.0");
    let resolver = InMemoryResolver {
        profiles: HashSet::from(["implementer@1.0"]),
    };
    let result = validate_with_resolver(&g, &resolver);
    let warnings = result.expect_valid("graph should validate");
    let no_resolver_errors = !warnings.iter().any(|e| {
        matches!(
            &e.kind,
            ValidationErrorKind::ProfileNotFound { .. }
                | ValidationErrorKind::TemplateNotFound { .. }
                | ValidationErrorKind::NamedAgentNotFound { .. }
        )
    });
    assert!(
        no_resolver_errors,
        "expected no resolver-related findings, got {warnings:?}"
    );
}

#[test]
fn no_op_resolver_accepts_anything() {
    let g = graph_with_agent("does-not-exist@9.9");
    let result = validate_with_resolver(&g, &NoOpResolver);
    result.expect_valid("NoOpResolver must accept any profile name");
}

#[test]
fn syntactic_validate_does_not_check_resolver() {
    // Even with an obviously fake profile, the no-resolver entry point
    // must succeed — its job is structural integrity only.
    let g = graph_with_agent("does-not-exist@9.9");
    let result = validate(&g);
    result.expect_valid("structural validate ignores profile registry");
}
