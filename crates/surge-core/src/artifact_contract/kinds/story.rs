//! Story artifact validation.

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::{require_acceptance_criteria, require_markdown_sections};

pub(in crate::artifact_contract) fn validate_story_markdown(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    require_markdown_sections(
        report,
        content,
        &[
            "Context",
            "What needs to be done",
            "Architecture decisions",
            "Files to modify",
            "Acceptance criteria",
            "Out of scope",
        ],
    );
    require_acceptance_criteria(report, content);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::super::contract::ArtifactKind;
    use super::super::super::diagnostic::ArtifactDiagnosticCode;
    use super::super::super::validate_artifact;

    #[test]
    fn rejects_story_markdown_placeholder_criterion() {
        let report = validate_artifact(
            ArtifactKind::Story,
            Some(Path::new("stories/story-002.md")),
            r#"# Story 002

## Context
Demonstrate story-level placeholder rejection.

## What needs to be done
Add validation.

## Architecture decisions
None.

## Files to modify
- crates/surge-core/src/artifact_contract.rs

## Acceptance criteria
- N/A

## Out of scope
- Other artifacts.
"#,
        );

        assert!(!report.is_valid());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
        }));
    }
}
