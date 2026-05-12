//! Roadmap artifact validation.

use std::path::Path;

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::{
    is_markdown_artifact, require_markdown_sections, validate_toml_artifact,
};

pub(in crate::artifact_contract) fn validate_roadmap(
    report: &mut ArtifactValidationReport,
    path: Option<&Path>,
    content: &str,
) {
    if is_markdown_artifact(path, content) {
        require_markdown_sections(report, content, &["Milestones", "Dependencies", "Risks"]);
    } else {
        let _ = validate_toml_artifact(report, content, &["milestones"]);
    }
}
