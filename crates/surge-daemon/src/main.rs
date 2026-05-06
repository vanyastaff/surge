//! `surge-daemon` binary entry point.

use clap::Parser;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::AcpBridge;
use surge_core::config::TaskSourceConfig;
use surge_daemon::{ServerConfig, lifecycle, pidfile, run_server};
use surge_intake::TaskSource;
use surge_intake::github::source::{GitHubConfig, GitHubIssuesTaskSource};
use surge_intake::linear::source::{LinearConfig, LinearTaskSource};
use surge_intake::router::{RouterOutput, TaskRouter};
use surge_orchestrator::engine::facade::LocalEngineFacade;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_persistence::runs::Storage;
use tokio::sync::{Mutex as TokioMutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(version, about = "surge-daemon — long-running engine host")]
struct Args {
    /// Maximum concurrent active runs.
    #[arg(long, default_value_t = 8)]
    max_active: usize,
    /// Maximum runs allowed to wait in the FIFO admission queue.
    /// When omitted, defaults to `max_active * 4`. When both
    /// `max_active` and this cap are saturated, further `StartRun`
    /// requests are rejected with `QueueFull` so the daemon's
    /// pending-start map cannot grow unboundedly under load.
    #[arg(long)]
    max_queue: Option<usize>,
    /// Graceful-shutdown grace window.
    #[arg(long, default_value = "30s", value_parser = parse_humantime)]
    shutdown_grace: Duration,
    /// Detach from the controlling terminal (Unix: `setsid` already
    /// handled by the spawning CLI; this flag is currently a no-op
    /// inside the daemon process itself but reserved for future use).
    #[arg(long)]
    detached: bool,
}

