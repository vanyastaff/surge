//! Plan artifact validation.

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::require_markdown_sections;

pub(in crate::artifact_contract) fn validate_plan_markdown(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    require_markdown_sections(report, content, &["Settings", "Tasks"]);
}
