//! Diagnostic and validation report types for artifact contracts.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::contract::ArtifactKind;

/// Severity for artifact validation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactDiagnosticSeverity {
    /// Validation cannot accept the artifact.
    Error,
    /// Validation accepts the artifact but found a compatibility issue.
    Warning,
}

/// Stable diagnostic code emitted by artifact validators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactDiagnosticCode {
    /// Artifact path/name does not match the selected contract.
    InvalidArtifactPath,
    /// Required markdown section is missing.
    MissingSection,
    /// Required machine-readable field is missing.
    MissingField,
    /// `schema_version` is missing.
    MissingSchemaVersion,
    /// `schema_version` is present but unsupported.
    UnsupportedSchemaVersion,
    /// TOML content failed to parse.
    InvalidToml,
    /// Markdown frontmatter is missing or malformed.
    InvalidFrontmatter,
    /// Acceptance criteria are missing.
    MissingAcceptanceCriteria,
    /// Artifact kind is not supported by this validator.
    UnsupportedArtifactKind,
    /// Flow graph parsed as TOML but failed engine-level graph validation.
    GraphValidationFailed,
    /// Flow graph failed to deserialize into the engine graph model.
    GraphParseFailed,
    /// Roadmap patch has no operations.
    MissingOperation,
    /// Roadmap patch add operation is missing an insertion point.
    MissingInsertionPoint,
    /// Roadmap patch references a missing roadmap item.
    InvalidReference,
}

impl ArtifactDiagnosticCode {
    /// Stable snake-case code for CLI JSON and tests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArtifactPath => "invalid_artifact_path",
            Self::MissingSection => "missing_section",
            Self::MissingField => "missing_field",
            Self::MissingSchemaVersion => "missing_schema_version",
            Self::UnsupportedSchemaVersion => "unsupported_schema_version",
            Self::InvalidToml => "invalid_toml",
            Self::InvalidFrontmatter => "invalid_frontmatter",
            Self::MissingAcceptanceCriteria => "missing_acceptance_criteria",
            Self::UnsupportedArtifactKind => "unsupported_artifact_kind",
            Self::GraphValidationFailed => "graph_validation_failed",
            Self::GraphParseFailed => "graph_parse_failed",
            Self::MissingOperation => "missing_operation",
            Self::MissingInsertionPoint => "missing_insertion_point",
            Self::InvalidReference => "invalid_reference",
        }
    }
}

impl fmt::Display for ArtifactDiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One validation diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactValidationDiagnostic {
    /// Stable machine-readable code.
    pub code: ArtifactDiagnosticCode,
    /// Severity of the finding.
    pub severity: ArtifactDiagnosticSeverity,
    /// Artifact family being validated.
    pub kind: ArtifactKind,
    /// Optional path or logical field location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Short user-safe message. Do not include full artifact contents.
    pub message: String,
}

impl ArtifactValidationDiagnostic {
    /// Construct an error diagnostic.
    #[must_use]
    pub fn error(
        kind: ArtifactKind,
        code: ArtifactDiagnosticCode,
        location: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: ArtifactDiagnosticSeverity::Error,
            kind,
            location,
            message: message.into(),
        }
    }

    /// Construct a warning diagnostic.
    #[must_use]
    pub fn warning(
        kind: ArtifactKind,
        code: ArtifactDiagnosticCode,
        location: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: ArtifactDiagnosticSeverity::Warning,
            kind,
            location,
            message: message.into(),
        }
    }
}

/// Validation report for one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactValidationReport {
    /// Artifact kind selected by the caller.
    pub kind: ArtifactKind,
    /// Diagnostics emitted by the validator.
    #[serde(default)]
    pub diagnostics: Vec<ArtifactValidationDiagnostic>,
}

impl ArtifactValidationReport {
    /// Start an empty report for `kind`.
    #[must_use]
    pub const fn new(kind: ArtifactKind) -> Self {
        Self {
            kind,
            diagnostics: Vec::new(),
        }
    }

    /// Append a diagnostic.
    pub fn push(&mut self, diagnostic: ArtifactValidationDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Return true when no error-severity diagnostics are present.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.error_count() == 0
    }

    /// Count error-severity diagnostics.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == ArtifactDiagnosticSeverity::Error)
            .count()
    }

    /// Count warning-severity diagnostics.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == ArtifactDiagnosticSeverity::Warning)
            .count()
    }

    /// Convert this report into a result that fails when errors exist.
    ///
    /// # Errors
    /// Returns [`ArtifactValidationError`] with the full report when any
    /// error-severity diagnostic is present.
    pub fn into_result(self) -> Result<(), ArtifactValidationError> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(ArtifactValidationError { report: self })
        }
    }
}

/// Error wrapper for invalid artifacts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{} artifact validation error(s) for {}",
    .report.error_count(),
    .report.kind
)]
pub struct ArtifactValidationError {
    /// Full validation report.
    pub report: ArtifactValidationReport,
}
