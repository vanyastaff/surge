//! `surge resolve <run>` — answer a run blocked on human input from the
//! terminal, making the fleet inbox actionable (Phase 2 B1 complement).
//!
//! Resolution is delivered to the **running daemon** that hosts the blocked
//! run (the pending gate lives in the daemon's in-memory channels), so a run
//! must be daemon-hosted and still active. With no resolution flag the command
//! prints the pending question and its valid outcomes ("inspect mode").

use anyhow::{Context, Result};
use clap::Args;
use surge_orchestrator::operator::{build_answer, deliver_answer, inspect_pending};
use surge_persistence::runs::Storage;

use crate::commands::common::{connect_daemon, resolve_run_id, surge_home_dir};

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
