//! Shared parsing and validation helpers used by per-kind validators.

use std::path::Path;

use super::contract::ARTIFACT_SCHEMA_VERSION;
use super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};

pub(super) fn validate_toml_artifact(
    report: &mut ArtifactValidationReport,
    content: &str,
    required_fields: &[&str],
) -> Option<toml::Value> {
    let value = parse_toml_value(report, content)?;
    validate_schema_version(report, &value, ARTIFACT_SCHEMA_VERSION);
    require_toml_fields(report, &value, required_fields);
    Some(value)
}

pub(super) fn parse_toml_value(
    report: &mut ArtifactValidationReport,
    content: &str,
) -> Option<toml::Value> {
    match content.parse::<toml::Value>() {
        Ok(value) => Some(value),
        Err(error) => {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::InvalidToml,
                None,
                format!("artifact TOML failed to parse: {error}"),
            ));
            None
        },
    }
}

pub(super) fn validate_schema_version(
    report: &mut ArtifactValidationReport,
    value: &toml::Value,
    expected: u32,
) {
    let Some(schema_version) = value.get("schema_version") else {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingSchemaVersion,
            Some("schema_version".to_string()),
            "artifact is missing schema_version",
        ));
        return;
    };

    if schema_version.as_integer() == Some(i64::from(expected)) {
        return;
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::UnsupportedSchemaVersion,
        Some("schema_version".to_string()),
        format!("expected schema_version {expected}"),
    ));
}

pub(super) fn require_toml_fields(
    report: &mut ArtifactValidationReport,
    value: &toml::Value,
    required_fields: &[&str],
) {
    for field in required_fields {
        if toml_field(value, field).is_some() {
            continue;
        }
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingField,
            Some((*field).to_string()),
            format!("artifact is missing required field {field}"),
        ));
    }
}

fn toml_field<'a>(value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

pub(super) fn parse_toml_frontmatter(
    report: &mut ArtifactValidationReport,
    content: &str,
) -> Option<toml::Value> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("+++") {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::InvalidFrontmatter,
            Some("frontmatter".to_string()),
            "ADR must start with TOML frontmatter delimited by +++",
        ));
        return None;
    }

    let mut frontmatter = Vec::new();
    for line in lines {
        if line.trim() == "+++" {
            return parse_toml_value(report, &frontmatter.join("\n"));
        }
        frontmatter.push(line);
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::InvalidFrontmatter,
        Some("frontmatter".to_string()),
        "ADR TOML frontmatter is missing the closing +++ delimiter",
    ));
    None
}

pub(super) fn require_markdown_sections(
    report: &mut ArtifactValidationReport,
    content: &str,
    required_sections: &[&str],
) {
    for section in required_sections {
        if has_markdown_heading(content, section) {
            continue;
        }
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingSection,
            Some(format!("## {section}")),
            format!("artifact is missing required section ## {section}"),
        ));
    }
}

pub(super) fn require_acceptance_criteria(report: &mut ArtifactValidationReport, content: &str) {
    if has_markdown_heading(content, "Acceptance Criteria")
        || content.to_ascii_lowercase().contains("acceptance criteria")
    {
        return;
    }
    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::MissingAcceptanceCriteria,
        Some("Acceptance Criteria".to_string()),
        "artifact must include acceptance criteria",
    ));
}

fn has_markdown_heading(content: &str, expected: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            return false;
        }
        let heading = trimmed.trim_start_matches('#').trim();
        strip_trailing_heading_marker(heading).eq_ignore_ascii_case(expected)
    })
}

fn strip_trailing_heading_marker(heading: &str) -> &str {
    heading.trim_end_matches('#').trim_end()
}

pub(super) fn is_markdown_artifact(path: Option<&Path>, content: &str) -> bool {
    if let Some(extension) = path
        .and_then(Path::extension)
        .and_then(|extension| extension.to_str())
    {
        return extension.eq_ignore_ascii_case("md");
    }

    let trimmed = content.trim_start();
    trimmed.starts_with('#') || trimmed.starts_with("+++")
}
