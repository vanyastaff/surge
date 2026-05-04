//! `surge engine` subtree — in-process M6 CLI for graph-based runs.

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use owo_colors::{OwoColorize, Stream};
use std::path::PathBuf;
use std::sync::Arc;
use surge_core::id::RunId;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_persistence::runs::Storage;

/// Subcommands under `surge engine`.
#[derive(Subcommand, Debug)]
pub enum EngineCommands {
    /// Start a new run from a flow.toml graph.
    Run {
        /// Path to the flow.toml file.
        spec_path: PathBuf,
        /// Stream events to stderr until the run terminates.
        #[arg(long)]
        watch: bool,
        /// Worktree path. Default: current working directory.
        #[arg(long)]
        worktree: Option<PathBuf>,
    },
    /// Tail events from an existing run by id (reads on-disk log).
    Watch {
        /// `RunId` (ULID).
        run_id: String,
    },
    /// Resume an interrupted run from its latest snapshot.
    Resume {
        /// `RunId` (ULID).
        run_id: String,
    },
    /// Cancel a run owned by the current process.
    Stop {
        /// `RunId` (ULID).
        run_id: String,
        /// Reason string recorded in the abort event.
        #[arg(long)]
        reason: Option<String>,
    },
    /// List runs from the on-disk store.
    Ls,
    /// Print events for a run.
    Logs {
        /// `RunId` (ULID).
        run_id: String,
        /// Start from this seq (default: 0 = beginning).
        #[arg(long)]
        since: Option<u64>,
        /// Tail (re-poll for new events).
        #[arg(long)]
        follow: bool,
    },
}

/// Top-level dispatcher for `surge engine` invocations.
pub async fn run(command: EngineCommands) -> Result<()> {
    match command {
        EngineCommands::Run {
            spec_path,
            watch,
            worktree,
        } => run_command(spec_path, watch, worktree).await,
        EngineCommands::Watch { run_id } => watch_command(run_id).await,
        EngineCommands::Resume { run_id } => resume_command(run_id).await,
        EngineCommands::Stop { run_id, reason } => stop_command(run_id, reason).await,
        EngineCommands::Ls => ls_command().await,
        EngineCommands::Logs {
            run_id,
            since,
            follow,
        } => logs_command(run_id, since, follow).await,
    }
}

async fn run_command(spec_path: PathBuf, watch: bool, worktree: Option<PathBuf>) -> Result<()> {
    use std::time::Duration;
    use surge_core::graph::Graph;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    let toml_text = std::fs::read_to_string(&spec_path)
        .with_context(|| format!("read {}", spec_path.display()))?;
    let graph: Graph =
        toml::from_str(&toml_text).with_context(|| format!("parse {}", spec_path.display()))?;

    let worktree_path = worktree.map_or_else(|| std::env::current_dir().context("cwd"), Ok)?;
    if !worktree_path.exists() {
        return Err(anyhow!(
            "worktree path does not exist: {}",
            worktree_path.display()
        ));
    }

    let storage = Storage::open(&surge_runs_dir()?)
        .await
        .context("open storage")?;

    let bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade> = Arc::new(
        surge_acp::bridge::AcpBridge::with_defaults().context("AcpBridge::with_defaults")?,
    );

    let tool_dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> = Arc::new(
        surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
            worktree_path.clone(),
        ),
    );

    let notifier = build_default_notifier();

    let engine = Engine::new_with_notifier(
        bridge,
        storage,
        tool_dispatcher,
        notifier,
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    println!("{run_id}");

    let handle = engine
        .start_run(run_id, graph, worktree_path, EngineRunConfig::default())
        .await?;

    if watch {
        let mut rx = handle.events;
        loop {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
                Ok(Ok(event)) => {
                    print_event(&event);
                    if matches!(event, EngineRunEvent::Terminal(_)) {
                        break;
                    }
                },
                Ok(Err(_)) => break, // sender dropped
                Err(_) => continue,  // 60s timeout, keep waiting
            }
        }
    }

    Ok(())
}

async fn watch_command(run_id: String) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    follow_log_from(id, 0).await?;
    Ok(())
}

async fn resume_command(run_id: String) -> Result<()> {
    let _ = run_id;
    Err(anyhow!(
        "M6: resume requires the engine to be running in this process; \
         use `surge engine run` instead, or wait for M7's daemon mode"
    ))
}

async fn stop_command(run_id: String, reason: Option<String>) -> Result<()> {
    let _ = (run_id, reason);
    Err(anyhow!(
        "M6: stop requires the engine to be running in this process; \
         M7's daemon mode adds out-of-process stop"
    ))
}

