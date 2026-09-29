//! A flow graph split into its levels, each with its own flow.
//!
//! A roadmap flow nests: the whole project's steps, a loop over milestones
//! whose body is the milestone's own flow, and inside it a loop over tasks
//! whose body is the task's own flow. Each level is shown on its own; a loop
//! appears in its parent level as one node that stands for the level below.
//! Level names come from the graph — "Each milestone" is the loop's
//! `iteration_var_name` — never from a hard-coded shape.

use std::collections::BTreeMap;

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
    }];

    // Breadth-first over loop / sub-flow bodies, guarding against cycles.
    let mut queue: Vec<(usize, BTreeMap<_, _>)> = vec![(0, graph.nodes.clone())];
    let mut seen = std::collections::BTreeSet::new();
    while let Some((depth, nodes)) = queue.first().cloned() {
        queue.remove(0);
        for (node_key, node) in &nodes {
            let (key, title) = match &node.config {
                NodeConfig::Loop(l) => (l.body.clone(), format!("Each {}", l.iteration_var_name.replace('_', " "))),
                NodeConfig::Subgraph(s) => (s.inner.clone(), format!("Sub-flow · {}", s.inner.as_str().replace('_', " "))),
                _ => continue,
            };
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(sub) = graph.subgraphs.get(&key) else {
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
            out.push(FlowLevel {
                title,
                depth: depth + 1,
                graph: level_graph,
                work_steps,
                approvals,
                opened_by: Some(node_key.as_str().to_string()),
            });
            queue.push((depth + 1, sub.nodes.clone()));
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
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).expect("fixture parses");
        let levels = levels(&graph);
        let titles: Vec<&str> = levels.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(titles, ["Whole project", "Each milestone", "Each task"]);
        assert_eq!(levels.iter().map(|l| l.depth).collect::<Vec<_>>(), [0, 1, 2]);
        // No level carries nested bodies; each is drawn on its own.
        assert!(levels.iter().all(|l| l.graph.subgraphs.is_empty()));
        // The task level is where the building happens.
        assert!(levels[2].work_steps >= 3);
        // Each nested level knows which loop opens it.
        assert_eq!(levels[1].opened_by.as_deref(), Some("milestone_loop"));
        assert_eq!(levels[2].opened_by.as_deref(), Some("task_loop"));
    }

    #[test]
    fn a_flat_flow_is_one_level() {
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../examples/flow_linear_3.toml")).expect("example parses");
        let levels = levels(&graph);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].work_steps, 3);
    }
}
