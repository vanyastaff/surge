//! Pending human input: inspect what a run is blocked on and build the answer
//! an operator gives it.
//!
//! Resolution is delivered to the **running daemon** that hosts the blocked
//! run (the pending gate lives in the daemon's in-memory channels), so a run
//! must be daemon-hosted and still active.

use std::sync::Arc;

use surge_core::RunId;
use surge_persistence::runs::Storage;

use crate::engine::daemon_facade::DaemonEngineFacade;
use crate::engine::facade::EngineFacade;
use crate::operator::error::OperatorError;
use crate::operator::journal::read_run_events;

mod decisions;

pub use decisions::{
    DecisionChange, DecisionId, DecisionKind, Fed, FoldStatus, Incomplete, PendingDecision,
    PendingDecisions, Retirement,
};

/// One outcome a `HumanGate` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOption {
    /// The outcome key an answer selects.
    pub outcome: String,
    /// The label shown to the operator.
    pub label: String,
}

/// What kind of answer the pending request accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingKind {
    /// A tool-driven free-form `request_human_input`, answered with text or
    /// JSON under its `call_id`.
    ToolCall {
        /// Scoped tool-call key the answer is delivered under.
        call_id: String,
    },
    /// A pipeline `HumanGate`, answered with one of its declared outcomes.
    Gate {
        /// Namespaced gate-request id the answer is delivered under, when the
        /// gate carries one.
        call_id: Option<String>,
        /// The outcomes the gate declares; empty when it declares none (any
        /// outcome key is then accepted).
        options: Vec<GateOption>,
    },
    /// A bootstrap-mode `HumanGate` (description / roadmap / flow approval).
    /// Those are human decisions by design: [`PendingInput::build_answer`]
    /// always refuses them.
    BootstrapGate,
}

/// The question a run is currently blocked on, as `surge resolve` (inspect
/// mode) and the MCP `surge_resolve` tool both present it.
#[derive(Debug, Clone)]
pub struct PendingInput {
    /// Graph node that issued the request.
    pub node: surge_core::keys::NodeKey,
    /// Prompt shown to the operator.
    pub prompt: String,
    /// What answer the request accepts.
    pub kind: PendingKind,
}

/// An operator's answer to a [`PendingInput`], before validation.
#[derive(Debug, Clone, PartialEq)]
pub enum OperatorAnswer {
    /// Select a `HumanGate` outcome, with an optional comment.
    Outcome {
        /// The outcome key.
        key: String,
        /// Optional operator comment attached to the decision.
        comment: Option<String>,
    },
    /// A free-form text answer to a tool-driven request.
    Text(String),
    /// A raw JSON answer to a tool-driven request.
    Json(serde_json::Value),
}

/// An answer that passed [`PendingInput::build_answer`]; the only thing
/// [`deliver_answer`] accepts, so the delivery target (node and call id) has
/// one source of truth.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedAnswer {
    node: surge_core::keys::NodeKey,
    call_id: Option<String>,
    response: serde_json::Value,
}

impl ValidatedAnswer {
    /// The response payload that will be delivered to the run.
    pub fn response(&self) -> &serde_json::Value {
        &self.response
    }
}

impl PendingInput {
    /// Validate `answer` against what this request accepts and build the
    /// payload to deliver. A tool call takes text or JSON; a gate takes one of
    /// its declared outcomes (any key when it declares none), plus an optional
    /// comment.
    ///
    /// # Errors
    /// Returns [`OperatorError::HumanOnlyGate`] for a bootstrap approval (never
    /// answerable here), [`OperatorError::MissingToolAnswer`] /
    /// [`OperatorError::MissingOutcome`] if the answer's shape does not match
    /// the request, and [`OperatorError::InvalidOutcome`] if the outcome is not
    /// one the gate declares.
    pub fn build_answer(&self, answer: OperatorAnswer) -> Result<ValidatedAnswer, OperatorError> {
        let (call_id, response) = match (&self.kind, answer) {
            (PendingKind::BootstrapGate, _) => {
                return Err(OperatorError::HumanOnlyGate {
                    node: self.node.to_string(),
                });
            },
            (PendingKind::ToolCall { call_id }, OperatorAnswer::Json(value)) => {
                (Some(call_id.clone()), value)
            },
            (PendingKind::ToolCall { call_id }, OperatorAnswer::Text(text)) => {
                (Some(call_id.clone()), serde_json::json!({ "text": text }))
            },
            (PendingKind::ToolCall { .. }, OperatorAnswer::Outcome { .. }) => {
                return Err(OperatorError::MissingToolAnswer);
            },
            (PendingKind::Gate { call_id, options }, OperatorAnswer::Outcome { key, comment }) => {
                if !options.is_empty() && !options.iter().any(|option| option.outcome == key) {
                    let valid: Vec<&str> = options.iter().map(|o| o.outcome.as_str()).collect();
                    return Err(OperatorError::InvalidOutcome {
                        outcome: key,
                        valid: valid.join(", "),
                    });
                }
                let mut response = serde_json::json!({ "outcome": key });
                if let Some(comment) = comment {
                    response["comment"] = serde_json::Value::String(comment);
                }
                (call_id.clone(), response)
            },
            (PendingKind::Gate { .. }, OperatorAnswer::Text(_) | OperatorAnswer::Json(_)) => {
                return Err(OperatorError::MissingOutcome);
            },
        };
        Ok(ValidatedAnswer {
            node: self.node.clone(),
            call_id,
            response,
        })
    }
}

