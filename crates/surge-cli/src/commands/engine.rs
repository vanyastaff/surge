//! `surge engine` subtree — in-process M6 CLI for graph-based runs.

use anyhow::{Context, Result, anyhow};
use clap::{Subcommand, ValueEnum};
use owo_colors::{OwoColorize, Stream};
use std::path::PathBuf;
use std::sync::Arc;
use surge_core::SurgeConfig;
use surge_core::id::RunId;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_persistence::runs::Storage;

/// Output format for read-only inspection commands.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text (default).
    Text,
    /// Machine-readable JSON.
    Json,
}

/// Subcommands under `surge engine`.
#[derive(Subcommand, Debug)]
pub enum EngineCommands {
    /// Start a new run from a flow.toml graph.
    Run {
        /// Path to the flow.toml file. Omit when using --template.
        spec_path: Option<PathBuf>,
        /// Bundled or user archetype template name. Skips bootstrap.
        #[arg(long)]
        template: Option<String>,
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
    /// Print the folded run state at a given event seq (CLI mirror of the
    /// replay scrubber; always reads from disk, daemon not required).
    Replay {
        /// `RunId` (ULID).
        run_id: String,
        /// Fold events up to and including this seq (default: latest).
        #[arg(long)]
        seq: Option<u64>,
        /// Output format (`text` or `json`).
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Fork a run at a given seq into a fresh run: copy events `1..=seq`
    /// (inheriting the parent snapshot so the child resumes at the fork
    /// point) and record `ForkCreated` lineage on the parent.
    Fork {
        /// Parent `RunId` (ULID) to fork from.
        run_id: String,
        /// Inclusive event seq to fork at (events `1..=seq` are inherited).
        #[arg(long)]
        seq: u64,
        /// Append text to an Agent node's system prompt in the fork,
        /// `--prompt <node>=<text>` (repeatable).
        #[arg(long = "prompt", value_name = "NODE=TEXT")]
        prompt: Vec<String>,
        /// Replace an Agent node's profile in the fork,
        /// `--profile <node>=<key>` (repeatable).
        #[arg(long = "profile", value_name = "NODE=KEY")]
        profile: Vec<String>,
    },
}

/// Top-level dispatcher for `surge engine` invocations.
pub async fn run(command: EngineCommands) -> Result<()> {
    match command {
        EngineCommands::Run {
            spec_path,
            template,
            watch,
            worktree,
            daemon,
        } => run_command(spec_path, template, watch, worktree, daemon).await,
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
        EngineCommands::Replay {
            run_id,
            seq,
            format,
        } => replay_command(run_id, seq, format).await,
        EngineCommands::Fork {
            run_id,
            seq,
            prompt,
            profile,
        } => fork_command(run_id, seq, prompt, profile).await,
    }
}

async fn run_command(
    spec_path: Option<PathBuf>,
    template: Option<String>,
    watch: bool,
    worktree: Option<PathBuf>,
    daemon: bool,
) -> Result<()> {
    use std::time::Duration;
    use surge_core::graph::Graph;
    use surge_orchestrator::engine::facade::EngineFacade;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    let graph: Graph = match (spec_path, template) {
        (Some(_), Some(template)) => {
            return Err(anyhow!(
                "pass either SPEC_PATH or --template {template}, not both; either form skips bootstrap"
            ));
        },
        (Some(path), None) => {
            let toml_text = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            toml::from_str(&toml_text).with_context(|| format!("parse {}", path.display()))?
        },
        (None, Some(template)) => {
            let registry = surge_orchestrator::archetype_registry::ArchetypeRegistry::load()
                .context("load archetype registry")?;
            let resolved = registry
                .resolve(&template)
                .with_context(|| format!("resolve template {template:?}"))?;
            eprintln!(
                "using template {} ({:?})",
                resolved.name, resolved.provenance
            );
            resolved.graph
        },
        (None, None) => {
            return Err(anyhow!(
                "provide SPEC_PATH or --template <name>; both forms skip bootstrap"
            ));
        },
    };

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

        // Load the profile registry on engine startup so agent stages
        // resolve `agent_config.profile` through it instead of the
        // M5 mock-only fallback. A registry-load failure is a hard
        // error here: production CLI runs should never silently fall
        // back to mocking.
        let profile_registry = Arc::new(
            surge_orchestrator::profile_loader::ProfileRegistry::load()
                .context("load profile registry")?,
        );

        let engine = Arc::new(Engine::new_full(
            bridge,
            storage,
            tool_dispatcher,
            notifier,
            None, // mcp_registry: not wired in the CLI in-process path
            Some(profile_registry),
            EngineConfig::default(),
        ));
        Arc::new(surge_orchestrator::engine::facade::LocalEngineFacade::new(
            engine,
        ))
    };

    let run_id = RunId::new();
    println!("{run_id}");

    let app_config =
        SurgeConfig::discover_from(&worktree_path).context("load surge config for worktree")?;
    let mut run_config = surge_orchestrator::project_context::with_project_context_seed(
        EngineRunConfig::default(),
        &worktree_path,
        &app_config,
    );
    // Freeze the operator's [analytics] budget into the run so the engine
    // enforces it at every stage boundary (warn → abort by default).
    run_config.budget = app_config.analytics.budget_guard();

    let handle = facade
        .start_run(run_id, graph, worktree_path, run_config)
        .await?;

    if watch {
        let mut rx = handle.events;
        loop {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
                Ok(Ok(event)) => {
                    print_event(&event);
                    if matches!(event, EngineRunEvent::Terminal { .. }) {
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
            // No per-run channel in the daemon for this id. Three
            // different states present the same way on the wire:
            //   1. The run already terminated (M7 fast-run case).
            //   2. The run is queued, awaiting admission (no events
            //      have been persisted yet — `follow_log_from` will
            //      also fail because the per-run DB doesn't exist
            //      until `Engine::start_run` runs).
            //   3. The run was never hosted by this daemon.
            // Try the disk-replay fallback: it covers (1) cleanly,
            // and emits a clear error for (2)/(3) so the user knows
            // to retry later or check the run id.
            eprintln!(
                "run {id} is not currently active in the daemon; \
                 attempting to read event history from disk."
            );
            follow_log_from(id, 0).await.with_context(|| {
                format!(
                    "reading events for {id} from disk (the run may be queued \
                     and not yet admitted, or unknown to this daemon)"
                )
            })?;
            return Ok(());
        },
        Err(e) => return Err(e.into()),
    };

    eprintln!("watching {id} (Ctrl+C to stop)…");

    loop {
        match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Ok(event)) => {
                print_event(&event);
                if matches!(event, EngineRunEvent::Terminal { .. }) {
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

/// `surge engine replay <run_id> --seq N` — fold the event log up to seq
/// `N` and print the resulting run state. CLI mirror of the replay
/// scrubber over the same fold primitive the engine/cockpit use.
async fn replay_command(run_id: String, seq: Option<u64>, format: OutputFormat) -> Result<()> {
    use surge_core::run_event::EventPayload;
    use surge_orchestrator::engine::build_replay_view;
    use surge_persistence::runs::{EventSeq, RunReader, aggregate_status};

    let id = parse_run_id(&run_id)?;
    let storage = Storage::open(&surge_runs_dir()?).await?;
    let reader: RunReader = storage.open_run_reader(id).await?;

    // `read_events` end is exclusive, so include seq N by reading up to N+1.
    let cutoff = seq.unwrap_or(u64::MAX);
    let end = if cutoff == u64::MAX {
        EventSeq(u64::MAX)
    } else {
        EventSeq(cutoff.saturating_add(1))
    };
    let events = reader.read_events(EventSeq(0)..end).await?;
    let snap = aggregate_status(id, &events);

    // The graph the run folds to at the cutoff: the last graph-bearing event.
    let graph = events.iter().rev().find_map(|e| match e.payload.payload() {
        EventPayload::PipelineMaterialized { graph, .. }
        | EventPayload::GraphRevisionAccepted { graph, .. } => Some((**graph).clone()),
        _ => None,
    });
    let view = graph.as_ref().map(|g| build_replay_view(g, &events));

    let seq_label = if cutoff == u64::MAX {
        "latest".to_string()
    } else {
        cutoff.to_string()
    };

    if matches!(format, OutputFormat::Json) {
        let seq_cutoff = if cutoff == u64::MAX {
            serde_json::Value::String("latest".into())
        } else {
            serde_json::json!(cutoff)
        };
        let json = serde_json::json!({
            "run": id.to_string(),
            "seq_cutoff": seq_cutoff,
            "events_folded": snap.event_count,
            "active_node": snap.active_node,
            "last_outcome": snap.last_outcome,
            "attempt": snap.last_attempt,
            "terminal": snap.terminal,
            "failed": snap.failed,
            "elapsed_ms": snap.elapsed_ms,
            "view": view,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    println!("run:           {id}");
    println!("seq cutoff:    {seq_label}");
    println!("events folded: {}", snap.event_count);
    println!(
        "active node:   {}",
        snap.active_node.as_deref().unwrap_or("-")
    );
    println!(
        "last outcome:  {}",
        snap.last_outcome.as_deref().unwrap_or("-")
    );
    println!(
        "attempt:       {}",
        snap.last_attempt
            .map_or_else(|| "-".to_string(), |a| a.to_string())
    );
    let terminal = if snap.terminal {
        if snap.failed { "yes (failed)" } else { "yes" }
    } else {
        "no"
    };
    println!("terminal:      {terminal}");
    if let Some(ms) = snap.elapsed_ms {
        println!("elapsed:       {ms} ms");
    }

    if let Some(view) = &view {
        println!(
            "cost:          {}+{} tok ({} cached), ${:.4}",
            view.cost.prompt_tokens,
            view.cost.output_tokens,
            view.cost.cache_hits,
            view.cost.cost_usd
        );
        println!("nodes:");
        for n in &view.nodes {
            let attempt = if n.attempts > 0 {
                format!(", attempt {}", n.attempts)
            } else {
                String::new()
            };
            let outcome = n
                .last_outcome
                .as_deref()
                .map(|o| format!(", outcome: {o}"))
                .unwrap_or_default();
            println!("  [{:<9}] {}{attempt}{outcome}", n.status.as_str(), n.node);
        }
        if !view.edges_traversed.is_empty() {
            println!("edges:");
            for e in &view.edges_traversed {
                println!("  {} --{}--> {}", e.from, e.edge, e.to);
            }
        }
    }
    Ok(())
}

/// `surge engine fork <run_id> --seq N [--prompt node=text] [--profile node=key]`
/// — copy the parent's event history `1..=N` into a fresh run (inheriting the
/// snapshot so it resumes at the fork point), optionally rewriting an Agent
/// node's prompt/profile in the child's graph, and record `ForkCreated` lineage
/// on the parent. The fork is inspectable via `surge engine replay <new_id>`
/// and resumable via `surge engine resume <new_id> --daemon`.
async fn fork_command(
    run_id: String,
    seq: u64,
    prompt: Vec<String>,
    profile: Vec<String>,
) -> Result<()> {
    use surge_core::keys::{NodeKey, ProfileKey};
    use surge_orchestrator::engine::fork::{ForkEdits, ForkRequest, fork};

    let parent = parse_run_id(&run_id)?;

    let mut edits = ForkEdits::default();
    for item in &prompt {
        let (node, text) = item
            .split_once('=')
            .ok_or_else(|| anyhow!("--prompt must be NODE=TEXT, got '{item}'"))?;
        let key = NodeKey::try_from(node).map_err(|e| anyhow!("invalid node '{node}': {e}"))?;
        edits.prompt_appends.insert(key, text.to_string());
    }
    for item in &profile {
        let (node, prof) = item
            .split_once('=')
            .ok_or_else(|| anyhow!("--profile must be NODE=KEY, got '{item}'"))?;
        let key = NodeKey::try_from(node).map_err(|e| anyhow!("invalid node '{node}': {e}"))?;
        let pkey =
            ProfileKey::try_from(prof).map_err(|e| anyhow!("invalid profile key '{prof}': {e}"))?;
        edits.profile_overrides.insert(key, pkey);
    }

    let storage = Storage::open(&surge_runs_dir()?).await?;
    let child = RunId::new();
    let outcome = fork(
        &storage,
        ForkRequest::new(parent, child, seq).with_edits(edits),
    )
    .await?;

    println!("forked {parent} @ seq {seq}");
    println!("  new run:       {}", outcome.new_run);
    println!("  events copied: {}", outcome.copied_events);
    if !prompt.is_empty() || !profile.is_empty() {
        println!(
            "  edits applied: {} prompt, {} profile",
            prompt.len(),
            profile.len()
        );
    }
    println!();
    println!("inspect:  surge engine replay {}", outcome.new_run);
    println!("resume:   surge engine resume {} --daemon", outcome.new_run);
    Ok(())
}

fn parse_run_id(s: &str) -> Result<RunId> {
    s.parse().map_err(|e| anyhow!("invalid run id '{s}': {e}"))
}

fn surge_runs_dir() -> Result<PathBuf> {
    // `SURGE_HOME`, when set and non-empty, IS the surge home dir itself
    // (matching `feature::surge_home_dir` and `profile_loader::paths::surge_home`);
    // otherwise fall back to `~/.surge`. It gives the durability harness an
    // isolated, cross-platform sandbox — `dirs::home_dir()` on Windows reads a
    // Win32 known-folder and ignores HOME/USERPROFILE overrides.
    let surge_home = match std::env::var("SURGE_HOME") {
        Ok(custom) if !custom.is_empty() => PathBuf::from(custom),
        _ => dirs::home_dir()
            .ok_or_else(|| anyhow!("SURGE_HOME unset and home directory unknown"))?
            .join(".surge"),
    };
    let runs = surge_home.join("runs");
    std::fs::create_dir_all(&runs).with_context(|| format!("create {}", runs.display()))?;
    // Storage::open expects the surge-home dir (parent of runs/), which it
    // populates with the runs/ subdir itself.
    Ok(surge_home)
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
            match payload.as_ref() {
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
        EngineRunEvent::Terminal { outcome } => {
            let label = "Terminal:".if_supports_color(Stream::Stderr, |s| s.yellow());
            eprintln!("{label} {outcome:?}");
        },
        _ => {},
    }
}

/// If `--daemon` is requested but no daemon is running, auto-spawn
/// one. Idempotent if a daemon is already alive.
pub(crate) async fn ensure_daemon_running() -> Result<()> {
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
