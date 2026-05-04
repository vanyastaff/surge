//! Graph validation. Non-fail-fast — collects all errors and warnings.

use crate::edge::EdgeKind;
use crate::graph::{Graph, Subgraph};
use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};
use crate::node::NodeConfig;
use crate::notify_config::NotifyFailureAction;

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
    /// A `NodeKind::Notify` node does not declare the required `delivered`
    /// outcome. Engine emits this on every successful delivery, so the
    /// outcome must exist for routing to work.
    NotifyMissingDelivered {
        node: NodeKey,
    },
    /// A `NodeKind::Notify` node configured with `on_failure: Fail`
    /// does not declare an `undeliverable` outcome. Without it, a
    /// failed delivery in `Fail` mode produces `StageFailed` and halts
    /// the run. Warning, not error — authors may want fail-fast.
    NotifyFailMissingUndeliverable {
        node: NodeKey,
    },
    /// A `LoopConfig::iterates_over::Static` carries more than
    /// `MAX_LOOP_ITEMS_STATIC` (1000) items. Bound at graph-load time
    /// to prevent unbounded memory growth in the engine's frame stack.
    LoopStaticTooLarge { node: NodeKey, count: usize, max: usize },
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
            // Warnings — informational, do not block the run.
            Self::EscalateTargetNotHumanOrNotify
            | Self::OrphanSubgraph { .. }
            | Self::NotifyFailMissingUndeliverable { .. } => Severity::Warning,

            // Errors — graph is structurally invalid or will misbehave at runtime.
            Self::StartNodeMissing
            | Self::EdgeFromUnknownNode
            | Self::EdgeToUnknownNode
            | Self::EdgeFromUndeclaredOutcome
            | Self::DuplicateEdgeFromSamePort
            | Self::OutcomeWithNoEdge
            | Self::UnreachableNode
            | Self::NoTerminalReachable
            | Self::InvalidProfileRef
            | Self::HumanGateWithoutOptions
            | Self::BranchWithoutArms
            | Self::LoopIterableInvalid
            | Self::LoopBodyMissingStart
            | Self::SubgraphInvalid
            | Self::TerminalOutcomeHasEdge
            | Self::BacktrackTargetUnreachable
            | Self::SchemaVersionMismatch
            | Self::KeyFormatViolation { .. }
            | Self::SubgraphRefMissing { .. }
            | Self::SubgraphReferenceCycle { .. }
            | Self::NodeKeyCollision { .. }
            | Self::NotifyMissingDelivered { .. }
            | Self::LoopStaticTooLarge { .. } => Severity::Error,
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
    warning_w3_notify_outcomes(graph, &mut findings);
    validate_loop_static_cap(graph, &mut findings);

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

fn validate_loop_static_cap(graph: &Graph, errors: &mut Vec<ValidationError>) {
    use crate::loop_config::{IterableSource, MAX_LOOP_ITEMS_STATIC};
    for node in graph.nodes.values() {
        let NodeConfig::Loop(cfg) = &node.config else { continue; };
        let IterableSource::Static(items) = &cfg.iterates_over else { continue; };
        if items.len() > MAX_LOOP_ITEMS_STATIC {
            errors.push(ValidationError {
                location: ErrorLocation::Node { id: node.id.clone() },
                kind: ValidationErrorKind::LoopStaticTooLarge {
                    node: node.id.clone(),
                    count: items.len(),
                    max: MAX_LOOP_ITEMS_STATIC,
                },
                message: format!(
                    "loop node `{}` static iterable has {} items (max {})",
                    node.id.as_str(),
                    items.len(),
                    MAX_LOOP_ITEMS_STATIC,
                ),
            });
        }
    }
    // Also recurse into subgraphs — Loop nodes can live inside subgraphs.
    for sg in graph.subgraphs.values() {
        for node in sg.nodes.values() {
            let NodeConfig::Loop(cfg) = &node.config else { continue; };
            let IterableSource::Static(items) = &cfg.iterates_over else { continue; };
            if items.len() > MAX_LOOP_ITEMS_STATIC {
                errors.push(ValidationError {
                    location: ErrorLocation::Node { id: node.id.clone() },
                    kind: ValidationErrorKind::LoopStaticTooLarge {
                        node: node.id.clone(),
                        count: items.len(),
                        max: MAX_LOOP_ITEMS_STATIC,
                    },
                    message: format!(
                        "loop node `{}` static iterable has {} items (max {})",
                        node.id.as_str(),
                        items.len(),
                        MAX_LOOP_ITEMS_STATIC,
                    ),
                });
            }
        }
    }
}

