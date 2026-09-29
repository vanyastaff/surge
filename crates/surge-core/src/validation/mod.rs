//! Graph validation. Non-fail-fast — collects all errors and warnings.

mod error;
mod resolver;

pub use error::{ErrorLocation, NodeKeyOrigin, Severity, ValidationError, ValidationErrorKind};
pub use resolver::{NoOpResolver, ReferenceResolver};

use crate::edge::EdgeKind;
use crate::graph::{Graph, Subgraph};
use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};
use crate::node::NodeConfig;
use crate::notify_config::NotifyFailureAction;

/// Validate a graph. Returns Ok(warnings_or_empty) if no errors;
/// Err(all_findings) if any errors are present.
#[must_use = "validation results carry errors that must be inspected"]
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
    rule_16_subgraph_cycle(graph, &mut findings);
    rule_17_node_key_uniqueness(graph, &mut findings);
    warning_w1_escalate_target(graph, &mut findings);
    warning_w2_orphan_subgraphs(graph, &mut findings);
    warning_w3_notify_outcomes(graph, &mut findings);
    warning_w4_unverified_success(graph, &mut findings);
    warning_w6_end_of_pipeline_verification(graph, &mut findings);
    validate_loop_static_cap(graph, &mut findings);
    validate_sandbox_custom_on_agents(graph, &mut findings);
    validate_declared_skills(graph, &mut findings);

    into_result(findings)
}

/// Splits findings into `Err` when any is an error, `Ok` (warnings only) otherwise.
fn into_result(
    findings: Vec<ValidationError>,
) -> Result<Vec<ValidationError>, Vec<ValidationError>> {
    if findings
        .iter()
        .any(|f| f.kind.severity() == Severity::Error)
    {
        Err(findings)
    } else {
        Ok(findings)
    }
}

/// Like [`validate`], but additionally resolves named references (profiles,
/// templates, named agents) through the supplied [`ReferenceResolver`]. The
/// orchestrator wires a real resolver backed by the project profile registry;
/// the syntactic [`validate`] entry point is left untouched for callers that
/// have no registry available. Also runs
/// [`ValidationErrorKind::SameRuntimeVerification`] (W5), the only rule that
/// needs [`ReferenceResolver::profile_runtime`] rather than just
/// `profile_exists`.
///
/// # Errors
/// Same shape as [`validate`]: returns `Err(findings)` when at least one
/// finding has [`Severity::Error`].
#[must_use = "validation results carry errors that must be inspected"]
pub fn validate_with_resolver(
    graph: &Graph,
    resolver: &dyn ReferenceResolver,
) -> Result<Vec<ValidationError>, Vec<ValidationError>> {
    let mut findings = match validate(graph) {
        Ok(warnings) => warnings,
        Err(errs) => errs,
    };
    apply_reference_checks(graph, resolver, &mut findings);
    warning_w5_same_runtime_verification(graph, resolver, &mut findings);

    into_result(findings)
}

fn apply_reference_checks(
    graph: &Graph,
    resolver: &dyn ReferenceResolver,
    out: &mut Vec<ValidationError>,
) {
    // Pipeline template metadata reference, if any.
    if let Some(template) = graph
        .metadata
        .template_origin
        .as_ref()
        .and_then(|t| t.as_str().split('@').next())
        && !template.is_empty()
        && !resolver.template_exists(template)
    {
        out.push(ValidationError {
            kind: ValidationErrorKind::TemplateNotFound {
                template: template.to_owned(),
            },
            location: ErrorLocation::Graph,
            message: format!("graph template `{template}` is not registered"),
        });
    }

    for (id, node) in &graph.nodes {
        if let NodeConfig::Agent(cfg) = &node.config {
            let profile_str = cfg.profile.as_str();
            if !profile_str.is_empty() && !resolver.profile_exists(profile_str) {
                out.push(ValidationError {
                    kind: ValidationErrorKind::ProfileNotFound {
                        node: id.clone(),
                        profile: profile_str.to_owned(),
                    },
                    location: ErrorLocation::Node { id: id.clone() },
                    message: format!(
                        "agent node `{}` references unknown profile `{}`",
                        id.as_str(),
                        profile_str
                    ),
                });
            }
        }
    }
}

