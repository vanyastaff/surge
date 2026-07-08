//! `discovered-tasks` artifact validation.

use serde::Deserialize;

use super::super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};
use super::super::parse::{parse_toml_value, require_toml_fields, validate_schema_version};
use crate::artifact_contract::ARTIFACT_SCHEMA_VERSION;
use crate::roadmap::DiscoveredTasksArtifact;

pub(in crate::artifact_contract) fn validate_discovered_tasks(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    let Some(value) = parse_toml_value(report, content) else {
        return;
    };
    validate_schema_version(report, &value, ARTIFACT_SCHEMA_VERSION);
    require_toml_fields(report, &value, &["tasks"]);
    if !report.is_valid() {
        return;
    }

    let artifact = match DiscoveredTasksArtifact::deserialize(value) {
        Ok(artifact) => artifact,
        Err(error) => {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::InvalidToml,
                None,
                format!("discovered-tasks failed to deserialize: {error}"),
            ));
            return;
        },
    };

    for issue in artifact.validate() {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::DuplicateIdentifier,
            None,
            issue,
        ));
    }
}
