//! `surge-daemon` binary entry point.

use clap::Parser;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::AcpBridge;
use surge_core::config::TaskSourceConfig;
use surge_daemon::broadcast::BroadcastRegistry;
use surge_daemon::{ServerConfig, intake_completion, lifecycle, pidfile, run_with_registry};
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

        // Load the profile registry once at daemon startup so every run
        // resolves agent_config.profile through it. A failure here is a
        // hard error: a daemon without a registry would silently drop
        // back to mocking every agent stage.
        let profile_registry = Arc::new(
            surge_orchestrator::profile_loader::ProfileRegistry::load().unwrap_or_else(|e| {
                tracing::error!(error = %e, "profile registry failed to load; daemon shutting down");
                std::process::exit(2);
            }),
        );

        let engine = Arc::new(Engine::new_full(
            Arc::clone(&bridge),
            Arc::clone(&storage),
            tool_dispatcher,
            Arc::clone(&notifier),
            None, // PR 5 simplification: registry is per-run, populated when run starts (PR 6 polish)
            Some(profile_registry),
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

        // The broadcast registry is owned by main so that the run-completion
        // consumer (RFC-0010 acceptance #5) can subscribe to global events
        // alongside the wire-level subscribers handled by `run_with_registry`.
        let broadcast_registry = Arc::new(BroadcastRegistry::new());
        let completion_rx = broadcast_registry.subscribe_global();

        if !sources.is_empty() {
            if let Some((source_map_arc, conn_arc)) = spawn_task_router(
                sources,
                Arc::clone(&source_registry),
                Arc::clone(&notifier),
                Arc::clone(&storage),
                Arc::clone(&bridge),
                Arc::clone(&facade),
            )
            .await
            {
                intake_completion::spawn(completion_rx, source_map_arc, conn_arc);
            } else {
                info!("intake disabled; run-completion → tracker-comment hook not started");
            }
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
            let broadcast = Arc::clone(&broadcast_registry);
            async move {
                if let Err(e) =
                    run_with_registry(server_cfg, facade, broadcast, shutdown_for_server).await
                {
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

/// Deliver a Medium-priority placeholder InboxCard for `event` via the inbox queue.
///
/// Used as the fallback path when (a) the source registry doesn't
/// know `event.source_id`, (b) `source.fetch_task` fails, or (c)
/// `dispatch_triage` returns `TriageError`. Enqueues onto
/// `inbox_delivery_queue` so the bot/desktop legs pick it up — the
/// same path the LLM-Enqueued case uses.
async fn deliver_fallback_inbox(storage: &Arc<Storage>, event: &surge_intake::types::TaskEvent) {
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
    if let Err(e) = surge_daemon::inbox::enqueue_inbox_card(storage, &payload).await {
        tracing::warn!(error = %e, task_id = %event.task_id, "fallback inbox enqueue failed");
    } else {
        tracing::info!(task_id = %event.task_id, "fallback InboxCard enqueued");
    }
}

/// Deliver a rendered notification through the Desktop channel.
///
/// Shared by the Enqueued and Unclear paths to avoid repeating the
/// `node_key` / channel-construction boilerplate inline. Callers
/// supply the `run_id` so it stays consistent with whatever id is
/// embedded in the rendered card (e.g., `InboxCardPayload.run_id`).
async fn deliver_desktop(
    notifier: &Arc<dyn surge_notify::NotifyDeliverer>,
    task_id: &surge_intake::types::TaskId,
    run_id: surge_core::id::RunId,
    rendered: surge_notify::RenderedNotification,
) {
    let node_key = match surge_core::keys::NodeKey::try_new("intake") {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(error = %e, "skipping desktop delivery: bad NodeKey");
            return;
        },
    };
    let channel = surge_core::notify_config::NotifyChannel::Desktop;
    let ctx = surge_notify::NotifyDeliveryContext {
        run_id,
        node: &node_key,
    };
    match notifier.deliver(&ctx, &channel, &rendered).await {
        Ok(()) => tracing::info!(task_id = %task_id, "desktop notification delivered"),
        Err(surge_notify::NotifyError::ChannelNotConfigured) => {
            tracing::debug!(task_id = %task_id, "Desktop channel not configured")
        },
        Err(e) => tracing::warn!(error = %e, task_id = %task_id, "desktop delivery failed"),
    }
}

/// Handle a single `RouterOutput::Triage` event end-to-end.
///
/// Pipeline: source lookup → `fetch_task` → candidate assembly →
/// active-runs snapshot → `dispatch_triage` → decision routing.
///
/// Always delivers some signal to the user — either an InboxCard
/// (Enqueued or fallback), a tracker comment (Duplicate / OOS),
/// or an Unclear notification. Errors at any step are logged and
/// fall back to a Medium-priority placeholder InboxCard.
async fn handle_triage_event(
    event: surge_intake::types::TaskEvent,
    source_map: &std::collections::HashMap<String, Arc<dyn TaskSource>>,
    notifier: &Arc<dyn surge_notify::NotifyDeliverer>,
    storage: &Arc<Storage>,
    bridge: &Arc<dyn surge_acp::bridge::facade::BridgeFacade>,
) {
    // Step 1: look up source.
    let source = match source_map.get(&event.source_id).cloned() {
        Some(s) => s,
        None => {
            tracing::warn!(
                source_id = %event.source_id,
                "no source registered for triage event; falling back"
            );
            deliver_fallback_inbox(storage, &event).await;
            return;
        },
    };

    // Step 2: fetch full task details.
    let task_details = match source.fetch_task(&event.task_id).await {
        Ok(td) => td,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %event.task_id, "fetch_task failed; falling back");
            deliver_fallback_inbox(storage, &event).await;
            return;
        },
    };

    // Step 2.5: tracker-automation policy gate.
    //
    // Resolve the tier from labels BEFORE paying the triage-author LLM
    // cost. L0 (`surge:disabled` or label absent) short-circuits here:
    // write a `Skipped` ticket_index row with `triage_decision = L0Skipped`
    // and return. L1/L2/L3 fall through into triage; the resolved policy
    // is then forwarded to `dispatch_triage_decision` so the Enqueued arm
    // branches without recomputing.
    let policy = surge_intake::resolve_policy(&task_details.labels);
    if policy.is_disabled() {
        apply_l0_short_circuit(storage, &event, &task_details).await;
        return;
    }

    // Step 3: candidates (soft failure → empty).
    let candidates = surge_intake::candidates::build_for_task(&*source, &task_details, 15)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                task_id = %event.task_id,
                "build_for_task failed; using empty candidates"
            );
            Vec::new()
        });

    // Step 4: active runs (soft failure → empty).
    let active_run_rows = storage.snapshot_active_runs(32).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "snapshot_active_runs failed; using empty list");
        Vec::new()
    });

    // Step 5: map rows → orchestrator type.
    let active_runs: Vec<surge_orchestrator::triage::ActiveRunSummary> = active_run_rows
        .into_iter()
        .map(|r| surge_orchestrator::triage::ActiveRunSummary {
            run_id: r.run_id,
            task_id: r.task_id,
            status: r.status,
            started_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(r.started_at_ms)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        })
        .collect();

    // Step 6: build input + options.
    let input = surge_orchestrator::triage::TriageInput {
        task: task_details.clone(),
        candidates,
        active_runs,
    };
    let scratch_root = surge_runs_dir().join("intake").join("triage");
    let opts = surge_orchestrator::triage::TriageOptions::with_scratch_root(
        scratch_root,
        surge_orchestrator::triage::find_claude_binary(),
    );

    // Step 7: dispatch and route the four-way decision.
    let decision =
        match surge_orchestrator::triage::dispatch_triage(Arc::clone(bridge), input, opts).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    task_id = %event.task_id,
                    "triage invariant failure; falling back"
                );
                deliver_fallback_inbox(storage, &event).await;
                return;
            },
        };

    dispatch_triage_decision(
        decision,
        policy,
        &event,
        &task_details,
        &source,
        notifier,
        storage,
    )
    .await;
}

