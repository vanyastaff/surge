//! `surge engine` subtree — in-process M6 CLI for graph-based runs.

use anyhow::{Context, Result, anyhow};
use clap::{Subcommand, ValueEnum};
use owo_colors::{OwoColorize, Stream};
use std::path::PathBuf;
use surge_core::id::RunId;
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
        /// What you want done. Templates without a spec step (`single-task`,
        /// `feature`, `code-review`, ...) treat this as the spec.
        #[arg(long, short = 'p')]
        prompt: Option<String>,
        /// Stable operation identity for exact retries after disconnects.
        #[arg(long)]
        operation_id: Option<surge_core::id::WorkItemOperationId>,
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
            prompt,
            operation_id,
        } => {
            run_command(
                spec_path,
                template,
                watch,
                worktree,
                daemon,
                prompt,
                operation_id,
            )
            .await
        },
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
    prompt: Option<String>,
    operation_id: Option<surge_core::id::WorkItemOperationId>,
) -> Result<()> {
    use surge_orchestrator::engine::owned_flow::{
        FlowInput, OwnedFlowRunConfig, OwnedFlowStart, WorkspaceRequest,
    };
    let input = match (spec_path, template) {
        (Some(locator), None) => FlowInput::ProjectFile { locator },
        (None, Some(key)) => FlowInput::Template { key },
        (Some(_), Some(_)) => return Err(anyhow!("pass either SPEC_PATH or --template, not both")),
        (None, None) => return Err(anyhow!("provide SPEC_PATH or --template <name>")),
    };
    let source_project = std::env::current_dir().context("cwd")?;
    let workspace = worktree.map_or(WorkspaceRequest::Managed, |path| {
        WorkspaceRequest::Explicit { path }
    });
    let request = OwnedFlowStart {
        operation_id: operation_id.unwrap_or_default(),
        source_project,
        input,
        config: OwnedFlowRunConfig {
            initial_prompt: prompt.unwrap_or_default(),
            ..OwnedFlowRunConfig::default()
        },
        workspace,
    };
    let request = request.normalize()?;
    // Immutable intent reaches the owner before reading source files, project
    // configuration, templates, profiles or credential discovery.
    ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade =
        surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    let receipt = facade.owned_flow_start(request).await?;
    println!("{}", receipt.run);
    if daemon && !watch {
        return Ok(());
    }
    wait_owned_startup(receipt.run).await?;
    watch_command(receipt.run.to_string(), true).await
}

async fn wait_owned_startup(run: RunId) -> Result<()> {
    let storage = Storage::open(&surge_runs_dir()?)
        .await
        .context("open accepted run storage")?;
    loop {
        let inspected = storage.inspect_folded_run(run).await?;
        if inspected
            .database
            .is_some_and(|history| history.event_count > 0)
        {
            return Ok(());
        }
        let attempt = storage
            .work_items()
            .for_run(run)?
            .ok_or_else(|| anyhow!("accepted run is missing"))?;
        if !attempt.state.is_active() {
            return Err(anyhow!(
                "accepted run {} ended before startup: {:?}",
                run,
                attempt.state
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn watch_command(run_id: String, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        follow_log_from(id, 0).await?;
        return require_completed_history(id).await;
    }

    // M7 daemon path: subscribe to per-run events and stream live.
    use surge_orchestrator::engine::EngineError;
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
            return require_completed_history(id).await;
        },
        Err(e) => return Err(e.into()),
    };

    eprintln!("watching {id} (Ctrl+C to stop)…");

    let result = watch_daemon_events(id, &mut rx).await;
    // Always unsubscribe, including when delivery ends without a confirmed outcome.
    let _ = facade.unsubscribe_from_run(id).await;
    result
}

async fn watch_daemon_events(
    run_id: RunId,
    rx: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::handle::EngineRunEvent>,
) -> Result<()> {
    use std::time::Duration;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    loop {
        match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Ok(EngineRunEvent::StreamError { message })) => {
                return Err(anyhow!("run outcome unconfirmed: {message}"));
            },
            Ok(Ok(event)) => {
                print_event(&event);
                if let EngineRunEvent::Terminal { outcome } = event {
                    return super::run_lifecycle::require_completed(run_id, outcome);
                }
            },
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err(anyhow!(
                    "daemon event stream closed before terminal confirmation; run outcome unconfirmed"
                ));
            },
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                return Err(anyhow!(
                    "missed {n} daemon events; run observation is incomplete"
                ));
            },
            Err(_timeout) => continue, // 60s without events; keep waiting
        }
    }
}

