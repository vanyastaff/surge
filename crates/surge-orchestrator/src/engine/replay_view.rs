//! Enriched, seq-bounded view of a run for `surge engine replay`: per-node
//! status (completed / active / failed / future), the edges traversed, and the
//! cost accumulated — folded from the event log against the materialized graph.
//!
//! Pure function (no I/O), so it is unit-testable against synthetic event
//! slices and shared by the CLI today and the cockpit scrubber later.

use std::collections::BTreeMap;

use surge_core::graph::Graph;
use surge_core::run_event::EventPayload;
use surge_persistence::runs::reader::ReadEvent;

/// Lifecycle status of a node as of the replay cutoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Declared in the graph but never entered up to the cutoff.
    Future,
    /// Entered but not yet completed or failed (the run's current frontier).
    Active,
    /// Reached a declared outcome.
    Completed,
    /// Failed at this node.
    Failed,
}

impl NodeStatus {
    /// Stable lowercase label for display / JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Future => "future",
            NodeStatus::Active => "active",
            NodeStatus::Completed => "completed",
            NodeStatus::Failed => "failed",
        }
    }
}

/// Per-node row in a [`ReplayView`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NodeView {
    /// Node id.
    pub node: String,
    /// Lifecycle status at the cutoff.
    pub status: NodeStatus,
    /// Most recent `StageEntered.attempt` for this node (0 if never entered).
    pub attempts: u32,
    /// Most recent reported outcome for this node, if any.
    pub last_outcome: Option<String>,
}

/// A traversed edge in a [`ReplayView`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EdgeView {
    /// Edge id.
    pub edge: String,
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
}

/// Cumulative cost accumulated up to the cutoff.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct CostView {
    /// Sum of `prompt_tokens` across `TokensConsumed`.
    pub prompt_tokens: u64,
    /// Sum of `output_tokens`.
    pub output_tokens: u64,
    /// Sum of `cache_hits`.
    pub cache_hits: u64,
    /// Sum of `cost_usd` (events without a cost contribute 0).
    pub cost_usd: f64,
}

/// Terminal disposition of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    /// `RunCompleted`.
    Completed,
    /// `RunFailed`.
    Failed,
    /// `RunAborted`.
    Aborted,
}

impl TerminalKind {
    /// Stable lowercase label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalKind::Completed => "completed",
            TerminalKind::Failed => "failed",
            TerminalKind::Aborted => "aborted",
        }
    }
}

/// The folded, enriched view of a run as of a replay cutoff.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ReplayView {
    /// Number of events folded.
    pub event_count: u64,
    /// The current frontier node (last `StageEntered`), or `None` for a
    /// terminal run or one not yet started.
    pub active_node: Option<String>,
    /// Terminal disposition, if the cutoff includes a terminal event.
    pub terminal: Option<TerminalKind>,
    /// Every graph node with its status, in graph (`BTreeMap`) order.
    pub nodes: Vec<NodeView>,
    /// Edges traversed up to the cutoff, in order.
    pub edges_traversed: Vec<EdgeView>,
    /// Cumulative cost up to the cutoff.
    pub cost: CostView,
}

