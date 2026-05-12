//! Spec artifact validation (markdown and TOML variants).

use std::path::Path;

use super::super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};
use super::super::parse::{
    is_markdown_artifact, require_acceptance_criteria, require_markdown_sections,
    validate_acceptance_criterion_text, validate_toml_artifact,
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
    let spec = value.get("spec").unwrap_or(value);
    let Some(subtasks) = spec.get("subtasks").and_then(toml::Value::as_array) else {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingAcceptanceCriteria,
            Some("spec.subtasks.acceptance_criteria".to_string()),
            "every spec subtask must include machine-readable acceptance criteria",
        ));
        return;
    };

    if subtasks.is_empty() {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingAcceptanceCriteria,
            Some("spec.subtasks.acceptance_criteria".to_string()),
            "every spec subtask must include machine-readable acceptance criteria",
        ));
        return;
    }

    for (subtask_index, subtask) in subtasks.iter().enumerate() {
        let criteria = subtask
            .get("acceptance_criteria")
            .and_then(toml::Value::as_array);
        let Some(criteria) = criteria else {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::MissingAcceptanceCriteria,
                Some(format!(
                    "spec.subtasks[{subtask_index}].acceptance_criteria"
                )),
                "every spec subtask must include machine-readable acceptance criteria",
            ));
            continue;
        };

        if criteria.is_empty() {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::MissingAcceptanceCriteria,
                Some(format!(
                    "spec.subtasks[{subtask_index}].acceptance_criteria"
                )),
                "every spec subtask must include machine-readable acceptance criteria",
            ));
            continue;
        }

        for (criterion_index, criterion) in criteria.iter().enumerate() {
            let location =
                format!("spec.subtasks[{subtask_index}].acceptance_criteria[{criterion_index}]");
            let Some(text) = extract_toml_criterion_text(criterion) else {
                report.push(ArtifactValidationDiagnostic::error(
                    report.kind,
                    ArtifactDiagnosticCode::EmptyAcceptanceCriteria,
                    Some(location),
                    "acceptance criterion must be a string or a table with a `description` field",
                ));
                continue;
            };
            validate_acceptance_criterion_text(report, location, text);
        }
    }
}

fn extract_toml_criterion_text(value: &toml::Value) -> Option<&str> {
    match value {
        toml::Value::String(text) => Some(text.as_str()),
        toml::Value::Table(table) => table.get("description").and_then(toml::Value::as_str),
        _ => None,
    }
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
                && diagnostic.location.as_deref() == Some("spec.subtasks[1].acceptance_criteria")
        }));
    }

    #[test]
    fn rejects_spec_markdown_placeholder_criterion() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria
- TBD
"#,
        );

        assert!(!report.is_valid(), "{report:#?}");
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
                && diagnostic.location.as_deref() == Some("Acceptance Criteria[0]")
        }));
    }

    #[test]
    fn rejects_spec_markdown_empty_checkbox_criterion() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria
- [ ]
"#,
        );

        assert!(!report.is_valid());
        assert!(
            report.diagnostics.iter().any(
                |diagnostic| diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
            )
        );
    }

    #[test]
    fn rejects_spec_markdown_single_question_mark_criterion() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria
- ?
"#,
        );

        assert!(!report.is_valid());
        assert!(
            report.diagnostics.iter().any(
                |diagnostic| diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
            )
        );
    }

    #[test]
    fn rejects_spec_markdown_too_short_criterion() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria
- works
"#,
        );

        assert!(!report.is_valid());
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(
                    |diagnostic| diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
                )
                .count(),
            1,
        );
    }

    #[test]
    fn rejects_spec_markdown_when_acceptance_section_is_empty() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria

## Constraints
- Stay pure.
"#,
        );

        assert!(!report.is_valid());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::MissingAcceptanceCriteria
        }));
    }

    #[test]
    fn rejects_spec_toml_when_criterion_is_placeholder_string() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["TBD"] },
]
"#,
        );

        assert!(!report.is_valid());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
                && diagnostic.location.as_deref() == Some("spec.subtasks[0].acceptance_criteria[0]")
        }));
    }

    #[test]
    fn rejects_spec_toml_when_criterion_table_description_is_too_short() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = [{ description = "ok" }] },
]
"#,
        );

        assert!(!report.is_valid());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
        }));
    }

    #[test]
    fn accepts_spec_toml_with_table_form_criterion() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = [{ description = "valid fixtures pass validation" }] },
]
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn rejects_spec_toml_criterion_of_wrong_shape_with_shape_message() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = [42] },
]
"#,
        );

        assert!(!report.is_valid());
        let shape_diag = report
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria
                    && diagnostic.location.as_deref()
                        == Some("spec.subtasks[0].acceptance_criteria[0]")
            })
            .expect("expected a shape diagnostic for the integer criterion");
        assert!(
            shape_diag.message.contains("string") && shape_diag.message.contains("description"),
            "diagnostic message should explain the expected criterion shape, got: {:?}",
            shape_diag.message,
        );
    }

    #[test]
    fn placeholder_match_is_case_insensitive_for_ascii() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["Tbd", "ToDo"] },
]
"#,
        );

        assert!(!report.is_valid());
        let empty_count = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::EmptyAcceptanceCriteria)
            .count();
        assert_eq!(empty_count, 2);
    }
}