/// L0 short-circuit: persist a `Skipped` ticket_index row with the
/// `L0Skipped` triage-decision discriminator and return without paying
/// the triage-author LLM cost. Idempotent on re-fire — if the row
/// already exists in a non-`Skipped` state, transition is attempted
/// through the validated FSM; on transition rejection (terminal state,
/// concurrent action) the policy gate silently no-ops.
async fn apply_l0_short_circuit(
    storage: &Arc<Storage>,
    event: &surge_intake::types::TaskEvent,
    task_details: &surge_intake::types::TaskDetails,
) {
    use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};

    let task_id_str = event.task_id.as_str();
    let provider = task_id_str.split(':').next().unwrap_or("unknown").to_string();
    let now = chrono::Utc::now();
    let _ = task_details; // currently unused; future tracker comments may want title/url.

    let conn = match storage.acquire_registry_conn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id_str, "L0: acquire registry conn failed");
            return;
        },
    };
    let repo = IntakeRepo::new(&conn);

    let existing = match repo.fetch(task_id_str) {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id_str, "L0: fetch failed");
            return;
        },
    };

    if existing.is_none() {
        let row = IntakeRow {
            task_id: task_id_str.into(),
            source_id: event.source_id.clone(),
            provider,
            run_id: None,
            triage_decision: Some(surge_intake::TRIAGE_DECISION_L0.into()),
            duplicate_of: None,
            priority: None,
            state: TicketState::Skipped,
            first_seen: now,
            last_seen: now,
            snooze_until: None,
            callback_token: None,
            tg_chat_id: None,
            tg_message_id: None,
        };
        if let Err(e) = repo.insert(&row) {
            tracing::warn!(error = %e, task_id = %task_id_str, "L0: ticket_index insert failed");
            return;
        }
    } else {
        match repo.update_state_validated(task_id_str, TicketState::Skipped) {
            Ok(()) => {},
            Err(surge_persistence::intake::IntakeError::InvalidTransition { from, .. }) => {
                tracing::info!(
                    target: "intake::policy",
                    task_id = %task_id_str,
                    ?from,
                    "L0: ticket already in terminal/inflight state; no transition needed"
                );
                return;
            },
            Err(e) => {
                tracing::warn!(error = %e, task_id = %task_id_str, "L0: state transition failed");
                return;
            },
        }
    }

    tracing::info!(
        target: "intake::policy",
        task_id = %task_id_str,
        triage_decision = surge_intake::TRIAGE_DECISION_L0,
        "L0 short-circuit applied — triage skipped"
    );
}