// ── Tests for ReferenceResolver path ─────────────────────────────

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

/// Outcome of visiting one node during [`walk_forward_reachable`].
enum ForwardStep {
    /// Keep expanding this node's `EdgeKind::Forward` successors.
    Continue,
    /// Do not expand past this node — it absorbs the path (W4's
    /// verification-gate semantics) — but keep processing the rest of the
    /// frontier.
    Absorb,
    /// Stop the entire walk immediately (W4's "one warning per graph,
    /// anchored at the first match" semantics).
    Halt,
}

/// Build the outer graph's forward-adjacency map once: each node to its
/// `EdgeKind::Forward` successors. [`walk_forward_reachable`] takes this by
/// reference so a caller that walks from several start nodes in one pass
/// (W5, once per implementer) builds it once instead of rescanning
/// `graph.edges` — O(E) — on every visited node.
fn forward_adjacency(graph: &Graph) -> std::collections::HashMap<NodeKey, Vec<NodeKey>> {
    let mut adjacency: std::collections::HashMap<NodeKey, Vec<NodeKey>> =
        std::collections::HashMap::new();
    for edge in &graph.edges {
        if edge.kind == EdgeKind::Forward {
            adjacency
                .entry(edge.from.node.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }
    adjacency
}

/// Stack-based depth-first walk of nodes forward-reachable from `start`
/// (inclusive), over `adjacency`'s `EdgeKind::Forward` edges — the one
/// traversal shared by [`warning_w4_unverified_success`] and
/// [`warning_w5_same_runtime_verification`], which differ only in what they
/// do at a verification gate (W4 absorbs the path there; W5 keeps
/// expanding, since every downstream verifier is a candidate pairing) and
/// whether they stop at the first match (W4 does; W5 does not) —
/// `on_visit`'s [`ForwardStep`] result expresses both per visited node.
/// Nodes named by an edge but missing from `graph.nodes` are silently
/// skipped (this walk runs *during* validation, before such dangling
/// references are themselves reported).
///
/// Complexity: O(V + E) per call given a reused `adjacency` — each node is
/// expanded at most once and each adjacency entry consumed at most once,
/// where V/E are the **outer** graph's node/edge counts; subgraph bodies are
/// never visited (see the "top-level only" note on
/// [`warning_w5_same_runtime_verification`]).
fn walk_forward_reachable(
    graph: &Graph,
    start: &NodeKey,
    adjacency: &std::collections::HashMap<NodeKey, Vec<NodeKey>>,
    mut on_visit: impl FnMut(&NodeKey, &crate::node::Node) -> ForwardStep,
) {
    let mut visited: std::collections::HashSet<NodeKey> = std::collections::HashSet::new();
    let mut stack = vec![start.clone()];
    while let Some(key) = stack.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(node) = graph.nodes.get(&key) else {
            continue;
        };
        match on_visit(&key, node) {
            ForwardStep::Halt => return,
            ForwardStep::Absorb => continue,
            ForwardStep::Continue => {
                if let Some(successors) = adjacency.get(&key) {
                    stack.extend(successors.iter().cloned());
                }
            },
        }
    }
}

/// W4 — warn when a `Terminal { Success }` is reachable from `start` without
/// passing a verification-authority node.
///
/// Forward DFS from `start` via [`walk_forward_reachable`]; a verification
/// gate absorbs the path (its downstream is considered verified, so
/// expansion does not continue past it). If a success terminal is still
/// reached, some run path can declare "done" without a verifier — one
/// warning per graph, anchored at the first such terminal (the walk halts
/// there, by design).
fn warning_w4_unverified_success(graph: &Graph, out: &mut Vec<ValidationError>) {
    use crate::node::NodeKind;
    use std::collections::HashSet;

    if !graph.nodes.contains_key(&graph.start) {
        return;
    }
    let adjacency = forward_adjacency(graph);
    walk_forward_reachable(graph, &graph.start, &adjacency, |key, node| {
        // A verification gate absorbs the path — stop expanding here.
        if node_is_verification_gate(graph, node, &mut HashSet::new()) {
            return ForwardStep::Absorb;
        }
        if node.kind() == NodeKind::Terminal && is_success_terminal(node) {
            out.push(ValidationError {
                kind: ValidationErrorKind::UnverifiedSuccessPath {
                    terminal: key.clone(),
                },
                location: ErrorLocation::Node { id: key.clone() },
                message: format!(
                    "success terminal `{}` is reachable without a verification node \
                     (no outcome declares ledger_effect = verified on the path)",
                    key.as_str()
                ),
            });
            return ForwardStep::Halt;
        }
        ForwardStep::Continue
    });
}

fn is_success_terminal(node: &crate::node::Node) -> bool {
    use crate::terminal_config::TerminalKind;
    matches!(
        &node.config,
        NodeConfig::Terminal(cfg) if cfg.kind == TerminalKind::Success
    )
}

/// True when `node` gates verification: it declares a `LedgerEffect::Verified`
/// outcome, or it is a Loop/Subgraph whose body (transitively) contains such a
/// node. `seen` guards against subgraph reference cycles.
fn node_is_verification_gate(
    graph: &Graph,
    node: &crate::node::Node,
    seen: &mut std::collections::HashSet<SubgraphKey>,
) -> bool {
    use crate::node::LedgerEffect;
    if node
        .declared_outcomes
        .iter()
        .any(|outcome| outcome.ledger_effect == LedgerEffect::Verified)
    {
        return true;
    }
    match &node.config {
        NodeConfig::Loop(cfg) => subgraph_has_verification_gate(graph, &cfg.body, seen),
        NodeConfig::Subgraph(cfg) => subgraph_has_verification_gate(graph, &cfg.inner, seen),
        _ => false,
    }
}

fn subgraph_has_verification_gate(
    graph: &Graph,
    key: &SubgraphKey,
    seen: &mut std::collections::HashSet<SubgraphKey>,
) -> bool {
    if !seen.insert(key.clone()) {
        return false;
    }
    let Some(subgraph) = graph.subgraphs.get(key) else {
        return false;
    };
    subgraph
        .nodes
        .values()
        .any(|node| node_is_verification_gate(graph, node, seen))
}

/// Returns `node`'s resolved agent runtime through `resolver`, or `None`
/// when `node` is not an `Agent` node (no profile to resolve) or its
/// profile does not resolve. `None` is *unknown* —
/// [`warning_w5_same_runtime_verification`] must never treat it as equal to
/// anything. A `Loop`/`Subgraph` container node returns `None` here even
/// when [`node_is_verification_gate`] reports it `true` (because its body
/// contains a verifier) — a container is not a `NodeConfig::Agent` and has
/// no single profile to resolve; see
/// [`warning_w5_same_runtime_verification`]'s "top-level only" doc for what
/// that means for the rule.
///
/// `cache` memoizes by profile string for the lifetime of one
/// [`warning_w5_same_runtime_verification`] pass: in production
/// `resolver.profile_runtime` is backed by `ProfileRegistry::resolve`, a
/// full extends-chain walk plus merge plus clone per call, and the same
/// profile is otherwise looked up once as an implementer and again every
/// time it is visited as a candidate verifier.
fn agent_runtime(
    node: &crate::node::Node,
    resolver: &dyn ReferenceResolver,
    cache: &mut std::collections::HashMap<String, Option<String>>,
) -> Option<String> {
    let NodeConfig::Agent(cfg) = &node.config else {
        return None;
    };
    let profile_str = cfg.profile.as_str();
    if let Some(cached) = cache.get(profile_str) {
        return cached.clone();
    }
    let runtime = resolver.profile_runtime(profile_str);
    cache.insert(profile_str.to_owned(), runtime.clone());
    runtime
}

/// W5 — warn when a verification-authority node resolves to the same agent
/// runtime as an implementer node it verifies (spec item 58).
///
/// For each node declaring a `LedgerEffect::ReadyForVerification` outcome,
/// forward-DFS from it via [`walk_forward_reachable`] — inclusive of the
/// node itself, so a node that is both implementer and verifier
/// (self-verification) is compared against itself — and flag every
/// [`node_is_verification_gate`] reached whose resolved runtime matches the
/// implementer's. Continues past a matched (or unmatched) gate rather than
/// absorbing the path, unlike [`warning_w4_unverified_success`]: every
/// verifier downstream of an implementer is a candidate pairing, not just
/// the first one reached. A profile that does not resolve on either side
/// (`resolver.profile_runtime` → `None`) is skipped, never treated as a
/// match.
///
/// # Top-level only
/// Both halves of this rule stop at the outer graph. The implementer set
/// below is collected from `graph.nodes` only — a `Subgraph`/`Loop` body's
/// own `nodes` map (`Subgraph::nodes`) is never inspected, so an implementer
/// living inside a body can never be found. The walk itself follows
/// `graph.edges` only — a body's `edges` are a disjoint list on
/// [`Subgraph`](crate::graph::Subgraph), never merged into the outer edge
/// set — so even a top-level implementer's walk cannot cross into a body.
/// And when a Loop/Subgraph *container* node standing in for that body's
/// execution is visited, [`node_is_verification_gate`] correctly reports it
/// as a gate (it resolves transitively into the body for that check alone —
/// see its own doc), but [`agent_runtime`] on that same container always
/// answers `None`, so the transitive gate result can never contribute a
/// runtime match here. That is left deliberately unexploited rather than
/// guessing which of a body's — possibly several, possibly
/// differently-runtimed — `Agent` nodes should stand in for the container's
/// runtime; answering that is a design decision that would extend the rule,
/// not a bug fix within it.
///
/// Only reachable via [`validate_with_resolver`] — needs a real
/// [`ReferenceResolver`] to answer runtime questions at all.
///
/// # Why the implementer comes from graph shape, not a ledger effect
/// Keying the implementer side on `LedgerEffect::ReadyForVerification` — the
/// obvious reading of "the node whose work this verifies" — makes the rule
/// fire on nothing. Verified across the tree: no flow declares that effect
/// (only `verified` and `failed_verification` appear anywhere), none carries
/// an outcome *named* `ready_for_verification`, and none binds the
/// `implementer@2.0` profile that `flow-generator-1.0.toml` names when it
/// describes that convention. The prompt and the shipped flows had drifted.
///
/// A verifier's direct incoming `EdgeKind::Forward` edge is what every flow
/// actually carries, and it is what "hands work to the verifier" means. On
/// the bundled set that makes the rule fire on four of thirteen flows —
/// `linear-3`, `linear-with-review`, `bug-fix`, `refactor` — because every
/// bundled profile bar one resolves to a single agent runtime. The exact
/// four are pinned by a test so a change that silently returns the rule to
/// silence fails.
fn warning_w5_same_runtime_verification(
    graph: &Graph,
    resolver: &dyn ReferenceResolver,
    out: &mut Vec<ValidationError>,
) {
    use std::collections::{HashMap, HashSet};

    // The implementer is the node that hands work to the verifier: its
    // direct `EdgeKind::Forward` predecessor.
    //
    // Keying the implementer side on `LedgerEffect::ReadyForVerification`
    // instead would make this rule inert. Verified 2026-09-07 across the
    // whole tree: no flow declares that effect (only `verified` and
    // `failed_verification` appear anywhere), no flow carries an outcome
    // *named* `ready_for_verification`, and no flow binds `implementer@2.0`
    // — the profile `flow-generator-1.0.toml` names when it describes that
    // convention. Graph shape is the one signal every flow actually
    // carries, and "the node whose work this verifies" is what an incoming
    // forward edge means.
    let mut predecessors: HashMap<&NodeKey, Vec<&NodeKey>> = HashMap::new();
    for edge in &graph.edges {
        if edge.kind == EdgeKind::Forward {
            predecessors
                .entry(&edge.to)
                .or_default()
                .push(&edge.from.node);
        }
    }

    let mut runtime_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut reported: HashSet<(NodeKey, NodeKey)> = HashSet::new();

    for (verifier_key, verifier_node) in &graph.nodes {
        if !node_is_verification_gate(graph, verifier_node, &mut HashSet::new()) {
            continue;
        }
        // Top-level only: `agent_runtime` answers `None` for any
        // non-`Agent` node, so a gate detected through
        // `node_is_verification_gate`'s transitive Loop/Subgraph descent
        // contributes nothing here. W4 still uses that descent correctly.
        let Some(verifier_runtime) = agent_runtime(verifier_node, resolver, &mut runtime_cache)
        else {
            continue;
        };
        // A node that declares both `ReadyForVerification` and `Verified`
        // verifies its own work — the same runtime by construction, and no
        // edge is needed to say so. Rare (nothing in the tree authors it),
        // but when someone does, it is an explicit authored statement and
        // the worst shape this rule exists to catch.
        if verifier_node
            .declared_outcomes
            .iter()
            .any(|o| o.ledger_effect == crate::node::LedgerEffect::ReadyForVerification)
            && reported.insert((verifier_key.clone(), verifier_key.clone()))
        {
            out.push(ValidationError {
                kind: ValidationErrorKind::SameRuntimeVerification {
                    implementer: verifier_key.clone(),
                    verifier: verifier_key.clone(),
                    runtime: verifier_runtime.clone(),
                },
                location: ErrorLocation::Node {
                    id: verifier_key.clone(),
                },
                message: format!(
                    "node `{}` both implements and verifies its own work on agent \
                     runtime `{}` (declared or defaulted) — a verifier cannot be \
                     its own check",
                    verifier_key.as_str(),
                    verifier_runtime
                ),
            });
        }

        let Some(incoming) = predecessors.get(verifier_key) else {
            continue;
        };
        for implementer_key in incoming {
            if *implementer_key == verifier_key {
                continue;
            }
            let Some(implementer_node) = graph.nodes.get(*implementer_key) else {
                continue;
            };
            let Some(implementer_runtime) =
                agent_runtime(implementer_node, resolver, &mut runtime_cache)
            else {
                continue;
            };
            if implementer_runtime != verifier_runtime {
                continue;
            }
            // Two outcomes of one node can both route into the verifier;
            // that is one finding about one pair, not two.
            if !reported.insert(((*implementer_key).clone(), verifier_key.clone())) {
                continue;
            }
            out.push(ValidationError {
                kind: ValidationErrorKind::SameRuntimeVerification {
                    implementer: (*implementer_key).clone(),
                    verifier: verifier_key.clone(),
                    runtime: verifier_runtime.clone(),
                },
                location: ErrorLocation::Node {
                    id: verifier_key.clone(),
                },
                message: format!(
                    "verifier `{}` and implementer `{}` both resolve to agent \
                     runtime `{}` (declared or defaulted) — same-vendor \
                     verification cannot catch that vendor's own blind spots",
                    verifier_key.as_str(),
                    implementer_key.as_str(),
                    verifier_runtime
                ),
            });
        }
    }
}

/// W6 — the graph verifies only at the end.
///
/// Counts `Agent` nodes, at the top level **and inside subgraph bodies**
/// (loop archetypes put the verifier in the task body), splitting them into
/// verification-authority nodes — those declaring an outcome with
/// [`LedgerEffect::Verified`](crate::node::LedgerEffect) — and work nodes.
/// One gate against two or more work nodes means at most the last hand-off
/// is checked; see [`ValidationErrorKind::EndOfPipelineVerification`] for the
/// measurement that makes this worth saying.
///
/// Zero gates is [`warning_w4_unverified_success`]'s finding, not this one,
/// and a single work node has only one boundary to gate — both are silent
/// here.
fn warning_w6_end_of_pipeline_verification(graph: &Graph, out: &mut Vec<ValidationError>) {
    use crate::node::{LedgerEffect, NodeKind};

    let mut work = 0usize;
    let mut gates: Vec<NodeKey> = Vec::new();

    let mut tally = |nodes: &std::collections::BTreeMap<NodeKey, crate::node::Node>| {
        for (key, node) in nodes {
            if node.kind() != NodeKind::Agent {
                continue;
            }
            if node
                .declared_outcomes
                .iter()
                .any(|outcome| outcome.ledger_effect == LedgerEffect::Verified)
            {
                gates.push(key.clone());
            } else {
                work += 1;
            }
        }
    };
    tally(&graph.nodes);
    for subgraph in graph.subgraphs.values() {
        tally(&subgraph.nodes);
    }

    let [gate] = gates.as_slice() else {
        return;
    };
    if work < 2 {
        return;
    }

    out.push(ValidationError {
        kind: ValidationErrorKind::EndOfPipelineVerification {
            gate: gate.clone(),
            work_nodes: work,
        },
        location: ErrorLocation::Node { id: gate.clone() },
        message: format!(
            "`{}` is the only verification-authority node across {work} work-producing \
             agent nodes, so at most the final hand-off is checked; verification placed \
             only at the end measured 58.4% hallucination survival against 60.7% for none \
             (arXiv:2608.14588)",
            gate.as_str()
        ),
    });
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
        let NodeConfig::Loop(cfg) = &node.config else {
            continue;
        };
        let IterableSource::Static(items) = &cfg.iterates_over else {
            continue;
        };
        if items.len() > MAX_LOOP_ITEMS_STATIC {
            errors.push(ValidationError {
                location: ErrorLocation::Node {
                    id: node.id.clone(),
                },
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
            let NodeConfig::Loop(cfg) = &node.config else {
                continue;
            };
            let IterableSource::Static(items) = &cfg.iterates_over else {
                continue;
            };
            if items.len() > MAX_LOOP_ITEMS_STATIC {
                errors.push(ValidationError {
                    location: ErrorLocation::Node {
                        id: node.id.clone(),
                    },
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

/// Walk every `Agent` node (root graph + subgraphs) and, when the node carries
/// a `sandbox_override` in `Custom` mode, run [`crate::sandbox::validate_custom`].
/// Each violation surfaces as a per-node [`ValidationError`].
fn validate_sandbox_custom_on_agents(graph: &Graph, out: &mut Vec<ValidationError>) {
    use crate::sandbox::{SandboxValidationError, validate_custom};

    let push = |node: &crate::node::Node,
                errs: Vec<SandboxValidationError>,
                out: &mut Vec<ValidationError>| {
        for e in errs {
            let loc = ErrorLocation::Node {
                id: node.id.clone(),
            };
            let (kind, message) = match e {
                SandboxValidationError::CustomAllAllowlistsEmpty => (
                    ValidationErrorKind::SandboxCustomEmpty {
                        node: node.id.clone(),
                    },
                    format!(
                        "agent node `{}` declared `sandbox.mode = custom` but every \
                         allowlist (writable_roots, network_allowlist, shell_allowlist) is empty",
                        node.id.as_str(),
                    ),
                ),
                SandboxValidationError::WritableRootEscape { path } => (
                    ValidationErrorKind::SandboxWritableRootEscape {
                        node: node.id.clone(),
                        path: path.clone(),
                    },
                    format!(
                        "agent node `{}` writable_root `{}` contains `..` path segments",
                        node.id.as_str(),
                        path,
                    ),
                ),
                SandboxValidationError::NetworkPatternInvalid { entry } => (
                    ValidationErrorKind::SandboxNetworkPatternInvalid {
                        node: node.id.clone(),
                        entry: entry.clone(),
                    },
                    format!(
                        "agent node `{}` network_allowlist entry `{}` is not a valid host/IP pattern",
                        node.id.as_str(),
                        entry,
                    ),
                ),
                SandboxValidationError::ShellMetacharacters { entry } => (
                    ValidationErrorKind::SandboxShellMetacharacters {
                        node: node.id.clone(),
                        entry: entry.clone(),
                    },
                    format!(
                        "agent node `{}` shell_allowlist entry `{}` contains shell metacharacters",
                        node.id.as_str(),
                        entry,
                    ),
                ),
            };
            out.push(ValidationError {
                kind,
                location: loc,
                message,
            });
        }
    };

    let walk_node = |node: &crate::node::Node, out: &mut Vec<ValidationError>| {
        let NodeConfig::Agent(cfg) = &node.config else {
            return;
        };
        let Some(sandbox) = cfg.sandbox_override.as_ref() else {
            return;
        };
        let errs = validate_custom(sandbox);
        if !errs.is_empty() {
            push(node, errs, out);
        }
    };

    for node in graph.nodes.values() {
        walk_node(node, out);
    }
    for sg in graph.subgraphs.values() {
        for node in sg.nodes.values() {
            walk_node(node, out);
        }
    }
}

/// Every `Agent` node's `custom_fields["skills"]` must deserialize as a list
/// of `surge_core::skill::SkillRef` (via `AgentConfig::declared_skills`).
/// Runs at graph-load time — before `PipelineMaterialized`, before a
/// worktree is created — so a malformed declaration is a validation
/// finding naming the node and the parse reason, not a run that starts,
/// spends setup work, and only then fails the first time that node's
/// stage is entered.
fn validate_declared_skills(graph: &Graph, out: &mut Vec<ValidationError>) {
    let walk_node = |node: &crate::node::Node, out: &mut Vec<ValidationError>| {
        let NodeConfig::Agent(cfg) = &node.config else {
            return;
        };
        if let Err(err) = cfg.declared_skills() {
            out.push(ValidationError {
                kind: ValidationErrorKind::InvalidSkillsDeclaration {
                    node: node.id.clone(),
                    reason: err.to_string(),
                },
                location: ErrorLocation::Node {
                    id: node.id.clone(),
                },
                message: format!(
                    "agent node `{}` declares an invalid `skills` list: {err}",
                    node.id.as_str(),
                ),
            });
        }
    };

    for node in graph.nodes.values() {
        walk_node(node, out);
    }
    for sg in graph.subgraphs.values() {
        for node in sg.nodes.values() {
            walk_node(node, out);
        }
    }
}

fn warning_w3_notify_outcomes(graph: &Graph, out: &mut Vec<ValidationError>) {
    let delivered = OutcomeKey::try_from("delivered").expect("'delivered' is valid OutcomeKey");
    let undeliverable =
        OutcomeKey::try_from("undeliverable").expect("'undeliverable' is valid OutcomeKey");

    // Inner helper — checks a single node and appends findings.
    let check_node = |node: &crate::node::Node, out: &mut Vec<ValidationError>| {
        let NodeConfig::Notify(cfg) = &node.config else {
            return;
        };

        let has_delivered = node.declared_outcomes.iter().any(|o| o.id == delivered);
        if !has_delivered {
            out.push(ValidationError {
                kind: ValidationErrorKind::NotifyMissingDelivered {
                    node: node.id.clone(),
                },
                location: ErrorLocation::Node {
                    id: node.id.clone(),
                },
                message: format!(
                    "notify node `{}` must declare a `delivered` outcome",
                    node.id.as_str()
                ),
            });
        }

        if matches!(cfg.on_failure, NotifyFailureAction::Fail) {
            let has_undeliverable = node.declared_outcomes.iter().any(|o| o.id == undeliverable);
            if !has_undeliverable {
                out.push(ValidationError {
                    kind: ValidationErrorKind::NotifyFailMissingUndeliverable {
                        node: node.id.clone(),
                    },
                    location: ErrorLocation::Node {
                        id: node.id.clone(),
                    },
                    message: format!(
                        "notify node `{}` uses on_failure: Fail but does not declare \
                         an `undeliverable` outcome; failed deliveries will halt the run",
                        node.id.as_str()
                    ),
                });
            }
        }
    };

    for node in graph.nodes.values() {
        check_node(node, out);
    }
    // Also recurse into subgraphs — Notify nodes can appear inside Loop/Subgraph
    // bodies (M6 primary use case: notify fan-out inside a loop body).
    for sg in graph.subgraphs.values() {
        for node in sg.nodes.values() {
            check_node(node, out);
        }
    }
}

/// Validate per-stage `ToolOverride::mcp_add` references resolve in
/// the run-level registry. Returns errors for unresolved names.
#[must_use]
pub fn validate_mcp_references(
    stage_name: &str,
    stage_cfg: &crate::agent_config::AgentConfig,
    registry: &[crate::mcp_config::McpServerRef],
) -> Vec<ValidationError> {
    let mut out = Vec::new();
    let Some(overrides) = &stage_cfg.tool_overrides else {
        return out;
    };
    let known: std::collections::HashSet<&str> = registry.iter().map(|r| r.name.as_str()).collect();
    for name in &overrides.mcp_add {
        if !known.contains(name.as_str()) {
            out.push(ValidationError {
                kind: ValidationErrorKind::McpServerUndeclared {
                    stage: stage_name.to_string(),
                    server: name.clone(),
                },
                location: ErrorLocation::Graph,
                message: format!(
                    "stage `{stage_name}` references MCP server `{name}` \
                     which is not declared in the run-level registry"
                ),
            });
        }
    }
    out
}

/// Validate a single `McpServerRef` for safety / well-formedness.
#[must_use]
pub fn validate_mcp_server_ref(r: &crate::mcp_config::McpServerRef) -> Vec<ValidationError> {
    let mut out = Vec::new();
    if r.name.is_empty() {
        out.push(ValidationError {
            kind: ValidationErrorKind::McpServerNameEmpty,
            location: ErrorLocation::Graph,
            message: "MCP server `name` must not be empty".into(),
        });
    }
    match &r.transport {
        crate::mcp_config::McpTransportConfig::Stdio { command, .. } => {
            // Reject `..` traversal segments. Pure-name (no slash) and
            // absolute paths are fine; only `..` as a path component is
            // rejected. Adding a new transport variant will cause a
            // compile-time exhaustiveness error here, forcing the validator
            // to explicitly handle it.
            let s = command.to_string_lossy();
            if s.split(['/', '\\']).any(|seg| seg == "..") {
                out.push(ValidationError {
                    kind: ValidationErrorKind::McpCommandPathUnsafe {
                        command: s.into_owned(),
                    },
                    location: ErrorLocation::Graph,
                    message: format!(
                        "MCP server `{}` command `{}` contains `..` path \
                         traversal segments",
                        r.name,
                        command.display()
                    ),
                });
            }
        },
    }
    out
}

/// Combined graph + run-config validation. Aggregates graph-level
/// errors with M7 MCP-aware rules that need both the graph (for
/// stage configs) and run config (for the MCP server registry).
///
/// Engine and editor consumers call this when they have both pieces;
/// pure-graph callers (e.g., TOML lint) use the existing graph-level
/// `validate(...)` directly.
#[must_use]
pub fn validate_with_run_config(
    graph: &crate::graph::Graph,
    run_config: &crate::run_event::RunConfig,
) -> Vec<ValidationError> {
    // Flatten the Result — both Ok (warnings) and Err (errors) are findings.
    let mut out = match validate(graph) {
        Ok(warnings) => warnings,
        Err(errors) => errors,
    };

    // Per-server well-formedness.
    for server in &run_config.mcp_servers {
        out.extend(validate_mcp_server_ref(server));
    }

    // Per-stage MCP allowlist resolution.
    for (key, node) in &graph.nodes {
        if let crate::node::NodeConfig::Agent(agent_cfg) = &node.config {
            out.extend(validate_mcp_references(
                key.as_str(),
                agent_cfg,
                &run_config.mcp_servers,
            ));
        }
    }

    out
}

// ── W4: unverified success path ──────────────────────────────────

// ── W5: same-runtime verification ────────────────────────────────

#[cfg(test)]
mod resolver_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod w4_tests;
#[cfg(test)]
mod w5_tests;
#[cfg(test)]
mod w6_tests;