/// Fold `events` (assumed already trimmed to the replay cutoff) against `graph`
/// into a [`ReplayView`]. Every graph node appears; nodes never entered are
/// `Future`.
#[must_use]
pub fn build_replay_view(graph: &Graph, events: &[ReadEvent]) -> ReplayView {
    // Seed every graph node as Future; events promote them.
    let mut status: BTreeMap<String, (NodeStatus, u32, Option<String>)> = graph
        .nodes
        .keys()
        .map(|k| (k.as_str().to_owned(), (NodeStatus::Future, 0, None)))
        .collect();
    let mut active_node = None;
    let mut terminal = None;
    let mut edges_traversed = Vec::new();
    let mut cost = CostView::default();

    for ev in events {
        match ev.payload.payload() {
            EventPayload::StageEntered { node, attempt } => {
                let e =
                    status
                        .entry(node.as_str().to_owned())
                        .or_insert((NodeStatus::Future, 0, None));
                e.0 = NodeStatus::Active;
                e.1 = *attempt;
                active_node = Some(node.as_str().to_owned());
            },
            EventPayload::StageCompleted { node, outcome } => {
                let e =
                    status
                        .entry(node.as_str().to_owned())
                        .or_insert((NodeStatus::Future, 0, None));
                e.0 = NodeStatus::Completed;
                e.2 = Some(outcome.as_str().to_owned());
            },
            EventPayload::StageFailed { node, .. } => {
                status
                    .entry(node.as_str().to_owned())
                    .or_insert((NodeStatus::Future, 0, None))
                    .0 = NodeStatus::Failed;
            },
            EventPayload::EdgeTraversed { edge, from, to, .. } => {
                edges_traversed.push(EdgeView {
                    edge: edge.as_str().to_owned(),
                    from: from.as_str().to_owned(),
                    to: to.as_str().to_owned(),
                });
            },
            EventPayload::TokensConsumed {
                prompt_tokens,
                output_tokens,
                cache_hits,
                cost_usd,
                ..
            } => {
                cost.prompt_tokens += u64::from(*prompt_tokens);
                cost.output_tokens += u64::from(*output_tokens);
                cost.cache_hits += u64::from(*cache_hits);
                cost.cost_usd += cost_usd.unwrap_or(0.0);
            },
            EventPayload::RunCompleted { .. } => terminal = Some(TerminalKind::Completed),
            EventPayload::RunFailed { .. } => terminal = Some(TerminalKind::Failed),
            EventPayload::RunAborted { .. } => terminal = Some(TerminalKind::Aborted),
            _ => {},
        }
    }

    // A terminal run has no live frontier.
    if terminal.is_some() {
        active_node = None;
    }

    let nodes = status
        .into_iter()
        .map(|(node, (status, attempts, last_outcome))| NodeView {
            node,
            status,
            attempts,
            last_outcome,
        })
        .collect();

    ReplayView {
        event_count: events.len() as u64,
        active_node,
        terminal,
        nodes,
        edges_traversed,
        cost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_core::agent_config::{AgentConfig, NodeLimits};
    use surge_core::graph::{GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::run_event::VersionedEventPayload;
    use surge_core::terminal_config::{TerminalConfig, TerminalKind as TermCfgKind};
    use surge_persistence::runs::seq::EventSeq;

    fn graph_impl_to_end() -> Graph {
        let impl_node = NodeKey::try_from("impl_1").unwrap();
        let end = NodeKey::try_from("end").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            impl_node.clone(),
            Node {
                id: impl_node.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Agent(AgentConfig {
                    profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                    prompt_overrides: None,
                    tool_overrides: None,
                    sandbox_override: None,
                    approvals_override: None,
                    bindings: vec![],
                    rules_overrides: None,
                    limits: NodeLimits::default(),
                    hooks: vec![],
                    custom_fields: BTreeMap::new(),
                }),
            },
        );
        nodes.insert(
            end.clone(),
            Node {
                id: end.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TermCfgKind::Success,
                    message: None,
                }),
            },
        );
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "replay-view-test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: impl_node,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    fn ev(seq: u64, payload: EventPayload) -> ReadEvent {
        let kind = payload.discriminant_str().to_owned();
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms: i64::try_from(seq).unwrap() * 1000,
            kind,
            payload: VersionedEventPayload::new(payload),
        }
    }

    fn node_view<'a>(view: &'a ReplayView, id: &str) -> &'a NodeView {
        view.nodes
            .iter()
            .find(|n| n.node == id)
            .expect("node present")
    }

    #[test]
    fn build_replay_view_classifies_nodes_edges_cost() {
        let graph = graph_impl_to_end();
        let impl_k = NodeKey::try_from("impl_1").unwrap();
        let end_k = NodeKey::try_from("end").unwrap();
        let done = OutcomeKey::try_from("done").unwrap();

        let events = vec![
            ev(
                3,
                EventPayload::StageEntered {
                    node: impl_k.clone(),
                    attempt: 1,
                },
            ),
            ev(
                4,
                EventPayload::TokensConsumed {
                    session: surge_core::id::SessionId::new(),
                    prompt_tokens: 100,
                    output_tokens: 50,
                    cache_hits: 10,
                    model: "m".into(),
                    cost_usd: Some(0.25),
                },
            ),
            ev(
                5,
                EventPayload::StageCompleted {
                    node: impl_k.clone(),
                    outcome: done.clone(),
                },
            ),
            ev(
                6,
                EventPayload::EdgeTraversed {
                    edge: EdgeKey::try_from("impl_to_end").unwrap(),
                    from: impl_k.clone(),
                    to: end_k.clone(),
                    kind: surge_core::edge::EdgeKind::Forward,
                },
            ),
            ev(
                7,
                EventPayload::StageEntered {
                    node: end_k.clone(),
                    attempt: 1,
                },
            ),
            ev(
                8,
                EventPayload::RunCompleted {
                    terminal_node: end_k.clone(),
                },
            ),
        ];

        let view = build_replay_view(&graph, &events);

        assert_eq!(view.event_count, 6);
        assert_eq!(view.terminal, Some(TerminalKind::Completed));
        // Terminal run → no active frontier.
        assert_eq!(view.active_node, None);

        // impl_1 entered then completed → Completed with the outcome.
        let impl_v = node_view(&view, "impl_1");
        assert_eq!(impl_v.status, NodeStatus::Completed);
        assert_eq!(impl_v.attempts, 1);
        assert_eq!(impl_v.last_outcome.as_deref(), Some("done"));

        // end entered but never completed → Active classification at node level.
        assert_eq!(node_view(&view, "end").status, NodeStatus::Active);

        // Edge captured.
        assert_eq!(view.edges_traversed.len(), 1);
        assert_eq!(view.edges_traversed[0].from, "impl_1");
        assert_eq!(view.edges_traversed[0].to, "end");

        // Cost summed.
        assert_eq!(view.cost.prompt_tokens, 100);
        assert_eq!(view.cost.output_tokens, 50);
        assert_eq!(view.cost.cache_hits, 10);
        assert!((view.cost.cost_usd - 0.25).abs() < 1e-9);
    }

    #[test]
    fn build_replay_view_future_node_and_active_frontier() {
        let graph = graph_impl_to_end();
        let impl_k = NodeKey::try_from("impl_1").unwrap();

        // Only impl_1 entered; not terminal. end stays Future, impl_1 Active.
        let events = vec![ev(
            3,
            EventPayload::StageEntered {
                node: impl_k,
                attempt: 2,
            },
        )];
        let view = build_replay_view(&graph, &events);

        assert_eq!(view.terminal, None);
        assert_eq!(view.active_node.as_deref(), Some("impl_1"));
        assert_eq!(node_view(&view, "impl_1").status, NodeStatus::Active);
        assert_eq!(node_view(&view, "impl_1").attempts, 2);
        assert_eq!(node_view(&view, "end").status, NodeStatus::Future);
    }
}