/// L1 / L3 path — produce the standard inbox card via the existing
/// `enqueue_inbox_card` helper. The user approves the card; the
/// consumer's launcher uses the configured bootstrap builder.
async fn enqueue_l1_inbox_card(
    storage: &Arc<Storage>,
    event: &surge_intake::types::TaskEvent,
    task_details: &surge_intake::types::TaskDetails,
    priority: surge_intake::Priority,
    summary: String,
) {
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
        title: task_details.title.clone(),
        summary,
        priority,
        task_url: task_details.url.clone(),
        callback_token,
    };
    tracing::info!(
        target: "intake::policy",
        task_id = %event.task_id,
        priority = ?payload.priority,
        "L1/L3: enqueuing inbox card"
    );
    if let Err(e) = surge_daemon::inbox::enqueue_inbox_card(storage, &payload).await {
        tracing::warn!(
            error = %e,
            task_id = %event.task_id,
            "L1/L3: inbox card enqueue failed"
        );
    }
}

/// L2 path — skip the inbox card and synthesize an `InboxActionRow {
/// kind: Start, policy_hint: Some(template_name) }` directly so the
/// consumer's launcher resolves the named template. The ticket_index
/// row is created/updated in `InboxNotified` state with a fresh
/// `callback_token` so [`crate::inbox::ticket_run_launcher::TicketRunLauncher::fetch_ticket_for_start`]
/// can locate it.
async fn enqueue_l2_template_start(
    storage: &Arc<Storage>,
    event: &surge_intake::types::TaskEvent,
    _task_details: &surge_intake::types::TaskDetails,
    template_name: &str,
    priority: surge_intake::Priority,
) {
    use surge_persistence::inbox_queue::{self, InboxActionKind};
    use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};

    let task_id_str = event.task_id.as_str();
    let provider = task_id_str.split(':').next().unwrap_or("unknown").to_string();
    let callback_token = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now();

    let conn = match storage.acquire_registry_conn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id_str, "L2: acquire registry conn failed");
            return;
        },
    };
    let repo = IntakeRepo::new(&conn);

    let existing = match repo.fetch(task_id_str) {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id_str, "L2: fetch failed");
            return;
        },
    };

    if existing.is_none() {
        let row = IntakeRow {
            task_id: task_id_str.into(),
            source_id: event.source_id.clone(),
            provider,
            run_id: None,
            triage_decision: Some("Enqueued".into()),
            duplicate_of: None,
            priority: Some(priority.label().into()),
            state: TicketState::InboxNotified,
            first_seen: now,
            last_seen: now,
            snooze_until: None,
            callback_token: Some(callback_token.clone()),
            tg_chat_id: None,
            tg_message_id: None,
        };
        if let Err(e) = repo.insert(&row) {
            tracing::warn!(error = %e, task_id = %task_id_str, "L2: ticket_index insert failed");
            return;
        }
    } else {
        match repo.update_state_validated(task_id_str, TicketState::InboxNotified) {
            Ok(()) => {},
            Err(surge_persistence::intake::IntakeError::InvalidTransition { from, .. }) => {
                tracing::info!(
                    target: "intake::policy",
                    task_id = %task_id_str,
                    ?from,
                    "L2: ticket not eligible for InboxNotified transition; skipping enqueue"
                );
                return;
            },
            Err(e) => {
                tracing::warn!(error = %e, task_id = %task_id_str, "L2: state transition failed");
                return;
            },
        }
        if let Err(e) = repo.set_callback_token(task_id_str, &callback_token) {
            tracing::warn!(error = %e, task_id = %task_id_str, "L2: set_callback_token failed");
            return;
        }
    }

    if let Err(e) = inbox_queue::append_action(
        &conn,
        InboxActionKind::Start,
        task_id_str,
        &callback_token,
        "auto",
        None,
        Some(template_name),
    ) {
        tracing::warn!(error = %e, task_id = %task_id_str, "L2: append_action failed");
        return;
    }

    tracing::info!(
        target: "intake::policy",
        task_id = %task_id_str,
        template = template_name,
        priority = ?priority,
        "L2 template start enqueued"
    );
}

