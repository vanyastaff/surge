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

/// The question a run is currently blocked on, as `surge resolve` (inspect
/// mode) and the MCP `surge_resolve` tool both present it.
#[derive(Debug, Clone)]
pub struct PendingInput {
    /// Graph node that issued the request.
    pub node: surge_core::keys::NodeKey,
    /// Scoped tool-call key or namespaced gate-request id.
    pub call_id: Option<String>,
    /// Prompt shown to the operator.
    pub prompt: String,
    /// `(outcome key, label)` pairs when the pending node is a `HumanGate`;
    /// empty for a tool-driven `request_human_input`.
    pub gate_options: Vec<(String, String)>,
    /// A tool-driven free-form request rather than a `HumanGate` outcome.
    pub is_tool_call: bool,
    /// The pending node is a bootstrap-mode `HumanGate` (description / roadmap
    /// / flow approval). Those are human decisions by design.
    pub is_bootstrap_gate: bool,
}

/// Fold `run_id`'s event log and return what it is blocked on.
///
/// # Errors
/// Returns [`OperatorError`] if the run cannot be read or is not waiting for
/// pipeline human input (bootstrap approvals are never answerable through
/// here).
pub async fn inspect_pending(
    storage: &Arc<Storage>,
    run_id: RunId,
) -> Result<PendingInput, OperatorError> {
    let reader = storage
        .open_run_reader(run_id)
        .await
        .map_err(|source| OperatorError::OpenRun { run_id, source })?;
    let state = fold_run_state(&reader, run_id).await?;

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
    let gate_options: Vec<(String, String)> = match gate_node.map(|n| &n.config) {
        Some(NodeConfig::HumanGate(cfg)) => cfg
            .options
            .iter()
            .map(|o| (o.outcome.to_string(), o.label.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let is_tool_call = pending
        .call_id
        .as_deref()
        .is_some_and(|id| surge_core::id::GateRequestId::from_event_call_id(id).is_none());
    let is_bootstrap_gate = matches!(
        gate_node.map(|n| &n.config),
        Some(NodeConfig::HumanGate(cfg))
            if matches!(cfg.mode, surge_core::human_gate_config::HumanGateMode::Bootstrap { .. })
    );
    Ok(PendingInput {
        node: pending.node.clone(),
        call_id: pending.call_id.clone(),
        prompt: pending.prompt.clone(),
        gate_options,
        is_tool_call,
        is_bootstrap_gate,
    })
}

/// Deliver an already-validated `response` to the daemon hosting `run_id`.
///
/// # Errors
/// Returns [`OperatorError::DeliveryFailed`] if the daemon is unreachable, does
/// not host the run, or declines the answer.
pub async fn deliver_answer(
    daemon: &DaemonEngineFacade,
    run_id: RunId,
    pending: &PendingInput,
    response: serde_json::Value,
) -> Result<(), OperatorError> {
    daemon
        .resolve_requested_input(
            run_id,
            pending.node.clone(),
            pending.call_id.clone(),
            response,
        )
        .await
        .map_err(|cause| OperatorError::DeliveryFailed { cause })
}

/// Build the `(call_id, response)` pair for `Engine::resolve_human_input` from
/// the operator's flags. A tool-driven call takes a free-form `--text`/`--json`
/// value under the pending `call_id`; a HumanGate takes an `--outcome` (checked
/// against the gate's declared options) with no `call_id`, plus an optional
/// comment.
///
/// # Errors
/// Returns [`OperatorError`] if a tool answer is missing or its JSON does not
/// parse, or a gate answer has no outcome or one the gate does not declare.
pub fn build_answer(
    is_tool_call: bool,
    call_id: Option<String>,
    gate_options: &[(String, String)],
    outcome: Option<&str>,
    comment: Option<&str>,
    text: Option<&str>,
    json: Option<&str>,
) -> Result<(Option<String>, serde_json::Value), OperatorError> {
    if is_tool_call {
        let value = if let Some(json) = json {
            serde_json::from_str(json).map_err(OperatorError::ParseAnswerJson)?
        } else if let Some(text) = text {
            serde_json::json!({ "text": text })
        } else {
            return Err(OperatorError::MissingToolAnswer);
        };
        return Ok((call_id, value));
    }

    let outcome = outcome.ok_or(OperatorError::MissingOutcome)?;
    if !gate_options.is_empty() && !gate_options.iter().any(|(key, _)| key == outcome) {
        let valid: Vec<&str> = gate_options.iter().map(|(key, _)| key.as_str()).collect();
        return Err(OperatorError::InvalidOutcome {
            outcome: outcome.to_owned(),
            valid: valid.join(", "),
        });
    }
    let mut response = serde_json::json!({ "outcome": outcome });
    if let Some(comment) = comment {
        response["comment"] = serde_json::Value::String(comment.to_owned());
    }
    Ok((None, response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<(String, String)> {
        vec![
            ("approve".into(), "Approve".into()),
            ("reject".into(), "Reject".into()),
        ]
    }

    #[test]
    fn gate_answer_builds_outcome_and_comment() {
        let (call_id, value) = build_answer(
            false,
            None,
            &opts(),
            Some("approve"),
            Some("lgtm"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(call_id, None);
        assert_eq!(value["outcome"], "approve");
        assert_eq!(value["comment"], "lgtm");
    }

    #[test]
    fn gate_answer_rejects_unknown_outcome() {
        let err = build_answer(false, None, &opts(), Some("bogus"), None, None, None).unwrap_err();
        assert!(err.to_string().contains("not valid"), "{err}");
    }

    #[test]
    fn gate_answer_requires_outcome() {
        let err = build_answer(false, None, &opts(), None, None, None, None).unwrap_err();
        assert!(err.to_string().contains("needs `--outcome"), "{err}");
    }

    #[test]
    fn tool_answer_wraps_text_under_call_id() {
        let (call_id, value) = build_answer(
            true,
            Some("call-1".into()),
            &[],
            None,
            None,
            Some("use the staging db"),
            None,
        )
        .unwrap();
        assert_eq!(call_id.as_deref(), Some("call-1"));
        assert_eq!(value["text"], "use the staging db");
    }

    #[test]
    fn tool_answer_parses_json() {
        let (_, value) = build_answer(
            true,
            Some("call-1".into()),
            &[],
            None,
            None,
            None,
            Some(r#"{"env":"prod"}"#),
        )
        .unwrap();
        assert_eq!(value["env"], "prod");
    }

    #[test]
    fn tool_answer_requires_text_or_json() {
        let err =
            build_answer(true, Some("call-1".into()), &[], None, None, None, None).unwrap_err();
        assert!(err.to_string().contains("--text or --json"), "{err}");
    }
}
