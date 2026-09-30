//! A flow graph split into its levels, each with its own flow.
//!
//! A roadmap flow nests: the whole project's steps, a loop over milestones
//! whose body is the milestone's own flow, and inside it a loop over tasks
//! whose body is the task's own flow. Each level is shown on its own; a loop
//! appears in its parent level as one node that stands for the level below.
//! Level names come from the graph — "Each milestone" is the loop's
//! `iteration_var_name` — never from a hard-coded shape.

use std::collections::{BTreeMap, VecDeque};

use surge_core::graph::Graph;
use surge_core::node::NodeConfig;

/// One level of a flow.
#[derive(Clone, Debug)]
pub struct FlowLevel {
    /// "Whole project", "Each milestone", "Each task", …
    pub title: String,
    /// Nesting depth (0 = the top graph).
    pub depth: usize,
    /// The level as a stand-alone graph (no nested bodies).
    pub graph: Graph,
    /// Steps someone does at this level (agents and approvals).
    pub work_steps: usize,
    pub approvals: usize,
    /// The loop / sub-flow node in the parent level that opens this level.
    pub opened_by: Option<String>,
    /// Parent occurrence, rather than the shared body's declaration.
    pub parent: Option<usize>,
    /// Child links that cannot safely be expanded in this preview.
    pub unopened: Vec<UnopenedFlow>,
}

/// Preview bounds include the root occurrence. They do not limit execution.
pub(crate) const MAX_FLOW_OCCURRENCES: usize = 256;
pub(crate) const MAX_FLOW_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnopenedReason {
    Recursive,
    MissingBody,
    DepthLimit,
    OccurrenceLimit,
}

