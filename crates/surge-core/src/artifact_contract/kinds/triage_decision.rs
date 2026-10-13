//! `triage-decision` artifact validation (`triage_decision.json`, #119).

use super::super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};
use crate::triage_decision::{TriageDecisionArtifact, TriageDecisionIssue};

pub(in crate::artifact_contract) fn validate_triage_decision(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    let artifact = match TriageDecisionArtifact::parse(content) {
        Ok(artifact) => artifact,
        Err(error) => {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::InvalidJson,
                None,
                format!("triage_decision.json is not a valid decision: {error}"),
            ));
            return;
        },
    };
    for issue in artifact.validate() {
        let location = match issue {
            TriageDecisionIssue::DuplicateWithoutTarget => "duplicate_of",
            TriageDecisionIssue::UnclearWithoutQuestion => "question",
        };
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingField,
            Some(location.to_owned()),
            issue.to_string(),
        ));
    }
}
