//! Golden checks for Surge artifact contracts.
//!
//! These fixtures are synthetic and safe to print by name on failure. The tests
//! intentionally assert stable diagnostic codes rather than prose so agent
//! wording can vary without breaking contract equivalence.

use std::path::PathBuf;

use surge_core::{
    ArtifactDiagnosticCode, ArtifactKind, ArtifactValidationReport, validate_artifact,
};

fn artifacts_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/artifacts")
}

fn load_fixture(relative: &str) -> (PathBuf, String) {
    let path = artifacts_root().join(relative);
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {relative}: {err}"));
    (path, content)
}

fn validate_fixture(kind: ArtifactKind, relative: &str) -> ArtifactValidationReport {
    let (path, content) = load_fixture(relative);
    let artifact_path = path
        .strip_prefix(artifacts_root().join("valid"))
        .or_else(|_| path.strip_prefix(artifacts_root().join("invalid")))
        .unwrap_or(path.as_path());
    validate_artifact(kind, Some(artifact_path), &content)
}

fn diagnostic_codes(report: &ArtifactValidationReport) -> Vec<ArtifactDiagnosticCode> {
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn valid_fixtures_pass_all_artifact_contracts() {
    let cases = [
        (ArtifactKind::Description, "valid/description.md"),
        (ArtifactKind::Requirements, "valid/requirements.md"),
        (ArtifactKind::Roadmap, "valid/roadmap.toml"),
        (ArtifactKind::Roadmap, "valid/roadmap.md"),
        (ArtifactKind::Spec, "valid/spec.toml"),
        (ArtifactKind::Spec, "valid/spec.md"),
        (
            ArtifactKind::Adr,
            "valid/docs/adr/0001-validate-artifacts.md",
        ),
        (ArtifactKind::Story, "valid/stories/story-001.md"),
        (ArtifactKind::Plan, "valid/plan.md"),
        (ArtifactKind::Flow, "valid/flow.toml"),
    ];

    for (kind, relative) in cases {
        let report = validate_fixture(kind, relative);
        assert!(
            report.is_valid(),
            "{relative} should be valid, got {report:#?}"
        );
    }
}

#[test]
fn invalid_fixtures_emit_stable_diagnostic_codes() {
    let cases: &[(ArtifactKind, &str, &[ArtifactDiagnosticCode])] = &[
        (
            ArtifactKind::Description,
            "invalid/description.md",
            &[
                ArtifactDiagnosticCode::MissingSection,
                ArtifactDiagnosticCode::MissingSection,
                ArtifactDiagnosticCode::MissingSection,
            ],
        ),
        (
            ArtifactKind::Requirements,
            "invalid/requirements.md",
            &[
                ArtifactDiagnosticCode::MissingSection,
                ArtifactDiagnosticCode::MissingSection,
                ArtifactDiagnosticCode::MissingSection,
                ArtifactDiagnosticCode::MissingSection,
            ],
        ),
        (
            ArtifactKind::Roadmap,
            "invalid/roadmap.toml",
            &[ArtifactDiagnosticCode::UnsupportedSchemaVersion],
        ),
        (
            ArtifactKind::Spec,
            "invalid/spec.toml",
            &[ArtifactDiagnosticCode::MissingAcceptanceCriteria],
        ),
        (
            ArtifactKind::Adr,
            "invalid/docs/adr/0001-missing-frontmatter.md",
            &[ArtifactDiagnosticCode::InvalidFrontmatter],
        ),
        (
            ArtifactKind::Story,
            "invalid/stories/story-1.md",
            &[ArtifactDiagnosticCode::InvalidArtifactPath],
        ),
        (
            ArtifactKind::Plan,
            "invalid/plan.md",
            &[ArtifactDiagnosticCode::MissingSection],
        ),
        (
            ArtifactKind::Flow,
            "invalid/flow.toml",
            &[ArtifactDiagnosticCode::UnsupportedSchemaVersion],
        ),
    ];

    for (kind, relative, expected_codes) in cases {
        let report = validate_fixture(*kind, relative);
        assert!(!report.is_valid(), "{relative} should be invalid");
        assert_eq!(
            diagnostic_codes(&report),
            *expected_codes,
            "{relative} diagnostic codes drifted"
        );
    }
}

#[test]
fn toml_and_markdown_compatibility_reports_are_contract_equivalent() {
    let roadmap_toml = validate_fixture(ArtifactKind::Roadmap, "valid/roadmap.toml");
    let roadmap_md = validate_fixture(ArtifactKind::Roadmap, "valid/roadmap.md");
    let spec_toml = validate_fixture(ArtifactKind::Spec, "valid/spec.toml");
    let spec_md = validate_fixture(ArtifactKind::Spec, "valid/spec.md");

    assert_contract_equivalent("roadmap", &roadmap_toml, &roadmap_md);
    assert_contract_equivalent("spec", &spec_toml, &spec_md);
}

fn assert_contract_equivalent(
    label: &str,
    left: &ArtifactValidationReport,
    right: &ArtifactValidationReport,
) {
    assert_eq!(left.kind, right.kind, "{label} kind differs");
    assert_eq!(
        left.is_valid(),
        right.is_valid(),
        "{label} validity differs"
    );
    assert_eq!(
        diagnostic_codes(left),
        diagnostic_codes(right),
        "{label} diagnostic codes differ"
    );
}