/// Reflect a non-`NewTask` event from the tracker into `ticket_index`.
///
/// Handles three flavors of `TaskEventKind`:
/// - `TaskClosed` / `StatusChanged { to: "closed" }`: if the ticket is
///   `Active`, call `EngineFacade::stop_run` with reason
///   `"closed externally"` and transition to `Aborted`. If the ticket
///   is `InboxNotified`, clear the callback token and transition to
///   `Skipped` with `triage_decision = "ExternallyClosed"`. Terminal
///   states are no-op.
/// - `LabelsChanged { added }` containing `surge:disabled`: triggers
///   the same graceful abort path as a status-close (the user wants
///   the run to stop). Other `surge:*` label transitions are INFO-
///   logged but do not escalate (the operator must restart).
/// - Other variants: ignored (defensive — `TaskEventKind` is
///   `#[serde(tag = "kind")]` rather than `#[non_exhaustive]`, so the
///   set is closed; the wildcard arm is purely a forward-compat
///   safety net).
async fn handle_external_update(
    event: surge_intake::types::TaskEvent,
    sources: &Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>>,
    storage: &Arc<Storage>,
    engine: &Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
) {
    use surge_intake::types::TaskEventKind;

    let task_id = event.task_id.as_str().to_string();
    match event.kind {
        TaskEventKind::TaskClosed => {
            apply_external_close(storage, engine, &task_id).await;
        },
        TaskEventKind::StatusChanged { ref to, .. } if to.eq_ignore_ascii_case("closed") => {
            apply_external_close(storage, engine, &task_id).await;
        },
        TaskEventKind::StatusChanged { ref from, ref to } => {
            tracing::info!(
                target: "intake::router",
                task_id = %task_id,
                from = %from,
                to = %to,
                "external status change (not closed); ticket_index untouched"
            );
        },
        TaskEventKind::LabelsChanged {
            ref added,
            ref removed,
        } => {
            let disabled_added = added
                .iter()
                .any(|l| l == surge_intake::policy::labels::DISABLED);
            if disabled_added {
                tracing::info!(
                    target: "intake::router",
                    task_id = %task_id,
                    "surge:disabled added mid-run; treating as graceful abort"
                );
                apply_external_close(storage, engine, &task_id).await;
                return;
            }
            tracing::info!(
                target: "intake::router",
                task_id = %task_id,
                added = ?added,
                removed = ?removed,
                "external labels changed; no FSM action"
            );
        },
        TaskEventKind::NewTask => {
            // Defensive: NewTask should never reach this handler — the
            // router gates on event.kind and only forwards non-NewTask
            // events here.
            tracing::warn!(
                target: "intake::router",
                task_id = %task_id,
                "NewTask reached external-update handler; routing bug"
            );
        },
    }
    let _ = sources; // reserved for future tracker-comment side-effects.
}

