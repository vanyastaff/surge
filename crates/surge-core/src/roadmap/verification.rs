//! `verification-report` artifact: a sealed verifier's record of its checks.

use super::*;

/// The `verification-report.toml` artifact — a sealed verifier's record of
/// the checks it ran against a task and the outcome it reached. Produced only
/// by verifier nodes (read-only sandbox + verification authority); the hash of
/// this artifact is the `evidence` carried on the matching `TaskVerified`
/// ledger event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    title = "VerificationReportArtifact",
    description = "Surge `verification-report.toml` artifact: a sealed verifier's record of the checks run against a task."
)]
pub struct VerificationReportArtifact {
    /// Artifact contract schema version.
    #[serde(default = "default_verification_report_schema_version")]
    pub schema_version: u32,
    /// The task this report covers.
    pub task_id: String,
    /// Verifier outcome.
    pub outcome: VerificationReportOutcome,
    /// Short human-readable summary of the verification.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Checks the verifier ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<VerificationCheck>,
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
        for (idx, check) in self.checks.iter().enumerate() {
            if check.command.trim().is_empty() {
                issues.push(format!(
                    "verification-report check #{idx} has an empty command"
                ));
            }
            if check.result.trim().is_empty() {
                issues.push(format!(
                    "verification-report check #{idx} ({:?}) has an empty result",
                    check.command
                ));
            }
        }
        issues
    }
}

/// Outcome a sealed verifier reached for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerificationReportOutcome {
    /// The task passed verification; the engine maps this to a `TaskVerified`
    /// ledger transition.
    #[default]
    Passed,
    /// The task failed verification; the engine maps this to a
    /// `FailedVerification` status.
    Failed,
}

/// One check recorded in a [`VerificationReportArtifact`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationCheck {
    /// What was checked (e.g. `cargo nextest run`).
    pub command: String,
    /// `passed` | `failed` | `skipped`.
    pub result: String,
    /// Optional note / observed output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
