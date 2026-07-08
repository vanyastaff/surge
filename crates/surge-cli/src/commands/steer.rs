//! `surge steer <run> "message"` — queue an operator steer for a live run
//! (Phase 2 B2). Non-destructive: the message is delivered at the next agent
//! stage boundary and prepended to that stage's prompt. ACP v1 has no mid-turn
//! injection channel, so Surge does not interrupt the working agent; use the
//! (documented) cancel path only when you accept losing in-flight work.
//!
//! `--list` shows the queued-but-undelivered steers; `--cancel <id>` drops one.
//! The run must be hosted by a running daemon (the queue lives in its memory).

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use surge_core::RunId;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::RunFilter;

/// Arguments for `surge steer`.
#[derive(Args, Debug)]
pub struct SteerArgs {
    /// Run id (or its short suffix as shown by `surge inbox`).
    pub run_id: String,
    /// The steer message. Omit to inspect (`--list`) or drop (`--cancel`).
    pub message: Option<String>,
    /// List the steer messages currently queued for the run.
    #[arg(long)]
    pub list: bool,
    /// Drop a queued steer by its id (from `--list`).
    #[arg(long, value_name = "ID")]
    pub cancel: Option<String>,
}

/// Run `surge steer`.
///
/// # Errors
/// Returns an error if the run cannot be resolved, the daemon cannot be
/// reached, or no action (message / `--list` / `--cancel`) was requested.
pub async fn run(args: SteerArgs) -> Result<()> {
    let message = args
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if args.cancel.is_none() && !args.list && message.is_none() {
        return Err(anyhow!(
            "nothing to do: pass a message to queue a steer, or --list / --cancel <id>"
        ));
    }
    // A message alongside --list/--cancel would be silently dropped; reject it.
    if message.is_some() && (args.list || args.cancel.is_some()) {
        return Err(anyhow!(
            "a steer message cannot be combined with --list or --cancel; run them separately"
        ));
    }

    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let run_id = resolve_run_id(&storage, &args.run_id).await?;
    let daemon = connect_daemon().await?;

    if let Some(steer_id) = &args.cancel {
        let removed = daemon
            .cancel_steer(run_id, steer_id.clone())
            .await
            .map_err(daemon_err)?;
        if removed {
            println!("✓ dropped queued steer {steer_id} on run {run_id}");
        } else {
            println!("no queued steer {steer_id:?} on run {run_id} (already delivered?)");
        }
        return Ok(());
    }

    if args.list {
        let steers = daemon.list_steers(run_id).await.map_err(daemon_err)?;
        if steers.is_empty() {
            println!("no steer messages queued for run {run_id}");
        } else {
            println!("Queued steers for run {run_id} (delivered at the next stage):");
            for steer in steers {
                println!("  {}  {}", steer.id, steer.message);
            }
        }
        return Ok(());
    }

    // `message` is present here: the no-action guard returned early otherwise,
    // and neither --cancel nor --list was set. `if let` keeps this panic-free.
    if let Some(message) = message {
        let steer_id = daemon
            .submit_steer(run_id, message.to_owned())
            .await
            .map_err(daemon_err)?;
        println!("✓ Steer queued for run {run_id} (id {steer_id})");
        println!("  Applies at the next step — not interrupting the current agent.");
        println!(
            "  Cancel with:  surge steer {} --cancel {steer_id}",
            args.run_id
        );
    }
    Ok(())
}

fn daemon_err(e: surge_orchestrator::engine::error::EngineError) -> anyhow::Error {
    anyhow!(
        "steer failed: {e}. The run must be active in a running daemon \
         (started with `surge engine run --daemon`)."
    )
}

/// Connect to the already-running daemon (does not spawn one — a fresh daemon
/// would not hold the run's steer queue).
async fn connect_daemon() -> Result<DaemonEngineFacade> {
    let socket = surge_daemon::pidfile::socket_path().context("resolve daemon socket path")?;
    DaemonEngineFacade::connect(socket)
        .await
        .map_err(|e| anyhow!("no running daemon to steer against: {e}"))
}

/// Resolve a run id, accepting the full ULID or a unique short suffix.
async fn resolve_run_id(storage: &std::sync::Arc<Storage>, value: &str) -> Result<RunId> {
    if let Ok(id) = value.parse::<RunId>() {
        return Ok(id);
    }
    let runs = storage
        .list_runs(RunFilter {
            status: None,
            project_path: None,
            limit: Some(500),
        })
        .await
        .context("list runs for id match")?;
    let matches: Vec<RunId> = runs
        .iter()
        .filter(|r| r.id.to_string().ends_with(value))
        .map(|r| r.id)
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(anyhow!("no run matching {value:?}")),
        many => Err(anyhow!(
            "{} runs match {value:?}; use the full run id",
            many.len()
        )),
    }
}

fn surge_home_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("SURGE_HOME")
        && !custom.is_empty()
    {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(base.join(".surge"))
}
