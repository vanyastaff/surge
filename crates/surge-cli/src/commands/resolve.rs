//! `surge resolve <run>` — answer a run blocked on human input from the
//! terminal, making the fleet inbox actionable (Phase 2 B1 complement).
//!
//! Resolution is delivered to the **running daemon** that hosts the blocked
//! run (the pending gate lives in the daemon's in-memory channels), so a run
//! must be daemon-hosted and still active. With no resolution flag the command
//! prints the pending question and its valid outcomes ("inspect mode").

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use surge_core::RunId;
use surge_core::node::NodeConfig;
use surge_core::run_state::RunState;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::runs::Storage;

use crate::commands::common::{connect_daemon, resolve_run_id, surge_home_dir};
use crate::commands::run_fold::fold_run_state;

/// Arguments for `surge resolve`.
#[derive(Args, Debug)]
pub struct ResolveArgs {
    /// Run id (or its short suffix as shown by `surge inbox`).
    pub run_id: String,
    /// HumanGate outcome key to select (see the inspect output for valid keys).
    #[arg(long)]
    pub outcome: Option<String>,
    /// Optional operator comment attached to the decision.
    #[arg(long)]
    pub comment: Option<String>,
    /// Free-form text answer for a tool-driven `request_human_input`.
    #[arg(long)]
    pub text: Option<String>,
    /// Raw JSON answer for a tool-driven `request_human_input`.
    #[arg(long)]
    pub json: Option<String>,
}

/// Run `surge resolve`.
///
/// # Errors
/// Returns an error if the run cannot be read, is not awaiting input, the
/// answer is invalid for the gate, or the daemon cannot be reached.
pub async fn run(args: ResolveArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let run_id = resolve_run_id(&storage, &args.run_id).await?;
    let pending = inspect_pending(&storage, run_id).await?;

    // Inspect mode: no resolution flag → show the question and how to answer.
    if args.outcome.is_none() && args.text.is_none() && args.json.is_none() {
        println!("Run {run_id} is blocked at @{}", pending.node);
        println!("  {}", pending.prompt.trim());
        if !pending.gate_options.is_empty() {
            println!("\nAnswer with `--outcome <key>`:");
            for (key, label) in &pending.gate_options {
                println!("  {key:<20} {label}");
            }
        } else if pending.is_tool_call {
            println!("\nAnswer with `--text <string>` or `--json <json>`.");
        }
        return Ok(());
    }

    let (_, response) = build_answer(
        pending.is_tool_call,
        pending.call_id.clone(),
        &pending.gate_options,
        args.outcome.as_deref(),
        args.comment.as_deref(),
        args.text.as_deref(),
        args.json.as_deref(),
    )?;

    let daemon = connect_daemon().await?;
    deliver_answer(&daemon, run_id, &pending, response).await?;
    println!("✓ resolved run {run_id}");
    Ok(())
}

/// The question a run is currently blocked on, as `surge resolve` (inspect
/// mode) and the MCP `surge_resolve` tool both present it.
#[derive(Debug, Clone)]
pub(crate) struct PendingInput {
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
/// Returns an error if the run cannot be read or is not waiting for pipeline
/// human input (bootstrap approvals are never answerable through here).
pub(crate) async fn inspect_pending(storage: &Arc<Storage>, run_id: RunId) -> Result<PendingInput> {
    let reader = storage
        .open_run_reader(run_id)
        .await
        .with_context(|| format!("open run {run_id}"))?;
    let state = fold_run_state(&reader, run_id).await?;

    let RunState::Pipeline {
        graph,
        pending_human_input: Some(pending),
        ..
    } = &state
    else {
        return Err(anyhow!(
            "run {run_id} is not waiting for human input (attention: {:?}). \
             Bootstrap approvals are answered via `surge bootstrap` or Telegram.",
            state.attention()
        ));
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
pub(crate) async fn deliver_answer(
    daemon: &DaemonEngineFacade,
    run_id: RunId,
    pending: &PendingInput,
    response: serde_json::Value,
) -> Result<()> {
    daemon
        .resolve_requested_input(
            run_id,
            pending.node.clone(),
            pending.call_id.clone(),
            response,
        )
        .await
        .map_err(|e| {
            anyhow!(
                "resolve failed: {e}. The run must be active in a running daemon \
                 (started with `surge engine run --daemon`)."
            )
        })
}

/// Build the `(call_id, response)` pair for `Engine::resolve_human_input` from
/// the operator's flags. A tool-driven call takes a free-form `--text`/`--json`
/// value under the pending `call_id`; a HumanGate takes an `--outcome` (checked
/// against the gate's declared options) with no `call_id`, plus an optional
/// comment.
pub(crate) fn build_answer(
    is_tool_call: bool,
    call_id: Option<String>,
    gate_options: &[(String, String)],
    outcome: Option<&str>,
    comment: Option<&str>,
    text: Option<&str>,
    json: Option<&str>,
) -> Result<(Option<String>, serde_json::Value)> {
    if is_tool_call {
        let value = if let Some(json) = json {
            serde_json::from_str(json).context("parse --json")?
        } else if let Some(text) = text {
            serde_json::json!({ "text": text })
        } else {
            return Err(anyhow!(
                "this run awaits a free-form tool response; pass --text or --json"
            ));
        };
        return Ok((call_id, value));
    }

    let outcome = outcome.ok_or_else(|| {
        anyhow!("this HumanGate needs `--outcome <key>`; run `surge resolve <run>` for options")
    })?;
    if !gate_options.is_empty() && !gate_options.iter().any(|(key, _)| key == outcome) {
        let valid: Vec<&str> = gate_options.iter().map(|(key, _)| key.as_str()).collect();
        return Err(anyhow!(
            "outcome {outcome:?} is not valid for this gate; valid: {}",
            valid.join(", ")
        ));
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
