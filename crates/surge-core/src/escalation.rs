//! Default escalation for exhausted retry loops (verifier rejection ladder:
//! an extra automatic attempt, then a human).
//!
//! An edge with `max_traversals` and `on_max_exceeded = escalate` re-routes
//! through the source node's `max_traversals_exceeded` outcome once its limit
//! is spent. Flows rarely declare that edge, and without it the run failed.
//! [`with_default_escalation_gates`] derives the *effective* graph a run routes
//! on: for every such source without a declared escalation edge it adds one
//! extra attempt of the loop's agent (on the configured retry agent when one is
//! set) and then a `HumanGate` that asks whether to retry once more or stop.
//!
//! The derivation is pure, deterministic and idempotent. The persisted graph
//! (and every hash over it) is unchanged; the engine, the run-state fold and
//! operator views apply this function to the persisted graph so replay sees
//! the same nodes.

use std::collections::{BTreeMap, BTreeSet};

use crate::edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
use crate::graph::Graph;
use crate::human_gate_config::{
    ApprovalOption, HumanGateConfig, HumanGateMode, OptionStyle, SummaryTemplate, TimeoutAction,
};
use crate::keys::{EdgeKey, NodeKey, OutcomeKey};
use crate::node::{Node, NodeConfig, OutcomeDecl, Position};
use crate::terminal_config::{TerminalConfig, TerminalKind};

/// Synthetic outcome routing takes once an escalating edge is exhausted.
pub const EXHAUSTED_OUTCOME: &str = "max_traversals_exceeded";
/// Gate outcome granting the loop one more attempt.
pub const RETRY_OUTCOME: &str = "retry";
/// Gate outcome ending the run, or the current loop iteration inside a loop.
pub const STOP_OUTCOME: &str = "stop";
/// Synthetic outcome routing takes when an escalation edge is itself
/// exhausted: the next rung of the ladder.
pub const ESCALATION_EXHAUSTED_OUTCOME: &str = "escalation_exhausted";
/// `EdgePolicy::label` of a derived edge that re-enters the loop's agent for
/// its one extra attempt. The engine runs that occurrence on the configured
/// retry agent. Only derived graphs carry it; it is never persisted.
pub const ALTERNATE_ATTEMPT_LABEL: &str = "surge:escalation:alternate-attempt";

/// Whether `edge` is a derived extra-attempt edge (see
/// [`ALTERNATE_ATTEMPT_LABEL`]).
#[must_use]
pub fn is_alternate_attempt(edge: &Edge) -> bool {
    edge.kind == EdgeKind::Escalate && edge.policy.label.as_deref() == Some(ALTERNATE_ATTEMPT_LABEL)
}

/// Whether `node`'s current occurrence is the extra attempt of an exhausted
/// loop: `via` (its latest entering edge, `RunMemory::entered_via`) is a
/// derived extra-attempt edge into `node` in the effective `graph`.
#[must_use]
pub fn entered_by_alternate_attempt(graph: &Graph, node: &NodeKey, via: Option<&EdgeKey>) -> bool {
    let Some(via) = via else {
        return false;
    };
    graph
        .edges
        .iter()
        .chain(graph.subgraphs.values().flat_map(|sg| sg.edges.iter()))
        .any(|edge| &edge.id == via && &edge.to == node && is_alternate_attempt(edge))
}

/// `[escalation]` in `surge.toml`: where the extra attempt of an exhausted
/// retry loop runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EscalationConfig {
    /// Agent (registry id, for example `codex-acp`) for the extra attempt.
    /// Unset: the extra attempt runs on the stage's own agent, with the
    /// latest findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_agent: Option<String>,
    /// Model for the extra attempt on `retry_agent`. Unset: that agent's
    /// default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_model: Option<String>,
}