/// Shared "close" path used by both `TaskClosed`/`StatusChanged → closed`
/// and the `surge:disabled` mid-run label transition.
async fn apply_external_close(
    storage: &Arc<Storage>,
    engine: &Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
    task_id: &str,
) {
    use surge_persistence::intake::{IntakeRepo, TicketState};

    // Phase 1 — snapshot state + run_id under a scoped connection so the
    // non-`Send` `IntakeRepo` doesn't survive across the upcoming await.
    let row = {
        let conn = match storage.acquire_registry_conn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, task_id = %task_id, "external-close: acquire conn failed");
                return;
            },
        };
        match IntakeRepo::new(&conn).fetch(task_id) {
            Ok(Some(r)) => r,
            Ok(None) => {
                tracing::info!(
                    target: "intake::router",
                    task_id = %task_id,
                    "external-close: unknown ticket; no FSM action"
                );
                return;
            },
            Err(e) => {
                tracing::warn!(error = %e, task_id = %task_id, "external-close: fetch failed");
                return;
            },
        }
    };

    // Phase 2 — engine-side abort if there is a live run. This is the
    // await point; no IntakeRepo is held here.
    let live_run = matches!(
        row.state,
        TicketState::Active | TicketState::RunStarted
    );
    if live_run
        && let Some(run_id_str) = row.run_id.as_deref()
        && let Ok(run_id) = run_id_str.parse::<surge_core::id::RunId>()
        && let Err(e) = engine
            .stop_run(run_id, "closed externally".into())
            .await
    {
        tracing::warn!(
            target: "intake::router",
            error = %e,
            task_id = %task_id,
            run_id = %run_id_str,
            "external-close: stop_run failed"
        );
    }

    // Phase 3 — apply the FSM transition under a fresh scoped connection.
    let conn = match storage.acquire_registry_conn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id, "external-close: re-acquire conn failed");
            return;
        },
    };
    let repo = IntakeRepo::new(&conn);

    match row.state {
        TicketState::Active | TicketState::RunStarted => {
            if let Err(e) = repo.update_state_validated(task_id, TicketState::Aborted) {
                tracing::warn!(
                    target: "intake::router",
                    error = ?e,
                    task_id = %task_id,
                    "external-close: transition → Aborted refused"
                );
            } else {
                tracing::info!(
                    target: "intake::router",
                    task_id = %task_id,
                    "external-close: ticket aborted (active run stopped)"
                );
            }
        },
        TicketState::InboxNotified | TicketState::Snoozed => {
            if let Err(e) = repo.clear_callback_token(task_id) {
                tracing::warn!(error = ?e, task_id = %task_id, "external-close: clear_callback_token failed");
            }
            if let Err(e) = repo.update_state_validated(task_id, TicketState::Skipped) {
                tracing::warn!(
                    target: "intake::router",
                    error = ?e,
                    task_id = %task_id,
                    "external-close: transition → Skipped refused"
                );
            } else {
                tracing::info!(
                    target: "intake::router",
                    task_id = %task_id,
                    triage_decision = surge_intake::TRIAGE_DECISION_EXTERNALLY_CLOSED,
                    "external-close: card invalidated"
                );
            }
        },
        // Terminal / pre-triage states: no FSM action.
        TicketState::Completed
        | TicketState::Failed
        | TicketState::Aborted
        | TicketState::Skipped
        | TicketState::Stale
        | TicketState::TriageStale => {
            tracing::info!(
                target: "intake::router",
                task_id = %task_id,
                state = ?row.state,
                "external-close: terminal state; no-op"
            );
        },
        _ => {
            tracing::info!(
                target: "intake::router",
                task_id = %task_id,
                state = ?row.state,
                "external-close: state has no explicit handling; no-op"
            );
        },
    }
}

