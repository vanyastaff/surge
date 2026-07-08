//! Roadmap artifact validation.

use std::path::Path;

use serde::Deserialize;

use super::super::contract::ROADMAP_SCHEMA_VERSION;
use super::super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};
use super::super::parse::{
    is_markdown_artifact, parse_toml_value, require_markdown_sections, require_toml_fields,
};
use crate::roadmap::{RoadmapArtifact, RoadmapLedgerIssue};

pub(in crate::artifact_contract) fn validate_roadmap(
    report: &mut ArtifactValidationReport,
    path: Option<&Path>,
    content: &str,
) {
    if is_markdown_artifact(path, content) {
        require_markdown_sections(report, content, &["Milestones", "Dependencies", "Risks"]);
        return;
    }

    let Some(value) = parse_toml_value(report, content) else {
        return;
    };
    validate_roadmap_schema_version(report, &value);
    require_toml_fields(report, &value, &["milestones"]);
    if !report.is_valid() {
        return;
    }

    let artifact = match RoadmapArtifact::deserialize(value) {
        Ok(artifact) => artifact,
        Err(error) => {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::InvalidToml,
                None,
                format!("roadmap failed to deserialize: {error}"),
            ));
            return;
        },
    };

    for issue in artifact.validate_ledger() {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ledger_issue_code(&issue),
            ledger_issue_location(&issue),
            issue.to_string(),
        ));
    }
}

/// Accept every supported roadmap schema version.
///
/// v1 artifacts stay valid (the ledger fields default); v2 artifacts opt in
/// to the stricter ledger rules via `RoadmapArtifact::validate_ledger`.
fn validate_roadmap_schema_version(report: &mut ArtifactValidationReport, value: &toml::Value) {
    let Some(schema_version) = value.get("schema_version") else {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingSchemaVersion,
            Some("schema_version".to_string()),
            "artifact is missing schema_version",
        ));
        return;
    };

    let supported = 1..=i64::from(ROADMAP_SCHEMA_VERSION);
    if schema_version
        .as_integer()
        .is_some_and(|version| supported.contains(&version))
    {
        return;
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::UnsupportedSchemaVersion,
        Some("schema_version".to_string()),
        format!("expected schema_version between 1 and {ROADMAP_SCHEMA_VERSION}"),
    ));
}

const fn ledger_issue_code(issue: &RoadmapLedgerIssue) -> ArtifactDiagnosticCode {
    match issue {
        RoadmapLedgerIssue::DuplicateMilestoneId { .. }
        | RoadmapLedgerIssue::DuplicateTaskId { .. } => ArtifactDiagnosticCode::DuplicateIdentifier,
        RoadmapLedgerIssue::SelfDependency { .. }
        | RoadmapLedgerIssue::UnknownDependsOn { .. }
        | RoadmapLedgerIssue::SelfDiscovery { .. }
        | RoadmapLedgerIssue::UnknownDiscoveredFrom { .. }
        | RoadmapLedgerIssue::UnknownMilestoneDependency { .. }
        | RoadmapLedgerIssue::MilestoneSelfDependency { .. } => {
            ArtifactDiagnosticCode::InvalidReference
        },
        RoadmapLedgerIssue::DependencyCycle { .. } => ArtifactDiagnosticCode::DependencyCycle,
        RoadmapLedgerIssue::MissingSize { .. } => ArtifactDiagnosticCode::MissingField,
    }
}

fn ledger_issue_location(issue: &RoadmapLedgerIssue) -> Option<String> {
    match issue {
        RoadmapLedgerIssue::DuplicateMilestoneId { milestone } => Some(milestone.clone()),
        RoadmapLedgerIssue::DuplicateTaskId { task }
        | RoadmapLedgerIssue::SelfDependency { task }
        | RoadmapLedgerIssue::UnknownDependsOn { task, .. }
        | RoadmapLedgerIssue::SelfDiscovery { task }
        | RoadmapLedgerIssue::UnknownDiscoveredFrom { task, .. } => Some(task.clone()),
        RoadmapLedgerIssue::MissingSize { task } => Some(format!("{task}.size")),
        RoadmapLedgerIssue::UnknownMilestoneDependency { missing } => Some(missing.clone()),
        RoadmapLedgerIssue::MilestoneSelfDependency { milestone } => Some(milestone.clone()),
        RoadmapLedgerIssue::DependencyCycle { cycle } => cycle.first().cloned(),
    }
}
