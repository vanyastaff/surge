//! Description artifact validation.

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::require_markdown_sections;

pub(in crate::artifact_contract) fn validate_description_markdown(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    require_markdown_sections(
        report,
        content,
        &["Goal", "Context", "Requirements", "Out of Scope"],
    );
}

#[cfg(test)]
mod tests {
    use super::super::super::contract::ArtifactKind;
    use super::super::super::diagnostic::ArtifactDiagnosticCode;
    use super::super::super::validate_artifact_text;

    #[test]
    fn validates_description_markdown_sections() {
        let report = validate_artifact_text(
            ArtifactKind::Description,
            r#"# Description

## Goal
Ship the feature.

## Context
Existing project context.

## Requirements
- Must be testable.

## Out of Scope
- Deployment.
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn reports_missing_markdown_sections() {
        let report = validate_artifact_text(ArtifactKind::Description, "## Goal\nOnly goal.");

        assert!(!report.is_valid());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::MissingSection)
        );
    }
}
