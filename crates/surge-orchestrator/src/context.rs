//! Subtask context — builds prompts for agent execution.

use std::path::Path;

use surge_core::spec::{Spec, Subtask};

/// Context for building prompts for a single subtask execution.
pub struct SubtaskContext<'a> {
    spec: &'a Spec,
    subtask: &'a Subtask,
    spec_dir: Option<&'a Path>,
}

impl<'a> SubtaskContext<'a> {
    /// Create a new subtask context.
    ///
    /// When `spec_dir` is provided and the subtask has a `story_file`, the prompt
    /// is read from that file instead of being assembled from struct fields.
    #[must_use]
    pub fn new(spec: &'a Spec, subtask: &'a Subtask, spec_dir: Option<&'a Path>) -> Self {
        Self { spec, subtask, spec_dir }
    }

    /// Build the prompt string sent to the coding agent for this subtask.
    ///
    /// If the subtask has a `story_file` and `spec_dir` is set, returns the file
    /// content directly. Otherwise falls back to field-assembled prompt.
    #[must_use]
    pub fn build_prompt(&self) -> String {
        if let (Some(story_file), Some(spec_dir)) = (&self.subtask.story_file, self.spec_dir) {
            let story_path = spec_dir.join(story_file);
            if let Ok(content) = std::fs::read_to_string(&story_path) {
                return content;
            }
        }

        self.build_prompt_from_fields()
    }

    /// Build prompt from subtask struct fields (original approach).
    fn build_prompt_from_fields(&self) -> String {
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
///
/// When `spec_dir` is provided, reads `requirements.md` and includes it for
/// richer QA context.
#[must_use]
pub fn build_qa_prompt(spec: &Spec, diff: &str, spec_dir: Option<&Path>) -> String {
    let mut prompt = String::new();

    prompt.push_str(&format!("# QA Review: {}\n\n", spec.title));
    prompt.push_str(&format!("{}\n\n", spec.description));

    // Include requirements if available
    if let Some(dir) = spec_dir
        && let Ok(requirements) = std::fs::read_to_string(dir.join("requirements.md"))
    {
        prompt.push_str("## Requirements\n\n");
        prompt.push_str(&requirements);
        prompt.push_str("\n\n");
    }

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
    prompt.push_str("Respond with a JSON object using one of these formats:\n\n");
    prompt.push_str("### Approved\n\n");
    prompt.push_str("```json\n");
    prompt.push_str("{\n");
    prompt.push_str("  \"verdict\": \"APPROVED\"\n");
    prompt.push_str("}\n");
    prompt.push_str("```\n\n");
    prompt.push_str("### Needs Fix\n\n");
    prompt.push_str("```json\n");
    prompt.push_str("{\n");
    prompt.push_str("  \"verdict\": \"NEEDS_FIX\",\n");
    prompt.push_str("  \"issues\": [\n");
    prompt.push_str("    \"Description of issue 1\",\n");
    prompt.push_str("    \"Description of issue 2\"\n");
    prompt.push_str("  ]\n");
    prompt.push_str("}\n");
    prompt.push_str("```\n");

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
                story_file: None,
                agent: None,
                execution: surge_core::spec::SubtaskExecution::default(),
            }],
        }
    }

    #[test]
    fn test_subtask_prompt_contains_key_parts() {
        let spec = sample_spec();
        let subtask = &spec.subtasks[0];
        let ctx = SubtaskContext::new(&spec, subtask, None);
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
        let prompt = build_qa_prompt(&spec, diff, None);

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

    #[test]
    fn test_story_file_prompt_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let stories_dir = dir.path().join("stories");
        std::fs::create_dir_all(&stories_dir).unwrap();
        std::fs::write(stories_dir.join("story-001.md"), "# Story 001: Setup\n\nDo the thing.")
            .unwrap();

        let mut spec = sample_spec();
        spec.subtasks[0].story_file = Some("stories/story-001.md".to_string());

        let ctx = SubtaskContext::new(&spec, &spec.subtasks[0], Some(dir.path()));
        let prompt = ctx.build_prompt();

        assert!(prompt.contains("# Story 001: Setup"), "should read story file");
        assert!(prompt.contains("Do the thing."));
        assert!(!prompt.contains("## Instructions"), "should NOT have field-based sections");
    }

    #[test]
    fn test_story_file_fallback_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = sample_spec();
        spec.subtasks[0].story_file = Some("stories/nonexistent.md".to_string());

        let ctx = SubtaskContext::new(&spec, &spec.subtasks[0], Some(dir.path()));
        let prompt = ctx.build_prompt();

        // Falls back to field-based prompt
        assert!(prompt.contains("## Instructions"), "should fall back to field-based prompt");
    }

    #[test]
    fn test_no_spec_dir_uses_field_prompt() {
        let mut spec = sample_spec();
        spec.subtasks[0].story_file = Some("stories/story-001.md".to_string());

        let ctx = SubtaskContext::new(&spec, &spec.subtasks[0], None);
        let prompt = ctx.build_prompt();

        assert!(prompt.contains("## Instructions"), "should use field-based prompt without spec_dir");
    }

    #[test]
    fn test_qa_prompt_includes_requirements_when_available() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("requirements.md"), "## Overview\nDo great things.")
            .unwrap();

        let spec = sample_spec();
        let diff = "+new code";
        let prompt = build_qa_prompt(&spec, diff, Some(dir.path()));

        assert!(prompt.contains("## Requirements"), "should include requirements section");
        assert!(prompt.contains("Do great things."), "should include requirements content");
    }

    #[test]
    fn test_build_qa_prompt() {
        let spec = sample_spec();
        let diff = "+use tracing::info;\n-use println;";
        let prompt = build_qa_prompt(&spec, diff, None);

        // Verify JSON format instructions are included
        assert!(prompt.contains("## Response Format"), "should contain Response Format section");
        assert!(prompt.contains("JSON object"), "should mention JSON object");
        assert!(prompt.contains("\"verdict\": \"APPROVED\""), "should include APPROVED JSON example");
        assert!(prompt.contains("\"verdict\": \"NEEDS_FIX\""), "should include NEEDS_FIX JSON example");
        assert!(prompt.contains("\"issues\""), "should include issues field in JSON example");
        assert!(prompt.contains("```json"), "should include JSON code blocks");
    }
}