impl EscalationConfig {
    /// The configured retry agent, trimmed; `None` when unset or blank.
    #[must_use]
    pub fn retry_agent(&self) -> Option<&str> {
        self.retry_agent
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    /// `agent` with its runtime moved to the retry agent for one extra
    /// attempt: `custom_fields["runtime"]` becomes `{ agent_id, model? }`.
    /// The profile (role and prompts) is kept; the stage's own model and
    /// effort are dropped because they belong to its original agent.
    /// `None` when no retry agent is configured.
    #[must_use]
    pub fn retry_config(
        &self,
        agent: &crate::agent_config::AgentConfig,
    ) -> Option<crate::agent_config::AgentConfig> {
        let agent_id = self.retry_agent()?;
        let mut runtime = toml::map::Map::new();
        runtime.insert("agent_id".into(), toml::Value::String(agent_id.into()));
        if let Some(model) = self
            .retry_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            runtime.insert("model".into(), toml::Value::String(model.into()));
        }
        let mut retry = agent.clone();
        retry
            .custom_fields
            .insert("runtime".into(), toml::Value::Table(runtime));
        Some(retry)
    }
}

/// Waits for the operator: an unattended run must not fail on a decision
/// deadline. About 136 years, stored in the gate's original schema.
const NO_PRACTICAL_DEADLINE_SECS: u32 = u32::MAX;

/// Return `graph` with a default escalation gate for every exhausted-loop
/// source that lacks a declared `max_traversals_exceeded` edge.
///
/// A source qualifies when, in one scope (the root or one subgraph), it has at
/// least one outgoing edge with `max_traversals` set and
/// `on_max_exceeded = escalate`, all such edges share one target (the retry
/// destination), and no edge from it already handles the exhausted outcome.
/// Each qualifying source gets, in its own scope:
///
/// - when the loop target is an agent node, an extra-attempt `escalate` edge
///   from the source's exhausted outcome back to that target, capped at one
///   traversal and labelled [`ALTERNATE_ATTEMPT_LABEL`]; once spent, routing
///   takes [`ESCALATION_EXHAUSTED_OUTCOME`] to the gate;
/// - a `HumanGate` with `retry` and `stop` options and no practical deadline;
/// - an `escalate` edge to the gate (from the exhausted outcome when there is
///   no extra attempt);
/// - a `backtrack` edge from `retry` to the loop target (one more attempt per
///   decision; the exhausted counter is not reset);
/// - a failure terminal reached by `stop`. Inside a loop body this fails the
///   iteration, so the loop's `on_iteration_failure` policy applies.
///
/// Sources with several distinct capped targets keep today's behaviour (the
/// run fails), because one gate cannot know which loop to retry.
#[must_use]
pub fn with_default_escalation_gates(graph: &Graph) -> Graph {
    let mut effective = graph.clone();
    let mut taken_nodes: BTreeSet<NodeKey> = effective.nodes.keys().cloned().collect();
    let mut taken_edges: BTreeSet<EdgeKey> = effective.edges.iter().map(|e| e.id.clone()).collect();
    for subgraph in effective.subgraphs.values() {
        taken_nodes.extend(subgraph.nodes.keys().cloned());
        taken_edges.extend(subgraph.edges.iter().map(|e| e.id.clone()));
    }
    let mut names = Names {
        nodes: taken_nodes,
        edges: taken_edges,
    };
    add_gates(&mut effective.nodes, &mut effective.edges, &mut names);
    for subgraph in effective.subgraphs.values_mut() {
        add_gates(&mut subgraph.nodes, &mut subgraph.edges, &mut names);
    }
    effective
}

struct Names {
    nodes: BTreeSet<NodeKey>,
    edges: BTreeSet<EdgeKey>,
}

impl Names {
    fn node(&mut self, preferred: &str, fallback: &str) -> Option<NodeKey> {
        let key = fresh(preferred, fallback, |key: &NodeKey| {
            self.nodes.contains(key)
        })?;
        self.nodes.insert(key.clone());
        Some(key)
    }

    fn edge(&mut self, preferred: &str, fallback: &str) -> Option<EdgeKey> {
        let key = fresh(preferred, fallback, |key: &EdgeKey| {
            self.edges.contains(key)
        })?;
        self.edges.insert(key.clone());
        Some(key)
    }
}

/// The preferred key when valid and free, else the first free `fallback_N`.
fn fresh<K>(preferred: &str, fallback: &str, taken: impl Fn(&K) -> bool) -> Option<K>
where
    K: for<'a> TryFrom<&'a str>,
{
    if let Ok(key) = K::try_from(preferred)
        && !taken(&key)
    {
        return Some(key);
    }
    (1..=10_000u32).find_map(|n| {
        K::try_from(format!("{fallback}_{n}").as_str())
            .ok()
            .filter(|key| !taken(key))
    })
}

