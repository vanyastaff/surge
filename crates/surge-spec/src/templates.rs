//! Built-in spec templates for common task types.

use crate::builder::{SpecBuilder, SubtaskBuilder};
use crate::parser::SpecFile;
use surge_core::SurgeError;
use surge_core::spec::Complexity;

/// Available built-in template types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Feature,
    Bugfix,
    Refactor,
    Performance,
    Security,
    Docs,
    Migration,
}

impl TemplateKind {
    /// Parse from string.
    pub fn parse(s: &str) -> Result<Self, SurgeError> {
        match s.to_lowercase().as_str() {
            "feature" => Ok(Self::Feature),
            "bugfix" | "fix" => Ok(Self::Bugfix),
            "refactor" => Ok(Self::Refactor),
            "performance" | "perf" => Ok(Self::Performance),
            "security" | "sec" => Ok(Self::Security),
            "docs" | "doc" => Ok(Self::Docs),
            "migration" | "migrate" => Ok(Self::Migration),
            _ => Err(SurgeError::Spec(format!(
                "Unknown template '{}'. Available: {}",
                s,
                Self::all().join(", ")
            ))),
        }
    }

    /// List all available template names.
    pub fn all() -> &'static [&'static str] {
        &[
            "feature",
            "bugfix",
            "refactor",
            "performance",
            "security",
            "docs",
            "migration",
        ]
    }
}

