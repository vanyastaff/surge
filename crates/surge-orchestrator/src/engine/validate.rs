//! Pre-execution graph validation for M6: allows Loop and Subgraph nodes,
//! rejects `gate_after_each: true` (M7) and multi-edge fanout from the same
//! `(node, outcome)` port (M8+), validates subgraph references, and — as of
//! Ticket 18 — delegates into [`surge_core::validate`] for the 21 structural
//! and content rules that module owns (reachability, terminal-reachable,
//! one-edge-per-outcome, node-key uniqueness, `InvalidSkillsDeclaration`,
//! etc.). Before Ticket 18, [`validate_for_m6`] was called on every
//! `Engine::start_run` but never ran those 21 rules — they were reachable
//! only through [`validate_for_m6_with_resolver`], which had zero
//! non-test callers. A measurement pass against all 13 bundled flows, all 9
//! example flows, and all repository/fixture graphs found exactly one
//! violation (a test-only fixture referencing a nonexistent artifact-producer
//! node, fixed alongside this wiring) — every shipped flow was already clean.

use crate::engine::error::EngineError;
use surge_core::edge::{Edge, EdgeKind};
use surge_core::graph::Graph;
use surge_core::keys::NodeKey;

/// Validate the graph for M6 execution. Allows Loop and Subgraph nodes
/// (M5 rejected them). Rejects multi-edge fanout (M8+) and
/// `gate_after_each: true` (M7). Also runs the full
/// [`surge_core::validate`] rule set (see module docs) — a graph that
/// passes the M6-specific checks below but fails a `surge_core` structural
/// rule (e.g. an unreachable node, or a malformed `skills` declaration) is
/// still rejected here, before the caller creates any run state or worktree.
///
/// # Errors
/// Returns [`EngineError::GraphInvalid`] for the first M6-specific
/// violation found, or for every `Severity::Error` finding from
/// [`surge_core::validate`] once the M6-specific checks pass.
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
        if let surge_core::node::NodeConfig::Subgraph(cfg) = &node.config
            && !graph.subgraphs.contains_key(&cfg.inner)
        {
            return Err(EngineError::SubgraphMissing(cfg.inner.clone()));
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

    // Pre-execution livelock guard: every cycle in the outer graph must
    // contain at least one `EdgeKind::Backtrack` edge. Pure-Forward cycles
    // would loop the engine forever — bootstrap edit loops use Backtrack
    // edges as the explicit, opt-in cycle marker (Task 27 wires the
    // runtime; this rule keeps validators in step with that contract).
    if let Some(cycle) = find_forward_only_cycle(&graph.edges) {
        return Err(EngineError::ForwardCycleDetected { nodes: cycle });
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
        // Same livelock guard inside each subgraph: pure-Forward cycles
        // in a body subgraph would loop the engine forever just like
        // outer-graph cycles do.
        if let Some(cycle) = find_forward_only_cycle(&sg.edges) {
            return Err(EngineError::ForwardCycleDetected { nodes: cycle });
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
            if let surge_core::node::NodeConfig::Subgraph(cfg) = &node.config
                && !graph.subgraphs.contains_key(&cfg.inner)
            {
                return Err(EngineError::SubgraphMissing(cfg.inner.clone()));
            }
        }
    }

    apply_surge_core_validation(graph)?;

    Ok(())
}