impl UnopenedReason {
    pub fn description(self) -> &'static str {
        match self {
            Self::Recursive => "recursive link; expansion stopped",
            Self::MissingBody => "body definition missing",
            Self::DepthLimit => "preview depth limit reached",
            Self::OccurrenceLimit => "preview occurrence limit reached",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UnopenedFlow {
    pub opened_by: String,
    pub body: String,
    pub reason: UnopenedReason,
}

fn count(graph: &Graph) -> (usize, usize) {
    let work = graph
        .nodes
        .values()
        .filter(|n| matches!(n.config, NodeConfig::Agent(_) | NodeConfig::HumanGate(_)))
        .count();
    let approvals = graph
        .nodes
        .values()
        .filter(|n| matches!(n.config, NodeConfig::HumanGate(_)))
        .count();
    (work, approvals)
}

/// Split `graph` into levels, outermost first.
pub fn levels(graph: &Graph) -> Vec<FlowLevel> {
    let mut top = graph.clone();
    top.subgraphs = BTreeMap::new();
    let (work_steps, approvals) = count(&top);
    let mut out = vec![FlowLevel {
        title: "Whole project".into(),
        depth: 0,
        graph: top,
        work_steps,
        approvals,
        opened_by: None,
        parent: None,
        unopened: Vec::new(),
    }];

    // Every call gets its own occurrence. Only ancestors prevent recursion.
    let mut queue = VecDeque::from([(0, &graph.nodes, Vec::new())]);
    while let Some((parent, nodes, ancestors)) = queue.pop_front() {
        for (node_key, node) in nodes {
            let (key, title) = match &node.config {
                NodeConfig::Loop(l) => (
                    &l.body,
                    format!("Each {}", l.iteration_var_name.replace('_', " ")),
                ),
                NodeConfig::Subgraph(s) => (
                    &s.inner,
                    format!("Sub-flow · {}", s.inner.as_str().replace('_', " ")),
                ),
                _ => continue,
            };
            let depth = out[parent].depth + 1;
            let reason = if ancestors.contains(key) {
                Some(UnopenedReason::Recursive)
            } else if !graph.subgraphs.contains_key(key) {
                Some(UnopenedReason::MissingBody)
            } else if depth > MAX_FLOW_DEPTH {
                Some(UnopenedReason::DepthLimit)
            } else if out.len() >= MAX_FLOW_OCCURRENCES {
                Some(UnopenedReason::OccurrenceLimit)
            } else {
                None
            };
            if let Some(reason) = reason {
                out[parent].unopened.push(UnopenedFlow {
                    opened_by: node_key.as_str().to_owned(),
                    body: key.as_str().to_owned(),
                    reason,
                });
                continue;
            }
            let Some(sub) = graph.subgraphs.get(key) else {
                continue;
            };
            let level_graph = Graph {
                schema_version: graph.schema_version,
                metadata: graph.metadata.clone(),
                start: sub.start.clone(),
                nodes: sub.nodes.clone(),
                edges: sub.edges.clone(),
                subgraphs: BTreeMap::new(),
            };
            let (work_steps, approvals) = count(&level_graph);
            let index = out.len();
            out.push(FlowLevel {
                title,
                depth,
                graph: level_graph,
                work_steps,
                approvals,
                opened_by: Some(node_key.as_str().to_owned()),
                parent: Some(parent),
                unopened: Vec::new(),
            });
            let mut branch = ancestors.clone();
            branch.push(key.clone());
            queue.push_back((index, &sub.nodes, branch));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::levels;

    #[test]
    fn nested_roadmap_flow_splits_into_its_three_levels() {
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml"))
                .expect("fixture parses");
        let levels = levels(&graph);
        let titles: Vec<&str> = levels.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(titles, ["Whole project", "Each milestone", "Each task"]);
        assert_eq!(
            levels.iter().map(|l| l.depth).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        // No level carries nested bodies; each is drawn on its own.
        assert!(levels.iter().all(|l| l.graph.subgraphs.is_empty()));
        // The task level is where the building happens.
        assert!(levels[2].work_steps >= 3);
        // Each nested level knows which loop opens it.
        assert_eq!(levels[1].opened_by.as_deref(), Some("milestone_loop"));
        assert_eq!(levels[2].opened_by.as_deref(), Some("task_loop"));
    }

    #[test]
    fn reused_body_keeps_both_callers_and_their_nested_occurrences() {
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).unwrap();
        let mut second =
            graph.nodes[&surge_core::keys::NodeKey::try_from("milestone_loop").unwrap()].clone();
        second.id = surge_core::keys::NodeKey::try_from("second_milestone_loop").unwrap();
        graph.nodes.insert(second.id.clone(), second);
        let levels = levels(&graph);
        // Two calls of the same milestone body each retain their task body.
        assert_eq!(levels.len(), 5);
        assert_eq!(
            levels.iter().map(|level| level.parent).collect::<Vec<_>>(),
            [None, Some(0), Some(0), Some(1), Some(2)]
        );
        assert_eq!(
            levels
                .iter()
                .filter(|level| level.opened_by.as_deref() == Some("task_loop"))
                .count(),
            2
        );
    }

    #[test]
    fn four_levels_have_fixed_parent_ancestry() {
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/four_level_flow.toml")).unwrap();
        let levels = levels(&graph);
        assert_eq!(
            levels
                .iter()
                .map(|level| level.title.as_str())
                .collect::<Vec<_>>(),
            [
                "Whole project",
                "Each milestone",
                "Each task",
                "Each subtask"
            ]
        );
        assert_eq!(
            levels.iter().map(|level| level.parent).collect::<Vec<_>>(),
            [None, Some(0), Some(1), Some(2)]
        );
        assert_eq!(
            levels.iter().map(|level| level.depth).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn recursive_and_missing_links_are_reported_on_their_parent() {
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).unwrap();
        let body = graph
            .subgraphs
            .get_mut(&surge_core::keys::SubgraphKey::try_from("milestone_body").unwrap())
            .unwrap();
        let node = body
            .nodes
            .get_mut(&surge_core::keys::NodeKey::try_from("task_loop").unwrap())
            .unwrap();
        let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
            panic!("loop")
        };
        config.body = surge_core::keys::SubgraphKey::try_from("milestone_body").unwrap();
        let recursive = levels(&graph);
        assert_eq!(recursive.len(), 2);
        assert_eq!(
            recursive[1].unopened[0].reason,
            super::UnopenedReason::Recursive
        );
        graph
            .subgraphs
            .remove(&surge_core::keys::SubgraphKey::try_from("milestone_body").unwrap());
        let missing = levels(&graph);
        assert_eq!(missing.len(), 1);
        assert_eq!(
            missing[0].unopened[0].reason,
            super::UnopenedReason::MissingBody
        );
    }

    #[test]
    fn branching_reuse_is_bounded_with_explicit_truncation() {
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).unwrap();
        let template =
            graph.nodes[&surge_core::keys::NodeKey::try_from("milestone_loop").unwrap()].clone();
        graph.nodes.clear();
        graph.subgraphs.clear();
        for depth in 0..10 {
            let mut nodes = std::collections::BTreeMap::new();
            for branch in ["left", "right"] {
                let mut node = template.clone();
                node.id = surge_core::keys::NodeKey::try_from(format!("{branch}_{depth}")).unwrap();
                let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
                    panic!("loop")
                };
                config.body =
                    surge_core::keys::SubgraphKey::try_from(format!("body_{}", depth + 1)).unwrap();
                nodes.insert(node.id.clone(), node);
            }
            if depth == 0 {
                graph.nodes = nodes;
            } else {
                graph.subgraphs.insert(
                    surge_core::keys::SubgraphKey::try_from(format!("body_{depth}")).unwrap(),
                    surge_core::graph::Subgraph {
                        start: nodes.keys().next().unwrap().clone(),
                        nodes,
                        edges: Vec::new(),
                    },
                );
            }
        }
        let result = levels(&graph);
        // This declared binary DAG would have over a thousand occurrences.
        assert_eq!(result.len(), 256);
        assert!(
            result
                .iter()
                .flat_map(|level| &level.unopened)
                .any(|link| link.reason == super::UnopenedReason::OccurrenceLimit)
        );
    }

    #[test]
    fn deep_chain_stops_at_fixed_depth_and_reports_the_unopened_link() {
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).unwrap();
        let template =
            graph.nodes[&surge_core::keys::NodeKey::try_from("milestone_loop").unwrap()].clone();
        graph.nodes.clear();
        graph.subgraphs.clear();
        for depth in 0..20 {
            let mut node = template.clone();
            node.id = surge_core::keys::NodeKey::try_from(format!("chain_{depth}")).unwrap();
            let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
                panic!("loop")
            };
            config.body =
                surge_core::keys::SubgraphKey::try_from(format!("chain_body_{}", depth + 1))
                    .unwrap();
            let start = node.id.clone();
            let nodes = std::collections::BTreeMap::from([(start.clone(), node)]);
            if depth == 0 {
                graph.nodes = nodes;
            } else {
                graph.subgraphs.insert(
                    surge_core::keys::SubgraphKey::try_from(format!("chain_body_{depth}")).unwrap(),
                    surge_core::graph::Subgraph {
                        start,
                        nodes,
                        edges: Vec::new(),
                    },
                );
            }
        }
        let result = levels(&graph);
        assert_eq!(result.len(), 17);
        assert_eq!(result.last().unwrap().depth, 16);
        assert_eq!(
            result.last().unwrap().unopened[0].reason,
            super::UnopenedReason::DepthLimit
        );
        let review = crate::flow_review::flow_review_markdown(&toml::to_string(&graph).unwrap());
        assert!(
            review
                .split("#### flow.toml")
                .next()
                .unwrap()
                .contains("preview depth limit reached")
        );
    }

    #[test]
    fn a_flat_flow_is_one_level() {
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../examples/flow_linear_3.toml"))
                .expect("example parses");
        let levels = levels(&graph);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].work_steps, 3);
    }
}