fn parse_humantime(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Acquire PID lock before touching the runtime — failure exits cheaply.
    if let Err(e) = pidfile::acquire_lock(std::process::id()) {
        eprintln!("surge-daemon: {e}");
        return std::process::ExitCode::from(2);
    }

    let socket_path = match pidfile::socket_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("surge-daemon: socket_path: {e}");
            let _ = pidfile::release_lock();
            return std::process::ExitCode::from(2);
        },
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("surge-daemon: tokio runtime: {e}");
            let _ = pidfile::release_lock();
            return std::process::ExitCode::from(2);
        },
    };

    let exit = rt.block_on(async {
        let shutdown = CancellationToken::new();
        lifecycle::install_signal_handlers(shutdown.clone());

        // Storage::open returns Arc<Storage> directly.
        let storage = match Storage::open(&surge_runs_dir()).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("surge-daemon: storage: {e}");
                return 2u8;
            },
        };

        let bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade> =
            match AcpBridge::with_defaults() {
                Ok(b) => Arc::new(b),
                Err(e) => {
                    eprintln!("surge-daemon: bridge: {e}");
                    return 2u8;
                },
            };

        // The constructor argument is a historical hint; per-run worktree
        // resolution happens in dispatch via ToolDispatchContext::worktree_root.
        // A single dispatcher instance is safe to share across all runs.
        let tool_dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> = Arc::new(
            surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                std::path::PathBuf::new(),
            ),
        );

        // F3: match the CLI's default notifier — Desktop + Webhook deliverers wired.
        // A bare MultiplexingNotifier silently drops all notifications.
        let notifier: Arc<dyn surge_notify::NotifyDeliverer> = Arc::new(
            surge_notify::MultiplexingNotifier::new()
                .with_desktop(Arc::new(surge_notify::DesktopDeliverer::new()))
                .with_webhook(Arc::new(surge_notify::WebhookDeliverer::new())),
        );

        let engine = Arc::new(Engine::new_with_mcp(
            bridge,
            Arc::clone(&storage),
            tool_dispatcher,
            Arc::clone(&notifier),
            None, // PR 5 simplification: registry is per-run, populated when run starts (PR 6 polish)
            EngineConfig::default(),
        ));

        let facade: Arc<dyn surge_orchestrator::engine::facade::EngineFacade> =
            Arc::new(LocalEngineFacade::new(engine));

        // Write version file so the CLI can read the running daemon's version.
        if let Ok(path) = pidfile::version_path() {
            let _ = std::fs::write(path, env!("CARGO_PKG_VERSION"));
        }

        // --- Plan C T9.2: Load surge.toml and spawn TaskRouter ---
        let config = match surge_core::config::SurgeConfig::discover() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to load surge.toml; skipping TaskRouter spawn");
                surge_core::config::SurgeConfig::default()
            }
        };

        let mut sources: Vec<Arc<dyn TaskSource>> = Vec::new();
        let mut source_map: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
        for src_cfg in &config.task_sources {
            match src_cfg {
                TaskSourceConfig::Linear(l) => {
                    let token = env::var(&l.api_token_env).unwrap_or_default();
                    if token.is_empty() {
                        warn!(env = %l.api_token_env, "env not set; skipping Linear source");
                        continue;
                    }
                    let cfg = LinearConfig {
                        id: l.id.clone(),
                        display_name: format!("Linear · {}", l.workspace_id),
                        workspace_id: l.workspace_id.clone(),
                        api_token: token,
                        poll_interval: l.poll_interval,
                        label_filters: l.label_filters.clone(),
                    };
                    match LinearTaskSource::new(cfg) {
                        Ok(s) => {
                            let arc: Arc<dyn TaskSource> = Arc::new(s);
                            source_map.insert(arc.id().to_string(), Arc::clone(&arc));
                            sources.push(arc);
                        }
                        Err(e) => {
                            warn!(error = %e, source_id = %l.id, "failed to init Linear source");
                        }
                    }
                }
                TaskSourceConfig::GithubIssues(g) => {
                    let token = env::var(&g.api_token_env).unwrap_or_default();
                    if token.is_empty() {
                        warn!(env = %g.api_token_env, "env not set; skipping GitHub source");
                        continue;
                    }
                    let (owner, repo) = match g.repo.split_once('/') {
                        Some((o, r)) => (o.to_string(), r.to_string()),
                        None => {
                            warn!(repo = %g.repo, "invalid repo format (expected owner/repo); skipping");
                            continue;
                        }
                    };
                    let cfg = GitHubConfig {
                        id: g.id.clone(),
                        display_name: format!("GitHub · {}", g.repo),
                        owner,
                        repo,
                        api_token: token,
                        poll_interval: g.poll_interval,
                        label_filters: g.label_filters.clone(),
                    };
                    match GitHubIssuesTaskSource::new(cfg) {
                        Ok(s) => {
                            let arc: Arc<dyn TaskSource> = Arc::new(s);
                            source_map.insert(arc.id().to_string(), Arc::clone(&arc));
                            sources.push(arc);
                        }
                        Err(e) => {
                            warn!(error = %e, source_id = %g.id, "failed to init GitHub source");
                        }
                    }
                }
            }
        }

        let source_registry: Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>> =
            Arc::new(source_map);

        if !sources.is_empty() {
            spawn_task_router(
                sources,
                Arc::clone(&source_registry),
                Arc::clone(&notifier),
                Arc::clone(&storage),
            ).await;
        } else {
            info!("no task sources configured; skipping TaskRouter spawn");
        }

        spawn_inbox_subsystems(
            Arc::clone(&storage),
            Arc::clone(&source_registry),
            Arc::clone(&facade),
            &config,
            shutdown.clone(),
        ).await;

        let max_queue = args.max_queue.unwrap_or(args.max_active.saturating_mul(4));
        let server_cfg = ServerConfig {
            max_active: args.max_active,
            max_queue,
            socket_path: socket_path.clone(),
        };
        let server_handle = tokio::spawn({
            let facade = facade.clone();
            let shutdown_for_server = shutdown.clone();
            // F1: keep a second clone so that a server error (e.g. bind failure)
            // cancels the outer shutdown token — otherwise lifecycle::drain
            // waits forever holding the pid lock.
            let shutdown_for_cancel = shutdown.clone();
            async move {
                if let Err(e) = run_server(server_cfg, facade, shutdown_for_server).await {
                    tracing::error!(err = %e, "server exited with error; cancelling shutdown token");
                    shutdown_for_cancel.cancel();
                }
            }
        });

        // Wait for shutdown signal, then give forwarders the grace window.
        lifecycle::drain(shutdown, args.shutdown_grace).await;
        server_handle.abort();
        0u8
    });

    let _ = pidfile::release_lock();
    let _ = std::fs::remove_file(&socket_path);
    std::process::ExitCode::from(exit)
}

