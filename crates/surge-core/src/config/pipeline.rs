//! Pipeline limits, human gates and the merge gate.

use super::*;

/// L3 auto-merge gate configuration (spec §10/R31).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MergeGateConfig {
    /// Attach the completed run's own Run Report to the merge gate's
    /// success comment on the tracker (a collapsible `<details>` block on
    /// providers that render inline HTML; a plain leading line otherwise).
    ///
    /// **Off by default.** Spec §10/R31 says the attachment is *optional*;
    /// consent to L3 auto-merge (the `surge:auto` label) is consent to
    /// *merge* the PR, not to publish the run's transcript-derived report to
    /// the tracker — that is a separate disclosure decision the operator has
    /// not made just by opting into auto-merge. Turning this on does not
    /// bypass redaction: the attachment still goes through
    /// `surge_acp::secrets::redact_secrets` and absolute-path shortening
    /// before it is ever posted.
    #[serde(default)]
    pub publish_run_report: bool,
}

/// Pipeline execution settings: parallelism, QA iteration limits, and gate configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_max_qa_iterations")]
    pub max_qa_iterations: u32,
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
    #[serde(default)]
    pub gates: GateConfig,
    /// Stop pipeline if estimated cost exceeds this (USD). None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    /// Stop pipeline if total tokens exceed this. None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_qa_iterations: default_max_qa_iterations(),
            max_parallel: default_max_parallel(),
            gates: GateConfig::default(),
            max_cost_usd: None,
            max_tokens: None,
        }
    }
}

impl PipelineConfig {
    /// Validate the pipeline configuration.
    pub(super) fn validate(&self) -> Result<(), crate::SurgeError> {
        // Validate max_qa_iterations is positive
        if self.max_qa_iterations == 0 {
            return Err(crate::SurgeError::Config(
                "pipeline.max_qa_iterations must be greater than 0".to_string(),
            ));
        }

        // Validate max_parallel is positive
        if self.max_parallel == 0 {
            return Err(crate::SurgeError::Config(
                "pipeline.max_parallel must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}

/// Controls which pipeline phases require human approval before proceeding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    #[serde(default = "default_true")]
    pub after_spec: bool,
    #[serde(default = "default_true")]
    pub after_plan: bool,
    #[serde(default)]
    pub after_each_subtask: bool,
    #[serde(default = "default_true")]
    pub after_qa: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            after_spec: true,
            after_plan: true,
            after_each_subtask: false,
            after_qa: true,
        }
    }
}

/// Decision made at a pipeline gate.
///
/// When the pipeline reaches a configured gate, execution pauses and waits for
/// a human decision. The decision determines whether the pipeline continues,
/// retries with feedback, or aborts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateDecision {
    /// Gate approved — continue to next phase.
    Approved {
        /// Optional feedback from the reviewer.
        feedback: Option<String>,
    },
    /// Gate rejected — re-run the phase with structured feedback.
    Rejected {
        /// Reason for rejection.
        reason: String,
        /// Structured feedback to inject into agent's next prompt.
        feedback: String,
    },
    /// Gate timed out — no human response within configured timeout.
    Timeout {
        /// Optional context about the timeout.
        context: Option<String>,
    },
    /// Gate explicitly aborted — transition task to Failed state.
    Aborted {
        /// Reason for abort.
        reason: String,
    },
}

impl GateDecision {
    /// Returns `true` if the decision allows the pipeline to proceed.
    #[must_use]
    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved { .. })
    }

    /// Returns `true` if the decision requires re-running the phase.
    #[must_use]
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }

    /// Returns `true` if the decision was a timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    /// Returns `true` if the decision should abort the task.
    #[must_use]
    pub fn is_aborted(&self) -> bool {
        matches!(self, Self::Aborted { .. })
    }

    /// Returns `true` if the decision terminates the task (abort or timeout).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Aborted { .. } | Self::Timeout { .. })
    }

    /// Returns the structured feedback for rejected gates, if any.
    #[must_use]
    pub fn rejection_feedback(&self) -> Option<&str> {
        match self {
            Self::Rejected { feedback, .. } => Some(feedback.as_str()),
            _ => None,
        }
    }

    /// Returns the reason for abort or rejection, if any.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Rejected { reason, .. } | Self::Aborted { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// 10 iterations is enough for most QA fix cycles (plan → execute → review).
/// Beyond 10, the issue is likely architectural, not fixable by iteration.
fn default_max_qa_iterations() -> u32 {
    10
}

/// 3 parallel subtasks balances throughput vs. agent API rate limits.
/// Most providers throttle at 3-5 concurrent sessions.
fn default_max_parallel() -> usize {
    3
}
pub(super) fn default_true() -> bool {
    true
}