/// Fold `run_id`'s event log and return what it is blocked on: the run's
/// current human-input request, as [`PendingDecisions`] classifies it.
///
/// # Errors
/// Returns [`OperatorError`] if the run cannot be read or is not waiting for
/// pipeline human input (bootstrap approvals surface as
/// [`PendingKind::BootstrapGate`] and are refused by
/// [`PendingInput::build_answer`]).
pub async fn inspect_pending(
    storage: &Arc<Storage>,
    run_id: RunId,
) -> Result<PendingInput, OperatorError> {
    let reader = storage
        .open_run_reader(run_id)
        .await
        .map_err(|source| OperatorError::OpenRun { run_id, source })?;
    let events = read_run_events(&reader).await?;
    drop(reader);
    let decisions = PendingDecisions::from_events(run_id, &events);
    if let FoldStatus::Incomplete(incomplete) = decisions.status() {
        let cause = match incomplete {
            Incomplete::Gap { expected, found } => {
                surge_core::run_state::FoldError::CorruptedSequence {
                    expected_seq: *expected,
                    got_seq: *found,
                }
            },
            Incomplete::Unfoldable(cause) => cause.clone(),
        };
        return Err(OperatorError::Fold { run_id, cause });
    }
    let Some(pending) = decisions.current_request() else {
        return Err(OperatorError::NotAwaitingInput {
            run_id,
            attention: decisions.run_state().attention(),
        });
    };
    let node = pending
        .node
        .clone()
        .ok_or_else(|| OperatorError::NotAwaitingInput {
            run_id,
            attention: decisions.run_state().attention(),
        })?;
    let kind = match &pending.kind {
        DecisionKind::BootstrapApproval { .. } => PendingKind::BootstrapGate,
        DecisionKind::ToolCallInput { call_id } => PendingKind::ToolCall {
            call_id: call_id.clone(),
        },
        kind => PendingKind::Gate {
            call_id: kind.call_id().map(str::to_owned),
            options: pending.options.clone(),
        },
    };
    Ok(PendingInput {
        node,
        prompt: pending.prompt.clone(),
        kind,
    })
}

/// Deliver an already-validated `answer` to the daemon hosting `run_id`.
///
/// # Errors
/// Returns [`OperatorError::DeliveryFailed`] if the daemon is unreachable, does
/// not host the run, or declines the answer.
pub async fn deliver_answer(
    daemon: &DaemonEngineFacade,
    run_id: RunId,
    answer: ValidatedAnswer,
) -> Result<(), OperatorError> {
    daemon
        .resolve_requested_input(run_id, answer.node, answer.call_id, answer.response)
        .await
        .map_err(|cause| OperatorError::DeliveryFailed { cause })
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::keys::NodeKey;

    fn pending(kind: PendingKind) -> PendingInput {
        PendingInput {
            node: NodeKey::try_from("plan_gate").unwrap(),
            prompt: "Approve?".into(),
            kind,
        }
    }

    fn gate() -> PendingInput {
        pending(PendingKind::Gate {
            call_id: None,
            options: vec![
                GateOption {
                    outcome: "approve".into(),
                    label: "Approve".into(),
                },
                GateOption {
                    outcome: "reject".into(),
                    label: "Reject".into(),
                },
            ],
        })
    }

    fn tool_call() -> PendingInput {
        pending(PendingKind::ToolCall {
            call_id: "call-1".into(),
        })
    }

    fn outcome(key: &str, comment: Option<&str>) -> OperatorAnswer {
        OperatorAnswer::Outcome {
            key: key.into(),
            comment: comment.map(Into::into),
        }
    }

    #[test]
    fn gate_answer_builds_outcome_and_comment() {
        let answer = gate()
            .build_answer(outcome("approve", Some("lgtm")))
            .unwrap();
        assert_eq!(answer.call_id, None);
        assert_eq!(answer.response()["outcome"], "approve");
        assert_eq!(answer.response()["comment"], "lgtm");
    }

    #[test]
    fn gate_answer_rejects_unknown_outcome() {
        let err = gate().build_answer(outcome("bogus", None)).unwrap_err();
        assert!(matches!(err, OperatorError::InvalidOutcome { .. }), "{err}");
    }

    #[test]
    fn gate_answer_requires_an_outcome() {
        let err = gate()
            .build_answer(OperatorAnswer::Text("nope".into()))
            .unwrap_err();
        assert!(matches!(err, OperatorError::MissingOutcome), "{err}");
    }

    #[test]
    fn tool_answer_wraps_text_under_call_id() {
        let answer = tool_call()
            .build_answer(OperatorAnswer::Text("use the staging db".into()))
            .unwrap();
        assert_eq!(answer.call_id.as_deref(), Some("call-1"));
        assert_eq!(answer.response()["text"], "use the staging db");
    }

    #[test]
    fn tool_answer_passes_json_through() {
        let answer = tool_call()
            .build_answer(OperatorAnswer::Json(serde_json::json!({"env": "prod"})))
            .unwrap();
        assert_eq!(answer.response()["env"], "prod");
    }

    #[test]
    fn tool_answer_requires_text_or_json() {
        let err = tool_call()
            .build_answer(outcome("approve", None))
            .unwrap_err();
        assert!(matches!(err, OperatorError::MissingToolAnswer), "{err}");
    }

    #[test]
    fn a_bootstrap_gate_is_never_answerable() {
        let bootstrap = pending(PendingKind::BootstrapGate);
        for answer in [
            outcome("approve", None),
            OperatorAnswer::Text("yes".into()),
            OperatorAnswer::Json(serde_json::json!(true)),
        ] {
            let err = bootstrap.build_answer(answer).unwrap_err();
            assert!(matches!(err, OperatorError::HumanOnlyGate { .. }), "{err}");
        }
    }
}
