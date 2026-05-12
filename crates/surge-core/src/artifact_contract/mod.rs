//! Artifact contract metadata.
//!
//! This module is intentionally pure: it defines canonical artifact names,
//! schema ownership, and validator identifiers without reading files or
//! invoking runtime services. CLI and orchestrator code compose these contracts
//! with I/O, logging, and engine-specific validation.

mod contract;
mod diagnostic;
mod kinds;
mod parse;
mod path;
mod schema;

use std::path::Path;

use crate::roadmap::RoadmapArtifact;

pub use contract::{
    ARTIFACT_SCHEMA_VERSION, ArtifactContract, ArtifactContractRef, ArtifactFormat, ArtifactKind,
    ParseArtifactKindError, SchemaVersionOwner, all_contracts, contract_for,
};
pub use diagnostic::{
    ArtifactDiagnosticCode, ArtifactDiagnosticSeverity, ArtifactValidationDiagnostic,
    ArtifactValidationError, ArtifactValidationReport,
};
pub use schema::{ContractSummary, contract_summary, json_schema_for, markdown_outline};

use path::normalize_path;

/// Validate an artifact path and contents against the selected contract.
#[must_use]
pub fn validate_artifact(
    kind: ArtifactKind,
    path: Option<&Path>,
    content: &str,
) -> ArtifactValidationReport {
    let mut report = ArtifactValidationReport::new(kind);
    if let Some(path) = path {
        validate_artifact_path_into(&mut report, path);
    }
    validate_artifact_text_into(&mut report, path, content);
    report
}

/// Validate an artifact path against the selected contract.
#[must_use]
pub fn validate_artifact_path(kind: ArtifactKind, path: &Path) -> ArtifactValidationReport {
    let mut report = ArtifactValidationReport::new(kind);
    validate_artifact_path_into(&mut report, path);
    report
}

/// Validate artifact contents against the selected contract's primary format.
#[must_use]
pub fn validate_artifact_text(kind: ArtifactKind, content: &str) -> ArtifactValidationReport {
    let mut report = ArtifactValidationReport::new(kind);
    validate_artifact_text_into(&mut report, None, content);
    report
}

/// Validate a roadmap patch and resolve references against a roadmap context.
#[must_use]
pub fn validate_roadmap_patch_text_with_context(
    content: &str,
    roadmap: &RoadmapArtifact,
) -> ArtifactValidationReport {
    let mut report = validate_artifact_text(ArtifactKind::RoadmapPatch, content);
    let Some(patch) = kinds::roadmap_patch::parse_roadmap_patch_for_context(&mut report, content)
    else {
        return report;
    };
    if report.is_valid() {
        kinds::roadmap_patch::validate_roadmap_patch_context(&mut report, &patch, roadmap);
    }
    report
}

fn validate_artifact_path_into(report: &mut ArtifactValidationReport, path: &Path) {
    let contract = contract_for(report.kind);
    if contract.accepts_path(path) {
        return;
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::InvalidArtifactPath,
        Some(normalize_path(path)),
        format!(
            "expected artifact path matching {} for {}",
            contract.canonical_path, report.kind
        ),
    ));
}

fn validate_artifact_text_into(
    report: &mut ArtifactValidationReport,
    path: Option<&Path>,
    content: &str,
) {
    match report.kind {
        ArtifactKind::Description => {
            kinds::description::validate_description_markdown(report, content)
        },
        ArtifactKind::Requirements => {
            kinds::requirements::validate_requirements_markdown(report, content)
        },
        ArtifactKind::Roadmap => kinds::roadmap::validate_roadmap(report, path, content),
        ArtifactKind::RoadmapPatch => kinds::roadmap_patch::validate_roadmap_patch(report, content),
        ArtifactKind::Spec => kinds::spec::validate_spec(report, path, content),
        ArtifactKind::Adr => kinds::adr::validate_adr_markdown(report, content),
        ArtifactKind::Story => kinds::story::validate_story_markdown(report, content),
        ArtifactKind::Plan => kinds::plan::validate_plan_markdown(report, content),
        ArtifactKind::Flow => kinds::flow::validate_flow_toml(report, content),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn contracts_are_in_stable_order() {
        let kinds: Vec<_> = all_contracts()
            .iter()
            .map(|contract| contract.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                ArtifactKind::Description,
                ArtifactKind::Requirements,
                ArtifactKind::Roadmap,
                ArtifactKind::RoadmapPatch,
                ArtifactKind::Spec,
                ArtifactKind::Adr,
                ArtifactKind::Story,
                ArtifactKind::Plan,
                ArtifactKind::Flow,
            ]
        );
    }

    #[test]
    fn artifact_kind_parses_aliases() {
        assert_eq!(
            "flow-toml".parse::<ArtifactKind>().unwrap(),
            ArtifactKind::Flow
        );
        assert_eq!(
            "spec-md".parse::<ArtifactKind>().unwrap(),
            ArtifactKind::Spec
        );
        assert_eq!(
            "roadmap-patch-toml".parse::<ArtifactKind>().unwrap(),
            ArtifactKind::RoadmapPatch
        );
        assert!("unknown".parse::<ArtifactKind>().is_err());
    }

    #[test]
    fn artifact_contract_refs_follow_schema_version_owner() {
        assert_eq!(
            ArtifactContractRef::current(ArtifactKind::Flow).schema_version,
            crate::graph::SCHEMA_VERSION
        );
        assert_eq!(
            contract_for(ArtifactKind::Flow).reference().schema_version,
            crate::graph::SCHEMA_VERSION
        );
        assert_eq!(
            ArtifactContractRef::current(ArtifactKind::Spec).schema_version,
            ARTIFACT_SCHEMA_VERSION
        );
        assert_eq!(
            ArtifactContractRef::current(ArtifactKind::RoadmapPatch).schema_version,
            ARTIFACT_SCHEMA_VERSION
        );
    }

    #[test]
    fn validation_report_counts_errors_and_warnings() {
        let mut report = ArtifactValidationReport::new(ArtifactKind::Description);
        report.push(ArtifactValidationDiagnostic::warning(
            ArtifactKind::Description,
            ArtifactDiagnosticCode::InvalidArtifactPath,
            Some("description.txt".to_string()),
            "non-canonical path",
        ));
        report.push(ArtifactValidationDiagnostic::error(
            ArtifactKind::Description,
            ArtifactDiagnosticCode::MissingSection,
            Some("## Goal".to_string()),
            "missing section",
        ));

        assert!(!report.is_valid());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.warning_count(), 1);
        assert!(report.into_result().is_err());
    }

    #[test]
    fn toml_paths_with_leading_comments_are_not_markdown() {
        let roadmap = validate_artifact(
            ArtifactKind::Roadmap,
            Some(Path::new("roadmap.toml")),
            r#"# Roadmap comment
schema_version = 1

[[milestones]]
id = "m1"
title = "Ship contract validation"
"#,
        );
        assert!(roadmap.is_valid(), "{roadmap:#?}");

        let spec = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"# Spec comment
schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["first check passes"] },
]
"#,
        );
        assert!(spec.is_valid(), "{spec:#?}");
    }
}