fn warning_w3_notify_outcomes(graph: &Graph, out: &mut Vec<ValidationError>) {
    let delivered =
        OutcomeKey::try_from("delivered").expect("'delivered' is valid OutcomeKey");
    let undeliverable =
        OutcomeKey::try_from("undeliverable").expect("'undeliverable' is valid OutcomeKey");

    for (id, node) in &graph.nodes {
        let NodeConfig::Notify(cfg) = &node.config else {
            continue;
        };

        let has_delivered = node.declared_outcomes.iter().any(|o| o.id == delivered);
        if !has_delivered {
            out.push(ValidationError {
                kind: ValidationErrorKind::NotifyMissingDelivered { node: id.clone() },
                location: ErrorLocation::Node { id: id.clone() },
                message: format!(
                    "notify node `{}` must declare a `delivered` outcome",
                    id.as_str()
                ),
            });
        }

        if matches!(cfg.on_failure, NotifyFailureAction::Fail) {
            let has_undeliverable =
                node.declared_outcomes.iter().any(|o| o.id == undeliverable);
            if !has_undeliverable {
                out.push(ValidationError {
                    kind: ValidationErrorKind::NotifyFailMissingUndeliverable {
                        node: id.clone(),
                    },
                    location: ErrorLocation::Node { id: id.clone() },
                    message: format!(
                        "notify node `{}` uses on_failure: Fail but does not declare \
                         an `undeliverable` outcome; failed deliveries will halt the run",
                        id.as_str()
                    ),
                });
            }
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

    mod m6_loop_static_cap_tests {
        use super::*;
        use crate::graph::{Graph, GraphMetadata, Subgraph, SCHEMA_VERSION};
        use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};
        use crate::node::{Node, NodeConfig, OutcomeDecl, Position};
        use crate::loop_config::{ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode, MAX_LOOP_ITEMS_STATIC};
        use crate::edge::EdgeKind;
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        fn graph_with_loop_node(items: Vec<toml::Value>) -> Graph {
            let loop_key = NodeKey::try_from("loop_1").unwrap();
            let body_key = SubgraphKey::try_from("body").unwrap();
            let body_start = NodeKey::try_from("body_start").unwrap();

            let loop_node = Node {
                id: loop_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![OutcomeDecl {
                    id: OutcomeKey::try_from("completed").unwrap(),
                    description: "done".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                }],
                config: NodeConfig::Loop(LoopConfig {
                    iterates_over: IterableSource::Static(items),
                    body: body_key.clone(),
                    iteration_var_name: "item".into(),
                    exit_condition: ExitCondition::AllItems,
                    on_iteration_failure: FailurePolicy::Abort,
                    parallelism: ParallelismMode::Sequential,
                    gate_after_each: false,
                }),
            };

            let body_node = Node {
                id: body_start.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            };

            let mut nodes = BTreeMap::new();
            nodes.insert(loop_key.clone(), loop_node);

            let mut body_nodes = BTreeMap::new();
            body_nodes.insert(body_start.clone(), body_node);

            let mut subgraphs = BTreeMap::new();
            subgraphs.insert(body_key, Subgraph {
                start: body_start,
                nodes: body_nodes,
                edges: vec![],
            });

            Graph {
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
            }
        }

        #[test]
        fn loop_static_size_at_cap_is_ok() {
            let items: Vec<toml::Value> = (0..MAX_LOOP_ITEMS_STATIC).map(|i| toml::Value::Integer(i as i64)).collect();
            let g = graph_with_loop_node(items);
            let result = validate(&g);
            // The minimal graph also triggers other structural errors (e.g., no Terminal at outer level).
            // Filter to only LoopStaticTooLarge findings.
            let findings = result.unwrap_or_else(|e| e);
            assert!(
                !findings.iter().any(|f| matches!(f.kind, ValidationErrorKind::LoopStaticTooLarge { .. })),
                "1000 items should not trigger LoopStaticTooLarge: {findings:?}"
            );
        }

        #[test]
        fn loop_static_size_above_cap_is_rejected() {
            let items: Vec<toml::Value> = (0..MAX_LOOP_ITEMS_STATIC + 1).map(|i| toml::Value::Integer(i as i64)).collect();
            let g = graph_with_loop_node(items);
            let result = validate(&g);
            let errors = result.expect_err("validation should fail");
            let cap_errors: Vec<_> = errors.iter()
                .filter(|f| matches!(f.kind, ValidationErrorKind::LoopStaticTooLarge { .. }))
                .collect();
            assert!(!cap_errors.is_empty(), "expected LoopStaticTooLarge, got {errors:?}");
        }
    }

    mod m6_notify_validation_tests {
        use super::*;
        use crate::edge::EdgeKind;
        use crate::graph::{GraphMetadata, SCHEMA_VERSION};
        use crate::keys::{NodeKey, OutcomeKey};
        use crate::node::{Node, NodeConfig, OutcomeDecl, Position};
        use crate::notify_config::{
            NotifyChannel, NotifyConfig, NotifyFailureAction, NotifySeverity, NotifyTemplate,
        };
        use std::collections::BTreeMap;

        fn notify_node_with_outcomes(
            outcomes: Vec<&str>,
            on_failure: NotifyFailureAction,
        ) -> Node {
            let key = NodeKey::try_from("notify_1").unwrap();
            Node {
                id: key.clone(),
                position: Position::default(),
                declared_outcomes: outcomes
                    .iter()
                    .map(|o| OutcomeDecl {
                        id: OutcomeKey::try_from(*o).unwrap(),
                        description: format!("{o} outcome"),
                        edge_kind_hint: EdgeKind::Forward,
                        is_terminal: false,
                    })
                    .collect(),
                config: NodeConfig::Notify(NotifyConfig {
                    channel: NotifyChannel::Desktop,
                    template: NotifyTemplate {
                        severity: NotifySeverity::Info,
                        title: "t".into(),
                        body: "b".into(),
                        artifacts: vec![],
                    },
                    on_failure,
                }),
            }
        }

        fn graph_with_node(node: Node) -> Graph {
            let key = node.id.clone();
            let mut nodes = BTreeMap::new();
            nodes.insert(key.clone(), node);
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
        fn notify_missing_delivered_outcome_is_error() {
            let n = notify_node_with_outcomes(vec!["sent"], NotifyFailureAction::Continue);
            let g = graph_with_node(n);
            let result = validate(&g);
            let errors = result.expect_err("validation should fail");
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e.kind, ValidationErrorKind::NotifyMissingDelivered { .. })),
                "expected NotifyMissingDelivered, got {errors:?}"
            );
        }

        #[test]
        fn notify_with_delivered_only_continue_is_ok() {
            let n =
                notify_node_with_outcomes(vec!["delivered"], NotifyFailureAction::Continue);
            let g = graph_with_node(n);
            let result = validate(&g);
            // ok branch returns Vec<ValidationError> (warnings only); should be empty.
            let warnings = result.unwrap_or_else(|errs| errs);
            assert!(
                !warnings.iter().any(|w| matches!(
                    w.kind,
                    ValidationErrorKind::NotifyMissingDelivered { .. }
                )),
                "delivered-only-Continue should not produce NotifyMissingDelivered, got {warnings:?}"
            );
        }

        #[test]
        fn notify_fail_without_undeliverable_is_warning() {
            let n =
                notify_node_with_outcomes(vec!["delivered"], NotifyFailureAction::Fail);
            let g = graph_with_node(n);
            // The minimal graph has no edges or Terminal node, so other structural
            // rules fire as errors. Use unwrap_or_else to get all findings regardless.
            let findings = match validate(&g) {
                Ok(w) => w,
                Err(all) => all,
            };
            assert!(
                findings.iter().any(|w| matches!(
                    w.kind,
                    ValidationErrorKind::NotifyFailMissingUndeliverable { .. }
                ) && w.kind.severity() == Severity::Warning),
                "expected NotifyFailMissingUndeliverable warning, got {findings:?}"
            );
        }
    }
}
