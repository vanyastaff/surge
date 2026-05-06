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

        if !sources.is_empty() {
            spawn_task_router(sources, source_map, Arc::clone(&notifier), Arc::clone(&storage)).await;
        } else {
            info!("no task sources configured; skipping TaskRouter spawn");
        }

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

/// Deliver a Medium-priority placeholder InboxCard for `event`.
///
/// Used as the fallback path when (a) the source registry doesn't
/// know `event.source_id`, (b) `source.fetch_task` fails, or (c)
/// `dispatch_triage` returns `TriageError`. Preserves Plan-C-MVP
/// behaviour for unrecoverable provider errors.
#[allow(dead_code)] // wired in T11
async fn deliver_fallback_inbox(
    notifier: &Arc<dyn surge_notify::NotifyDeliverer>,
    event: &surge_intake::types::TaskEvent,
) {
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
    let run_id_str = ulid::Ulid::new().to_string();
    let payload = surge_notify::messages::InboxCardPayload {
        task_id: event.task_id.clone(),
        source_id: event.source_id.clone(),
        provider,
        title,
        summary: String::new(),
        priority: surge_intake::types::Priority::Medium,
        task_url,
        run_id: run_id_str.clone(),
    };
    let rendered_desktop = surge_notify::desktop::format_inbox_card_desktop(&payload);
    let rendered = surge_notify::RenderedNotification {
        severity: surge_core::notify_config::NotifySeverity::Info,
        title: rendered_desktop.title.clone(),
        body: rendered_desktop.body.clone(),
        artifact_paths: vec![],
    };
    let run_id = match run_id_str.parse::<surge_core::id::RunId>() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse run_id; skipping fallback delivery");
            return;
        },
    };
    let node_key = match surge_core::keys::NodeKey::try_new("intake") {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(error = %e, "failed to construct intake NodeKey");
            return;
        },
    };
    let channel = surge_core::notify_config::NotifyChannel::Desktop;
    let ctx = surge_notify::NotifyDeliveryContext {
        run_id,
        node: &node_key,
    };
    match notifier.deliver(&ctx, &channel, &rendered).await {
        Ok(()) => tracing::info!(task_id = %event.task_id, "fallback InboxCard delivered"),
        Err(surge_notify::NotifyError::ChannelNotConfigured) => {
            tracing::debug!(task_id = %event.task_id, "Desktop channel not configured")
        },
        Err(e) => tracing::warn!(error = %e, task_id = %event.task_id, "fallback delivery failed"),
    }
}

/// Spawn the TaskRouter and its output consumer (placeholder for T9.3).
async fn spawn_task_router(
    sources: Vec<Arc<dyn TaskSource>>,
    source_map: HashMap<String, Arc<dyn TaskSource>>,
    notifier: Arc<dyn surge_notify::NotifyDeliverer>,
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

    // Consume router output and build InboxCard payloads (T9.3).
    let source_map_for_consumer = Arc::new(source_map);
    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                surge_intake::router::RouterOutput::Triage { event } => {
                    // MVP placeholder: build an InboxCardPayload and log it.
                    // Triage Author LLM dispatch + actual delivery are Plan-C-polish.
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
                    let run_id_str = ulid::Ulid::new().to_string();
                    let payload = surge_notify::messages::InboxCardPayload {
                        task_id: event.task_id.clone(),
                        source_id: event.source_id.clone(),
                        provider: provider.clone(),
                        title: title.clone(),
                        summary: String::new(),
                        priority: surge_intake::types::Priority::Medium,
                        task_url,
                        run_id: run_id_str.clone(),
                    };
                    let msg = surge_notify::messages::NotifyMessage::InboxCard(payload.clone());
                    tracing::info!(
                        task_id = %event.task_id,
                        "router output: triage → InboxCard built"
                    );

                    // Render to RenderedNotification using the desktop formatter
                    let rendered_desktop =
                        surge_notify::desktop::format_inbox_card_desktop(&payload);
                    let rendered = surge_notify::RenderedNotification {
                        severity: surge_core::notify_config::NotifySeverity::Info,
                        title: rendered_desktop.title.clone(),
                        body: rendered_desktop.body.clone(),
                        artifact_paths: vec![],
                    };

                    // Parse run_id and construct NodeKey for delivery context
                    let run_id = match run_id_str.parse::<surge_core::id::RunId>() {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to parse run_id as RunId; skipping delivery");
                            continue;
                        },
                    };
                    let node_key = match surge_core::keys::NodeKey::try_new("intake") {
                        Ok(key) => key,
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to construct intake NodeKey; skipping delivery");
                            continue;
                        },
                    };

                    // Attempt delivery through Desktop channel
                    let channel = surge_core::notify_config::NotifyChannel::Desktop;
                    let ctx = surge_notify::NotifyDeliveryContext {
                        run_id,
                        node: &node_key,
                    };
                    match notifier.deliver(&ctx, &channel, &rendered).await {
                        Ok(()) => {
                            tracing::info!(task_id = %event.task_id, "InboxCard delivered to Desktop")
                        },
                        Err(surge_notify::NotifyError::ChannelNotConfigured) => {
                            tracing::debug!(task_id = %event.task_id, "Desktop channel not configured; skipping");
                        },
                        Err(e) => {
                            tracing::warn!(error = %e, task_id = %event.task_id, "InboxCard delivery to Desktop failed")
                        },
                    }
                    let _ = msg; // keep msg in scope for now
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