fn add_gates(nodes: &mut BTreeMap<NodeKey, Node>, edges: &mut Vec<Edge>, names: &mut Names) {
    let Ok(exhausted) = OutcomeKey::try_from(EXHAUSTED_OUTCOME) else {
        return;
    };
    let mut capped: BTreeMap<NodeKey, Vec<Edge>> = BTreeMap::new();
    for edge in edges.iter() {
        if edge.policy.max_traversals.is_some()
            && edge.policy.on_max_exceeded == ExceededAction::Escalate
        {
            capped
                .entry(edge.from.node.clone())
                .or_default()
                .push(edge.clone());
        }
    }
    for (source, loop_edges) in capped {
        if !nodes.contains_key(&source)
            || edges
                .iter()
                .any(|edge| edge.from.node == source && edge.from.outcome == exhausted)
        {
            continue;
        }
        let targets: BTreeSet<&NodeKey> = loop_edges.iter().map(|edge| &edge.to).collect();
        let [target] = targets.into_iter().collect::<Vec<_>>()[..] else {
            continue;
        };
        let target = target.clone();
        let alternate = matches!(
            nodes.get(&target).map(|node| &node.config),
            Some(NodeConfig::Agent(_))
        );
        let Some(addition) = gate_for(&source, &target, &loop_edges, &exhausted, alternate, names)
        else {
            continue;
        };
        for node in addition.nodes {
            nodes.insert(node.id.clone(), node);
        }
        edges.extend(addition.edges);
    }
}

