//! Revision-scoped verification. The host owns subjects and accepted criteria;
//! agents only submit observations. Pure projections describe journal history.

use crate::roadmap::{
    RoadmapTask, RoadmapTaskId, ValidationAssertion, VerificationReportArtifact,
    VerificationReportOutcome,
};
use crate::{ContentHash, Graph};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Immutable checkpoint locator and the actual equality key for the checked code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationSubject {
    pub repository: PathBuf,
    pub worktree: PathBuf,
    pub tree: String,
    pub checkpoint: String,
}
impl VerificationSubject {
    #[must_use]
    pub fn same_code(&self, other: &Self) -> bool {
        self.repository == other.repository
            && self.worktree == other.worktree
            && self.tree == other.tree
    }
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.repository.is_absolute()
            && self.worktree.is_absolute()
            && [self.tree.as_str(), self.checkpoint.as_str()]
                .iter()
                .all(|oid| oid.len() == 40 && oid.bytes().all(|byte| byte.is_ascii_hexdigit()))
    }
}

/// Authoritative, ordered coverage contract. Every entry is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationCriteria {
    pub hash: ContentHash,
    /// Host-owned accepted version; changes even if semantic contents return to A.
    #[serde(default = "crate::id::StageGenerationId::nil")]
    pub epoch: crate::id::StageGenerationId,
    pub required: Vec<String>,
    pub definitions: Vec<(String, String)>,
}
impl VerificationCriteria {
    /// Hash only semantic inputs, never a task's status or agent's required flags.
    pub fn from_task(
        task: &RoadmapTask,
        assertions: &[ValidationAssertion],
    ) -> Result<Self, String> {
        let mut requirements = Vec::new();
        for (index, text) in task.acceptance_criteria.iter().enumerate() {
            if text.trim().is_empty() {
                return Err("empty acceptance criterion".into());
            }
            requirements.push((format!("criterion:{}", index + 1), text.clone()));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut selected = Vec::new();
        for id in &task.fulfills {
            if !seen.insert(id.as_str()) {
                return Err("duplicate assertion reference".into());
            }
            let matches: Vec<_> = assertions
                .iter()
                .filter(|assertion| &assertion.id == id)
                .collect();
            if matches.len() != 1 {
                return Err(format!("assertion {id} is missing or ambiguous"));
            }
            let assertion = matches[0];
            requirements.push((format!("assertion:{id}"), assertion.pass_condition.clone()));
            selected.push(assertion);
        }
        if requirements.is_empty() {
            return Err("verification requires accepted criteria".into());
        }
        let bytes = serde_json::to_vec(&(task.id.as_str(), &requirements, &selected))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            epoch: crate::id::StageGenerationId::nil(),
            hash: ContentHash::compute(&bytes),
            required: requirements.iter().map(|(id, _)| id.clone()).collect(),
            definitions: requirements,
        })
    }
}

/// Host-sealed identity, included in the report's content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationBinding {
    pub subject: VerificationSubject,
    pub criteria: VerificationCriteria,
}

/// Separates unauthorized claims from authorized claims lacking trusted proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationClaim {
    Unauthorized,
    Unbound,
    Verified,
}
#[must_use]
pub fn classify_claim(
    graph: Option<&Graph>,
    node: &crate::NodeKey,
    task: &RoadmapTaskId,
    evidence: ContentHash,
    report: Option<&VerificationReportArtifact>,
    subject: Option<&VerificationSubject>,
    criteria: Option<&VerificationCriteria>,
) -> VerificationClaim {
    if !graph.is_some_and(|graph| crate::run_state::node_has_verification_authority(graph, node)) {
        VerificationClaim::Unauthorized
    } else if accepts_claim(graph, node, task, evidence, report, subject, criteria) {
        VerificationClaim::Verified
    } else {
        VerificationClaim::Unbound
    }
}

/// The common claim gate for live replay, reports and SQLite projections.
#[must_use]
pub fn accepts_claim(
    graph: Option<&Graph>,
    node: &crate::NodeKey,
    task: &RoadmapTaskId,
    evidence: ContentHash,
    report: Option<&VerificationReportArtifact>,
    subject: Option<&VerificationSubject>,
    criteria: Option<&VerificationCriteria>,
) -> bool {
    let Some(report) = report else {
        return false;
    };
    let Some(binding) = report.binding.as_ref() else {
        return false;
    };
    graph.is_some_and(|graph| crate::run_state::node_has_verification_authority(graph, node))
        && binding.subject.is_valid()
        && binding.criteria.epoch != crate::id::StageGenerationId::nil()
        && subject.is_some_and(|current| current.same_code(&binding.subject))
        && criteria == Some(&binding.criteria)
        && report.task_id == task.as_str()
        && report.outcome == VerificationReportOutcome::Passed
        && report.validate().is_empty()
        && report.covers(&binding.criteria.required)
        && toml::to_string(report)
            .is_ok_and(|text| ContentHash::compute(text.as_bytes()) == evidence)
}

/// Historical verification context retained independently of lifecycle state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerificationContext {
    pub subject: Option<VerificationSubject>,
    pub criteria: BTreeMap<RoadmapTaskId, VerificationCriteria>,
}

