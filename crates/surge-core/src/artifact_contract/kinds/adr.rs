//! ADR artifact validation.

use super::super::diagnostic::ArtifactValidationReport;
use super::super::parse::{parse_toml_frontmatter, require_markdown_sections, require_toml_fields};

pub(in crate::artifact_contract) fn validate_adr_markdown(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    validate_adr_frontmatter(report, content);
    require_markdown_sections(
        report,
        content,
        &[
            "Status",
            "Context",
            "Decision",
            "Alternatives considered",
            "Consequences",
        ],
    );
}

fn validate_adr_frontmatter(report: &mut ArtifactValidationReport, content: &str) {
    let Some(frontmatter) = parse_toml_frontmatter(report, content) else {
        return;
    };
    require_toml_fields(report, &frontmatter, &["status", "deciders", "date"]);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::super::contract::ArtifactKind;
    use super::super::super::validate_artifact;

    #[test]
    fn validates_adr_toml_frontmatter() {
        let report = validate_artifact(
            ArtifactKind::Adr,
            Some(Path::new("docs/adr/0001-record-choice.md")),
            r#"+++
status = "accepted"
deciders = ["core team"]
date = "2026-05-11"
+++

# ADR 0001: Record choice

## Status
Accepted.

## Context
We need a decision.

## Decision
Use the contract module.

## Alternatives considered
- Keep conventions implicit.

## Consequences
- Validators can share diagnostics.
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }
}
