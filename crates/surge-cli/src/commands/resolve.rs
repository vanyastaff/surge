//! `surge resolve <run>` — answer a run blocked on human input from the
//! terminal, making the fleet inbox actionable (Phase 2 B1 complement).
//!
//! Resolution is delivered to the **running daemon** that hosts the blocked
//! run (the pending gate lives in the daemon's in-memory channels), so a run
//! must be daemon-hosted and still active. With no resolution flag the command
//! prints the pending question and its valid outcomes ("inspect mode").

use anyhow::{Context, Result, anyhow};
use clap::Args;
use surge_orchestrator::operator::{
    OperatorAnswer, PendingInput, PendingKind, ValidatedAnswer, deliver_answer, inspect_pending,
};
use surge_persistence::runs::Storage;

use crate::commands::common::{connect_daemon, operator_failure, resolve_run_id, surge_home_dir};

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
    let pending = inspect_pending(&storage, run_id)
        .await
        .map_err(operator_failure)?;

    // Inspect mode: no resolution flag → show the question and how to answer.
    if args.outcome.is_none() && args.text.is_none() && args.json.is_none() {
        println!("Run {run_id} is blocked at @{}", pending.node);
        println!("  {}", pending.prompt.trim());
        match &pending.kind {
            PendingKind::Gate { options, .. } if !options.is_empty() => {
                println!("\nAnswer with `--outcome <key>`:");
                for option in options {
                    println!("  {:<20} {}", option.outcome, option.label);
                }
            },
            PendingKind::ToolCall { .. } => {
                println!("\nAnswer with `--text <string>` or `--json <json>`.");
            },
            PendingKind::Gate { .. } | PendingKind::BootstrapGate => {},
        }
        return Ok(());
    }

    let answer = validated_answer(&pending, &args)?;
    let daemon = connect_daemon().await?;
    deliver_answer(&daemon, run_id, answer)
        .await
        .map_err(operator_failure)?;
    println!("✓ resolved run {run_id}");
    Ok(())
}

/// Turn the resolution flags into the answer `pending` accepts: a tool call
/// reads `--json` before `--text`, a gate reads `--outcome`. A flag of the
/// wrong shape is still passed on, so the service reports what the request
/// actually needs.
fn validated_answer(pending: &PendingInput, args: &ResolveArgs) -> Result<ValidatedAnswer> {
    let json = || -> Result<Option<OperatorAnswer>> {
        args.json
            .as_deref()
            .map(|raw| {
                serde_json::from_str(raw)
                    .map(OperatorAnswer::Json)
                    .context("parse --json")
            })
            .transpose()
    };
    let text = || args.text.clone().map(OperatorAnswer::Text);
    let outcome = || {
        args.outcome.clone().map(|key| OperatorAnswer::Outcome {
            key,
            comment: args.comment.clone(),
        })
    };
    let answer = match &pending.kind {
        PendingKind::ToolCall { .. } => json()?.or_else(text).or_else(outcome),
        PendingKind::Gate { .. } | PendingKind::BootstrapGate => {
            outcome().or_else(text).map_or_else(json, |a| Ok(Some(a)))?
        },
    };
    let answer = answer.ok_or_else(|| anyhow!("pass --outcome, --text or --json"))?;
    pending.build_answer(answer).map_err(operator_failure)
}
