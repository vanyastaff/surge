//! Spec validation — check integrity, references, and cycles.

use std::collections::{HashMap, HashSet};
use surge_core::id::SubtaskId;
use surge_core::spec::Spec;
use surge_core::SurgeError;

/// Validation result with warnings and errors.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn into_result(self) -> Result<Vec<String>, SurgeError> {
        if self.errors.is_empty() {
            Ok(self.warnings)
        } else {
            Err(SurgeError::Spec(format!(
                "Spec validation failed:\n{}",
                self.errors.join("\n")
            )))
        }
    }
}

/// Validate a spec for correctness.
pub fn validate(spec: &Spec) -> ValidationResult {
    let mut result = ValidationResult::default();

    if spec.title.trim().is_empty() {
        result.errors.push("Spec title is empty".to_string());
    }

    if spec.description.trim().is_empty() {
        result.errors.push("Spec description is empty".to_string());
    }

    if spec.subtasks.is_empty() {
        result.warnings.push("Spec has no subtasks".to_string());
        return result;
    }

    let valid_ids: HashSet<SubtaskId> = spec.subtasks.iter().map(|s| s.id).collect();

    if valid_ids.len() != spec.subtasks.len() {
        result.errors.push("Duplicate subtask IDs found".to_string());
    }

    for subtask in &spec.subtasks {
        if subtask.title.trim().is_empty() {
            result.errors.push(format!("Subtask {} has empty title", subtask.id));
        }
        if subtask.description.trim().is_empty() {
            result.warnings.push(format!("Subtask '{}' has empty description", subtask.title));
        }
        for dep_id in &subtask.depends_on {
            if !valid_ids.contains(dep_id) {
                result.errors.push(format!(
                    "Subtask '{}' depends on non-existent subtask {}",
                    subtask.title, dep_id
                ));
            }
        }
        if subtask.depends_on.contains(&subtask.id) {
            result.errors.push(format!(
                "Subtask '{}' depends on itself",
                subtask.title
            ));
        }
    }

    if has_cycle(spec) {
        result.errors.push("Dependency cycle detected among subtasks".to_string());
    }

    result
}

fn has_cycle(spec: &Spec) -> bool {
    let id_to_deps: HashMap<SubtaskId, &Vec<SubtaskId>> = spec
        .subtasks
        .iter()
        .map(|s| (s.id, &s.depends_on))
        .collect();

    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();

    for subtask in &spec.subtasks {
        if !visited.contains(&subtask.id)
            && dfs_has_cycle(subtask.id, &id_to_deps, &mut visited, &mut in_stack)
        {
            return true;
        }
    }

    false
}

fn dfs_has_cycle(
    node: SubtaskId,
    graph: &HashMap<SubtaskId, &Vec<SubtaskId>>,
    visited: &mut HashSet<SubtaskId>,
    in_stack: &mut HashSet<SubtaskId>,
) -> bool {
    visited.insert(node);
    in_stack.insert(node);

    if let Some(deps) = graph.get(&node) {
        for dep in *deps {
            if !visited.contains(dep) {
                if dfs_has_cycle(*dep, graph, visited, in_stack) {
                    return true;
                }
            } else if in_stack.contains(dep) {
                return true;
            }
        }
    }

    in_stack.remove(&node);
    false
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
    fn test_valid_spec() {
        let sub1 = make_subtask("Step 1", vec![]);
        let sub2 = make_subtask("Step 2", vec![sub1.id]);
        let spec = Spec {
            id: SpecId::new(),
            title: "Valid spec".to_string(),
            description: "A valid spec".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![sub1, sub2],
        };
        let result = validate(&spec);
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn test_empty_title() {
        let spec = Spec {
            id: SpecId::new(),
            title: "".to_string(),
            description: "Desc".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("title is empty")));
    }

    #[test]
    fn test_invalid_dependency_ref() {
        let fake_id = SubtaskId::new();
        let sub1 = make_subtask("Step 1", vec![fake_id]);
        let spec = Spec {
            id: SpecId::new(),
            title: "Bad refs".to_string(),
            description: "Has bad refs".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![sub1],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("non-existent")));
    }

    #[test]
    fn test_self_dependency() {
        let mut sub1 = make_subtask("Step 1", vec![]);
        sub1.depends_on = vec![sub1.id];
        let spec = Spec {
            id: SpecId::new(),
            title: "Self dep".to_string(),
            description: "Self dependency".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![sub1],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("depends on itself")));
    }

    #[test]
    fn test_cycle_detection() {
        let id_a = SubtaskId::new();
        let id_b = SubtaskId::new();
        let sub_a = Subtask {
            id: id_a,
            title: "A".to_string(),
            description: "A".to_string(),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![id_b],
        };
        let sub_b = Subtask {
            id: id_b,
            title: "B".to_string(),
            description: "B".to_string(),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![id_a],
        };
        let spec = Spec {
            id: SpecId::new(),
            title: "Cycle".to_string(),
            description: "Has cycle".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![sub_a, sub_b],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn test_no_subtasks_warning() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Empty".to_string(),
            description: "No subtasks".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![],
        };
        let result = validate(&spec);
        assert!(result.is_ok());
        assert!(result.warnings.iter().any(|w| w.contains("no subtasks")));
    }
}
