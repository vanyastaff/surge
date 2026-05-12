//! Flow (graph) artifact validation.

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::{parse_toml_value, require_toml_fields, validate_schema_version};

pub(in crate::artifact_contract) fn validate_flow_toml(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    let Some(value) = parse_toml_value(report, content) else {
        return;
    };
    validate_schema_version(report, &value, crate::graph::SCHEMA_VERSION);
    require_toml_fields(report, &value, &["metadata", "start", "nodes", "edges"]);
}

#[cfg(test)]
mod tests {
    use super::super::super::contract::ArtifactKind;
    use super::super::super::diagnostic::ArtifactDiagnosticCode;
    use super::super::super::validate_artifact_text;

    #[test]
    fn validates_flow_schema_version_without_graph_validation() {
        let report = validate_artifact_text(
            ArtifactKind::Flow,
            r#"schema_version = 2
start = "missing"
edges = []

[metadata]
name = "Invalid"

[nodes]
"#,
        );

        assert_eq!(report.error_count(), 1);
        assert_eq!(
            report.diagnostics[0].code,
            ArtifactDiagnosticCode::UnsupportedSchemaVersion
        );
    }
}