/// Run [`surge_core::validate`] and surface every `Severity::Error` finding
/// as a single [`EngineError::GraphInvalid`]. `Severity::Warning` findings
/// (e.g. `UnverifiedSuccessPath`) do not block the run.
///
/// `UnverifiedSuccessPath` specifically is a *design-time* lint — could this
/// graph's shape let some run declare success without a verifier, considered
/// once at validation time, independent of any actual run — not itself
/// rendered by the operator-facing surfaces. The *run-time* counterpart this
/// comment used to point to unqualified ("rendered elsewhere") now has a
/// real home: `surge_core::evidence::is_evidence_backed` answers, from a
/// specific run's own event log, whether *that run's* completion was
/// actually verified, and Run Report / `surge inbox` / `surge ledger` all
/// render it (spec §10/R30). See `surge_core::evidence`'s module doc for
/// exactly how the two relate — they are complementary questions, not the
/// same check surfaced twice.
///
/// Factored out of [`validate_for_m6`] so [`validate_for_m6_with_resolver`]
/// does not need to duplicate the finding-to-message conversion; the latter
/// still makes its own (second, cheap) call into `surge_core::validate_with_resolver`
/// for the reference-specific findings `validate_for_m6` cannot see.
fn apply_surge_core_validation(graph: &Graph) -> Result<(), EngineError> {
    let Err(findings) = surge_core::validate(graph) else {
        return Ok(());
    };
    let messages: Vec<String> = findings
        .into_iter()
        .filter(|f| f.kind.severity() == surge_core::Severity::Error)
        .map(|f| format!("[structural] {}", f.message))
        .collect();
    if messages.is_empty() {
        return Ok(());
    }
    Err(EngineError::GraphInvalid(format!(
        "surge_core::validate failed: {}",
        messages.join("; ")
    )))
}

// Back-compat alias for any internal caller still using the M5 name.
#[allow(dead_code)]
pub use validate_for_m6 as validate_for_m5;

/// Validate that the graph's declared archetype matches its topology.
///
/// Runs in addition to [`validate_for_m6`] when the graph carries an
/// `[metadata.archetype]` block. Today only the `multi-milestone` archetype
/// has a structural rule: the topology must contain at least one `Loop` node
/// whose `iterates_over` resolves to an artifact-derived iterable named
/// `roadmap.milestones`. Other archetypes are linear-shaped and impose no
/// extra constraints; they pass through silently.
///
/// Used by the post-Flow-Generator validation hook (Task 11). When the graph
/// has no archetype block, this function is a no-op so legacy graphs continue
/// to validate without modification.
///
/// # Errors
/// Returns [`EngineError::ArchetypeMismatch`] when the declared archetype is
/// `multi-milestone` but no qualifying `Loop` is present.
pub fn validate_archetype_topology(graph: &Graph) -> Result<(), EngineError> {
    use surge_core::ArchetypeName;

    let Some(archetype) = graph.metadata.archetype.as_ref() else {
        return Ok(());
    };

    match archetype.name {
        ArchetypeName::MultiMilestone => {
            if contains_roadmap_milestones_loop(graph) {
                Ok(())
            } else {
                Err(EngineError::ArchetypeMismatch {
                    declared: archetype.name.as_str().to_owned(),
                    detected: "no Loop node iterating over an artifact named 'roadmap.milestones'"
                        .to_owned(),
                })
            }
        },
        // Linear / single-task archetypes have no extra structural rule
        // beyond `validate_for_m6`. The wildcard arm covers the
        // `#[non_exhaustive]` future; new archetypes that need a topology
        // rule must add an explicit arm above before relying on the
        // post-Flow-Generator validator to enforce it.
        _ => Ok(()),
    }
}

/// Whether `graph` contains at least one `Loop` node whose `iterates_over`
/// is an artifact-derived iterable named `roadmap.milestones`. Searches the
/// outer graph and every body subgraph so milestone loops nested in a
/// containing subgraph still satisfy the multi-milestone invariant.
fn contains_roadmap_milestones_loop(graph: &Graph) -> bool {
    use surge_core::loop_config::IterableSource;
    use surge_core::node::NodeConfig;

    fn loop_matches_milestones(cfg: &surge_core::loop_config::LoopConfig) -> bool {
        matches!(
            &cfg.iterates_over,
            IterableSource::Artifact { name, .. } if name == "roadmap.milestones"
        )
    }

    let outer_match = graph.nodes.values().any(|node| match &node.config {
        NodeConfig::Loop(cfg) => loop_matches_milestones(cfg),
        _ => false,
    });
    if outer_match {
        return true;
    }
    graph.subgraphs.values().any(|sg| {
        sg.nodes.values().any(|node| match &node.config {
            NodeConfig::Loop(cfg) => loop_matches_milestones(cfg),
            _ => false,
        })
    })
}