async fn require_completed_history(run_id: RunId) -> Result<()> {
    use surge_persistence::runs::EventSeq;
    let storage = Storage::open(&surge_runs_dir()?).await?;
    let reader = storage.open_run_reader(run_id).await?;
    let events = reader.read_events(EventSeq(0)..EventSeq(u64::MAX)).await?;
    require_completed_events(run_id, &events)
}

fn require_completed_events(
    run_id: RunId,
    events: &[surge_persistence::runs::reader::ReadEvent],
) -> Result<()> {
    use surge_core::run_event::EventPayload;
    use surge_orchestrator::engine::handle::RunOutcome;
    let mut outcome = None;
    for event in events {
        let candidate = match event.payload.payload() {
            EventPayload::RunCompleted { terminal_node } => RunOutcome::Completed {
                terminal: terminal_node.clone(),
            },
            EventPayload::RunFailed { error } => RunOutcome::Failed {
                error: error.clone(),
            },
            EventPayload::RunAborted { reason } => RunOutcome::Aborted {
                reason: reason.clone(),
            },
            _ => continue,
        };
        if outcome.replace(candidate).is_some() {
            return Err(anyhow!(
                "run {run_id} has conflicting terminal history; completion is unconfirmed"
            ));
        }
    }
    let outcome = outcome.ok_or_else(|| anyhow!(
        "run {run_id} has no durable terminal outcome; completion is unconfirmed; inspect `surge engine replay {run_id}`"
    ))?;
    super::run_lifecycle::require_completed(run_id, outcome)
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
    let storage = Storage::open(&surge_runs_dir()?).await?;
    let summary = storage
        .get_run(&id)
        .await?
        .ok_or_else(|| anyhow!("run {id} was not found"))?;
    if !summary.project_path.is_dir() {
        return Err(anyhow!(
            "recorded worktree is missing: {}",
            summary.project_path.display()
        ));
    }
    use surge_orchestrator::engine::facade::EngineFacade;
    let _handle = facade.resume_run(id, summary.project_path).await?;
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

/// List runs from the on-disk run registry (daemon not required).
///
/// `surge_runs_dir()` returns the surge *home* directory, so listing its
/// entries directly would print `daemon/`, `db/`, `profiles/`… instead of
/// runs. The registry is the authoritative index and also carries status.
async fn legacy_ls_command() -> Result<()> {
    use surge_persistence::runs::RunFilter;

    let storage = Storage::open(&surge_runs_dir()?).await?;
    let runs = storage.list_runs(RunFilter::default()).await?;
    print!("{}", render_run_table(&runs));
    Ok(())
}

/// Render registry rows as the `surge engine ls` table, newest first.
fn render_run_table(runs: &[surge_persistence::runs::RunSummary]) -> String {
    use std::fmt::Write as _;

    let mut out = format!("{:<32} {:<12} STARTED\n", "ID", "STATUS");
    for r in runs {
        let started = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(r.started_at_ms)
            .map_or_else(
                || "?".to_string(),
                |t| t.format("%Y-%m-%d %H:%M:%S").to_string(),
            );
        let _ = writeln!(
            out,
            "{:<32} {:<12} {started}",
            r.id.to_string(),
            r.status.as_str()
        );
    }
    out
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
    let worktrees = storage.home().join("worktrees");
    tokio::fs::create_dir_all(&worktrees).await?;
    let destination = tokio::fs::canonicalize(&worktrees)
        .await?
        .join(child.to_string());
    let outcome = fork(
        &storage,
        ForkRequest::new(parent, child, seq)
            .with_edits(edits)
            .with_worktree(destination.clone()),
    )
    .await?;

    println!("forked {parent} @ seq {seq}");
    println!("  new run:       {}", outcome.new_run);
    println!("  events copied: {}", outcome.copied_events);
    println!("  worktree:      {}", destination.display());
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
    if let Some(p) = pidfile::read_pid(&pidfile::pid_path()?)?
        && pidfile::is_alive(p)
    {
        return Ok(());
    }
    eprintln!("note: daemon not running; auto-spawning…");
    crate::commands::daemon::run(crate::commands::daemon::DaemonCommands::Start {
        detached: true,
        max_active: 8,
    })
    .await
}

#[cfg(test)]
mod watch_tests {
    use super::watch_daemon_events;
    use surge_orchestrator::engine::handle::{EngineRunEvent, RunOutcome};

    fn history(
        payloads: Vec<surge_core::run_event::EventPayload>,
    ) -> Vec<surge_persistence::runs::reader::ReadEvent> {
        payloads
            .into_iter()
            .enumerate()
            .map(
                |(index, payload)| surge_persistence::runs::reader::ReadEvent {
                    seq: surge_persistence::runs::EventSeq(index as u64 + 1),
                    timestamp_ms: 0,
                    kind: payload.discriminant_str().into(),
                    payload: surge_core::run_event::VersionedEventPayload::new(payload),
                },
            )
            .collect()
    }

    #[test]
    fn disk_history_requires_one_successful_terminal() {
        use surge_core::run_event::EventPayload;
        let run = surge_core::id::RunId::new();
        let completed = EventPayload::RunCompleted {
            terminal_node: "end".try_into().unwrap(),
        };
        super::require_completed_events(run, &history(vec![completed.clone()])).unwrap();
        for (events, diagnostic) in [
            (vec![], "no durable terminal"),
            (
                vec![EventPayload::RunFailed {
                    error: "agent failed".into(),
                }],
                "agent failed",
            ),
            (
                vec![EventPayload::RunAborted {
                    reason: "operator cancelled".into(),
                }],
                "operator cancelled",
            ),
            (
                vec![
                    completed.clone(),
                    EventPayload::RunFailed {
                        error: "conflict".into(),
                    },
                ],
                "conflicting",
            ),
            (vec![completed.clone(), completed], "conflicting"),
        ] {
            let error = super::require_completed_events(run, &history(events)).unwrap_err();
            assert!(error.to_string().contains(diagnostic), "{error}");
        }
    }

    #[tokio::test]
    async fn stream_error_is_not_successful_observation() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(2);
        tx.send(EngineRunEvent::StreamError {
            message: "durable catch-up failed".into(),
        })
        .unwrap();
        drop(tx);
        let error = watch_daemon_events(surge_core::id::RunId::new(), &mut rx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("durable catch-up failed"));
    }

    #[tokio::test]
    async fn closed_stream_without_terminal_is_unconfirmed() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(2);
        drop(tx);
        let error = watch_daemon_events(surge_core::id::RunId::new(), &mut rx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unconfirmed"));
    }

    #[tokio::test]
    async fn lost_events_do_not_become_success_after_a_terminal() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        for _ in 0..3 {
            tx.send(EngineRunEvent::Terminal {
                outcome: RunOutcome::Failed {
                    error: "agent failed".into(),
                },
            })
            .unwrap();
        }
        drop(tx);
        let error = watch_daemon_events(surge_core::id::RunId::new(), &mut rx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missed"));
    }

    #[tokio::test]
    async fn explicit_terminal_confirms_observation() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tx.send(EngineRunEvent::Terminal {
            outcome: RunOutcome::Completed {
                terminal: "end".try_into().unwrap(),
            },
        })
        .unwrap();
        drop(tx);
        watch_daemon_events(surge_core::id::RunId::new(), &mut rx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn failed_terminal_is_not_a_successful_command() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tx.send(EngineRunEvent::Terminal {
            outcome: RunOutcome::Failed {
                error: "agent failed".into(),
            },
        })
        .unwrap();
        drop(tx);
        let error = watch_daemon_events(surge_core::id::RunId::new(), &mut rx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("agent failed"));
    }

    #[tokio::test]
    async fn aborted_terminal_is_not_a_successful_command() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tx.send(EngineRunEvent::Terminal {
            outcome: RunOutcome::Aborted {
                reason: "operator cancelled".into(),
            },
        })
        .unwrap();
        drop(tx);
        let error = watch_daemon_events(surge_core::id::RunId::new(), &mut rx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("operator cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::render_run_table;
    use std::path::PathBuf;
    use surge_core::id::RunId;
    use surge_core::run_status::RunStatus;
    use surge_persistence::runs::RunSummary;

    #[test]
    fn run_table_lists_registry_runs_with_status_not_home_dirs() {
        let id = RunId::new();
        let runs = vec![RunSummary {
            id,
            project_path: PathBuf::from("/tmp/p"),
            pipeline_template: None,
            status: RunStatus::Failed,
            started_at_ms: 1_700_000_000_000,
            ended_at_ms: Some(1_700_000_000_500),
            daemon_pid: None,
            wake_at_ms: None,
        }];
        let table = render_run_table(&runs);
        let mut lines = table.lines();
        assert!(lines.next().unwrap().starts_with("ID"));
        let row = lines.next().unwrap();
        assert!(row.starts_with(&id.to_string()), "{row}");
        assert!(row.contains("failed"), "{row}");
        assert!(row.contains("2023-11-14 22:13:20"), "{row}");
        assert!(!table.contains("profiles"));
    }
}
