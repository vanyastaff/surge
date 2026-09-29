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

/// Reject profiles whose required prompt inputs are missing or ambiguous.
pub(crate) fn validate_agent_inputs(
    config: &surge_core::agent_config::AgentConfig,
    profile: &surge_core::profile::Profile,
) -> Result<(), EngineError> {
    for expected in profile
        .bindings
        .expected
        .iter()
        .filter(|input| !input.optional)
    {
        let bindings: Vec<_> = config
            .bindings
            .iter()
            .filter(|binding| binding.target.0 == expected.name)
            .collect();
        let [binding] = bindings.as_slice() else {
            return Err(EngineError::GraphInvalid(format!(
                "profile {} requires exactly one binding for '{}', found {}",
                config.profile,
                expected.name,
                bindings.len()
            )));
        };
        if binding.optional
            || matches!(
                &binding.source,
                surge_core::agent_config::ArtifactSource::Static { content } if content.trim().is_empty()
            )
        {
            return Err(EngineError::GraphInvalid(format!(
                "profile {} requires nonoptional, nonempty input '{}'",
                config.profile, expected.name
            )));
        }
    }
    Ok(())
}

/// Validate required profile bindings in the root graph and every loop/subgraph body.
pub(crate) fn validate_profile_inputs(
    graph: &Graph,
    registry: &crate::profile_loader::ProfileRegistry,
) -> Result<(), EngineError> {
    for node in graph
        .nodes
        .values()
        .chain(graph.subgraphs.values().flat_map(|g| g.nodes.values()))
    {
        let surge_core::node::NodeConfig::Agent(config) = &node.config else {
            continue;
        };
        let key = surge_core::profile::keyref::parse_key_ref(config.profile.as_str())
            .map_err(|e| EngineError::GraphInvalid(format!("node {}: {e}", node.id)))?;
        let profile = registry
            .resolve(&key)
            .map_err(|e| EngineError::GraphInvalid(format!("node {}: {e}", node.id)))?;
        validate_agent_inputs(config, &profile.profile)
            .map_err(|e| EngineError::GraphInvalid(format!("node {}: {e}", node.id)))?;
        validate_binding_sources(graph, config)?;
    }
    Ok(())
}

fn validate_binding_sources(
    graph: &Graph,
    config: &surge_core::agent_config::AgentConfig,
) -> Result<(), EngineError> {
    use surge_core::agent_config::ArtifactSource;
    for binding in &config.bindings {
        match &binding.source {
            ArtifactSource::GlobPattern { .. } => {
                return Err(EngineError::GraphInvalid(format!(
                    "binding '{}' uses unsupported glob_pattern source",
                    binding.target.0
                )));
            },
            ArtifactSource::NodeOutput { node, .. }
                if !graph.nodes.contains_key(node)
                    && !graph
                        .subgraphs
                        .values()
                        .any(|body| body.nodes.contains_key(node)) =>
            {
                return Err(EngineError::GraphInvalid(format!(
                    "binding '{}' references missing producer node '{node}'",
                    binding.target.0
                )));
            },
            _ => {},
        }
    }
    Ok(())
}

/// Check externally supplied loop inputs before creating a run or dispatching agents.
pub(crate) fn validate_loop_seeds(
    graph: &Graph,
    seeds: &[crate::engine::config::RunSeedArtifact],
) -> Result<(), EngineError> {
    use crate::engine::stage::loop_stage::{parse_iterable_artifact, resolve_array_path};
    use surge_core::{loop_config::IterableSource, node::NodeConfig};

    let nodes = graph.nodes.values().chain(
        graph
            .subgraphs
            .values()
            .flat_map(|body| body.nodes.values()),
    );
    for node in nodes {
        let NodeConfig::Loop(config) = &node.config else {
            continue;
        };
        let IterableSource::RunArtifact { name, jsonpath } = &config.iterates_over else {
            continue;
        };
        let matches: Vec<_> = seeds.iter().filter(|seed| seed.name == *name).collect();
        let [seed] = matches.as_slice() else {
            return Err(EngineError::GraphInvalid(format!(
                "loop {} requires exactly one run artifact '{name}', found {}",
                node.id,
                matches.len()
            )));
        };
        let parsed = parse_iterable_artifact(name, &seed.content)
            .map_err(|e| EngineError::GraphInvalid(format!("loop {}: {e}", node.id)))?;
        resolve_array_path(&parsed, jsonpath)
            .map_err(|e| EngineError::GraphInvalid(format!("loop {}: {e}", node.id)))?;
    }
    Ok(())
}

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
/// (e.g. `UnverifiedSuccessPath`) do not block the run, but — unlike before
/// this function logged them — they are no longer silently dropped: each is
/// emitted as a `tracing::warn!` event under the `engine::validate` target,
/// so an operator watching the run's logs sees the same findings
/// `surge_core::validate` produced instead of losing them at this one call
/// site. `surge_core::validate` itself returns `Ok(warnings)` when there are
/// no errors — a bug here previously matched only the `Err` arm, so the
/// entire `Ok` branch (a graph with warnings but no errors — the common
/// case) never even inspected its findings.
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
    let (Ok(findings) | Err(findings)) = surge_core::validate(graph);

    let mut error_messages: Vec<String> = Vec::new();
    for finding in findings {
        if finding.kind.severity() == surge_core::Severity::Error {
            error_messages.push(format!("[structural] {}", finding.message));
            continue;
        }
        tracing::warn!(
            target: "engine::validate",
            kind = ?finding.kind,
            location = ?finding.location,
            "{}",
            finding.message,
        );
    }
    if error_messages.is_empty() {
        return Ok(());
    }
    Err(EngineError::GraphInvalid(format!(
        "surge_core::validate failed: {}",
        error_messages.join("; ")
    )))
}

