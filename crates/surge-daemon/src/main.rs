//! `surge-daemon` binary entry point.

use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::AcpBridge;
use surge_daemon::{ServerConfig, lifecycle, pidfile, run_server};
use surge_orchestrator::engine::facade::LocalEngineFacade;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_persistence::runs::Storage;
use tokio_util::sync::CancellationToken;

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
            storage,
            tool_dispatcher,
            notifier,
            None, // PR 5 simplification: registry is per-run, populated when run starts (PR 6 polish)
            EngineConfig::default(),
        ));

        let facade: Arc<dyn surge_orchestrator::engine::facade::EngineFacade> =
            Arc::new(LocalEngineFacade::new(engine));

        // Write version file so the CLI can read the running daemon's version.
        if let Ok(path) = pidfile::version_path() {
            let _ = std::fs::write(path, env!("CARGO_PKG_VERSION"));
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
