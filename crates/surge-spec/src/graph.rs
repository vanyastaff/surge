//! Dependency graph for spec subtasks — topological sorting and batch grouping.

use std::collections::HashMap;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::toposort;
use petgraph::Direction;
use surge_core::id::SubtaskId;
use surge_core::spec::Spec;
use surge_core::SurgeError;

/// A dependency graph of subtasks.
pub struct DependencyGraph {
    graph: DiGraph<SubtaskId, ()>,
    id_to_node: HashMap<SubtaskId, NodeIndex>,
    node_to_id: HashMap<NodeIndex, SubtaskId>,
}

impl DependencyGraph {
    /// Build a dependency graph from a spec.
    /// Edges point from dependency TO dependent (dep → subtask that needs it).
    pub fn from_spec(spec: &Spec) -> Result<Self, SurgeError> {
        let mut graph = DiGraph::new();
        let mut id_to_node = HashMap::new();
        let mut node_to_id = HashMap::new();

        for subtask in &spec.subtasks {
            let node = graph.add_node(subtask.id);
            id_to_node.insert(subtask.id, node);
            node_to_id.insert(node, subtask.id);
        }

        for subtask in &spec.subtasks {
            let target = id_to_node[&subtask.id];
            for dep_id in &subtask.depends_on {
                let source = id_to_node.get(dep_id).ok_or_else(|| {
                    SurgeError::Spec(format!(
                        "Subtask '{}' depends on unknown subtask {}",
                        subtask.title, dep_id
                    ))
                })?;
                graph.add_edge(*source, target, ());
            }
        }

        Ok(Self { graph, id_to_node, node_to_id })
    }

    /// Get topologically sorted subtask IDs.
    pub fn topological_order(&self) -> Result<Vec<SubtaskId>, SurgeError> {
        let sorted = toposort(&self.graph, None)
            .map_err(|_| SurgeError::Spec("Dependency cycle detected".to_string()))?;
        Ok(sorted.into_iter().map(|n| self.node_to_id[&n]).collect())
    }

    /// Group subtasks into batches for parallel execution.
    /// Each batch contains subtasks that can execute in parallel.
    pub fn topological_batches(&self) -> Result<Vec<Vec<SubtaskId>>, SurgeError> {
        let sorted = toposort(&self.graph, None)
            .map_err(|_| SurgeError::Spec("Dependency cycle detected".to_string()))?;

        let mut depths: HashMap<NodeIndex, usize> = HashMap::new();
        let mut max_depth = 0;

        for node in &sorted {
            let depth = self.graph
                .neighbors_directed(*node, Direction::Incoming)
                .map(|dep| depths.get(&dep).copied().unwrap_or(0) + 1)
                .max()
                .unwrap_or(0);
            depths.insert(*node, depth);
            max_depth = max_depth.max(depth);
        }

        let mut batches: Vec<Vec<SubtaskId>> = vec![vec![]; max_depth + 1];
        for (node, depth) in &depths {
            batches[*depth].push(self.node_to_id[node]);
        }

        batches.retain(|b| !b.is_empty());
        Ok(batches)
    }

    /// Render dependency graph as ASCII text.
    pub fn to_ascii(&self, spec: &Spec) -> String {
        let title_map: HashMap<SubtaskId, &str> = spec
            .subtasks
            .iter()
            .map(|s| (s.id, s.title.as_str()))
            .collect();

        let Ok(batches) = self.topological_batches() else {
            return "Error: cycle in dependency graph".to_string();
        };

        let mut lines = vec![];

        for (i, batch) in batches.iter().enumerate() {
            lines.push(format!("Batch {}:", i + 1));
            for id in batch {
                let title = title_map.get(id).unwrap_or(&"???");
                let deps: Vec<&str> = spec.subtasks.iter()
                    .find(|s| s.id == *id)
                    .map(|s| {
                        s.depends_on.iter()
                            .filter_map(|d| title_map.get(d).copied())
                            .collect()
                    })
                    .unwrap_or_default();

                if deps.is_empty() {
                    lines.push(format!("  ├── {title}"));
                } else {
                    lines.push(format!("  ├── {title} (after: {})", deps.join(", ")));
                }
            }
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SpecId;
    use surge_core::spec::{Complexity, Subtask};

    fn make_subtask(title: &str, depends_on: Vec<SubtaskId>) -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: title.to_string(),
            description: format!("Do {title}"),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on,
        }
    }

    #[test]
    fn test_no_dependencies_single_batch() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Parallel".to_string(),
            description: "All parallel".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![
                make_subtask("A", vec![]),
                make_subtask("B", vec![]),
                make_subtask("C", vec![]),
            ],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let batches = graph.topological_batches().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 3);
    }

    #[test]
    fn test_linear_chain_n_batches() {
        let a = make_subtask("A", vec![]);
        let b = make_subtask("B", vec![a.id]);
        let c = make_subtask("C", vec![b.id]);

        let spec = Spec {
            id: SpecId::new(),
            title: "Linear".to_string(),
            description: "Linear chain".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![a, b, c],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let batches = graph.topological_batches().unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 1);
        assert_eq!(batches[2].len(), 1);
    }

    #[test]
    fn test_diamond_dependency() {
        let a = make_subtask("A", vec![]);
        let b = make_subtask("B", vec![a.id]);
        let c = make_subtask("C", vec![a.id]);
        let d = make_subtask("D", vec![b.id, c.id]);

        let spec = Spec {
            id: SpecId::new(),
            title: "Diamond".to_string(),
            description: "Diamond".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![a, b, c, d],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let batches = graph.topological_batches().unwrap();

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 1); // A
        assert_eq!(batches[1].len(), 2); // B, C parallel
        assert_eq!(batches[2].len(), 1); // D
    }

    #[test]
    fn test_topological_order() {
        let a = make_subtask("A", vec![]);
        let b = make_subtask("B", vec![a.id]);
        let a_id = a.id;
        let b_id = b.id;

        let spec = Spec {
            id: SpecId::new(),
            title: "Order".to_string(),
            description: "Order test".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![a, b],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let order = graph.topological_order().unwrap();
        let a_pos = order.iter().position(|id| *id == a_id).unwrap();
        let b_pos = order.iter().position(|id| *id == b_id).unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn test_ascii_output() {
        let a = make_subtask("Setup", vec![]);
        let b = make_subtask("Implement", vec![a.id]);

        let spec = Spec {
            id: SpecId::new(),
            title: "Test".to_string(),
            description: "Test".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![a, b],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let ascii = graph.to_ascii(&spec);
        assert!(ascii.contains("Batch 1:"));
        assert!(ascii.contains("Setup"));
        assert!(ascii.contains("Implement"));
        assert!(ascii.contains("after: Setup"));
    }
}