// Back-compat alias for any internal caller still using the M5 name.
#[allow(dead_code)]
pub use validate_for_m6 as validate_for_m5;

/// Require a `[metadata.archetype]` block on a generated graph.
///
/// ADR 0005: Flow Generator output must name its archetype so telemetry,
/// replay and the flow gate can bucket the run. Hand-authored templates are
/// not held to this; only the post-Flow-Generator hook calls it.
///
/// # Errors
/// Returns [`EngineError::ArchetypeMissing`] when the block is absent.
pub fn require_archetype(graph: &Graph) -> Result<surge_core::ArchetypeName, EngineError> {
    graph
        .metadata
        .archetype
        .as_ref()
        .map(|archetype| archetype.name)
        .ok_or_else(|| {
            EngineError::ArchetypeMissing(format!(
                "flow {:?} has no [metadata.archetype] block; pick exactly one of: {}",
                graph.metadata.name,
                surge_core::ArchetypeName::ALL
                    .iter()
                    .map(surge_core::ArchetypeName::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// Validate that the graph's declared archetype matches its topology.
///
/// Runs in addition to [`validate_for_m6`] when the graph carries an
/// `[metadata.archetype]` block. The one strict rule is the milestone loop,
/// because the runtime's milestone progression depends on it:
///
/// - `multi-milestone` must contain a `Loop` over `roadmap.milestones`;
/// - every other archetype must not, since iterating the roadmap's
///   milestones *is* the multi-milestone shape and a graph that does it
///   under another name mislabels the run.
///
/// Archetype-specific first steps (reproduce, characterise, baseline, audit,
/// plan) are advisory per ADR 0005 — see [`archetype_advisories`]; each one
/// found is logged as a warning here.
///
/// When the graph has no archetype block, this function is a no-op so
/// hand-authored graphs continue to validate; [`require_archetype`] is the
/// separate check for generated flows.
///
/// # Errors
/// Returns [`EngineError::ArchetypeMismatch`] when the milestone-loop rule is
/// violated in either direction.
pub fn validate_archetype_topology(graph: &Graph) -> Result<(), EngineError> {
    use surge_core::ArchetypeName;

    let Some(archetype) = graph.metadata.archetype.as_ref() else {
        return Ok(());
    };

    let has_milestone_loop = contains_roadmap_milestones_loop(graph);
    match (archetype.name, has_milestone_loop) {
        (ArchetypeName::MultiMilestone, false) => {
            return Err(EngineError::ArchetypeMismatch {
                declared: archetype.name.as_str().to_owned(),
                detected: "no Loop node iterating over an artifact named 'roadmap.milestones'"
                    .to_owned(),
            });
        },
        (name, true) if name != ArchetypeName::MultiMilestone => {
            return Err(EngineError::ArchetypeMismatch {
                declared: name.as_str().to_owned(),
                detected: "a Loop over 'roadmap.milestones', which is the multi-milestone shape; \
                           declare multi-milestone or iterate a single milestone's tasks"
                    .to_owned(),
            });
        },
        _ => {},
    }

    for advisory in archetype_advisories(graph) {
        tracing::warn!(
            target: "engine::validate",
            archetype = archetype.name.as_str(),
            flow = %graph.metadata.name,
            "{advisory}"
        );
    }
    Ok(())
}

/// Advisory (non-blocking) checks that a graph contains the stage its
/// archetype is named for.
///
/// Specialised archetypes exist because their first step — reproducing the
/// bug, pinning behaviour before a refactor, measuring a baseline — is what
/// makes the rest checkable. A graph labelled `bug-fix` with no reproduce
/// stage has lost that. Detection is by node key or bound profile name, so
/// the result is a hint, not proof; ADR 0005 keeps these soft.
///
/// Returns one message per missing stage; empty when the graph has no
/// archetype block or the archetype has no named stage.
#[must_use]
pub fn archetype_advisories(graph: &Graph) -> Vec<String> {
    use surge_core::ArchetypeName;

    let Some(archetype) = graph.metadata.archetype.as_ref() else {
        return Vec::new();
    };
    let (stage, markers): (&str, &[&str]) = match archetype.name {
        ArchetypeName::BugFix => ("reproduce", &["reproduc", "bug-fix-implementer"]),
        ArchetypeName::Refactor => (
            "behaviour characterisation",
            &[
                "characteri",
                "behaviour",
                "behavior",
                "refactor-implementer",
            ],
        ),
        ArchetypeName::Performance => ("baseline measurement", &["baseline", "bench"]),
        ArchetypeName::Security => ("security audit", &["audit", "security"]),
        ArchetypeName::Migration => ("migration plan or rollback check", &["migrat", "rollback"]),
        ArchetypeName::Docs => ("documentation", &["doc"]),
        ArchetypeName::LinearWithReview | ArchetypeName::CodeReview => ("review", &["review"]),
        _ => return Vec::new(),
    };

    let mentions_marker = agent_stage_labels(graph).any(|label| {
        let label = label.to_ascii_lowercase();
        markers.iter().any(|marker| label.contains(marker))
    });
    if mentions_marker {
        Vec::new()
    } else {
        vec![format!(
            "{} flow has no {stage} stage: no agent node key or profile mentions any of {markers:?}",
            archetype.name.as_str()
        )]
    }
}

/// Node keys and bound profile names of every Agent node, outer graph and
/// subgraphs alike.
fn agent_stage_labels(graph: &Graph) -> impl Iterator<Item = &str> {
    use surge_core::node::NodeConfig;

    graph
        .nodes
        .iter()
        .chain(graph.subgraphs.values().flat_map(|sg| sg.nodes.iter()))
        .filter_map(|(key, node)| match &node.config {
            NodeConfig::Agent(cfg) => Some([key.as_str(), cfg.profile.as_str()]),
            _ => None,
        })
        .flatten()
}

/// Whether `graph` contains at least one `Loop` node whose `iterates_over`
/// is an artifact-derived iterable named `roadmap.milestones`. Searches the
/// outer graph and every body subgraph so milestone loops nested in a
/// containing subgraph still satisfy the multi-milestone invariant.
fn contains_roadmap_milestones_loop(graph: &Graph) -> bool {
    use surge_core::loop_config::IterableSource;
    use surge_core::node::NodeConfig;

    fn loop_matches_milestones(cfg: &surge_core::loop_config::LoopConfig) -> bool {
        match &cfg.iterates_over {
            IterableSource::RunArtifact { name, jsonpath } => {
                name == "roadmap" && matches!(jsonpath.as_str(), "milestones" | "$.milestones[*]")
            },
            IterableSource::Artifact { name, .. } => name == "roadmap.milestones",
            _ => false,
        }
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
/// profiles, templates, named agents, and same-runtime verification.
///
/// As of Ticket 18, `validate_for_m6` itself already runs the full
/// `surge_core::validate` structural rule set (see that function's docs),
/// so the *additional* value this wrapper adds is narrower than its name
/// suggests: only the resolver-backed checks below.
///
/// `Engine::start_run` calls this instead of the resolver-free
/// `validate_for_m6` whenever `self.config.profile_registry` is `Some`;
/// `ProfileRegistry`'s `ReferenceResolver` impl (`profile_loader::resolver`)
/// is the production adapter passed in. That adapter actually answers two
/// of the four resolver-backed rules — `ProfileNotFound` and
/// `SameRuntimeVerification` (Warning) — while `TemplateNotFound` /
/// `NamedAgentNotFound` stay permissively silent: `ProfileRegistry` has no
/// template or named-agent registry to check against (ticket 22).
///
/// # Errors
/// - All `validate_for_m6` errors (M6-specific checks plus the full
///   `surge_core::validate` structural rule set).
/// - [`EngineError::GraphInvalid`] for every `Severity::Error` finding
///   reported by `surge_core::validate_with_resolver` — in practice, since
///   `validate_for_m6` above already ran the structural rules and returned
///   early on any failure, only the resolver-specific diagnostics
///   (`ProfileNotFound`, `TemplateNotFound`, `NamedAgentNotFound`) can still
///   surface here as errors. Tagged with a `[ref]` prefix (`[structural]`
///   for the defensive fallback arm, which today cannot be reached for the
///   reason above) so callers can distinguish them at a glance.
/// - `Severity::Warning` findings the resolver pass adds beyond what
///   `validate_for_m6` already saw (currently only `SameRuntimeVerification`)
///   never turn into an `Err` here — each is logged via `tracing::warn!`
///   under the `engine::validate` target rather than silently dropped, the
///   same way the private helper `validate_for_m6` calls for the structural
///   pass does. See this function's body comment for why only the
///   *resolver-added* findings are re-examined here at all: logging every
///   finding `surge_core::validate_with_resolver` returns would re-log the
///   structural (W1–W4) findings `validate_for_m6` already logged once.
pub fn validate_for_m6_with_resolver(
    graph: &Graph,
    resolver: &dyn surge_core::ReferenceResolver,
) -> Result<(), EngineError> {
    validate_for_m6(graph)?;

    // `validate_for_m6` above already ran `surge_core::validate`'s
    // structural rules once (via its private `apply_surge_core_validation`
    // helper) and logged every non-Error finding under `engine::validate`.
    // `surge_core::validate_with_resolver` below re-runs that same
    // structural pass internally (it calls `validate(graph)` as its first
    // step) purely to fold the resolver-specific findings into one Vec —
    // so `findings` here is a superset: the identical structural findings
    // already logged above, plus whatever the resolver-backed rules
    // (`ProfileNotFound`/`TemplateNotFound`/`NamedAgentNotFound`/
    // `SameRuntimeVerification`) newly contribute. Only that added subset
    // is inspected below; the rest is skipped so nothing is logged twice
    // under the same target.
    let (Ok(findings) | Err(findings)) = surge_core::validate_with_resolver(graph, resolver);
    let mut error_messages: Vec<String> = Vec::new();
    for finding in findings {
        let resolver_added = matches!(
            finding.kind,
            surge_core::ValidationErrorKind::ProfileNotFound { .. }
                | surge_core::ValidationErrorKind::TemplateNotFound { .. }
                | surge_core::ValidationErrorKind::NamedAgentNotFound { .. }
                | surge_core::ValidationErrorKind::SameRuntimeVerification { .. }
        );
        if !resolver_added {
            // Already logged (Warning) or already turned this call into an
            // `Err` before reaching here (Error), by `validate_for_m6`'s
            // call into `apply_surge_core_validation` above.
            continue;
        }
        let label = match finding.kind {
            surge_core::ValidationErrorKind::ProfileNotFound { .. }
            | surge_core::ValidationErrorKind::TemplateNotFound { .. }
            | surge_core::ValidationErrorKind::NamedAgentNotFound { .. } => "ref",
            // SameRuntimeVerification (W5) — resolver-backed but
            // Warning-only, so it never reaches `error_messages` below;
            // this label only ever surfaces in the `tracing::warn!` call.
            _ => "structural",
        };
        if finding.kind.severity() != surge_core::Severity::Error {
            tracing::warn!(
                target: "engine::validate",
                kind = ?finding.kind,
                location = ?finding.location,
                label,
                "{}",
                finding.message,
            );
            continue;
        }
        error_messages.push(format!("[{label}] {}", finding.message));
    }
    if !error_messages.is_empty() {
        return Err(EngineError::GraphInvalid(format!(
            "validate_with_resolver failed: {}",
            error_messages.join("; ")
        )));
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
    fn required_profile_input_cannot_be_omitted_optional_empty_or_duplicated() {
        use surge_core::agent_config::{ArtifactSource, Binding, TemplateVar};
        let profile: surge_core::profile::Profile = toml::from_str(include_str!(
            "../../../surge-core/bundled/profiles/implementer-2.0.toml"
        ))
        .unwrap();
        let graph: Graph =
            toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
        let surge_core::node::NodeConfig::Agent(mut config) =
            graph.nodes[&graph.start].config.clone()
        else {
            panic!("expected agent");
        };
        config.bindings.clear();
        assert!(
            validate_agent_inputs(&config, &profile)
                .unwrap_err()
                .to_string()
                .contains("spec")
        );
        let binding = Binding {
            target: TemplateVar("spec".into()),
            source: ArtifactSource::Static {
                content: "Approved task specification".into(),
            },
            optional: false,
        };
        config.bindings.push(binding.clone());
        assert!(validate_agent_inputs(&config, &profile).is_ok());
        config.bindings[0].optional = true;
        assert!(validate_agent_inputs(&config, &profile).is_err());
        config.bindings[0] = binding.clone();
        config.bindings[0].source = ArtifactSource::Static {
            content: "  ".into(),
        };
        assert!(validate_agent_inputs(&config, &profile).is_err());
        config.bindings = vec![binding.clone()];
        config.bindings[0].source = ArtifactSource::NodeOutput {
            node: NodeKey::try_from("missing").unwrap(),
            artifact: "spec".into(),
        };
        assert!(validate_binding_sources(&graph, &config).is_err());
        config.bindings[0].source = ArtifactSource::GlobPattern {
            node: graph.start.clone(),
            pattern: "*.md".into(),
        };
        assert!(validate_binding_sources(&graph, &config).is_err());
        config.bindings = vec![binding.clone(), binding];
        assert!(validate_agent_inputs(&config, &profile).is_err());
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
    fn run_artifact_loop_requires_unique_valid_seed() {
        use crate::engine::config::RunSeedArtifact;

        let source: IterableSource = toml::from_str(
            "type = 'run_artifact'\n[value]\nname = 'roadmap'\njsonpath = 'milestones'",
        )
        .unwrap();
        let graph = graph_with_archetype_and_loop(Some(multi_milestone_meta()), source.clone());
        assert!(validate_archetype_topology(&graph).is_ok());
        assert!(validate_loop_seeds(&graph, &[]).is_err());
        let seed = RunSeedArtifact::new(
            "roadmap",
            "roadmap.toml",
            "[[milestones]]\nid = 'approved'\ntasks = []",
            "bootstrap_parent",
        )
        .unwrap();
        assert!(validate_loop_seeds(&graph, std::slice::from_ref(&seed)).is_ok());
        assert!(validate_loop_seeds(&graph, &[seed.clone(), seed]).is_err());
        let invalid = RunSeedArtifact::new(
            "roadmap",
            "roadmap.toml",
            "milestones = 42",
            "bootstrap_parent",
        )
        .unwrap();
        assert!(validate_loop_seeds(&graph, &[invalid]).is_err());
        let roundtrip: IterableSource = toml::from_str(&toml::to_string(&source).unwrap()).unwrap();
        assert_eq!(source, roundtrip);
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
        for variant in ArchetypeName::ALL
            .into_iter()
            .filter(|name| *name != ArchetypeName::MultiMilestone)
        {
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
    fn archetype_topology_rejects_milestone_loop_under_another_name() {
        let iterable = IterableSource::Artifact {
            node: NodeKey::try_from("roadmap_planner").unwrap(),
            name: "roadmap.milestones".into(),
            jsonpath: "$".into(),
        };
        let meta = ArchetypeMetadata {
            name: ArchetypeName::Feature,
            milestones: None,
            edit_loop_cap: None,
        };
        let g = graph_with_archetype_and_loop(Some(meta), iterable);
        match validate_archetype_topology(&g).unwrap_err() {
            EngineError::ArchetypeMismatch { declared, detected } => {
                assert_eq!(declared, "feature");
                assert!(detected.contains("multi-milestone"), "{detected}");
            },
            other => panic!("expected ArchetypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn require_archetype_rejects_missing_block_and_names_the_catalog() {
        let g = graph_with_archetype_and_loop(None, IterableSource::Static(vec![]));
        match require_archetype(&g).unwrap_err() {
            EngineError::ArchetypeMissing(message) => {
                for archetype in ArchetypeName::ALL {
                    assert!(message.contains(archetype.as_str()), "{message}");
                }
            },
            other => panic!("expected ArchetypeMissing, got {other:?}"),
        }
        let g = graph_with_archetype_and_loop(
            Some(multi_milestone_meta()),
            IterableSource::Static(vec![]),
        );
        assert_eq!(
            require_archetype(&g).unwrap(),
            ArchetypeName::MultiMilestone
        );
    }

    #[test]
    fn archetype_advisories_flag_a_missing_named_stage() {
        let meta = ArchetypeMetadata {
            name: ArchetypeName::BugFix,
            milestones: None,
            edit_loop_cap: None,
        };
        let g = graph_with_archetype_and_loop(Some(meta), IterableSource::Static(vec![]));
        let advisories = archetype_advisories(&g);
        assert_eq!(advisories.len(), 1, "{advisories:?}");
        assert!(advisories[0].contains("reproduce"), "{advisories:?}");
    }

    #[test]
    fn bundled_archetype_flows_pass_topology_and_advisories() {
        for archetype in ArchetypeName::ALL {
            let flow = surge_core::bundled_flows::BundledFlows::by_name_latest(archetype.as_str())
                .expect("bundled flow");
            assert_eq!(require_archetype(&flow.graph).unwrap(), archetype);
            validate_archetype_topology(&flow.graph)
                .unwrap_or_else(|e| panic!("{}: {e}", archetype.as_str()));
            assert_eq!(
                archetype_advisories(&flow.graph),
                Vec::<String>::new(),
                "{}",
                archetype.as_str()
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
        let registry = crate::profile_loader::ProfileRegistry::new(
            crate::profile_loader::DiskProfileSet::empty(),
        );
        validate_profile_inputs(&graph, &registry).unwrap();
        for flow in surge_core::BundledFlows::all().into_iter().filter(|flow| {
            matches!(
                flow.name.as_str(),
                "bootstrap" | "linear-3" | "multi-milestone" | "bug-fix" | "refactor" | "spike"
            )
        }) {
            validate_profile_inputs(&flow.graph, &registry)
                .unwrap_or_else(|error| panic!("{} profile inputs: {error}", flow.name));
        }
    }

    // ── Warning-severity findings are logged, not dropped (A8) ──────────
    //
    // Captures via a `tracing_subscriber::fmt` subscriber piped into an
    // in-memory buffer — the same `CaptureWriter` shape
    // `surge-persistence/tests/runs_inner/drop_warn.rs` uses for its own
    // tracing-capture test — installed per test with
    // `tracing::subscriber::with_default` (thread-local). Unlike
    // `drop_warn.rs`'s async, multi-threaded case, `validate_for_m6` and
    // `validate_for_m6_with_resolver` are synchronous, so there is no
    // thread-hop hazard motivating that file's process-global
    // `set_global_default` instead — scoped `with_default` is both simpler
    // and safer here (no risk of one test's subscriber leaking into
    // another's).

    #[derive(Clone)]
    struct CaptureWriter {
        buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `f` under a scoped `tracing_subscriber::fmt` subscriber and
    /// return `f`'s result alongside every captured line whose target is
    /// `engine::validate`. The default (non-pretty) formatter writes one
    /// line per event, so counting lines counts events — the assertion T2
    /// needs (`.any(..)` alone cannot distinguish "logged once" from
    /// "logged twice", which is exactly how the F2 double-logging defect
    /// went unnoticed).
    fn capture_engine_validate_lines<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = CaptureWriter { buf: buf.clone() };
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        let result = tracing::subscriber::with_default(subscriber, f);
        let raw = String::from_utf8(buf.lock().unwrap().clone()).expect("log output is utf8");
        let lines = raw
            .lines()
            .filter(|line| line.contains("engine::validate"))
            .map(str::to_owned)
            .collect();
        (result, lines)
    }

    #[test]
    fn warning_finding_is_logged_not_dropped_and_run_still_starts() {
        // A single Terminal{Success} node as `start` is reachable from
        // itself without passing a verification gate — trips exactly one
        // Warning (`UnverifiedSuccessPath`) and no Error.
        let g = graph_with_one_terminal("end");
        let (result, lines) = capture_engine_validate_lines(|| validate_for_m6(&g));
        assert!(
            result.is_ok(),
            "a Warning-only graph must not block the run"
        );
        assert_eq!(
            lines.len(),
            1,
            "expected exactly one engine::validate log line (UnverifiedSuccessPath), got {lines:?}"
        );
        assert!(
            lines[0].contains("without a verification node"),
            "expected the UnverifiedSuccessPath warning under engine::validate: {lines:?}"
        );
    }

    #[test]
    fn resolver_path_does_not_double_log_structural_warnings() {
        // F2 regression: `validate_for_m6_with_resolver` used to log every
        // Warning-severity *structural* (W1–W4) finding TWICE — once from
        // `validate_for_m6`'s call into the private `apply_surge_core_validation`,
        // and again from `validate_with_resolver`'s own internal (redundant)
        // structural pass, which re-ran and re-logged the identical finding
        // under the same `engine::validate` target. `graph_with_one_terminal`
        // trips exactly one `UnverifiedSuccessPath` warning; the
        // resolver-backed entry point must still log it exactly once, not
        // once per validation pass.
        let g = graph_with_one_terminal("end");
        let (result, lines) = capture_engine_validate_lines(|| {
            validate_for_m6_with_resolver(&g, &surge_core::NoOpResolver)
        });
        assert!(
            result.is_ok(),
            "a Warning-only graph must not block the run"
        );
        assert_eq!(
            lines.len(),
            1,
            "UnverifiedSuccessPath must be logged exactly once, not once per validation pass: {lines:?}"
        );
    }

    /// T3 (second case) — `ValidationErrorKind::ProfileNotFound` just became
    /// a run-blocking Error on `Engine::start_run` for the first time (see
    /// `validate_for_m6_with_resolver`'s doc). This exercises the exact
    /// function that gates it: every one of the
    /// [`surge_core::BUNDLED_FLOW_COUNT`] bundled flows must still resolve
    /// under the real `ProfileRegistry` (bundled profiles only, no disk
    /// overlay) — otherwise shipping this rule would reject every bundled
    /// flow at `start_run` on day one.
    #[test]
    fn every_bundled_flow_resolves_under_the_real_profile_registry() {
        let registry = crate::profile_loader::ProfileRegistry::new(
            crate::profile_loader::DiskProfileSet::empty(),
        );
        for flow in surge_core::BundledFlows::all() {
            validate_for_m6_with_resolver(&flow.graph, &registry).unwrap_or_else(|e| {
                panic!(
                    "bundled flow `{}` ({}) failed to resolve under the real profile \
                     registry: {e:?}",
                    flow.name, flow.version
                )
            });
        }
    }

    /// Pins W5's blast radius on the shipped set. Five bundled flows pair a
    /// verifier with an agent predecessor on the same runtime; the other
    /// eight either have no top-level verification gate or no agent feeding
    /// one. Every bundled profile carries `agent_id = "claude-code"`, so
    /// these five are a true statement about what we ship, not a defect in
    /// the rule.
    ///
    /// The assertion is exact on purpose: a change that silently drops the
    /// rule to zero (the state it was in when first written) fails here,
    /// and so does one that widens it to every flow.
    #[test]
    fn w5_warns_on_exactly_the_five_bundled_flows_that_verify_in_one_vendor() {
        let registry = crate::profile_loader::ProfileRegistry::new(
            crate::profile_loader::DiskProfileSet::empty(),
        );
        let mut warned: Vec<String> = Vec::new();
        for flow in surge_core::BundledFlows::all() {
            let findings = match surge_core::validate_with_resolver(&flow.graph, &registry) {
                Ok(f) | Err(f) => f,
            };
            if findings.iter().any(|f| {
                matches!(
                    f.kind,
                    surge_core::ValidationErrorKind::SameRuntimeVerification { .. }
                )
            }) {
                warned.push(flow.name.clone());
            }
        }
        warned.sort_unstable();
        assert_eq!(
            warned,
            vec![
                "bug-fix",
                "linear-3",
                "linear-with-review",
                "multi-milestone",
                "refactor"
            ],
            "W5's blast radius on the bundled set changed"
        );
    }

    struct StubRuntimeResolver {
        runtimes: std::collections::HashMap<&'static str, &'static str>,
    }

    impl surge_core::ReferenceResolver for StubRuntimeResolver {
        fn profile_exists(&self, _name: &str) -> bool {
            true
        }
        fn template_exists(&self, _name: &str) -> bool {
            true
        }
        fn named_agent_exists(&self, _id: &str) -> bool {
            true
        }
        fn profile_runtime(&self, name: &str) -> Option<String> {
            self.runtimes.get(name).map(|runtime| (*runtime).to_owned())
        }
    }

    fn agent_node_with_profile(key: &str, profile: &str, effect: surge_core::LedgerEffect) -> Node {
        Node {
            id: NodeKey::try_from(key).unwrap(),
            position: Position::default(),
            declared_outcomes: vec![surge_core::node::OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "ok".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: effect,
            }],
            config: NodeConfig::Agent(surge_core::agent_config::AgentConfig {
                profile: surge_core::keys::ProfileKey::try_from(profile).unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields: std::collections::BTreeMap::default(),
            }),
        }
    }

    /// The remedy exists and works. Same graph twice through the **real**
    /// `ProfileRegistry`: pointed at `verifier@2.0` it raises W5, pointed at
    /// `cross-verifier@1.0` it does not — because that profile resolves to
    /// Codex while the implementer resolves to Claude Code.
    ///
    /// Without this pairing W5 is a rule that can only nag: before
    /// `cross-verifier@1.0` shipped, every bundled profile resolved to one
    /// runtime, so no flow assembled from the bundled set could answer the
    /// finding.
    #[test]
    fn cross_verifier_profile_answers_the_same_runtime_finding() {
        fn graph_verified_by(profile: &str) -> Graph {
            let impl_key = NodeKey::try_from("impl_1").unwrap();
            let mut nodes = BTreeMap::new();
            nodes.insert(
                impl_key.clone(),
                agent_node_with_profile(
                    "impl_1",
                    "implementer@1.0",
                    surge_core::LedgerEffect::None,
                ),
            );
            nodes.insert(
                NodeKey::try_from("verify_1").unwrap(),
                agent_node_with_profile("verify_1", profile, surge_core::LedgerEffect::Verified),
            );
            nodes.insert(NodeKey::try_from("end").unwrap(), terminal_node("end"));
            Graph {
                schema_version: SCHEMA_VERSION,
                metadata: GraphMetadata {
                    name: "cross-verifier-fixture".into(),
                    description: None,
                    template_origin: None,
                    created_at: chrono::Utc::now(),
                    author: None,
                    archetype: None,
                },
                start: impl_key,
                nodes,
                edges: vec![
                    forward_edge("e1", "impl_1", "done", "verify_1"),
                    forward_edge("e2", "verify_1", "done", "end"),
                ],
                subgraphs: BTreeMap::new(),
            }
        }

        fn same_runtime_findings(
            graph: &Graph,
            registry: &crate::profile_loader::ProfileRegistry,
        ) -> usize {
            let findings = match surge_core::validate_with_resolver(graph, registry) {
                Ok(f) | Err(f) => f,
            };
            findings
                .iter()
                .filter(|f| {
                    matches!(
                        f.kind,
                        surge_core::ValidationErrorKind::SameRuntimeVerification { .. }
                    )
                })
                .count()
        }

        let registry = crate::profile_loader::ProfileRegistry::new(
            crate::profile_loader::DiskProfileSet::empty(),
        );

        assert_eq!(
            same_runtime_findings(&graph_verified_by("verifier@2.0"), &registry),
            1,
            "verifier@2.0 shares the implementer's runtime, so W5 must fire"
        );
        assert_eq!(
            same_runtime_findings(&graph_verified_by("cross-verifier@1.0"), &registry),
            0,
            "cross-verifier@1.0 runs Codex against a Claude Code implementer — \
             the finding must go away, and for that reason"
        );
    }

    #[test]
    fn same_runtime_verification_warning_is_logged_via_resolver_path() {
        let impl_key = NodeKey::try_from("impl_1").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            impl_key.clone(),
            agent_node_with_profile(
                "impl_1",
                "implementer@1.0",
                surge_core::LedgerEffect::ReadyForVerification,
            ),
        );
        nodes.insert(
            NodeKey::try_from("verify_1").unwrap(),
            agent_node_with_profile(
                "verify_1",
                "verifier@1.0",
                surge_core::LedgerEffect::Verified,
            ),
        );
        nodes.insert(NodeKey::try_from("end").unwrap(), terminal_node("end"));

        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "w5-orchestrator".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: impl_key,
            nodes,
            edges: vec![
                forward_edge("e1", "impl_1", "done", "verify_1"),
                forward_edge("e2", "verify_1", "done", "end"),
            ],
            subgraphs: BTreeMap::new(),
        };

        let resolver = StubRuntimeResolver {
            runtimes: std::collections::HashMap::from([
                ("implementer@1.0", "claude-code"),
                ("verifier@1.0", "claude-code"),
            ]),
        };

        let (result, lines) =
            capture_engine_validate_lines(|| validate_for_m6_with_resolver(&g, &resolver));
        assert!(
            result.is_ok(),
            "a Warning-only resolver finding must not block the run"
        );

        // Exactly one line: this graph's only structural (W1–W4) finding
        // would be `UnverifiedSuccessPath`, but `verify_1`'s `Verified`
        // outcome absorbs the path before `end` is reached, so the only
        // finding at all is the resolver-added `SameRuntimeVerification` —
        // asserting the count (not `.any(..)`) is what would have caught F2
        // on a fixture that also had a structural warning to double-log.
        assert_eq!(
            lines.len(),
            1,
            "expected exactly one engine::validate log line (SameRuntimeVerification), got {lines:?}"
        );
        assert!(
            lines[0].contains("claude-code"),
            "expected SameRuntimeVerification under engine::validate: {lines:?}"
        );
    }
}
