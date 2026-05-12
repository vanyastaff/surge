//! Spec artifact validation (markdown and TOML variants).

use std::path::Path;

use super::super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};
use super::super::parse::{
    is_markdown_artifact, require_acceptance_criteria, require_markdown_sections,
    validate_toml_artifact,
};

pub(in crate::artifact_contract) fn validate_spec(
    report: &mut ArtifactValidationReport,
    path: Option<&Path>,
    content: &str,
) {
    if is_markdown_artifact(path, content) {
        require_markdown_sections(
            report,
            content,
            &["Goal", "Subtasks", "Acceptance Criteria"],
        );
        require_acceptance_criteria(report, content);
    } else {
        if let Some(value) = validate_toml_artifact(report, content, &["spec"]) {
            validate_spec_toml_acceptance(report, &value);
        }
    }
}

fn validate_spec_toml_acceptance(report: &mut ArtifactValidationReport, value: &toml::Value) {
    if spec_toml_has_acceptance_criteria(value) {
        return;
    }
    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::MissingAcceptanceCriteria,
        Some("spec.subtasks.acceptance_criteria".to_string()),
        "every spec subtask must include machine-readable acceptance criteria",
    ));
}

fn spec_toml_has_acceptance_criteria(value: &toml::Value) -> bool {
    let spec = value.get("spec").unwrap_or(value);
    let Some(subtasks) = spec.get("subtasks").and_then(toml::Value::as_array) else {
        return false;
    };

    !subtasks.is_empty()
        && subtasks
            .iter()
            .all(|subtask| toml_array_is_non_empty(subtask, "acceptance_criteria"))
}

fn toml_array_is_non_empty(value: &toml::Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(toml::Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::super::contract::ArtifactKind;
    use super::super::super::diagnostic::ArtifactDiagnosticCode;
    use super::super::super::validate_artifact;

    #[test]
    fn validates_spec_markdown_acceptance_criteria() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria
- [ ] Invalid schema versions produce diagnostics.
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn validates_spec_toml_when_every_subtask_has_acceptance_criteria() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["first check passes"] },
  { id = "two", acceptance_criteria = ["second check passes"] },
]
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn rejects_spec_toml_when_any_subtask_lacks_acceptance_criteria() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1
acceptance_criteria = ["global criteria must not mask per-subtask gaps"]

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["first check passes"] },
  { id = "two" },
]
"#,
        );

        assert!(!report.is_valid());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::MissingAcceptanceCriteria
                && diagnostic.location.as_deref() == Some("spec.subtasks.acceptance_criteria")
        }));
    }
}
