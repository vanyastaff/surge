//! `verification-report`: application evidence, audits and sealed verification.
//! Unbound evidence reports do not prove task verification.

use super::*;

/// The `verification-report.toml` artifact records checks and their outcome.
/// Application testers and generic audits produce unbound evidence. Only the
/// host-sealed verifier path can bind a report to accepted task criteria and
/// carry its hash as evidence on a `TaskVerified` ledger event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    title = "VerificationReportArtifact",
    description = "Surge `verification-report.toml` artifact: evidence or a host-sealed record of checks run against a task."
)]
pub struct VerificationReportArtifact {
    /// Artifact contract schema version.
    #[serde(default = "default_verification_report_schema_version")]
    pub schema_version: u32,
    /// The task this report covers.
    pub task_id: String,
    /// Observed report outcome.
    pub outcome: VerificationReportOutcome,
    /// Short human-readable summary of the verification.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Checks actually run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<VerificationCheck>,
    /// Host-owned binding; absent on application evidence and unbound audits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<crate::verification_evidence::VerificationBinding>,
    /// References to evidence (content hashes or artifact paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

impl VerificationReportArtifact {
    /// Validate the artifact: `task_id` and `outcome` are required and every
    /// check has a non-empty command. Returns an empty vector when
    /// well-formed.
    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.task_id.trim().is_empty() {
            issues.push("verification-report has an empty task_id".to_owned());
        }
        if self.summary.trim().is_empty() && self.checks.is_empty() {
            issues.push(format!(
                "verification-report for {:?} has no summary and no checks",
                self.task_id
            ));
        }
        if self.outcome == VerificationReportOutcome::Passed && self.checks.is_empty() {
            issues.push("passed verification requires actual checks".into());
        }
        for (idx, check) in self.checks.iter().enumerate() {
            if check.command.trim().is_empty() {
                issues.push(format!(
                    "verification-report check #{idx} has an empty command"
                ));
            }
            if self.outcome == VerificationReportOutcome::Passed
                && check.result != VerificationCheckResult::Passed
            {
                issues.push(format!(
                    "verification-report check #{idx} ({:?}) did not pass",
                    check.command
                ));
            }
        }
        issues
    }

    /// Every authoritative criterion must be covered by a passing check.
    #[must_use]
    pub fn covers(&self, required: &[String]) -> bool {
        !required.is_empty()
            && required.iter().all(|id| {
                self.checks.iter().any(|check| {
                    check.result == VerificationCheckResult::Passed && check.covers.contains(id)
                })
            })
            && self
                .checks
                .iter()
                .flat_map(|check| &check.covers)
                .all(|id| required.contains(id))
    }
}

/// Observed outcome recorded in application evidence, an audit or a sealed
/// verification report. This value alone has no ledger effect. Task ledger
/// transitions require a declared, host-accepted verification effect through
/// the sealed verifier path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerificationReportOutcome {
    /// The reported checks passed. Unbound evidence does not certify a task;
    /// `TaskVerified` requires the host-accepted sealed verification path.
    #[default]
    Passed,
    /// The report records failed or unfulfilled checks. A `FailedVerification`
    /// status requires the corresponding declared, host-accepted ledger effect.
    Failed,
}

/// One check recorded in a [`VerificationReportArtifact`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationCheck {
    /// What was checked (e.g. `cargo nextest run`).
    pub command: String,
    /// `passed` | `failed` | `skipped`.
    pub result: VerificationCheckResult,
    /// Host-declared criterion IDs covered by this check.
    #[serde(default)]
    pub covers: Vec<String>,
    /// Optional note / observed output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A cancelled or skipped check is never a successful observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerificationCheckResult {
    Passed,
    Failed,
    Skipped,
    Cancelled,
}