/// Shared invalidation decision; stores apply it to their own normalized rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationInvalidation {
    None,
    All,
    Task(RoadmapTaskId),
}
impl VerificationContext {
    pub fn observe(&mut self, event: &crate::run_event::EventPayload) -> VerificationInvalidation {
        use crate::run_event::EventPayload;
        match event {
            EventPayload::VerificationSubjectObserved { subject } => {
                let changed = subject.is_none()
                    || self
                        .subject
                        .as_ref()
                        .zip(subject.as_ref())
                        .is_none_or(|(old, new)| !old.same_code(new));
                self.subject = subject.clone();
                if changed {
                    self.criteria.clear();
                    VerificationInvalidation::All
                } else {
                    VerificationInvalidation::None
                }
            },
            EventPayload::VerificationCriteriaAccepted { task_id, criteria } => {
                let changed = criteria.is_none() || self.criteria.get(task_id) != criteria.as_ref();
                if let Some(criteria) = criteria {
                    self.criteria.insert(task_id.clone(), criteria.clone());
                } else {
                    self.criteria.remove(task_id);
                }
                if changed {
                    VerificationInvalidation::Task(task_id.clone())
                } else {
                    VerificationInvalidation::None
                }
            },
            EventPayload::TaskStatusChanged { task_id, .. } => {
                self.criteria.remove(task_id);
                VerificationInvalidation::Task(task_id.clone())
            },
            EventPayload::RoadmapUpdated { .. } => {
                self.criteria.clear();
                VerificationInvalidation::All
            },
            _ => VerificationInvalidation::None,
        }
    }
}

/// Current workspace freshness; historical compilation alone cannot claim Current.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofFreshness {
    Current,
    Stale,
    #[default]
    Unknown,
    Unbound,
}
impl ProofFreshness {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "needs re-verification",
            Self::Unknown => "unknown",
            Self::Unbound => "unbound",
        }
    }
}

#[cfg(test)]
pub(crate) fn bind_fixture(events: &[crate::RunEvent]) -> Vec<crate::RunEvent> {
    let mut bound = Vec::new();
    for event in events {
        let mut event = event.clone();
        if let crate::EventPayload::TaskVerified {
            task_id,
            evidence,
            report,
            ..
        } = &mut event.payload
        {
            let worktree = std::env::temp_dir().join("surge-verification-fixture-repo");
            assert!(worktree.is_absolute());
            let subject = VerificationSubject {
                repository: worktree.join(".git"),
                worktree,
                tree: "a".repeat(40),
                checkpoint: "b".repeat(40),
            };
            let criteria = VerificationCriteria {
                hash: ContentHash::compute(b"fixed accepted criterion"),
                epoch: "00000000000000000000000001".parse().unwrap(),
                required: vec!["criterion:1".into()],
                definitions: vec![("criterion:1".into(), "fixed accepted criterion".into())],
            };
            let sealed = VerificationReportArtifact {
                schema_version: 1,
                task_id: task_id.to_string(),
                outcome: VerificationReportOutcome::Passed,
                summary: "fixed passing fixture".into(),
                checks: vec![crate::roadmap::VerificationCheck {
                    command: "fixture acceptance".into(),
                    result: crate::roadmap::VerificationCheckResult::Passed,
                    covers: vec!["criterion:1".into()],
                    note: None,
                }],
                evidence: vec![],
                binding: Some(VerificationBinding {
                    subject: subject.clone(),
                    criteria: criteria.clone(),
                }),
            };
            *evidence = ContentHash::compute(toml::to_string(&sealed).unwrap().as_bytes());
            *report = Some(sealed);
            for payload in [
                crate::EventPayload::VerificationSubjectObserved {
                    subject: Some(subject),
                },
                crate::EventPayload::VerificationCriteriaAccepted {
                    task_id: task_id.clone(),
                    criteria: Some(criteria),
                },
            ] {
                bound.push(crate::RunEvent {
                    run_id: event.run_id,
                    seq: event.seq,
                    timestamp: event.timestamp,
                    payload,
                });
            }
        }
        bound.push(event);
    }
    for (index, event) in bound.iter_mut().enumerate() {
        event.seq = index as u64 + 1;
    }
    bound
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observed_a_b_a_does_not_restore_any_proof() {
        let subject = VerificationSubject {
            repository: "/repo/.git".into(),
            worktree: "/repo".into(),
            tree: "a".repeat(40),
            checkpoint: "b".repeat(40),
        };
        let mut changed = subject.clone();
        changed.tree = "c".repeat(40);
        let mut context = VerificationContext::default();
        let mut verified = true;
        for subject in [subject.clone(), changed, subject] {
            if context.observe(&crate::EventPayload::VerificationSubjectObserved {
                subject: Some(subject),
            }) == VerificationInvalidation::All
            {
                verified = false;
            }
        }
        assert!(
            !verified,
            "matching contents again never replay an old proof"
        );
    }
    #[test]
    fn missing_observation_invalidates_every_time() {
        let mut context = VerificationContext::default();
        assert_eq!(
            context.observe(&crate::EventPayload::VerificationSubjectObserved { subject: None }),
            VerificationInvalidation::All
        );
    }
}
