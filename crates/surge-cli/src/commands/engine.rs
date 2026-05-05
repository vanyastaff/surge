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
        /// Route through the long-running surge-daemon (auto-spawn if not running).
        #[arg(long)]
        daemon: bool,
    },
    /// Tail events from an existing run by id.
    Watch {
        /// `RunId` (ULID).
        run_id: String,
        /// Subscribe to live events via the daemon. Without this flag
        /// the command reads from disk (M6 mode).
        #[arg(long)]
        daemon: bool,
    },
    /// Resume an interrupted run.
    Resume {
        /// `RunId` (ULID).
        run_id: String,
        /// Required — resume needs the engine to be alive (i.e., the daemon).
        #[arg(long)]
        daemon: bool,
    },
    /// Cancel a run.
    Stop {
        /// `RunId` (ULID).
        run_id: String,
        /// Reason string recorded in the abort event.
        #[arg(long)]
        reason: Option<String>,
        /// Required — cross-process stop needs the daemon.
        #[arg(long)]
        daemon: bool,
    },
    /// List runs.
    Ls {
        /// List runs the daemon currently hosts (default: list on-disk).
        #[arg(long)]
        daemon: bool,
    },
    /// Print events for a run (always reads from disk; daemon not required).
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
            daemon,
        } => run_command(spec_path, watch, worktree, daemon).await,
        EngineCommands::Watch { run_id, daemon } => watch_command(run_id, daemon).await,
        EngineCommands::Resume { run_id, daemon } => resume_command(run_id, daemon).await,
        EngineCommands::Stop {
            run_id,
            reason,
            daemon,
        } => stop_command(run_id, reason, daemon).await,
        EngineCommands::Ls { daemon } => ls_command(daemon).await,
        EngineCommands::Logs {
            run_id,
            since,
            follow,
        } => logs_command(run_id, since, follow).await,
    }
}

async fn run_command(
    spec_path: PathBuf,
    watch: bool,
    worktree: Option<PathBuf>,
    daemon: bool,
) -> Result<()> {
    use std::time::Duration;
    use surge_core::graph::Graph;
    use surge_orchestrator::engine::facade::EngineFacade;
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

    let facade: Arc<dyn EngineFacade> = if daemon {
        ensure_daemon_running().await?;
        let socket = surge_daemon::pidfile::socket_path()?;
        Arc::new(
            surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?,
        )
    } else {
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

        let engine = Arc::new(Engine::new_with_notifier(
            bridge,
            storage,
            tool_dispatcher,
            notifier,
            EngineConfig::default(),
        ));
        Arc::new(surge_orchestrator::engine::facade::LocalEngineFacade::new(
            engine,
        ))
    };

    let run_id = RunId::new();
    println!("{run_id}");

    let handle = facade
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

async fn watch_command(run_id: String, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        // Existing M6 disk-tail behavior preserved.
        follow_log_from(id, 0).await?;
        return Ok(());
    }

    // M7 daemon path: subscribe to per-run events and stream live.
    use std::time::Duration;
    use surge_orchestrator::engine::EngineError;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade =
        surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    let mut rx = match facade.subscribe_to_run(id).await {
        Ok(rx) => rx,
        Err(EngineError::RunNotActive(_)) => {
            // The run already terminated (or was never registered with
            // this daemon). Fall back to disk-replay so users still see
            // the run's history. This is the same "fast-run" race the
            // M7 polish #2 PR called out as out-of-scope: a run can
            // finish + deregister between the user observing the
            // RunId and `watch --daemon` arriving at the daemon.
            eprintln!(
                "run {id} is not currently active in the daemon; \
                 reading event history from disk instead."
            );
            follow_log_from(id, 0).await?;
            return Ok(());
        },
        Err(e) => return Err(e.into()),
    };

    eprintln!("watching {id} (Ctrl+C to stop)…");

    loop {
        match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Ok(event)) => {
                print_event(&event);
                if matches!(event, EngineRunEvent::Terminal(_)) {
                    break;
                }
            },
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                eprintln!("daemon closed the per-run channel; run may have terminated");
                break;
            },
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                eprintln!("note: dropped {n} events (subscriber lagged); continuing");
            },
            Err(_timeout) => continue, // 60s without events; keep waiting
        }
    }

    // Best-effort unsubscribe so the daemon stops pumping events to this
    // connection. If it fails (e.g., daemon died), we don't care.
    let _ = facade.unsubscribe_from_run(id).await;
    Ok(())
}

async fn resume_command(run_id: String, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        return Err(anyhow!(
            "resume requires --daemon (the engine must be alive to resume); \
             use `surge engine resume {id} --daemon`"
        ));
    }
    ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade =
        surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    let cwd = std::env::current_dir().context("cwd")?;
    use surge_orchestrator::engine::facade::EngineFacade;
    let _handle = facade.resume_run(id, cwd).await?;
    println!("resumed {id}");
    Ok(())
}

async fn stop_command(run_id: String, reason: Option<String>, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        return Err(anyhow!(
            "stop requires --daemon (cross-process cancel); \
             use `surge engine stop {id} --daemon`"
        ));
    }
    ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade =
        surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    use surge_orchestrator::engine::facade::EngineFacade;
    facade
        .stop_run(id, reason.unwrap_or_else(|| "user-requested".into()))
        .await?;
    println!("stopped {id}");
    Ok(())
}

async fn ls_command(daemon: bool) -> Result<()> {
    if daemon {
        ensure_daemon_running().await?;
        let socket = surge_daemon::pidfile::socket_path()?;
        use surge_orchestrator::engine::facade::EngineFacade;
        let facade =
            surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
        let runs = facade.list_runs().await?;
        println!("{:<32} {:<10} STARTED", "ID", "STATUS");
        for r in runs {
            println!(
                "{:<32} {:<10} {}",
                r.run_id,
                format!("{:?}", r.status).to_lowercase(),
                r.started_at.format("%Y-%m-%d %H:%M:%S")
            );
        }
        return Ok(());
    }
    legacy_ls_command().await
}

async fn legacy_ls_command() -> Result<()> {
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

/// If `--daemon` is requested but no daemon is running, auto-spawn
/// one. Idempotent if a daemon is already alive.
async fn ensure_daemon_running() -> Result<()> {
    use surge_daemon::pidfile;
    if let Some(p) = pidfile::read_pid(&pidfile::pid_path()?)? {
        if pidfile::is_alive(p) {
            return Ok(());
        }
    }
    eprintln!("note: daemon not running; auto-spawning…");
    crate::commands::daemon::run(crate::commands::daemon::DaemonCommands::Start {
        detached: true,
        max_active: 8,
    })
    .await
}