fn surge_runs_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".surge"))
        .unwrap_or_else(|| std::path::PathBuf::from(".surge"))
}

/// Spawn the TaskRouter and its output consumer.
async fn spawn_task_router(
    sources: Vec<Arc<dyn TaskSource>>,
    source_map: Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>>,
    _notifier: Arc<dyn surge_notify::NotifyDeliverer>,
    storage: Arc<Storage>,
) {
    use rusqlite::Connection;

    info!(
        count = sources.len(),
        "spawning TaskRouter for {} task sources",
        sources.len()
    );

    let (tx, mut rx) = mpsc::channel::<RouterOutput>(64);

    // Acquire a dedicated connection from the registry pool for the TaskRouter's
    // Tier-1 dedup queries. By opening directly to the same registry DB file that
    // the engine's pool uses, both the router and the engine's persistence layer
    // query the same persistent ticket_index. This ensures Tier-1 dedup state
    // survives daemon restarts and stays in sync with the engine's writes.
    let registry_db_path = storage.registry_db_path();
    let conn = match Connection::open(&registry_db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                error = %e,
                path = ?registry_db_path,
                "failed to open persistent registry DB for TaskRouter; intake disabled"
            );
            return;
        },
    };

    // Enable foreign keys for consistency with the registry pool's pragmas.
    if let Err(e) = conn.execute("PRAGMA foreign_keys = ON;", []) {
        tracing::error!(error = %e, "failed to enable foreign keys on dedup connection; intake disabled");
        return;
    }

    let conn_arc = Arc::new(TokioMutex::new(conn));
    let router = TaskRouter::new(sources, Arc::clone(&conn_arc), tx);

    tokio::spawn(async move {
        if let Err(e) = router.run().await {
            tracing::error!(error = %e, "task router exited with error");
        } else {
            info!("task router exited cleanly");
        }
    });

    // Consume router output and enqueue InboxCard payloads.
    let source_map_for_consumer = source_map;
    let storage_for_router = storage;
    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                surge_intake::router::RouterOutput::Triage { event } => {
                    let title = event
                        .raw_payload
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("New ticket")
                        .to_string();
                    let task_url = event
                        .raw_payload
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let provider = event
                        .task_id
                        .as_str()
                        .split(':')
                        .next()
                        .unwrap_or("unknown")
                        .to_string();
                    let callback_token = ulid::Ulid::new().to_string();
                    let payload = surge_notify::messages::InboxCardPayload {
                        task_id: event.task_id.clone(),
                        source_id: event.source_id.clone(),
                        provider,
                        title,
                        summary: String::new(),
                        priority: surge_intake::types::Priority::Medium,
                        task_url,
                        callback_token,
                    };
                    if let Err(e) =
                        surge_daemon::inbox::enqueue_inbox_card(&storage_for_router, &payload).await
                    {
                        tracing::warn!(error = %e, task_id = %event.task_id, "failed to enqueue inbox card");
                    } else {
                        tracing::info!(task_id = %event.task_id, "inbox card enqueued");
                    }
                },
                surge_intake::router::RouterOutput::EarlyDuplicate { event, run_id } => {
                    let source_id = event.source_id.clone();
                    let Some(source) = source_map_for_consumer.get(&source_id) else {
                        tracing::warn!(
                            source_id = %source_id,
                            "no source registered for this ID; cannot post duplicate comment"
                        );
                        continue;
                    };
                    let body = format!(
                        "Surge run #{run_id}: detected duplicate of an active run for this ticket. Skipping re-triage."
                    );
                    match source.post_comment(&event.task_id, &body).await {
                        Ok(()) => tracing::info!(
                            task_id = %event.task_id,
                            %run_id,
                            "posted duplicate comment to tracker"
                        ),
                        Err(e) => tracing::warn!(
                            error = %e,
                            task_id = %event.task_id,
                            %run_id,
                            "failed to post duplicate comment"
                        ),
                    }
                },
            }
        }
    });
}

