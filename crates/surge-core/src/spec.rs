//! Spec types for Surge task definitions.

use serde::{Deserialize, Serialize};
use crate::id::{SpecId, SubtaskId};

/// Complexity level for a task or subtask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Complexity {
    Simple,
    Standard,
    Complex,
}

/// Acceptance criteria for a subtask.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriteria {
    /// Human-readable description of what must be true.
    pub description: String,
    /// Whether this criterion is currently met.
    #[serde(default)]
    pub met: bool,
}

/// A subtask within a spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    /// Unique identifier.
    pub id: SubtaskId,
    /// Short title.
    pub title: String,
    /// Detailed description of work to do.
    pub description: String,
    /// Estimated complexity.
    pub complexity: Complexity,
    /// Files this subtask will touch.
    #[serde(default)]
    pub files: Vec<String>,
    /// Acceptance criteria that must pass.
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriteria>,
    /// Dependencies on other subtask IDs (must complete first).
    #[serde(default)]
    pub depends_on: Vec<SubtaskId>,
}

/// A complete spec describing a unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// Unique identifier.
    pub id: SpecId,
    /// Short title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Overall complexity.
    pub complexity: Complexity,
    /// Ordered list of subtasks.
    #[serde(default)]
    pub subtasks: Vec<Subtask>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complexity_roundtrip() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Wrapper {
            complexity: Complexity,
        }

        for variant in [Complexity::Simple, Complexity::Standard, Complexity::Complex] {
            let wrapper = Wrapper { complexity: variant };
            let serialized = toml::to_string(&wrapper).unwrap();
            let deserialized: Wrapper = toml::from_str(&serialized).unwrap();
            assert_eq!(deserialized.complexity, variant);
        }
    }

    #[test]
    fn test_spec_toml_roundtrip() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Test spec".to_string(),
            description: "A test specification".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![Subtask {
                id: SubtaskId::new(),
                title: "First subtask".to_string(),
                description: "Do the thing".to_string(),
                complexity: Complexity::Simple,
                files: vec!["src/main.rs".to_string()],
                acceptance_criteria: vec![AcceptanceCriteria {
                    description: "It compiles".to_string(),
                    met: false,
                }],
                depends_on: vec![],
            }],
        };

        let toml_str = toml::to_string(&spec).unwrap();
        let deserialized: Spec = toml::from_str(&toml_str).unwrap();

        assert_eq!(deserialized.id, spec.id);
        assert_eq!(deserialized.title, spec.title);
        assert_eq!(deserialized.description, spec.description);
        assert_eq!(deserialized.complexity, spec.complexity);
        assert_eq!(deserialized.subtasks.len(), 1);
        assert_eq!(deserialized.subtasks[0].title, "First subtask");
        assert_eq!(deserialized.subtasks[0].files, vec!["src/main.rs"]);
        assert_eq!(deserialized.subtasks[0].acceptance_criteria.len(), 1);
        assert_eq!(
            deserialized.subtasks[0].acceptance_criteria[0].description,
            "It compiles"
        );
    }

    #[test]
    fn test_acceptance_criteria_default_met() {
        let toml_str = r#"description = "Tests pass""#;
        let criteria: AcceptanceCriteria = toml::from_str(toml_str).unwrap();
        assert_eq!(criteria.description, "Tests pass");
        assert!(!criteria.met);
    }
}
