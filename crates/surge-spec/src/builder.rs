//! Builder pattern for constructing specs programmatically.

use surge_core::id::{SpecId, SubtaskId};
use surge_core::spec::{AcceptanceCriteria, Complexity, Spec, Subtask};
use surge_core::SurgeError;

/// Builder for constructing Spec instances.
pub struct SpecBuilder {
    title: Option<String>,
    description: Option<String>,
    complexity: Complexity,
    subtasks: Vec<Subtask>,
}

impl SpecBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            complexity: Complexity::Standard,
            subtasks: vec![],
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    #[must_use]
    pub fn complexity(mut self, complexity: Complexity) -> Self {
        self.complexity = complexity;
        self
    }

    #[must_use]
    pub fn subtask(mut self, subtask: Subtask) -> Self {
        self.subtasks.push(subtask);
        self
    }

    pub fn build(self) -> Result<Spec, SurgeError> {
        let title = self.title
            .ok_or_else(|| SurgeError::Spec("Spec title is required".to_string()))?;
        let description = self.description
            .ok_or_else(|| SurgeError::Spec("Spec description is required".to_string()))?;

        Ok(Spec {
            id: SpecId::new(),
            title,
            description,
            complexity: self.complexity,
            subtasks: self.subtasks,
        })
    }
}

impl Default for SpecBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for constructing Subtask instances.
pub struct SubtaskBuilder {
    title: Option<String>,
    description: Option<String>,
    complexity: Complexity,
    files: Vec<String>,
    acceptance_criteria: Vec<AcceptanceCriteria>,
    depends_on: Vec<SubtaskId>,
}

impl SubtaskBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![],
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    #[must_use]
    pub fn complexity(mut self, complexity: Complexity) -> Self {
        self.complexity = complexity;
        self
    }

    #[must_use]
    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.files.push(path.into());
        self
    }

    #[must_use]
    pub fn criterion(mut self, description: impl Into<String>) -> Self {
        self.acceptance_criteria.push(AcceptanceCriteria {
            description: description.into(),
            met: false,
        });
        self
    }

    #[must_use]
    pub fn depends_on(mut self, id: SubtaskId) -> Self {
        self.depends_on.push(id);
        self
    }

    pub fn build(self) -> Result<Subtask, SurgeError> {
        let title = self.title
            .ok_or_else(|| SurgeError::Spec("Subtask title is required".to_string()))?;
        let description = self.description
            .ok_or_else(|| SurgeError::Spec("Subtask description is required".to_string()))?;

        Ok(Subtask {
            id: SubtaskId::new(),
            title,
            description,
            complexity: self.complexity,
            files: self.files,
            acceptance_criteria: self.acceptance_criteria,
            depends_on: self.depends_on,
            story_file: None,
            agent: None,
            execution: surge_core::spec::SubtaskExecution::default(),
        })
    }
}

impl Default for SubtaskBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_builder_basic() {
        let spec = SpecBuilder::new()
            .title("My feature")
            .description("Build a feature")
            .complexity(Complexity::Complex)
            .build()
            .unwrap();

        assert_eq!(spec.title, "My feature");
        assert_eq!(spec.description, "Build a feature");
        assert_eq!(spec.complexity, Complexity::Complex);
        assert!(spec.subtasks.is_empty());
    }

    #[test]
    fn test_spec_builder_with_subtasks() {
        let sub1 = SubtaskBuilder::new()
            .title("Step 1")
            .description("Do step 1")
            .file("src/lib.rs")
            .criterion("Compiles")
            .build()
            .unwrap();

        let sub1_id = sub1.id;

        let sub2 = SubtaskBuilder::new()
            .title("Step 2")
            .description("Do step 2")
            .depends_on(sub1_id)
            .build()
            .unwrap();

        let spec = SpecBuilder::new()
            .title("Feature")
            .description("A feature")
            .subtask(sub1)
            .subtask(sub2)
            .build()
            .unwrap();

        assert_eq!(spec.subtasks.len(), 2);
        assert_eq!(spec.subtasks[0].files, vec!["src/lib.rs"]);
        assert_eq!(spec.subtasks[0].acceptance_criteria.len(), 1);
        assert_eq!(spec.subtasks[1].depends_on, vec![sub1_id]);
    }

    #[test]
    fn test_spec_builder_missing_title() {
        let result = SpecBuilder::new()
            .description("No title")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_subtask_builder_missing_title() {
        let result = SubtaskBuilder::new()
            .description("No title")
            .build();
        assert!(result.is_err());
    }
}