/// Find a cycle whose edges are all `EdgeKind::Forward`. Returns the
/// cycle's nodes in traversal order with the entry node repeated at the
/// end (e.g. `[a, b, a]`), or `None` if no such cycle exists.
///
/// The implementation filters the edge set down to Forward edges first
/// and runs an iterative DFS for back-edges on the resulting subgraph.
/// Backtrack edges (and the not-yet-implemented Escalate kind) are
/// excluded by construction, so any cycle reported here has every edge
/// equal to `EdgeKind::Forward`.
fn find_forward_only_cycle(edges: &[Edge]) -> Option<Vec<NodeKey>> {
    use std::collections::HashMap;

    #[derive(Clone, Copy)]
    enum Color {
        Gray,
        Black,
    }

    let forward_edges: Vec<&Edge> = edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Forward))
        .collect();

    let mut adj: HashMap<&NodeKey, Vec<&NodeKey>> = HashMap::new();
    for e in &forward_edges {
        adj.entry(&e.from.node).or_default().push(&e.to);
    }

    let mut color: HashMap<&NodeKey, Color> = HashMap::new();

    let starts: Vec<&NodeKey> = adj.keys().copied().collect();
    for start in starts {
        if color.contains_key(start) {
            continue;
        }
        // Iterative DFS keeping a per-frame index into the adjacency list
        // so we can resume neighbour iteration after recursion.
        let mut path: Vec<&NodeKey> = vec![start];
        let mut iter_idx: Vec<usize> = vec![0];
        color.insert(start, Color::Gray);

        while let Some(&node) = path.last() {
            let &i = iter_idx.last()?;
            let neighbours = adj.get(node);
            let len = neighbours.map_or(0, Vec::len);
            if i < len {
                let last_idx = iter_idx.last_mut()?;
                *last_idx += 1;
                let target = neighbours.and_then(|n| n.get(i)).copied()?;
                match color.get(target) {
                    None => {
                        color.insert(target, Color::Gray);
                        path.push(target);
                        iter_idx.push(0);
                    },
                    Some(Color::Gray) => {
                        // Back-edge → cycle. Reconstruct the cycle starting
                        // from the first occurrence of `target` in the
                        // current DFS path and append `target` at the end
                        // for human-readable reporting (`[a, b, a]`).
                        let idx = path.iter().position(|n| *n == target)?;
                        let mut report: Vec<NodeKey> =
                            path[idx..].iter().map(|n| (*n).clone()).collect();
                        report.push(target.clone());
                        return Some(report);
                    },
                    Some(Color::Black) => {
                        // Already finished — no new cycle through it.
                    },
                }
            } else {
                color.insert(node, Color::Black);
                path.pop();
                iter_idx.pop();
            }
        }
    }
    None
}