async fn ls_command() -> Result<()> {
    let runs_dir = surge_runs_dir()?;
    // Use storage registry for accurate metadata when available; fall back to
    // raw directory listing if the registry isn't open.
    let mut entries: Vec<_> = std::fs::read_dir(&runs_dir)
        .with_context(|| format!("read_dir {}", runs_dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    println!("{:32}  STARTED", "ID");
    for entry in entries {
        let name = entry.file_name();
        let id_str = name.to_string_lossy();
        let metadata = entry.metadata().ok();
        let started = metadata.and_then(|m| m.created().ok()).map_or_else(
            || "?".to_string(),
            |t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            },
        );
        println!("{id_str:32}  {started}");
    }
    Ok(())
}

async fn logs_command(run_id: String, since: Option<u64>, follow: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    let mut last_seq = since.unwrap_or(0);
    last_seq = follow_log_from(id, last_seq).await?;
    if follow {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            last_seq = follow_log_from(id, last_seq).await?;
        }
    }
    Ok(())
}

/// Read events from `since_seq` onwards and print them to stderr.
/// Returns the seq of the last event printed (or `since_seq` if none).
async fn follow_log_from(run_id: RunId, since_seq: u64) -> Result<u64> {
    use surge_persistence::runs::{EventSeq, RunReader};

    let storage = Storage::open(&surge_runs_dir()?).await?;
    let reader: RunReader = storage.open_run_reader(run_id).await?;

    let start = EventSeq(since_seq);
    let end = EventSeq(u64::MAX);
    let events = reader.read_events(start..end).await?;

    let mut max_seq = since_seq;
    for ev in events {
        let seq_val = ev.seq.as_u64();
        eprintln!("[{}] {}", seq_val, ev.payload.payload().discriminant_str());
        if seq_val > max_seq {
            max_seq = seq_val;
        }
    }
    Ok(max_seq)
}

fn parse_run_id(s: &str) -> Result<RunId> {
    s.parse().map_err(|e| anyhow!("invalid run id '{s}': {e}"))
}

fn surge_runs_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("HOME not set"))?;
    let dir = home.join(".surge").join("runs");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    // Storage::open expects the parent .surge dir, not .surge/runs;
    // storage itself creates the runs/ subdir. Return .surge.
    Ok(home.join(".surge"))
}

fn build_default_notifier() -> Arc<dyn surge_notify::NotifyDeliverer> {
    Arc::new(
        surge_notify::MultiplexingNotifier::new()
            .with_desktop(Arc::new(surge_notify::DesktopDeliverer::new()))
            .with_webhook(Arc::new(surge_notify::WebhookDeliverer::new())),
    )
}

fn print_event(event: &surge_orchestrator::engine::handle::EngineRunEvent) {
    use surge_core::run_event::EventPayload;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    match event {
        EngineRunEvent::Persisted { seq, payload } => {
            let prefix = format!("[{seq}]")
                .if_supports_color(Stream::Stderr, |s| s.dimmed())
                .to_string();
            match payload {
                EventPayload::StageEntered { node, attempt } => {
                    eprintln!(
                        "{prefix} [{}] StageEntered (attempt {})",
                        node.if_supports_color(Stream::Stderr, |s| s.cyan()),
                        attempt.if_supports_color(Stream::Stderr, |s| s.dimmed())
                    );
                },
                EventPayload::StageCompleted { node, outcome } => {
                    eprintln!(
                        "{prefix} [{}] StageCompleted \u{2192} {}",
                        node.if_supports_color(Stream::Stderr, |s| s.cyan()),
                        outcome.if_supports_color(Stream::Stderr, |s| s.green())
                    );
                },
                EventPayload::StageFailed { node, reason, .. } => {
                    eprintln!(
                        "{prefix} [{}] StageFailed: {reason}",
                        node.if_supports_color(Stream::Stderr, |s| s.red())
                    );
                },
                EventPayload::LoopIterationStarted { loop_id, index, .. } => {
                    eprintln!(
                        "{prefix} [{}] LoopIterationStarted (index {index})",
                        loop_id.if_supports_color(Stream::Stderr, |s| s.magenta())
                    );
                },
                EventPayload::LoopCompleted {
                    loop_id,
                    completed_iterations,
                    final_outcome,
                } => {
                    eprintln!(
                        "{prefix} [{}] LoopCompleted ({completed_iterations} iterations, final: {})",
                        loop_id.if_supports_color(Stream::Stderr, |s| s.magenta()),
                        final_outcome.if_supports_color(Stream::Stderr, |s| s.green())
                    );
                },
                other => eprintln!("{prefix} {}", other.discriminant_str()),
            }
        },
        EngineRunEvent::Terminal(outcome) => {
            let label = "Terminal:".if_supports_color(Stream::Stderr, |s| s.yellow());
            eprintln!("{label} {outcome:?}");
        },
        _ => {},
    }
}
