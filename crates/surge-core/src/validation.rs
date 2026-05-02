//! Graph validation. Non-fail-fast — collects all errors and warnings.

use crate::edge::EdgeKind;
use crate::graph::{Graph, Subgraph};
use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};
use crate::node::NodeConfig;

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub location: ErrorLocation,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorLocation {
    Graph,
    Node { id: NodeKey },
    Edge { id: crate::keys::EdgeKey },
    Outcome { node: NodeKey, outcome: OutcomeKey },
    Subgraph { path: Vec<SubgraphKey> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationErrorKind {
    StartNodeMissing,
    EdgeFromUnknownNode,
    EdgeToUnknownNode,
    EdgeFromUndeclaredOutcome,
    DuplicateEdgeFromSamePort,
    OutcomeWithNoEdge,
    UnreachableNode,
    NoTerminalReachable,
    InvalidProfileRef,
    HumanGateWithoutOptions,
    BranchWithoutArms,
    LoopIterableInvalid,
    LoopBodyMissingStart,
    SubgraphInvalid,
    TerminalOutcomeHasEdge,
    BacktrackTargetUnreachable,
    EscalateTargetNotHumanOrNotify,
    SchemaVersionMismatch,
    KeyFormatViolation {
        key: String,
    },
    SubgraphRefMissing {
        subgraph: SubgraphKey,
    },
    SubgraphReferenceCycle {
        cycle: Vec<SubgraphKey>,
    },
    NodeKeyCollision {
        key: NodeKey,
        locations: Vec<NodeKeyOrigin>,
    },
    OrphanSubgraph {
        key: SubgraphKey,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKeyOrigin {
    Root,
    Subgraph(SubgraphKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl ValidationErrorKind {
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self {
            Self::EscalateTargetNotHumanOrNotify | Self::OrphanSubgraph { .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

/// Validate a graph. Returns Ok(warnings_or_empty) if no errors;
/// Err(all_findings) if any errors are present.
pub fn validate(graph: &Graph) -> Result<Vec<ValidationError>, Vec<ValidationError>> {
    let mut findings = Vec::new();

    rule_1_start_exists(graph, &mut findings);
    rules_2_3_4_edge_endpoints(graph, &mut findings);
    rule_5_one_edge_per_outcome(graph, &mut findings);
    rule_6_reachability(graph, &mut findings);
    rule_7_terminal_reachable(graph, &mut findings);
    rules_8_9_10_node_specific(graph, &mut findings);
    rule_11_loop_iterable(graph, &mut findings);
    rule_11b_subgraph_refs_exist(graph, &mut findings);
    rules_12_13_subgraphs_well_formed(graph, &mut findings);
    rule_14_terminal_outcome_no_edge(graph, &mut findings);
    rule_15_backtrack_target_reachable(graph, &mut findings);
    rule_16_subgraph_cycle(graph, &mut findings);
    rule_17_node_key_uniqueness(graph, &mut findings);
    warning_w1_escalate_target(graph, &mut findings);
    warning_w2_orphan_subgraphs(graph, &mut findings);

    let has_error = findings
        .iter()
        .any(|f| f.kind.severity() == Severity::Error);
    if has_error {
        Err(findings)
    } else {
        Ok(findings)
    }
}

// ── Rule helpers ─────────────────────────────────────────────────

fn rule_1_start_exists(graph: &Graph, out: &mut Vec<ValidationError>) {
    if !graph.nodes.contains_key(&graph.start) {
        out.push(ValidationError {
            kind: ValidationErrorKind::StartNodeMissing,
            location: ErrorLocation::Graph,
            message: format!(
                "start node `{}` not found in nodes map",
                graph.start.as_str()
            ),
        });
    }
}

fn rules_2_3_4_edge_endpoints(graph: &Graph, out: &mut Vec<ValidationError>) {
    for edge in &graph.edges {
        if !graph.nodes.contains_key(&edge.from.node) {
            out.push(ValidationError {
                kind: ValidationErrorKind::EdgeFromUnknownNode,
                location: ErrorLocation::Edge {
                    id: edge.id.clone(),
                },
                message: format!(
                    "edge `{}` references missing source node `{}`",
                    edge.id.as_str(),
                    edge.from.node.as_str()
                ),
            });
        }
        if !graph.nodes.contains_key(&edge.to) {
            out.push(ValidationError {
                kind: ValidationErrorKind::EdgeToUnknownNode,
                location: ErrorLocation::Edge {
                    id: edge.id.clone(),
                },
                message: format!(
                    "edge `{}` references missing target node `{}`",
                    edge.id.as_str(),
                    edge.to.as_str()
                ),
            });
        }
        if let Some(node) = graph.nodes.get(&edge.from.node) {
            let declared = node
                .declared_outcomes
                .iter()
                .any(|o| o.id == edge.from.outcome);
            if !declared {
                out.push(ValidationError {
                    kind: ValidationErrorKind::EdgeFromUndeclaredOutcome,
                    location: ErrorLocation::Edge {
                        id: edge.id.clone(),
                    },
                    message: format!(
                        "edge `{}` from undeclared outcome `{}` on node `{}`",
                        edge.id.as_str(),
                        edge.from.outcome.as_str(),
                        edge.from.node.as_str()
                    ),
                });
            }
        }
    }
}

fn rule_5_one_edge_per_outcome(graph: &Graph, out: &mut Vec<ValidationError>) {
    use std::collections::HashMap;
    let mut counts: HashMap<(NodeKey, OutcomeKey), Vec<crate::keys::EdgeKey>> = HashMap::new();
    for e in &graph.edges {
        counts
            .entry((e.from.node.clone(), e.from.outcome.clone()))
            .or_default()
            .push(e.id.clone());
    }
    for ((node, outcome), edges) in counts {
        if edges.len() > 1 {
            out.push(ValidationError {
                kind: ValidationErrorKind::DuplicateEdgeFromSamePort,
                location: ErrorLocation::Outcome {
                    node: node.clone(),
                    outcome: outcome.clone(),
                },
                message: format!(
                    "outcome `{}` on `{}` has {} outgoing edges (must be 0 or 1)",
                    outcome.as_str(),
                    node.as_str(),
                    edges.len()
                ),
            });
        }
    }
    for (id, node) in &graph.nodes {
        for outcome in &node.declared_outcomes {
            let port = (id.clone(), outcome.id.clone());
            let has_edge = graph
                .edges
                .iter()
                .any(|e| e.from.node == port.0 && e.from.outcome == port.1);
            if !has_edge && !outcome.is_terminal {
                out.push(ValidationError {
                    kind: ValidationErrorKind::OutcomeWithNoEdge,
                    location: ErrorLocation::Outcome {
                        node: id.clone(),
                        outcome: outcome.id.clone(),
                    },
                    message: format!(
                        "outcome `{}` on node `{}` has no edge and is not terminal",
                        outcome.id.as_str(),
                        id.as_str()
                    ),
                });
            }
        }
    }
}

fn rule_6_reachability(graph: &Graph, out: &mut Vec<ValidationError>) {
    use std::collections::HashSet;
    if !graph.nodes.contains_key(&graph.start) {
        return;
    }
    let mut reachable = HashSet::new();
    let mut frontier = vec![graph.start.clone()];
    while let Some(n) = frontier.pop() {
        if !reachable.insert(n.clone()) {
            continue;
        }
        for e in &graph.edges {
            if e.from.node == n && e.kind == EdgeKind::Forward {
                frontier.push(e.to.clone());
            }
        }
    }
    for id in graph.nodes.keys() {
        if !reachable.contains(id) {
            out.push(ValidationError {
                kind: ValidationErrorKind::UnreachableNode,
                location: ErrorLocation::Node { id: id.clone() },
                message: format!(
                    "node `{}` not reachable from start via forward edges",
                    id.as_str()
                ),
            });
        }
    }
}

fn rule_7_terminal_reachable(graph: &Graph, out: &mut Vec<ValidationError>) {
    use crate::node::NodeKind;
    if !graph.nodes.contains_key(&graph.start) {
        return;
    }
    let found_terminal = graph.nodes.values().any(|n| n.kind() == NodeKind::Terminal);
    if !found_terminal {
        out.push(ValidationError {
            kind: ValidationErrorKind::NoTerminalReachable,
            location: ErrorLocation::Graph,
            message: "graph has no Terminal node — runs cannot end".into(),
        });
    }
}

fn rules_8_9_10_node_specific(graph: &Graph, out: &mut Vec<ValidationError>) {
    for (id, node) in &graph.nodes {
        match &node.config {
            NodeConfig::Agent(cfg) if cfg.profile.as_str().is_empty() => {
                out.push(ValidationError {
                    kind: ValidationErrorKind::InvalidProfileRef,
                    location: ErrorLocation::Node { id: id.clone() },
                    message: format!("agent node `{}` has empty profile reference", id.as_str()),
                });
            },
            NodeConfig::HumanGate(cfg) if cfg.options.is_empty() => {
                out.push(ValidationError {
                    kind: ValidationErrorKind::HumanGateWithoutOptions,
                    location: ErrorLocation::Node { id: id.clone() },
                    message: format!("human-gate node `{}` has no options", id.as_str()),
                });
            },
            NodeConfig::Branch(cfg) if cfg.predicates.is_empty() => {
                out.push(ValidationError {
                    kind: ValidationErrorKind::BranchWithoutArms,
                    location: ErrorLocation::Node { id: id.clone() },
                    message: format!("branch node `{}` has no predicates", id.as_str()),
                });
            },
            _ => {},
        }
    }
}

fn rule_11_loop_iterable(graph: &Graph, out: &mut Vec<ValidationError>) {
    for (id, node) in &graph.nodes {
        if let NodeConfig::Loop(cfg) = &node.config
            && let crate::loop_config::IterableSource::Artifact { node: src, .. } =
                &cfg.iterates_over
            && !graph.nodes.contains_key(src)
        {
            out.push(ValidationError {
                kind: ValidationErrorKind::LoopIterableInvalid,
                location: ErrorLocation::Node { id: id.clone() },
                message: format!(
                    "loop `{}` iterates over artifact from missing node `{}`",
                    id.as_str(),
                    src.as_str()
                ),
            });
        }
    }
}

fn rule_11b_subgraph_refs_exist(graph: &Graph, out: &mut Vec<ValidationError>) {
    for (id, node) in &graph.nodes {
        let target = match &node.config {
            NodeConfig::Loop(cfg) => Some(&cfg.body),
            NodeConfig::Subgraph(cfg) => Some(&cfg.inner),
            _ => None,
        };
        if let Some(sk) = target
            && !graph.subgraphs.contains_key(sk)
        {
            out.push(ValidationError {
                kind: ValidationErrorKind::SubgraphRefMissing {
                    subgraph: sk.clone(),
                },
                location: ErrorLocation::Node { id: id.clone() },
                message: format!(
                    "node `{}` references missing subgraph `{}`",
                    id.as_str(),
                    sk.as_str()
                ),
            });
        }
    }
}

fn rules_12_13_subgraphs_well_formed(graph: &Graph, out: &mut Vec<ValidationError>) {
    for (sk, sub) in &graph.subgraphs {
        if !sub.nodes.contains_key(&sub.start) {
            out.push(ValidationError {
                kind: ValidationErrorKind::LoopBodyMissingStart,
                location: ErrorLocation::Subgraph {
                    path: vec![sk.clone()],
                },
                message: format!(
                    "subgraph `{}` start `{}` not in its nodes",
                    sk.as_str(),
                    sub.start.as_str()
                ),
            });
        }
        validate_subgraph_structure(sk, sub, out);
    }
}

fn validate_subgraph_structure(sk: &SubgraphKey, sub: &Subgraph, out: &mut Vec<ValidationError>) {
    for edge in &sub.edges {
        if !sub.nodes.contains_key(&edge.from.node) {
            out.push(ValidationError {
                kind: ValidationErrorKind::EdgeFromUnknownNode,
                location: ErrorLocation::Subgraph {
                    path: vec![sk.clone()],
                },
                message: format!(
                    "subgraph `{}`: edge `{}` from missing node `{}`",
                    sk.as_str(),
                    edge.id.as_str(),
                    edge.from.node.as_str()
                ),
            });
        }
        if !sub.nodes.contains_key(&edge.to) {
            out.push(ValidationError {
                kind: ValidationErrorKind::EdgeToUnknownNode,
                location: ErrorLocation::Subgraph {
                    path: vec![sk.clone()],
                },
                message: format!(
                    "subgraph `{}`: edge `{}` to missing node `{}`",
                    sk.as_str(),
                    edge.id.as_str(),
                    edge.to.as_str()
                ),
            });
        }
    }
}

fn rule_14_terminal_outcome_no_edge(graph: &Graph, out: &mut Vec<ValidationError>) {
    for (id, node) in &graph.nodes {
        for o in &node.declared_outcomes {
            if o.is_terminal {
                let has_edge = graph
                    .edges
                    .iter()
                    .any(|e| e.from.node == *id && e.from.outcome == o.id);
                if has_edge {
                    out.push(ValidationError {
                        kind: ValidationErrorKind::TerminalOutcomeHasEdge,
                        location: ErrorLocation::Outcome {
                            node: id.clone(),
                            outcome: o.id.clone(),
                        },
                        message: format!(
                            "terminal outcome `{}` on `{}` has an outgoing edge",
                            o.id.as_str(),
                            id.as_str()
                        ),
                    });
                }
            }
        }
    }
}

fn rule_15_backtrack_target_reachable(_graph: &Graph, _out: &mut Vec<ValidationError>) {
    // Backtrack edges should form valid cycles. Full implementation deferred to M5
    // (executor) when traversal semantics are concrete. M1 stub.
}

fn rule_16_subgraph_cycle(graph: &Graph, out: &mut Vec<ValidationError>) {
    use std::collections::{HashMap, HashSet};
    let mut edges: HashMap<SubgraphKey, Vec<SubgraphKey>> = HashMap::new();
    for (sk, sub) in &graph.subgraphs {
        let mut targets = Vec::new();
        for n in sub.nodes.values() {
            match &n.config {
                NodeConfig::Loop(cfg) => targets.push(cfg.body.clone()),
                NodeConfig::Subgraph(cfg) => targets.push(cfg.inner.clone()),
                _ => {},
            }
        }
        edges.insert(sk.clone(), targets);
    }
    let mut root_targets = Vec::new();
    for n in graph.nodes.values() {
        match &n.config {
            NodeConfig::Loop(cfg) => root_targets.push(cfg.body.clone()),
            NodeConfig::Subgraph(cfg) => root_targets.push(cfg.inner.clone()),
            _ => {},
        }
    }

    fn dfs(
        node: &SubgraphKey,
        edges: &HashMap<SubgraphKey, Vec<SubgraphKey>>,
        stack: &mut Vec<SubgraphKey>,
        visited: &mut HashSet<SubgraphKey>,
    ) -> Option<Vec<SubgraphKey>> {
        if let Some(pos) = stack.iter().position(|s| s == node) {
            return Some(stack[pos..].to_vec());
        }
        if visited.contains(node) {
            return None;
        }
        stack.push(node.clone());
        if let Some(targets) = edges.get(node) {
            for t in targets {
                if let Some(cycle) = dfs(t, edges, stack, visited) {
                    return Some(cycle);
                }
            }
        }
        stack.pop();
        visited.insert(node.clone());
        None
    }

    let mut visited = HashSet::new();
    let mut reported: HashSet<Vec<SubgraphKey>> = HashSet::new();
    for sk in graph.subgraphs.keys().chain(root_targets.iter()) {
        let mut stack = Vec::new();
        if let Some(cycle) = dfs(sk, &edges, &mut stack, &mut visited) {
            let mut canonical = cycle.clone();
            canonical.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            if reported.insert(canonical) {
                out.push(ValidationError {
                    kind: ValidationErrorKind::SubgraphReferenceCycle {
                        cycle: cycle.clone(),
                    },
                    location: ErrorLocation::Subgraph {
                        path: cycle.clone(),
                    },
                    message: format!(
                        "subgraph reference cycle: {}",
                        cycle
                            .iter()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                });
            }
        }
    }
}

fn rule_17_node_key_uniqueness(graph: &Graph, out: &mut Vec<ValidationError>) {
    use std::collections::HashMap;
    let mut seen: HashMap<NodeKey, Vec<NodeKeyOrigin>> = HashMap::new();
    for k in graph.nodes.keys() {
        seen.entry(k.clone()).or_default().push(NodeKeyOrigin::Root);
    }
    for (sk, sub) in &graph.subgraphs {
        for k in sub.nodes.keys() {
            seen.entry(k.clone())
                .or_default()
                .push(NodeKeyOrigin::Subgraph(sk.clone()));
        }
    }
    for (key, locs) in seen {
        if locs.len() > 1 {
            out.push(ValidationError {
                kind: ValidationErrorKind::NodeKeyCollision {
                    key: key.clone(),
                    locations: locs.clone(),
                },
                location: ErrorLocation::Node { id: key.clone() },
                message: format!(
                    "node key `{}` appears in {} locations across graph",
                    key.as_str(),
                    locs.len()
                ),
            });
        }
    }
}

fn warning_w1_escalate_target(graph: &Graph, out: &mut Vec<ValidationError>) {
    use crate::node::NodeKind;
    for edge in &graph.edges {
        if edge.kind == EdgeKind::Escalate
            && let Some(target) = graph.nodes.get(&edge.to)
        {
            let kind = target.kind();
            if !matches!(kind, NodeKind::HumanGate | NodeKind::Notify) {
                out.push(ValidationError {
                    kind: ValidationErrorKind::EscalateTargetNotHumanOrNotify,
                    location: ErrorLocation::Edge { id: edge.id.clone() },
                    message: format!(
                        "escalate edge `{}` targets `{}` (kind {:?}); typically should target HumanGate or Notify",
                        edge.id.as_str(),
                        edge.to.as_str(),
                        kind
                    ),
                });
            }
        }
    }
}

fn warning_w2_orphan_subgraphs(graph: &Graph, out: &mut Vec<ValidationError>) {
    use std::collections::HashSet;
    let mut referenced: HashSet<SubgraphKey> = HashSet::new();
    for n in graph.nodes.values() {
        match &n.config {
            NodeConfig::Loop(cfg) => {
                referenced.insert(cfg.body.clone());
            },
            NodeConfig::Subgraph(cfg) => {
                referenced.insert(cfg.inner.clone());
            },
            _ => {},
        }
    }
    for sub in graph.subgraphs.values() {
        for n in sub.nodes.values() {
            match &n.config {
                NodeConfig::Loop(cfg) => {
                    referenced.insert(cfg.body.clone());
                },
                NodeConfig::Subgraph(cfg) => {
                    referenced.insert(cfg.inner.clone());
                },
                _ => {},
            }
        }
    }
    for sk in graph.subgraphs.keys() {
        if !referenced.contains(sk) {
            out.push(ValidationError {
                kind: ValidationErrorKind::OrphanSubgraph { key: sk.clone() },
                location: ErrorLocation::Subgraph {
                    path: vec![sk.clone()],
                },
                message: format!("subgraph `{}` is defined but never referenced", sk.as_str()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use crate::graph::{GraphMetadata, SCHEMA_VERSION};
    use crate::node::{Node, NodeConfig, Position};
    use crate::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::BTreeMap;

    fn minimal_terminal_only_graph() -> Graph {
        let end = NodeKey::try_from("end").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            end.clone(),
            Node {
                id: end.clone(),
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
                name: "test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: end,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    #[test]
    fn empty_terminal_graph_validates() {
        let g = minimal_terminal_only_graph();
        let result = validate(&g);
        assert!(result.is_ok(), "expected ok, got {:?}", result);
    }

    #[test]
    fn missing_start_reports_rule_1() {
        let mut g = minimal_terminal_only_graph();
        g.start = NodeKey::try_from("nonexistent").unwrap();
        let result = validate(&g);
        let errs = result.unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::StartNodeMissing))
        );
    }

    #[test]
    fn edge_to_unknown_node_reports_rule_3() {
        let mut g = minimal_terminal_only_graph();
        g.edges.push(Edge {
            id: crate::keys::EdgeKey::try_from("e1").unwrap(),
            from: PortRef {
                node: g.start.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: NodeKey::try_from("ghost").unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        });
        let result = validate(&g);
        let errs = result.unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::EdgeToUnknownNode))
        );
    }

    #[test]
    fn graph_with_no_terminal_reports_rule_7() {
        let mut g = minimal_terminal_only_graph();
        let bn = NodeKey::try_from("branch_only").unwrap();
        g.nodes.clear();
        g.nodes.insert(
            bn.clone(),
            Node {
                id: bn.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Branch(crate::branch_config::BranchConfig {
                    predicates: vec![],
                    default_outcome: OutcomeKey::try_from("default").unwrap(),
                }),
            },
        );
        g.start = bn;
        let result = validate(&g);
        let errs = result.unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::NoTerminalReachable))
        );
    }

    #[test]
    fn missing_subgraph_ref_reports_rule_11b() {
        use crate::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };
        let loop_node_key = NodeKey::try_from("loopn").unwrap();
        let mut g = minimal_terminal_only_graph();
        g.start = loop_node_key.clone();
        g.nodes.insert(
            loop_node_key.clone(),
            Node {
                id: loop_node_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Loop(LoopConfig {
                    iterates_over: IterableSource::Static(vec![]),
                    body: SubgraphKey::try_from("ghost_body").unwrap(),
                    iteration_var_name: "x".into(),
                    exit_condition: ExitCondition::AllItems,
                    on_iteration_failure: FailurePolicy::Abort,
                    parallelism: ParallelismMode::Sequential,
                    gate_after_each: false,
                }),
            },
        );
        let result = validate(&g);
        let errs = result.unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::SubgraphRefMissing { .. }))
        );
    }

    #[test]
    fn duplicate_node_key_in_subgraph_reports_rule_17() {
        use crate::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };
        let shared = NodeKey::try_from("shared_id").unwrap();
        let loop_node_key = NodeKey::try_from("loopn").unwrap();
        let sub_key = SubgraphKey::try_from("sub").unwrap();

        let mut g = minimal_terminal_only_graph();
        g.start = loop_node_key.clone();
        g.nodes.insert(
            loop_node_key.clone(),
            Node {
                id: loop_node_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Loop(LoopConfig {
                    iterates_over: IterableSource::Static(vec![]),
                    body: sub_key.clone(),
                    iteration_var_name: "x".into(),
                    exit_condition: ExitCondition::AllItems,
                    on_iteration_failure: FailurePolicy::Abort,
                    parallelism: ParallelismMode::Sequential,
                    gate_after_each: false,
                }),
            },
        );
        g.nodes.insert(
            shared.clone(),
            Node {
                id: shared.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let mut sub_nodes = BTreeMap::new();
        sub_nodes.insert(
            shared.clone(),
            Node {
                id: shared.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        g.subgraphs.insert(
            sub_key.clone(),
            Subgraph {
                start: shared.clone(),
                nodes: sub_nodes,
                edges: vec![],
            },
        );

        let result = validate(&g);
        let errs = result.unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::NodeKeyCollision { .. }))
        );
    }

    #[test]
    fn orphan_subgraph_reports_warning_not_error() {
        let mut g = minimal_terminal_only_graph();
        let mut m = BTreeMap::new();
        let inner_k = NodeKey::try_from("inner").unwrap();
        m.insert(
            inner_k.clone(),
            Node {
                id: inner_k.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        g.subgraphs.insert(
            SubgraphKey::try_from("orphan").unwrap(),
            Subgraph {
                start: inner_k,
                nodes: m,
                edges: vec![],
            },
        );
        let result = validate(&g);
        let warnings = result.expect("expected ok-with-warnings");
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w.kind, ValidationErrorKind::OrphanSubgraph { .. }))
        );
        assert!(
            warnings
                .iter()
                .all(|w| w.kind.severity() == Severity::Warning)
        );
    }

    #[test]
    fn severity_classification_correct() {
        assert_eq!(
            ValidationErrorKind::OrphanSubgraph {
                key: SubgraphKey::try_from("x").unwrap()
            }
            .severity(),
            Severity::Warning,
        );
        assert_eq!(
            ValidationErrorKind::StartNodeMissing.severity(),
            Severity::Error
        );
    }
}
