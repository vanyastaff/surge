//! Spec validation — check integrity, references, and cycles.

use std::collections::{HashMap, HashSet};
use surge_core::SurgeError;
use surge_core::id::SubtaskId;
use surge_core::spec::Spec;

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

    const RECOMMENDED_MAX_SUBTASKS: usize = 7;
    const HARD_MAX_SUBTASKS: usize = 15;

    if spec.subtasks.len() > HARD_MAX_SUBTASKS {
        result.errors.push(format!(
            "Too many subtasks: {}. Max is {}. Split into multiple specs.",
            spec.subtasks.len(),
            HARD_MAX_SUBTASKS
        ));
    } else if spec.subtasks.len() > RECOMMENDED_MAX_SUBTASKS {
        result.warnings.push(format!(
            "Many subtasks: {}. Consider splitting to avoid context overflow (recommended max: {}).",
            spec.subtasks.len(),
            RECOMMENDED_MAX_SUBTASKS
        ));
    }

    let valid_ids: HashSet<SubtaskId> = spec.subtasks.iter().map(|s| s.id).collect();

    if valid_ids.len() != spec.subtasks.len() {
        result
            .errors
            .push("Duplicate subtask IDs found".to_string());
    }

    for subtask in &spec.subtasks {
        if subtask.title.trim().is_empty() {
            result
                .errors
                .push(format!("Subtask {} has empty title", subtask.id));
        }
        if subtask.description.trim().is_empty() {
            result
                .warnings
                .push(format!("Subtask '{}' has empty description", subtask.title));
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
            result
                .errors
                .push(format!("Subtask '{}' depends on itself", subtask.title));
        }
    }

    if has_cycle(spec) {
        result
            .errors
            .push("Dependency cycle detected among subtasks".to_string());
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
    use surge_core::spec::{Complexity, Subtask};

    fn make_subtask(title: &str, depends_on: Vec<SubtaskId>) -> Subtask {
        let mut subtask = Subtask::new(title, format!("Do {title}"), Complexity::Simple);
        subtask.depends_on = depends_on;
        subtask
    }

    fn spec_with(title: &str, complexity: Complexity, subtasks: Vec<Subtask>) -> Spec {
        let description = if subtasks.is_empty() {
            "No subtasks".to_string()
        } else {
            format!("{title} description")
        };
        let mut spec = Spec::new(title, description, complexity);
        spec.subtasks = subtasks;
        spec
    }

    #[test]
    fn test_valid_spec() {
        let sub1 = make_subtask("Step 1", vec![]);
        let sub2 = make_subtask("Step 2", vec![sub1.id]);
        let spec = spec_with("Valid spec", Complexity::Standard, vec![sub1, sub2]);
        let result = validate(&spec);
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn test_empty_title() {
        let mut spec = Spec::new("", "Desc", Complexity::Simple);
        spec.subtasks = vec![];
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("title is empty")));
    }

    #[test]
    fn test_invalid_dependency_ref() {
        let fake_id = SubtaskId::new();
        let sub1 = make_subtask("Step 1", vec![fake_id]);
        let spec = spec_with("Bad refs", Complexity::Simple, vec![sub1]);
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("non-existent")));
    }

    #[test]
    fn test_self_dependency() {
        let mut sub1 = make_subtask("Step 1", vec![]);
        sub1.depends_on = vec![sub1.id];
        let spec = spec_with("Self dep", Complexity::Simple, vec![sub1]);
        let result = validate(&spec);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("depends on itself"))
        );
    }

    #[test]
    fn test_cycle_detection() {
        let mut sub_a = Subtask::new("A", "A", Complexity::Simple);
        let mut sub_b = Subtask::new("B", "B", Complexity::Simple);
        sub_a.depends_on = vec![sub_b.id];
        sub_b.depends_on = vec![sub_a.id];
        let spec = spec_with("Cycle", Complexity::Simple, vec![sub_a, sub_b]);
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn test_subtask_count_warning_at_eight() {
        let subtasks: Vec<Subtask> = (0..8)
            .map(|i| make_subtask(&format!("Step {i}"), vec![]))
            .collect();
        let spec = spec_with("Many subtasks", Complexity::Complex, subtasks);
        let result = validate(&spec);
        assert!(result.is_ok(), "errors: {:?}", result.errors);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("Consider splitting"))
        );
    }

    #[test]
    fn test_subtask_count_error_at_sixteen() {
        let subtasks: Vec<Subtask> = (0..16)
            .map(|i| make_subtask(&format!("Step {i}"), vec![]))
            .collect();
        let spec = spec_with("Too many subtasks", Complexity::Complex, subtasks);
        let result = validate(&spec);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("Too many subtasks"))
        );
    }

    #[test]
    fn test_no_subtasks_warning() {
        let spec = spec_with("Empty", Complexity::Simple, vec![]);
        let result = validate(&spec);
        assert!(result.is_ok());
        assert!(result.warnings.iter().any(|w| w.contains("no subtasks")));
    }
}
