//! Subtask context — builds prompts for agent execution.

use surge_core::spec::{Spec, Subtask};

/// Context for building prompts for a single subtask execution.
pub struct SubtaskContext<'a> {
    spec: &'a Spec,
    subtask: &'a Subtask,
}

impl<'a> SubtaskContext<'a> {
    /// Create a new subtask context.
    #[must_use]
    pub fn new(spec: &'a Spec, subtask: &'a Subtask) -> Self {
        Self { spec, subtask }
    }

    /// Build the prompt string sent to the coding agent for this subtask.
    #[must_use]
    pub fn build_prompt(&self) -> String {
        let mut prompt = String::new();

        // Spec-level context
        prompt.push_str(&format!("# Spec: {}\n\n", self.spec.title));
        prompt.push_str(&format!("{}\n\n", self.spec.description));

        // Subtask details
        prompt.push_str(&format!("## Subtask: {}\n\n", self.subtask.title));
        prompt.push_str(&format!("{}\n\n", self.subtask.description));

        // Files to work on
        if !self.subtask.files.is_empty() {
            prompt.push_str("## Files\n\n");
            for file in &self.subtask.files {
                prompt.push_str(&format!("- {file}\n"));
            }
            prompt.push('\n');
        }

        // Acceptance criteria
        if !self.subtask.acceptance_criteria.is_empty() {
            prompt.push_str("## Acceptance Criteria\n\n");
            for criterion in &self.subtask.acceptance_criteria {
                prompt.push_str(&format!("- {}\n", criterion.description));
            }
            prompt.push('\n');
        }

        // Instructions
        prompt.push_str("## Instructions\n\n");
        prompt.push_str("Implement the subtask described above. ");
        prompt.push_str("Make sure all acceptance criteria are met. ");
        prompt.push_str("Only modify the listed files unless absolutely necessary.\n");

        prompt
    }
}

/// Build a QA review prompt from all subtask acceptance criteria and the diff.
#[must_use]
pub fn build_qa_prompt(spec: &Spec, diff: &str) -> String {
    let mut prompt = String::new();

    prompt.push_str(&format!("# QA Review: {}\n\n", spec.title));
    prompt.push_str(&format!("{}\n\n", spec.description));

    // Collect acceptance criteria from all subtasks
    prompt.push_str("## Acceptance Criteria\n\n");
    for subtask in &spec.subtasks {
        prompt.push_str(&format!("### {}\n\n", subtask.title));
        for criterion in &subtask.acceptance_criteria {
            prompt.push_str(&format!("- {}\n", criterion.description));
        }
        prompt.push('\n');
    }

    // The diff to review
    prompt.push_str("## Diff\n\n");
    prompt.push_str("```diff\n");
    prompt.push_str(diff);
    prompt.push_str("\n```\n\n");

    // Response format
    prompt.push_str("## Response Format\n\n");
    prompt.push_str("Respond with exactly one of:\n");
    prompt.push_str("- APPROVED — if all acceptance criteria are met\n");
    prompt.push_str("- NEEDS_FIX: <description of issues> — if changes are needed\n");

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::{SpecId, SubtaskId};
    use surge_core::spec::{AcceptanceCriteria, Complexity};

    fn sample_spec() -> Spec {
        Spec {
            id: SpecId::new(),
            title: "Add logging".to_string(),
            description: "Add structured logging to the service".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![Subtask {
                id: SubtaskId::new(),
                title: "Add tracing crate".to_string(),
                description: "Wire up the tracing crate with JSON output".to_string(),
                complexity: Complexity::Simple,
                files: vec!["src/main.rs".to_string(), "Cargo.toml".to_string()],
                acceptance_criteria: vec![
                    AcceptanceCriteria {
                        description: "tracing subscriber is initialized".to_string(),
                        met: false,
                    },
                    AcceptanceCriteria {
                        description: "logs are output in JSON format".to_string(),
                        met: false,
                    },
                ],
                depends_on: vec![],
                agent: None,
                execution: surge_core::spec::SubtaskExecution::default(),
            }],
        }
    }

    #[test]
    fn test_subtask_prompt_contains_key_parts() {
        let spec = sample_spec();
        let subtask = &spec.subtasks[0];
        let ctx = SubtaskContext::new(&spec, subtask);
        let prompt = ctx.build_prompt();

        assert!(prompt.contains("Add logging"), "should contain spec title");
        assert!(
            prompt.contains("Add structured logging"),
            "should contain spec description"
        );
        assert!(
            prompt.contains("Add tracing crate"),
            "should contain subtask title"
        );
        assert!(
            prompt.contains("Wire up the tracing crate"),
            "should contain subtask description"
        );
        assert!(prompt.contains("src/main.rs"), "should contain files");
        assert!(prompt.contains("Cargo.toml"), "should contain files");
        assert!(
            prompt.contains("tracing subscriber is initialized"),
            "should contain acceptance criteria"
        );
        assert!(
            prompt.contains("Instructions"),
            "should contain instructions section"
        );
    }

    #[test]
    fn test_qa_prompt_contains_diff() {
        let spec = sample_spec();
        let diff = "+use tracing::info;\n-use println;";
        let prompt = build_qa_prompt(&spec, diff);

        assert!(prompt.contains("QA Review"), "should contain QA header");
        assert!(
            prompt.contains("+use tracing::info;"),
            "should contain the diff"
        );
        assert!(prompt.contains("APPROVED"), "should mention APPROVED format");
        assert!(
            prompt.contains("NEEDS_FIX"),
            "should mention NEEDS_FIX format"
        );
        assert!(
            prompt.contains("tracing subscriber is initialized"),
            "should contain criteria"
        );
    }
}
