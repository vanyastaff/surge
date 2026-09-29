//! The outcome of validating a graph: every finding, with errors and warnings
//! kept apart by their severity rather than by which arm of a `Result` they
//! landed in.

use super::error::{Severity, ValidationError};

/// Every finding a validation pass produced.
///
/// Validation is non-fail-fast, so a graph with errors still reports its
/// warnings and a clean graph may still carry warnings. Ask for what you need:
/// [`errors`](Self::errors) to decide whether the graph may run,
/// [`warnings`](Self::warnings) to surface advice.
#[must_use = "a validation report carries errors that must be inspected"]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    findings: Vec<ValidationError>,
}

impl ValidationReport {
    /// Wrap raw findings.
    pub fn new(findings: Vec<ValidationError>) -> Self {
        Self { findings }
    }

    /// Every finding, errors and warnings, in the order the rules ran.
    #[must_use]
    pub fn findings(&self) -> &[ValidationError] {
        &self.findings
    }

    /// Findings that make the graph unrunnable.
    pub fn errors(&self) -> impl Iterator<Item = &ValidationError> {
        self.findings
            .iter()
            .filter(|f| f.severity() == Severity::Error)
    }

    /// Advisory findings that do not block a run.
    pub fn warnings(&self) -> impl Iterator<Item = &ValidationError> {
        self.findings
            .iter()
            .filter(|f| f.severity() == Severity::Warning)
    }

    /// Whether any finding is an error.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// Whether the graph has no errors (it may still have warnings).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.has_errors()
    }

    /// Consume the report into its findings.
    #[must_use]
    pub fn into_findings(self) -> Vec<ValidationError> {
        self.findings
    }
}

impl IntoIterator for ValidationReport {
    type Item = ValidationError;
    type IntoIter = std::vec::IntoIter<ValidationError>;

    fn into_iter(self) -> Self::IntoIter {
        self.findings.into_iter()
    }
}

/// Test-only conveniences that read like the `Result` helpers the old API had.
#[cfg(test)]
impl ValidationReport {
    /// All findings, asserting the report has at least one error.
    pub(crate) fn expect_errors(self, msg: &str) -> Vec<ValidationError> {
        assert!(self.has_errors(), "{msg}: expected errors, got {self:?}");
        self.findings
    }

    /// All findings (warnings only), asserting the report has no errors.
    pub(crate) fn expect_valid(self, msg: &str) -> Vec<ValidationError> {
        assert!(self.is_valid(), "{msg}: {self:?}");
        self.findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::{ErrorLocation, ValidationErrorKind};

    fn finding(kind: ValidationErrorKind) -> ValidationError {
        ValidationError {
            kind,
            location: ErrorLocation::Graph,
            message: String::new(),
        }
    }

    #[test]
    fn errors_and_warnings_are_separated_by_severity_not_position() {
        let report = ValidationReport::new(vec![
            finding(ValidationErrorKind::StartNodeMissing),
            finding(ValidationErrorKind::EscalateTargetNotHumanOrNotify),
        ]);
        assert!(report.has_errors());
        assert!(!report.is_valid());
        assert_eq!(report.errors().count(), 1);
        assert_eq!(report.warnings().count(), 1);
        assert_eq!(report.findings().len(), 2);
    }

    #[test]
    fn a_report_with_only_warnings_is_valid() {
        let report = ValidationReport::new(vec![finding(
            ValidationErrorKind::EscalateTargetNotHumanOrNotify,
        )]);
        assert!(report.is_valid());
        assert_eq!(report.warnings().count(), 1);
    }
}