/// Route a `TriageDecision` to the appropriate output channel.
///
/// `policy` is the resolved tier from `handle_triage_event` (L0 already
/// short-circuited upstream; this function sees L1/L2/L3 only). The
/// `Enqueued` arm branches on the tier:
/// - L1 (`Standard`): the existing inbox-card path — user approves
///   before any run starts.
/// - L2 (`Template { name }`): skip the inbox card entirely and
///   synthesize an `InboxActionRow { kind: Start, policy_hint }`
///   directly so the consumer's launcher resolves the named template.
/// - L3 (`Auto`): identical to L1 for the inbox-card leg in this
///   milestone — auto-approve at HumanGate and post-completion merge
///   are wired by [`crate::AutomationMergeGate`] / future tasks.
async fn dispatch_triage_decision(
    decision: surge_intake::types::TriageDecision,
    policy: surge_intake::AutomationPolicy,
    event: &surge_intake::types::TaskEvent,
    task_details: &surge_intake::types::TaskDetails,
    source: &Arc<dyn TaskSource>,
    notifier: &Arc<dyn surge_notify::NotifyDeliverer>,
    storage: &Arc<Storage>,
) {
    use surge_intake::AutomationPolicy;
    use surge_intake::types::TriageDecision;
    let _ = source; // currently unused in Enqueued/Unclear paths after the inbox-queue cut over.
    match decision {
        TriageDecision::Enqueued {
            priority, summary, ..
        } => match policy {
            AutomationPolicy::Template { name } => {
                enqueue_l2_template_start(storage, event, task_details, &name, priority).await;
            },
            AutomationPolicy::Disabled => {
                // Defensive: L0 should have been short-circuited upstream.
                tracing::warn!(
                    target: "intake::policy",
                    task_id = %event.task_id,
                    "L0 policy reached dispatch_triage_decision — should have been short-circuited"
                );
            },
            AutomationPolicy::Standard | AutomationPolicy::Auto { .. } => {
                enqueue_l1_inbox_card(storage, event, task_details, priority, summary).await;
            },
            // `AutomationPolicy` is `#[non_exhaustive]` — any tier added
            // in a future milestone falls through to the safe L1 path
            // (visible card; operator approval) until explicitly wired.
            _ => {
                tracing::warn!(
                    target: "intake::policy",
                    task_id = %event.task_id,
                    "unknown AutomationPolicy variant; falling back to L1 inbox card"
                );
                enqueue_l1_inbox_card(storage, event, task_details, priority, summary).await;
            },
        },
        TriageDecision::Duplicate { of, reasoning } => {
            let body = format!(
                "Surge: detected duplicate of {}. {}",
                of.as_str(),
                reasoning
            );
            match source.post_comment(&event.task_id, &body).await {
                Ok(()) => {
                    tracing::info!(task_id = %event.task_id, duplicate_of = %of, "duplicate comment posted")
                },
                Err(e) => tracing::warn!(
                    error = %e,
                    task_id = %event.task_id,
                    "duplicate comment post failed"
                ),
            }
        },
        TriageDecision::OutOfScope { reasoning } => {
            let body = format!("Surge: out of scope. {}", reasoning);
            match source.post_comment(&event.task_id, &body).await {
                Ok(()) => {
                    tracing::info!(task_id = %event.task_id, "out_of_scope comment posted")
                },
                Err(e) => tracing::warn!(
                    error = %e,
                    task_id = %event.task_id,
                    "out_of_scope comment post failed"
                ),
            }
        },
        TriageDecision::Unclear { question } => {
            // Unclear has no associated InboxCardPayload (no payload run_id
            // to mirror), so we mint a fresh ulid for the delivery context.
            let run_id_str = ulid::Ulid::new().to_string();
            let run_id = match run_id_str.parse::<surge_core::id::RunId>() {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping unclear delivery: bad run_id");
                    return;
                },
            };
            let rendered = surge_notify::RenderedNotification {
                severity: surge_core::notify_config::NotifySeverity::Warn,
                title: format!("Triage unclear · {}", event.task_id.as_str()),
                body: question,
                artifact_paths: vec![],
            };
            deliver_desktop(notifier, &event.task_id, run_id, rendered).await;
        },
    }
}

