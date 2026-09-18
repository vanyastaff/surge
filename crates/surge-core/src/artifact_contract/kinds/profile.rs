//! Profile artifact validation.
//!
//! Contract-level checks only: the file is TOML, declares the profile
//! schema version, and has the fields every [`crate::profile::Profile`]
//! needs. Strict typed loading, the `authority = true` rejection and the
//! bundled/home shadowing rule for composed profiles belong to the profile
//! registry install path, not here.

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::{parse_toml_value, require_toml_fields, validate_schema_version};

pub(in crate::artifact_contract) fn validate_profile_toml(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    let Some(value) = parse_toml_value(report, content) else {
        return;
    };
    validate_schema_version(report, &value, crate::profile::SCHEMA_VERSION);
    require_toml_fields(report, &value, &["role", "runtime", "outcomes", "prompt"]);
}

#[cfg(test)]
mod tests {
    use super::super::super::contract::ArtifactKind;
    use super::super::super::diagnostic::ArtifactDiagnosticCode;
    use super::super::super::validate_artifact_text;

    #[test]
    fn accepts_a_minimal_profile() {
        let report = validate_artifact_text(
            ArtifactKind::Profile,
            r#"schema_version = 1
outcomes = []

[role]
name = "reviewer"

[runtime]
agent_id = "claude-acp"

[prompt]
system = "review"
"#,
        );
        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn rejects_wrong_schema_version_and_missing_fields() {
        let report = validate_artifact_text(
            ArtifactKind::Profile,
            r#"schema_version = 2
[role]
name = "reviewer"
"#,
        );
        let codes: Vec<_> = report.diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            vec![
                ArtifactDiagnosticCode::UnsupportedSchemaVersion,
                ArtifactDiagnosticCode::MissingField,
                ArtifactDiagnosticCode::MissingField,
                ArtifactDiagnosticCode::MissingField,
            ]
        );
    }
}