async fn spawn_inbox_subsystems(
    storage: Arc<surge_persistence::runs::storage::Storage>,
    sources: Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>>,
    engine: Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
    config: &surge_core::config::SurgeConfig,
    shutdown: CancellationToken,
) {
    use surge_daemon::inbox::{
        consumer::InboxActionConsumer, desktop_listener::DesktopActionListener,
        snooze_scheduler::SnoozeScheduler, tg_bot::TgInboxBot,
    };
    use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, MinimalBootstrapGraphBuilder};

    let bootstrap: Arc<dyn BootstrapGraphBuilder> = Arc::new(MinimalBootstrapGraphBuilder::new());
    let worktrees_root = surge_runs_dir().join("worktrees");
    if let Err(e) = std::fs::create_dir_all(&worktrees_root) {
        tracing::warn!(error = %e, path = %worktrees_root.display(), "failed to create worktrees root");
    }

    // Consumer.
    let consumer = InboxActionConsumer {
        storage: Arc::clone(&storage),
        bootstrap: Arc::clone(&bootstrap),
        engine: Arc::clone(&engine),
        sources: Arc::clone(&sources),
        worktrees_root,
        poll_interval: std::time::Duration::from_millis(500),
    };
    let shutdown_for_consumer = shutdown.clone();
    tokio::spawn(consumer.run(shutdown_for_consumer));

    // Snooze scheduler.
    let scheduler = SnoozeScheduler {
        storage: Arc::clone(&storage),
        poll_interval: config.inbox.snooze_poll_interval,
    };
    let shutdown_for_scheduler = shutdown.clone();
    tokio::spawn(scheduler.run(shutdown_for_scheduler));

    // Desktop listener.
    let desktop = DesktopActionListener::new(Arc::clone(&storage));
    let shutdown_for_desktop = shutdown.clone();
    tokio::spawn(desktop.run(shutdown_for_desktop));

    // Telegram bot (only if config provides a chat ID + token).
    if let Some(tg_cfg) = config.telegram.as_ref() {
        let chat_id = tg_cfg.chat_id.or_else(|| {
            tg_cfg
                .chat_id_env
                .as_deref()
                .and_then(|env| std::env::var(env).ok().and_then(|s| s.parse::<i64>().ok()))
        });
        let token = tg_cfg
            .bot_token_env
            .as_deref()
            .and_then(|env| std::env::var(env).ok());
        match (chat_id, token) {
            (Some(chat_id), Some(token)) => {
                let bot = teloxide::Bot::new(token);
                let tg =
                    TgInboxBot::new(bot, teloxide::types::ChatId(chat_id), Arc::clone(&storage));
                let shutdown_for_tg = shutdown.clone();
                tokio::spawn(tg.run(shutdown_for_tg));
            },
            _ => {
                tracing::warn!(
                    "telegram config present but chat_id or bot_token missing — TgInboxBot not spawned"
                );
            },
        }
    } else {
        tracing::info!("no [telegram] config — TgInboxBot skipped");
    }
}
