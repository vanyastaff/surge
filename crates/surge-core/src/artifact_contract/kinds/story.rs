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