/// `validate_for_m6` plus the `surge_core::ReferenceResolver` lookups for
/// profiles, templates, and named agents.
///
/// As of Ticket 18, `validate_for_m6` itself already runs the full
/// `surge_core::validate` structural rule set (see that function's docs),
/// so the *additional* value this wrapper adds is narrower than its name
/// suggests: only the three resolver-backed reference checks below.
///
/// # ⚠️ These three rules are inert in production today
/// `ProfileNotFound`, `TemplateNotFound`, and `NamedAgentNotFound` only ever
/// fire when called with a `ReferenceResolver` that actually knows the
/// registry contents. Every `impl ReferenceResolver` in this workspace is
/// test-only (`surge-core`'s own test module, `surge-cli/tests/`,
/// `surge-orchestrator/tests/`) — there is no `ProfileRegistry`-backed
/// production implementation, and nothing in the engine's production start
/// path (`Engine::start_run`, `commands/engine.rs::run_command`) calls this
/// function at all; they call the resolver-free `validate_for_m6`. Building
/// that adapter (wrapping `crate::profile_loader::ProfileRegistry` — plus
/// whatever registries would answer `template_exists`/`named_agent_exists`,
/// neither of which exists yet either) and wiring it into `Engine::start_run`
/// is a separate, larger task: it is out of Ticket 18's declared zone
/// (`engine/validate.rs`, `engine/bootstrap.rs`, `commands/engine.rs` — a
/// resolver adapter is a new production type, not a call-site wiring) and
/// deserves its own measurement pass the same way this ticket demanded one,
/// since flipping it on can start rejecting graphs whose profile references
/// have never been checked before.
///
/// # Errors
/// - All `validate_for_m6` errors (M6-specific checks plus the full
///   `surge_core::validate` structural rule set).
/// - [`EngineError::GraphInvalid`] for every `Severity::Error` finding
///   reported by `surge_core::validate_with_resolver` — in practice, since
///   `validate_for_m6` above already ran the structural rules and returned
///   early on any failure, only the resolver-specific diagnostics
///   (`ProfileNotFound`, `TemplateNotFound`, `NamedAgentNotFound`) can still
///   surface here. Tagged with a `[ref]` prefix (`[structural]` for the
///   defensive fallback arm, which today cannot be reached for the reason
///   above) so callers can distinguish them at a glance.
pub fn validate_for_m6_with_resolver(
    graph: &Graph,
    resolver: &dyn surge_core::ReferenceResolver,
) -> Result<(), EngineError> {
    validate_for_m6(graph)?;

    // `validate_for_m6` above already ran `surge_core::validate`'s
    // structural rules via `apply_surge_core_validation` and returned early
    // on any Severity::Error finding, so `validate_with_resolver`'s own
    // (redundant, cheap) structural pass below cannot contribute a new
    // error — only the resolver-specific reference checks can.
    if let Err(findings) = surge_core::validate_with_resolver(graph, resolver) {
        let mut messages: Vec<String> = Vec::new();
        for finding in findings {
            if finding.kind.severity() != surge_core::Severity::Error {
                continue;
            }
            let label = match finding.kind {
                surge_core::ValidationErrorKind::ProfileNotFound { .. }
                | surge_core::ValidationErrorKind::TemplateNotFound { .. }
                | surge_core::ValidationErrorKind::NamedAgentNotFound { .. } => "ref",
                _ => "structural",
            };
            messages.push(format!("[{label}] {}", finding.message));
        }
        if !messages.is_empty() {
            return Err(EngineError::GraphInvalid(format!(
                "validate_with_resolver failed: {}",
                messages.join("; ")
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
                archetype: None,
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
        use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
        use surge_core::graph::Subgraph;
        use surge_core::keys::{EdgeKey, SubgraphKey};
        use surge_core::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };
        use surge_core::node::OutcomeDecl;

        let loop_key = NodeKey::try_from("loop_1").unwrap();
        let end_key = NodeKey::try_from("end").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("body_start").unwrap();
        let completed_outcome = OutcomeKey::try_from("completed").unwrap();

        let mut nodes = BTreeMap::new();
        nodes.insert(
            loop_key.clone(),
            Node {
                id: loop_key.clone(),
                position: Position::default(),
                // A declared "completed" outcome, wired below to the outer
                // Terminal node — a bare Loop node with zero declared
                // outcomes and no Terminal node anywhere in the outer graph
                // is what `surge_core::validate`'s `NoTerminalReachable` rule
                // (correctly) rejects; this test is about Loop nodes being
                // *structurally* permitted in M6, not about that rule.
                declared_outcomes: vec![OutcomeDecl {
                    id: completed_outcome.clone(),
                    description: "all items processed".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: surge_core::LedgerEffect::default(),
                }],
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
        nodes.insert(
            end_key.clone(),
            Node {
                id: end_key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
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
                archetype: None,
            },
            start: loop_key.clone(),
            nodes,
            edges: vec![Edge {
                id: EdgeKey::try_from("e_loop_done").unwrap(),
                from: PortRef {
                    node: loop_key,
                    outcome: completed_outcome,
                },
                to: end_key,
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            }],
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
                archetype: None,
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
                archetype: None,
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

    fn terminal_node(name: &str) -> Node {
        terminal_node_with_outcomes(name, &[])
    }

    /// Like [`terminal_node`], but declares the given outcome ids —
    /// `surge_core::validate`'s `EdgeFromUndeclaredOutcome`/`OutcomeWithNoEdge`
    /// rules require every edge's source outcome to be declared on that node
    /// (used by [`graph_with_nodes_and_edges`], which derives the right set
    /// from the edges it's given).
    fn terminal_node_with_outcomes(name: &str, outcomes: &[&str]) -> Node {
        let key = NodeKey::try_from(name).unwrap();
        Node {
            id: key,
            position: Position::default(),
            declared_outcomes: outcomes
                .iter()
                .map(|o| surge_core::node::OutcomeDecl {
                    id: OutcomeKey::try_from(*o).unwrap(),
                    description: format!(
                        "synthetic outcome `{o}` for a validate.rs cycle-detection test"
                    ),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: surge_core::LedgerEffect::default(),
                })
                .collect(),
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        }
    }

    fn forward_edge(
        id: &str,
        from_node: &str,
        from_outcome: &str,
        to: &str,
    ) -> surge_core::edge::Edge {
        use surge_core::edge::{EdgePolicy, PortRef};
        use surge_core::keys::EdgeKey;
        surge_core::edge::Edge {
            id: EdgeKey::try_from(id).unwrap(),
            from: PortRef {
                node: NodeKey::try_from(from_node).unwrap(),
                outcome: OutcomeKey::try_from(from_outcome).unwrap(),
            },
            to: NodeKey::try_from(to).unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }
    }

    fn graph_with_nodes_and_edges(
        nodes: &[&str],
        start: &str,
        edges: Vec<surge_core::edge::Edge>,
    ) -> Graph {
        let mut node_map = BTreeMap::new();
        for n in nodes {
            let node_key = NodeKey::try_from(*n).unwrap();
            // Declare exactly the outcomes this node actually has outgoing
            // edges from — these are cycle-detection fixtures, so the node
            // kind (always Terminal here) is a stand-in and the specific
            // outcome id only matters insofar as an edge names it.
            let mut declared: Vec<&str> = edges
                .iter()
                .filter(|e| e.from.node == node_key)
                .map(|e| e.from.outcome.as_str())
                .collect();
            declared.sort_unstable();
            declared.dedup();
            node_map.insert(node_key, terminal_node_with_outcomes(n, &declared));
        }
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "cycle-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: NodeKey::try_from(start).unwrap(),
            nodes: node_map,
            edges,
            subgraphs: BTreeMap::new(),
        }
    }

    #[test]
    fn forward_only_cycle_a_b_a_is_rejected() {
        // Pure-Forward cycle a -> b -> a is a livelock; the validator
        // refuses to start the run.
        let g = graph_with_nodes_and_edges(
            &["a", "b"],
            "a",
            vec![
                forward_edge("e_ab", "a", "done", "b"),
                forward_edge("e_ba", "b", "done", "a"),
            ],
        );
        let err = validate_for_m6(&g).unwrap_err();
        match err {
            EngineError::ForwardCycleDetected { nodes } => {
                let labels: Vec<String> = nodes.iter().map(ToString::to_string).collect();
                assert!(
                    labels.contains(&"a".to_string()) && labels.contains(&"b".to_string()),
                    "cycle report mentions both nodes: {labels:?}",
                );
                assert!(
                    labels.first() == labels.last(),
                    "cycle report repeats the entry node at the end: {labels:?}",
                );
            },
            other => panic!("expected ForwardCycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn cycle_with_one_backtrack_edge_is_accepted() {
        // a -> b (Forward), b -> a (Backtrack) — the bootstrap edit-loop
        // shape. Cycle is permitted because at least one edge is Backtrack.
        use surge_core::edge::{EdgePolicy, PortRef};
        use surge_core::keys::EdgeKey;

        let mut backtrack = surge_core::edge::Edge {
            id: EdgeKey::try_from("e_back").unwrap(),
            from: PortRef {
                node: NodeKey::try_from("b").unwrap(),
                outcome: OutcomeKey::try_from("edit").unwrap(),
            },
            to: NodeKey::try_from("a").unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        };
        backtrack.kind = EdgeKind::Backtrack;

        let g = graph_with_nodes_and_edges(
            &["a", "b"],
            "a",
            vec![forward_edge("e_ab", "a", "done", "b"), backtrack],
        );
        validate_for_m6(&g).expect("Backtrack-containing cycle is permitted");
    }

    #[test]
    fn dag_with_no_cycle_is_accepted() {
        // Regression guard for the previous (cycle-free) behaviour.
        let g = graph_with_nodes_and_edges(
            &["a", "b", "c"],
            "a",
            vec![
                forward_edge("e_ab", "a", "done", "b"),
                forward_edge("e_bc", "b", "done", "c"),
            ],
        );
        validate_for_m6(&g).expect("acyclic graph remains valid");
    }

    #[test]
    fn forward_self_loop_is_rejected() {
        // A self-loop a -> a with kind=Forward is a degenerate cycle of
        // length 1. Should still be rejected.
        let g = graph_with_nodes_and_edges(
            &["a"],
            "a",
            vec![forward_edge("e_self", "a", "again", "a")],
        );
        match validate_for_m6(&g).unwrap_err() {
            EngineError::ForwardCycleDetected { nodes } => {
                assert_eq!(nodes.len(), 2, "self-loop reports [a, a]");
                assert_eq!(nodes[0], NodeKey::try_from("a").unwrap());
                assert_eq!(nodes[1], NodeKey::try_from("a").unwrap());
            },
            other => panic!("expected ForwardCycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn forward_only_cycle_inside_subgraph_is_rejected() {
        // The same livelock guard applies to body subgraphs of Loop
        // nodes. A Forward-only cycle in a subgraph is also lethal.
        use surge_core::graph::Subgraph;
        use surge_core::keys::SubgraphKey;
        use surge_core::loop_config::{
            ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
        };

        let loop_key = NodeKey::try_from("loop_1").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_a = NodeKey::try_from("body_a").unwrap();
        let body_b = NodeKey::try_from("body_b").unwrap();

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
        body_nodes.insert(body_a.clone(), terminal_node("body_a"));
        body_nodes.insert(body_b.clone(), terminal_node("body_b"));

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            body_key,
            Subgraph {
                start: body_a,
                nodes: body_nodes,
                edges: vec![
                    forward_edge("e_body_ab", "body_a", "done", "body_b"),
                    forward_edge("e_body_ba", "body_b", "done", "body_a"),
                ],
            },
        );

        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "subgraph-cycle".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: loop_key,
            nodes,
            edges: vec![],
            subgraphs,
        };

        match validate_for_m6(&g).unwrap_err() {
            EngineError::ForwardCycleDetected { .. } => {},
            other => panic!("expected ForwardCycleDetected, got {other:?}"),
        }
    }

    // --- Task 11: validate_archetype_topology ---

    use surge_core::archetype::{ArchetypeMetadata, ArchetypeName};
    use surge_core::graph::Subgraph;
    use surge_core::keys::SubgraphKey;
    use surge_core::loop_config::{
        ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
    };

    fn loop_node(loop_key: &NodeKey, body_key: &SubgraphKey, iterable: IterableSource) -> Node {
        Node {
            id: loop_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: iterable,
                body: body_key.clone(),
                iteration_var_name: "milestone".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        }
    }

    fn graph_with_archetype_and_loop(
        archetype: Option<ArchetypeMetadata>,
        iterable: IterableSource,
    ) -> Graph {
        let loop_key = NodeKey::try_from("milestones").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("body_start").unwrap();

        let mut nodes = BTreeMap::new();
        nodes.insert(loop_key.clone(), loop_node(&loop_key, &body_key, iterable));

        let mut body_nodes = BTreeMap::new();
        body_nodes.insert(body_start.clone(), terminal_node("body_start"));

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            body_key,
            Subgraph {
                start: body_start,
                nodes: body_nodes,
                edges: vec![],
            },
        );

        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "archetype-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype,
            },
            start: loop_key,
            nodes,
            edges: vec![],
            subgraphs,
        }
    }

    fn multi_milestone_meta() -> ArchetypeMetadata {
        ArchetypeMetadata {
            name: ArchetypeName::MultiMilestone,
            milestones: Some(3),
            edit_loop_cap: None,
        }
    }

    #[test]
    fn archetype_topology_no_archetype_block_is_no_op() {
        let g = graph_with_archetype_and_loop(None, IterableSource::Static(vec![]));
        assert!(validate_archetype_topology(&g).is_ok());
    }

    #[test]
    fn archetype_topology_multi_milestone_with_matching_loop_passes() {
        let iterable = IterableSource::Artifact {
            node: NodeKey::try_from("roadmap_planner").unwrap(),
            name: "roadmap.milestones".into(),
            jsonpath: "$".into(),
        };
        let g = graph_with_archetype_and_loop(Some(multi_milestone_meta()), iterable);
        assert!(validate_archetype_topology(&g).is_ok());
    }

    #[test]
    fn archetype_topology_multi_milestone_without_loop_fails_with_mismatch() {
        let iterable = IterableSource::Artifact {
            node: NodeKey::try_from("roadmap_planner").unwrap(),
            name: "wrong.name".into(),
            jsonpath: "$".into(),
        };
        let g = graph_with_archetype_and_loop(Some(multi_milestone_meta()), iterable);
        match validate_archetype_topology(&g).unwrap_err() {
            EngineError::ArchetypeMismatch { declared, detected } => {
                assert_eq!(declared, "multi-milestone");
                assert!(detected.contains("roadmap.milestones"));
            },
            other => panic!("expected ArchetypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn archetype_topology_linear_archetypes_have_no_extra_rule() {
        let iterable = IterableSource::Static(vec![]);
        for variant in [
            ArchetypeName::Linear3,
            ArchetypeName::LinearWithReview,
            ArchetypeName::BugFix,
            ArchetypeName::Refactor,
            ArchetypeName::Spike,
            ArchetypeName::SingleTask,
        ] {
            let meta = ArchetypeMetadata {
                name: variant,
                milestones: None,
                edit_loop_cap: None,
            };
            let g = graph_with_archetype_and_loop(Some(meta), iterable.clone());
            assert!(
                validate_archetype_topology(&g).is_ok(),
                "{variant:?} should not require a milestone loop"
            );
        }
    }

    #[test]
    fn archetype_topology_multi_milestone_loop_inside_subgraph_passes() {
        // Outer node is Terminal; the milestone loop lives in a body subgraph.
        // The detector must descend into subgraphs for the rule to apply.
        let outer_key = NodeKey::try_from("entry").unwrap();
        let inner_loop_key = NodeKey::try_from("inner_loop").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("inner_start").unwrap();

        let mut outer_nodes = BTreeMap::new();
        outer_nodes.insert(outer_key.clone(), terminal_node("entry"));

        let mut body_nodes = BTreeMap::new();
        body_nodes.insert(body_start.clone(), terminal_node("inner_start"));
        body_nodes.insert(
            inner_loop_key.clone(),
            loop_node(
                &inner_loop_key,
                &body_key,
                IterableSource::Artifact {
                    node: NodeKey::try_from("roadmap_planner").unwrap(),
                    name: "roadmap.milestones".into(),
                    jsonpath: "$".into(),
                },
            ),
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
                name: "subgraph-archetype".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: Some(multi_milestone_meta()),
            },
            start: outer_key,
            nodes: outer_nodes,
            edges: vec![],
            subgraphs,
        };

        assert!(validate_archetype_topology(&g).is_ok());
    }

    #[test]
    fn golden_multi_milestone_flow_from_mock_agent_validates() {
        let raw = include_str!("../../tests/fixtures/golden_multi_milestone_flow.toml");
        let graph: Graph = toml::from_str(raw).expect("golden flow TOML parses");

        validate_for_m6(&graph).expect("golden multi-milestone graph is structurally valid");
        validate_archetype_topology(&graph)
            .expect("golden multi-milestone graph matches declared archetype");

        let archetype = graph
            .metadata
            .archetype
            .as_ref()
            .expect("golden flow declares archetype metadata");
        assert_eq!(archetype.name, ArchetypeName::MultiMilestone);
        assert_eq!(archetype.milestones, Some(3));
    }
}