/// Spawn the TaskRouter and its output consumer.
///
/// Returns `Some((source_map, conn))` so callers can plug additional
/// consumers (e.g., the run-completion → tracker-comment hook) into the
/// same source registry and SQLite connection. Returns `None` when
/// intake setup fails (registry DB open / pragma failure); the caller
/// should skip wiring downstream consumers in that case.
#[allow(clippy::too_many_arguments)]
async fn spawn_task_router(
    sources: Vec<Arc<dyn TaskSource>>,
    source_map: Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>>,
    notifier: Arc<dyn surge_notify::NotifyDeliverer>,
    storage: Arc<Storage>,
    bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade>,
    engine: Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
) -> Option<(
    Arc<HashMap<String, Arc<dyn TaskSource>>>,
    Arc<TokioMutex<rusqlite::Connection>>,
)> {
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
            return None;
        },
    };

    // Enable foreign keys for consistency with the registry pool's pragmas.
    if let Err(e) = conn.execute("PRAGMA foreign_keys = ON;", []) {
        tracing::error!(error = %e, "failed to enable foreign keys on dedup connection; intake disabled");
        return None;
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

    // Consume router output. Triage events go through the LLM dispatch path
    // (which on Enqueued enqueues an inbox card via `enqueue_inbox_card`);
    // EarlyDuplicate events post a tracker comment; ExternalUpdate events
    // reflect external status/label/closed changes into ticket_index.
    let source_map_for_consumer = source_map;
    let bridge_for_consumer = Arc::clone(&bridge);
    let storage_for_consumer = Arc::clone(&storage);
    let notifier_for_consumer = Arc::clone(&notifier);
    let engine_for_consumer = engine;
    let source_map_for_caller = Arc::clone(&source_map_for_consumer);
    let conn_for_caller = Arc::clone(&conn_arc);
    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                surge_intake::router::RouterOutput::Triage { event } => {
                    handle_triage_event(
                        event,
                        &source_map_for_consumer,
                        &notifier_for_consumer,
                        &storage_for_consumer,
                        &bridge_for_consumer,
                    )
                    .await;
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
                surge_intake::router::RouterOutput::ExternalUpdate { event } => {
                    handle_external_update(
                        event,
                        &source_map_for_consumer,
                        &storage_for_consumer,
                        &engine_for_consumer,
                    )
                    .await;
                },
                // `RouterOutput` is `#[non_exhaustive]` — defensive
                // catch for any future variant that lands without an
                // explicit handler. Logging is enough; events fall on
                // the floor rather than corrupting state.
                other => {
                    tracing::warn!(
                        target: "intake::router",
                        kind = ?std::mem::discriminant(&other),
                        "router emitted unhandled RouterOutput variant; dropping"
                    );
                },
            }
        }
    });

    Some((source_map_for_caller, conn_for_caller))
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
    use surge_orchestrator::archetype_registry::ArchetypeRegistry;
    use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, MinimalBootstrapGraphBuilder};

    let bootstrap: Arc<dyn BootstrapGraphBuilder> = Arc::new(MinimalBootstrapGraphBuilder::new());
    let archetypes: Arc<ArchetypeRegistry> = Arc::new(
        ArchetypeRegistry::load().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "ArchetypeRegistry::load failed; using empty registry");
            ArchetypeRegistry::from_dir(std::path::Path::new("definitely-missing"))
                .expect("from_dir on missing path returns empty registry")
        }),
    );
    let worktrees_root = surge_runs_dir().join("worktrees");
    if let Err(e) = std::fs::create_dir_all(&worktrees_root) {
        tracing::warn!(error = %e, path = %worktrees_root.display(), "failed to create worktrees root");
    }
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Consumer.
    let consumer = InboxActionConsumer {
        storage: Arc::clone(&storage),
        bootstrap: Arc::clone(&bootstrap),
        engine: Arc::clone(&engine),
        archetypes: Arc::clone(&archetypes),
        sources: Arc::clone(&sources),
        worktrees_root,
        project_root,
        config: config.clone(),
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