struct Addition {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

fn gate_for(
    source: &NodeKey,
    target: &NodeKey,
    loop_edges: &[Edge],
    exhausted: &OutcomeKey,
    alternate: bool,
    names: &mut Names,
) -> Option<Addition> {
    let retry = OutcomeKey::try_from(RETRY_OUTCOME).ok()?;
    let stop = OutcomeKey::try_from(STOP_OUTCOME).ok()?;
    let escalation_exhausted = OutcomeKey::try_from(ESCALATION_EXHAUSTED_OUTCOME).ok()?;
    let alternate_edge = if alternate {
        Some(names.edge(&format!("{source}_alternate"), "escalation_edge")?)
    } else {
        None
    };
    let gate = names.node(&format!("{source}_escalation"), "escalation")?;
    let stopped = names.node(&format!("{source}_stopped"), "escalation_stop")?;
    let to_gate = names.edge(&format!("{source}_exhausted"), "escalation_edge")?;
    let retry_edge = names.edge(&format!("{source}_retry"), "escalation_edge")?;
    let stop_edge = names.edge(&format!("{source}_stop"), "escalation_edge")?;

    use std::fmt::Write as _;
    let mut body = String::new();
    for edge in loop_edges {
        if let Some(max) = edge.policy.max_traversals {
            let _ = writeln!(
                body,
                "`{source}` reported `{}` more than {max} times (edge `{}`).",
                edge.from.outcome, edge.id
            );
        }
    }
    if alternate {
        let _ = writeln!(
            body,
            "One extra attempt of `{target}` (on the retry agent, when one is configured) \
             was also sent back."
        );
    }
    let _ = write!(
        body,
        "\nRetry gives `{target}` one more attempt with the latest findings. \
         Stop ends this run, or this task when it runs inside a loop."
    );
    let gate_node = Node {
        id: gate.clone(),
        position: Position::default(),
        declared_outcomes: vec![
            OutcomeDecl {
                id: retry.clone(),
                description: "Give the loop one more attempt".into(),
                edge_kind_hint: EdgeKind::Backtrack,
                is_terminal: false,
                ledger_effect: Default::default(),
            },
            OutcomeDecl {
                id: stop.clone(),
                description: "Stop after the attempt limit".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            },
        ],
        config: NodeConfig::HumanGate(HumanGateConfig {
            delivery_channels: Vec::new(),
            timeout_seconds: Some(NO_PRACTICAL_DEADLINE_SECS),
            on_timeout: TimeoutAction::Reject,
            summary: SummaryTemplate {
                title: format!("Attempt limit reached at {source}"),
                body,
                show_artifacts: Vec::new(),
            },
            options: vec![
                ApprovalOption {
                    outcome: retry.clone(),
                    label: "Retry once more".into(),
                    style: OptionStyle::Primary,
                },
                ApprovalOption {
                    outcome: stop.clone(),
                    label: "Stop".into(),
                    style: OptionStyle::Danger,
                },
            ],
            allow_freetext: false,
            mode: HumanGateMode::Generic,
        }),
    };
    let stopped_node = Node {
        id: stopped.clone(),
        position: Position::default(),
        declared_outcomes: Vec::new(),
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Failure { exit_code: 1 },
            message: Some(format!(
                "stopped by the operator after `{source}` reached its attempt limit"
            )),
        }),
    };
    let edge = |id: EdgeKey, from: &NodeKey, outcome: &OutcomeKey, to: &NodeKey, kind| Edge {
        id,
        from: PortRef {
            node: from.clone(),
            outcome: outcome.clone(),
        },
        to: to.clone(),
        kind,
        policy: EdgePolicy::default(),
    };
    let mut edges = Vec::with_capacity(4);
    if let Some(id) = alternate_edge {
        let mut extra = edge(id, source, exhausted, target, EdgeKind::Escalate);
        extra.policy = EdgePolicy {
            max_traversals: Some(1),
            on_max_exceeded: ExceededAction::Escalate,
            label: Some(ALTERNATE_ATTEMPT_LABEL.into()),
        };
        edges.push(extra);
        edges.push(edge(
            to_gate,
            source,
            &escalation_exhausted,
            &gate,
            EdgeKind::Escalate,
        ));
    } else {
        edges.push(edge(to_gate, source, exhausted, &gate, EdgeKind::Escalate));
    }
    edges.push(edge(retry_edge, &gate, &retry, target, EdgeKind::Backtrack));
    edges.push(edge(stop_edge, &gate, &stop, &stopped, EdgeKind::Forward));
    Some(Addition {
        nodes: vec![gate_node, stopped_node],
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled_flows::BundledFlows;

    fn key<K: for<'a> TryFrom<&'a str>>(s: &str) -> K
    where
        for<'a> <K as TryFrom<&'a str>>::Error: std::fmt::Debug,
    {
        K::try_from(s).unwrap()
    }

    fn gate_config<'g>(graph: &'g Graph, node: &str) -> &'g HumanGateConfig {
        match &graph.find_node(&key(node)).expect("gate node").config {
            NodeConfig::HumanGate(config) => config,
            other => panic!("expected a human gate, got {other:?}"),
        }
    }

    #[test]
    fn linear_flow_gets_one_gate_per_capped_source() {
        let graph = BundledFlows::by_name_latest("linear-3").unwrap().graph;
        let effective = with_default_escalation_gates(&graph);

        let config = gate_config(&effective, "verify_1_escalation");
        let outcomes: Vec<&str> = config.options.iter().map(|o| o.outcome.as_str()).collect();
        assert_eq!(outcomes, ["retry", "stop"]);
        assert_eq!(config.timeout_seconds, Some(u32::MAX));
        assert!(config.summary.body.contains("`verify_1` reported `failed`"));
        let edge_to = |from: &str, outcome: &str| {
            effective
                .edges
                .iter()
                .find(|e| e.from.node.as_str() == from && e.from.outcome.as_str() == outcome)
                .map(|e| (e.to.as_str().to_owned(), e.kind))
        };
        // The implementer is an agent: one extra attempt first, then the gate.
        assert_eq!(
            edge_to("verify_1", EXHAUSTED_OUTCOME),
            Some(("implement_1".into(), EdgeKind::Escalate))
        );
        let extra = effective
            .edges
            .iter()
            .find(|e| e.id.as_str() == "verify_1_alternate")
            .unwrap();
        assert!(is_alternate_attempt(extra));
        assert_eq!(extra.policy.max_traversals, Some(1));
        assert_eq!(
            edge_to("verify_1", ESCALATION_EXHAUSTED_OUTCOME),
            Some(("verify_1_escalation".into(), EdgeKind::Escalate))
        );
        assert!(
            config
                .summary
                .body
                .contains("One extra attempt of `implement_1`")
        );
        assert!(entered_by_alternate_attempt(
            &effective,
            &key("implement_1"),
            Some(&key("verify_1_alternate"))
        ));
        assert!(!entered_by_alternate_attempt(
            &effective,
            &key("implement_1"),
            Some(&key("verify_1_retry"))
        ));
        assert!(!entered_by_alternate_attempt(
            &effective,
            &key("implement_1"),
            None
        ));
        // The persisted graph never carries the marker.
        assert!(!entered_by_alternate_attempt(
            &graph,
            &key("implement_1"),
            Some(&key("verify_1_alternate"))
        ));
        assert_eq!(
            edge_to("verify_1_escalation", "retry"),
            Some(("implement_1".into(), EdgeKind::Backtrack))
        );
        assert_eq!(
            edge_to("verify_1_escalation", "stop"),
            Some(("verify_1_stopped".into(), EdgeKind::Forward))
        );
        // The implementer's own `partial` self-loop gets its own gate.
        assert_eq!(
            edge_to("implement_1_escalation", "retry"),
            Some(("implement_1".into(), EdgeKind::Backtrack))
        );
        // Sources never learn the synthetic outcome: agents cannot report it.
        let verify = effective.find_node(&key("verify_1")).unwrap();
        assert!(
            verify
                .declared_outcomes
                .iter()
                .all(|o| o.id.as_str() != EXHAUSTED_OUTCOME)
        );
    }

    #[test]
    fn gates_land_in_the_subgraph_scope_of_their_loop() {
        let graph = BundledFlows::by_name_latest("multi-milestone")
            .unwrap()
            .graph;
        let effective = with_default_escalation_gates(&graph);
        let body = effective
            .subgraphs
            .values()
            .find(|sg| sg.nodes.contains_key(&key::<NodeKey>("verify_task")))
            .expect("task body subgraph");
        assert!(
            body.nodes
                .contains_key(&key::<NodeKey>("verify_task_escalation"))
        );
        assert!(
            body.nodes
                .contains_key(&key::<NodeKey>("verify_task_stopped"))
        );
        assert!(
            !effective
                .nodes
                .contains_key(&key::<NodeKey>("verify_task_escalation"))
        );
    }

    #[test]
    fn derivation_is_idempotent_and_respects_declared_escalation() {
        for flow in ["linear-3", "bug-fix", "refactor", "multi-milestone"] {
            let graph = BundledFlows::by_name_latest(flow).unwrap().graph;
            let once = with_default_escalation_gates(&graph);
            assert_ne!(once, graph, "{flow} has capped loops");
            assert_eq!(with_default_escalation_gates(&once), once, "{flow}");
        }
        let graph = BundledFlows::by_name_latest("linear-with-review")
            .unwrap()
            .graph;
        assert_eq!(with_default_escalation_gates(&graph), graph);
    }

    #[test]
    fn ambiguous_targets_and_taken_names_are_handled() {
        let mut graph = BundledFlows::by_name_latest("linear-3").unwrap().graph;
        // A second capped edge from verify_1 to another target: no safe retry.
        let mut other = graph
            .edges
            .iter()
            .find(|e| e.from.node.as_str() == "verify_1" && e.policy.max_traversals.is_some())
            .unwrap()
            .clone();
        other.id = key("e_verify_other");
        other.from.outcome = key("other");
        other.to = key("spec_1");
        graph.edges.push(other);
        // The preferred implementer gate name is already used.
        let mut squatter = graph
            .nodes
            .get(&key::<NodeKey>("implement_1"))
            .unwrap()
            .clone();
        squatter.id = key("implement_1_escalation");
        graph.nodes.insert(squatter.id.clone(), squatter);

        let effective = with_default_escalation_gates(&graph);
        assert!(
            !effective
                .nodes
                .contains_key(&key::<NodeKey>("verify_1_escalation"))
        );
        let fallback = effective
            .edges
            .iter()
            .find(|e| {
                e.from.node.as_str() == "implement_1"
                    && e.from.outcome.as_str() == ESCALATION_EXHAUSTED_OUTCOME
            })
            .expect("implementer still escalates");
        assert_eq!(fallback.to.as_str(), "escalation_1");
    }

    #[test]
    fn a_non_agent_loop_target_escalates_straight_to_the_gate() {
        let mut graph = BundledFlows::by_name_latest("linear-3").unwrap().graph;
        let implement = graph.nodes.get_mut(&key::<NodeKey>("implement_1")).unwrap();
        let reference =
            with_default_escalation_gates(&BundledFlows::by_name_latest("linear-3").unwrap().graph);
        implement.config =
            NodeConfig::HumanGate(gate_config(&reference, "verify_1_escalation").clone());
        let effective = with_default_escalation_gates(&graph);
        let exhausted = effective
            .edges
            .iter()
            .find(|e| {
                e.from.node.as_str() == "verify_1" && e.from.outcome.as_str() == EXHAUSTED_OUTCOME
            })
            .unwrap();
        assert_eq!(exhausted.to.as_str(), "verify_1_escalation");
        assert!(!is_alternate_attempt(exhausted));
        assert!(
            effective
                .edges
                .iter()
                .all(|e| e.from.outcome.as_str() != ESCALATION_EXHAUSTED_OUTCOME
                    || e.from.node.as_str() != "verify_1")
        );
    }

    #[test]
    fn retry_config_moves_only_the_runtime() {
        let graph = BundledFlows::by_name_latest("linear-3").unwrap().graph;
        let NodeConfig::Agent(agent) = &graph.find_node(&key("implement_1")).unwrap().config else {
            panic!("agent");
        };
        let mut agent = agent.clone();
        let mut runtime = toml::map::Map::new();
        runtime.insert("agent_id".into(), "claude-acp".into());
        runtime.insert("model".into(), "opus".into());
        runtime.insert("effort".into(), "high".into());
        agent
            .custom_fields
            .insert("runtime".into(), toml::Value::Table(runtime));

        assert_eq!(EscalationConfig::default().retry_config(&agent), None);
        let blank = EscalationConfig {
            retry_agent: Some("  ".into()),
            retry_model: None,
        };
        assert_eq!(blank.retry_config(&agent), None);

        let config = EscalationConfig {
            retry_agent: Some(" codex-acp ".into()),
            retry_model: Some("gpt-5".into()),
        };
        let retry = config.retry_config(&agent).unwrap();
        assert_eq!(retry.runtime_override(), Some("codex-acp"));
        assert_eq!(retry.model_override(), Some("gpt-5"));
        assert_eq!(retry.effort_override(), None);
        assert_eq!(retry.profile, agent.profile);
        let agent_only = EscalationConfig {
            retry_agent: Some("codex-acp".into()),
            retry_model: None,
        };
        assert_eq!(
            agent_only.retry_config(&agent).unwrap().model_override(),
            None
        );
    }

    #[test]
    fn routing_climbs_from_the_extra_attempt_to_the_gate() {
        let graph =
            with_default_escalation_gates(&BundledFlows::by_name_latest("linear-3").unwrap().graph);
        let verify: NodeKey = key("verify_1");
        let failed: OutcomeKey = key("failed");
        let mut counts = std::collections::HashMap::new();
        let mut targets = Vec::new();
        for _ in 0..6 {
            let routed = crate::route_selection::resolve_stage_route(
                &graph.edges,
                &verify,
                &failed,
                &mut counts,
            )
            .unwrap();
            targets.push((
                routed.target.as_str().to_owned(),
                routed.edge_id.as_str().to_owned(),
            ));
        }
        let max = graph
            .edges
            .iter()
            .find(|e| e.from.node == verify && e.from.outcome == failed)
            .and_then(|e| e.policy.max_traversals)
            .unwrap() as usize;
        assert!(targets[..max].iter().all(|(to, _)| to == "implement_1"));
        assert_eq!(
            targets[max],
            ("implement_1".into(), "verify_1_alternate".into())
        );
        assert!(
            targets[max + 1..]
                .iter()
                .all(|(to, _)| to == "verify_1_escalation")
        );
    }
}