/// Generate a spec from a template.
pub fn generate(kind: TemplateKind, description: &str) -> Result<SpecFile, SurgeError> {
    let spec = match kind {
        TemplateKind::Feature => {
            let sub1 = SubtaskBuilder::new()
                .title("Design and plan")
                .description(
                    "Define the approach, identify files to modify, write acceptance criteria",
                )
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
        TemplateKind::Performance => {
            let sub1 = SubtaskBuilder::new()
                .title("Profile and identify bottlenecks")
                .description("Measure current performance, identify hot paths and bottlenecks")
                .complexity(Complexity::Simple)
                .criterion("Benchmark baseline established")
                .criterion("Bottlenecks identified and documented")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Implement optimizations")
                .description("Apply targeted optimizations to identified bottlenecks")
                .complexity(Complexity::Standard)
                .criterion("Optimizations implemented")
                .criterion("No correctness regressions")
                .depends_on(sub1_id)
                .build()?;
            let sub2_id = sub2.id;

            let sub3 = SubtaskBuilder::new()
                .title("Benchmark and validate")
                .description("Run benchmarks to verify improvements, document results")
                .complexity(Complexity::Simple)
                .criterion("Measurable performance improvement confirmed")
                .criterion("Benchmark results documented")
                .depends_on(sub2_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Performance: {description}"))
                .complexity(Complexity::Standard)
                .subtask(sub1)
                .subtask(sub2)
                .subtask(sub3)
                .build()?
        }
        TemplateKind::Security => {
            let sub1 = SubtaskBuilder::new()
                .title("Security audit")
                .description("Audit the target area for vulnerabilities, document findings")
                .complexity(Complexity::Standard)
                .criterion("Vulnerabilities identified and documented")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Implement fixes")
                .description("Fix identified vulnerabilities with minimal surface area changes")
                .complexity(Complexity::Standard)
                .criterion("All identified vulnerabilities addressed")
                .criterion("No new attack surface introduced")
                .depends_on(sub1_id)
                .build()?;
            let sub2_id = sub2.id;

            let sub3 = SubtaskBuilder::new()
                .title("Security tests")
                .description("Add tests verifying the fixes and covering edge cases")
                .complexity(Complexity::Simple)
                .criterion("Tests for each fixed vulnerability")
                .criterion("Regression tests pass")
                .depends_on(sub2_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Security: {description}"))
                .complexity(Complexity::Complex)
                .subtask(sub1)
                .subtask(sub2)
                .subtask(sub3)
                .build()?
        }
        TemplateKind::Docs => {
            let sub1 = SubtaskBuilder::new()
                .title("Research and outline")
                .description("Understand the subject, identify audience, create outline")
                .complexity(Complexity::Simple)
                .criterion("Outline approved")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Write documentation")
                .description("Write the documentation following the approved outline")
                .complexity(Complexity::Standard)
                .criterion("All sections written")
                .criterion("Examples included")
                .depends_on(sub1_id)
                .build()?;
            let sub2_id = sub2.id;

            let sub3 = SubtaskBuilder::new()
                .title("Review and publish")
                .description("Review for accuracy, fix issues, publish or merge")
                .complexity(Complexity::Simple)
                .criterion("Technical accuracy verified")
                .criterion("Documentation published")
                .depends_on(sub2_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Docs: {description}"))
                .complexity(Complexity::Simple)
                .subtask(sub1)
                .subtask(sub2)
                .subtask(sub3)
                .build()?
        }
        TemplateKind::Migration => {
            let sub1 = SubtaskBuilder::new()
                .title("Migration plan")
                .description("Document migration steps, data mapping, and rollback procedure")
                .complexity(Complexity::Standard)
                .criterion("Migration plan documented")
                .criterion("Rollback plan documented")
                .build()?;
            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Implement migration")
                .description("Execute the migration according to the plan")
                .complexity(Complexity::Complex)
                .criterion("Migration executes without errors")
                .criterion("Data integrity verified post-migration")
                .depends_on(sub1_id)
                .build()?;
            let sub2_id = sub2.id;

            let sub3 = SubtaskBuilder::new()
                .title("Validate rollback plan")
                .description("Test the rollback procedure to confirm it works correctly")
                .complexity(Complexity::Simple)
                .criterion("Rollback tested and confirmed working")
                .depends_on(sub2_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Migration: {description}"))
                .complexity(Complexity::Complex)
                .subtask(sub1)
                .subtask(sub2)
                .subtask(sub3)
                .build()?
        }
    };

    Ok(SpecFile { spec, path: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_kind_from_str() {
        assert_eq!(
            TemplateKind::parse("feature").unwrap(),
            TemplateKind::Feature
        );
        assert_eq!(TemplateKind::parse("bugfix").unwrap(), TemplateKind::Bugfix);
        assert_eq!(TemplateKind::parse("fix").unwrap(), TemplateKind::Bugfix);
        assert_eq!(
            TemplateKind::parse("refactor").unwrap(),
            TemplateKind::Refactor
        );
        assert_eq!(
            TemplateKind::parse("performance").unwrap(),
            TemplateKind::Performance
        );
        assert_eq!(
            TemplateKind::parse("perf").unwrap(),
            TemplateKind::Performance
        );
        assert_eq!(
            TemplateKind::parse("security").unwrap(),
            TemplateKind::Security
        );
        assert_eq!(TemplateKind::parse("sec").unwrap(), TemplateKind::Security);
        assert_eq!(TemplateKind::parse("docs").unwrap(), TemplateKind::Docs);
        assert_eq!(TemplateKind::parse("doc").unwrap(), TemplateKind::Docs);
        assert_eq!(
            TemplateKind::parse("migration").unwrap(),
            TemplateKind::Migration
        );
        assert_eq!(
            TemplateKind::parse("migrate").unwrap(),
            TemplateKind::Migration
        );
        assert!(TemplateKind::parse("unknown").is_err());
    }

    #[test]
    fn test_feature_template() {
        let spec_file = generate(TemplateKind::Feature, "Add user auth").unwrap();
        assert_eq!(spec_file.spec.title, "Add user auth");
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        assert_eq!(spec_file.spec.subtasks[1].depends_on.len(), 1);
        assert_eq!(
            spec_file.spec.subtasks[1].depends_on[0],
            spec_file.spec.subtasks[0].id
        );
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
    fn test_performance_template() {
        let spec_file = generate(TemplateKind::Performance, "Speed up query engine").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        // profile → optimize → benchmark (linear chain)
        assert_eq!(
            spec_file.spec.subtasks[1].depends_on[0],
            spec_file.spec.subtasks[0].id
        );
        assert_eq!(
            spec_file.spec.subtasks[2].depends_on[0],
            spec_file.spec.subtasks[1].id
        );
    }

    #[test]
    fn test_security_template() {
        let spec_file = generate(TemplateKind::Security, "Fix auth bypass").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        assert_eq!(spec_file.spec.complexity, Complexity::Complex);
    }

    #[test]
    fn test_docs_template() {
        let spec_file = generate(TemplateKind::Docs, "Write API reference").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 3);
    }

    #[test]
    fn test_migration_template() {
        let spec_file = generate(TemplateKind::Migration, "Migrate to Postgres").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        assert_eq!(spec_file.spec.complexity, Complexity::Complex);
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
