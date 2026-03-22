//! Built-in spec templates for common task types.

use surge_core::spec::Complexity;
use crate::builder::{SpecBuilder, SubtaskBuilder};
use crate::parser::SpecFile;
use surge_core::SurgeError;

/// Available built-in template types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Feature,
    Bugfix,
    Refactor,
}

impl TemplateKind {
    /// Parse from string.
    pub fn from_str(s: &str) -> Result<Self, SurgeError> {
        match s.to_lowercase().as_str() {
            "feature" => Ok(Self::Feature),
            "bugfix" | "fix" => Ok(Self::Bugfix),
            "refactor" => Ok(Self::Refactor),
            _ => Err(SurgeError::Spec(format!(
                "Unknown template '{}'. Available: feature, bugfix, refactor", s
            ))),
        }
    }

    /// List all available template names.
    pub fn all() -> &'static [&'static str] {
        &["feature", "bugfix", "refactor"]
    }
}

/// Generate a spec from a template.
pub fn generate(kind: TemplateKind, description: &str) -> Result<SpecFile, SurgeError> {
    let spec = match kind {
        TemplateKind::Feature => {
            let sub1 = SubtaskBuilder::new()
                .title("Design and plan")
                .description("Define the approach, identify files to modify, write acceptance criteria")
                .complexity(Complexity::Simple)
                .criterion("Approach documented")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Implement core logic")
                .description("Write the main implementation code")
                .complexity(Complexity::Standard)
                .criterion("Core logic works")
                .criterion("Tests pass")
                .depends_on(sub1_id)
                .build()?;
            let sub2_id = sub2.id;

            let sub3 = SubtaskBuilder::new()
                .title("Integration and tests")
                .description("Wire up the feature, add integration tests, update docs")
                .complexity(Complexity::Simple)
                .criterion("Integration tests pass")
                .criterion("Documentation updated")
                .depends_on(sub2_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Feature: {description}"))
                .complexity(Complexity::Standard)
                .subtask(sub1)
                .subtask(sub2)
                .subtask(sub3)
                .build()?
        }
        TemplateKind::Bugfix => {
            let sub1 = SubtaskBuilder::new()
                .title("Reproduce and diagnose")
                .description("Write a failing test that reproduces the bug")
                .complexity(Complexity::Simple)
                .criterion("Failing test exists")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Fix the bug")
                .description("Implement the minimal fix to make the test pass")
                .complexity(Complexity::Simple)
                .criterion("Failing test now passes")
                .criterion("No regressions")
                .depends_on(sub1_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Bugfix: {description}"))
                .complexity(Complexity::Simple)
                .subtask(sub1)
                .subtask(sub2)
                .build()?
        }
        TemplateKind::Refactor => {
            let sub1 = SubtaskBuilder::new()
                .title("Ensure test coverage")
                .description("Add tests for current behavior before refactoring")
                .complexity(Complexity::Simple)
                .criterion("Existing behavior covered by tests")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Refactor")
                .description("Apply the refactoring, keep tests green")
                .complexity(Complexity::Standard)
                .criterion("All tests still pass")
                .criterion("Code is cleaner")
                .depends_on(sub1_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Refactor: {description}"))
                .complexity(Complexity::Simple)
                .subtask(sub1)
                .subtask(sub2)
                .build()?
        }
    };

    Ok(SpecFile { spec })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_kind_from_str() {
        assert_eq!(TemplateKind::from_str("feature").unwrap(), TemplateKind::Feature);
        assert_eq!(TemplateKind::from_str("bugfix").unwrap(), TemplateKind::Bugfix);
        assert_eq!(TemplateKind::from_str("fix").unwrap(), TemplateKind::Bugfix);
        assert_eq!(TemplateKind::from_str("refactor").unwrap(), TemplateKind::Refactor);
        assert!(TemplateKind::from_str("unknown").is_err());
    }

    #[test]
    fn test_feature_template() {
        let spec_file = generate(TemplateKind::Feature, "Add user auth").unwrap();
        assert_eq!(spec_file.spec.title, "Add user auth");
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        assert_eq!(spec_file.spec.subtasks[1].depends_on.len(), 1);
        assert_eq!(spec_file.spec.subtasks[1].depends_on[0], spec_file.spec.subtasks[0].id);
    }

    #[test]
    fn test_bugfix_template() {
        let spec_file = generate(TemplateKind::Bugfix, "Fix login crash").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 2);
        assert_eq!(spec_file.spec.complexity, Complexity::Simple);
    }

    #[test]
    fn test_refactor_template() {
        let spec_file = generate(TemplateKind::Refactor, "Extract auth module").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 2);
    }

    #[test]
    fn test_template_toml_roundtrip() {
        let spec_file = generate(TemplateKind::Feature, "Test feature").unwrap();
        let toml_str = spec_file.to_toml().unwrap();
        let parsed = SpecFile::from_toml(&toml_str).unwrap();
        assert_eq!(parsed.spec.title, "Test feature");
        assert_eq!(parsed.spec.subtasks.len(), 3);
    }
}
