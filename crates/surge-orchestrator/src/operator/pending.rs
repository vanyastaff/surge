//! Pending human input: inspect what a run is blocked on and build the answer
//! an operator gives it.
//!
//! Resolution is delivered to the **running daemon** that hosts the blocked
//! run (the pending gate lives in the daemon's in-memory channels), so a run
//! must be daemon-hosted and still active.

use std::sync::Arc;

use surge_core::RunId;
use surge_core::node::NodeConfig;
use surge_core::run_state::RunState;
use surge_persistence::runs::Storage;

use crate::engine::daemon_facade::DaemonEngineFacade;
use crate::engine::facade::EngineFacade;
use crate::operator::error::OperatorError;
use crate::operator::journal::fold_run_state;

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

/// Fold `run_id`'s event log and return what it is blocked on.
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
    let state = fold_run_state(&reader).await?;

    let RunState::Pipeline {
        graph,
        pending_human_input: Some(pending),
        ..
    } = &state
    else {
        return Err(OperatorError::NotAwaitingInput {
            run_id,
            attention: state.attention(),
        });
    };

    // Gate options come from the pending node's HumanGate config, if any. The
    // node may live at the top level or inside a subgraph (e.g. a gate in a
    // loop body), so search both — a top-level-only lookup shows no options for
    // subgraph gates and skips client-side outcome validation.
    let gate_node = graph.find_node(&pending.node);
    let is_bootstrap_gate = matches!(
        gate_node.map(|n| &n.config),
        Some(NodeConfig::HumanGate(cfg))
            if matches!(cfg.mode, surge_core::human_gate_config::HumanGateMode::Bootstrap { .. })
    );
    let tool_call_id = pending
        .call_id
        .as_deref()
        .filter(|id| surge_core::id::GateRequestId::from_event_call_id(id).is_none());
    let kind = if is_bootstrap_gate {
        PendingKind::BootstrapGate
    } else if let Some(call_id) = tool_call_id {
        PendingKind::ToolCall {
            call_id: call_id.to_owned(),
        }
    } else {
        let options = match gate_node.map(|n| &n.config) {
            Some(NodeConfig::HumanGate(cfg)) => cfg
                .options
                .iter()
                .map(|o| GateOption {
                    outcome: o.outcome.to_string(),
                    label: o.label.clone(),
                })
                .collect(),
            _ => Vec::new(),
        };
        PendingKind::Gate {
            call_id: pending.call_id.clone(),
            options,
        }
    };
    Ok(PendingInput {
        node: pending.node.clone(),
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
