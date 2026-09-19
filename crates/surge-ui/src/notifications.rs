use gpui::SharedString;
use gpui_component::notification::Notification;

/// Surge notification builders — convenience wrappers around gpui-component Notification.
///
/// Usage: call `window.push_notification(SurgeNotification::task_completed("my-task"), cx);`
/// from any context with access to `&mut Window` and `&mut App`.
pub struct SurgeNotification;

impl SurgeNotification {
    pub fn task_completed(task_name: &str) -> Notification {
        Notification::success(SharedString::from(format!(
            "{task_name} finished successfully"
        )))
        .title("Task Completed")
    }

    pub fn task_failed(task_name: &str, reason: &str) -> Notification {
        Notification::error(SharedString::from(format!("{task_name}: {reason}")))
            .title("Task Failed")
            .autohide(false)
    }

    pub fn agent_connected(agent: &str) -> Notification {
        Notification::info(SharedString::from(format!("{agent} is ready"))).title("Agent Connected")
    }

    pub fn agent_disconnected(agent: &str) -> Notification {
        Notification::warning(SharedString::from(format!("{agent} connection lost")))
            .title("Agent Disconnected")
            .autohide(false)
    }

    pub fn review_needed(task_name: &str) -> Notification {
        Notification::warning(SharedString::from(format!("{task_name} needs your review")))
            .title("Review Required")
            .autohide(false)
    }

    pub fn rate_limit_warning(agent: &str, reset_secs: u64) -> Notification {
        Notification::warning(SharedString::from(format!(
            "{agent} rate limited — resets in {reset_secs}s"
        )))
        .title("Rate Limit")
        .autohide(false)
    }

    pub fn run_accepted(run_short: &str) -> Notification {
        Notification::info(SharedString::from(format!("Run {run_short} accepted")))
            .title("Run Started")
    }

    pub fn run_completed(run_short: &str) -> Notification {
        Notification::success(SharedString::from(format!(
            "Run {run_short} completed successfully"
        )))
        .title("Run Completed")
    }

    pub fn run_failed(run_short: &str, reason: &str) -> Notification {
        Notification::error(SharedString::from(format!("{run_short}: {reason}")))
            .title("Run Failed")
            .autohide(false)
    }

    pub fn run_aborted(run_short: &str, reason: &str) -> Notification {
        Notification::warning(SharedString::from(format!("{run_short}: {reason}")))
            .title("Run Aborted")
            .autohide(false)
    }

    pub fn daemon_shutting_down() -> Notification {
        Notification::warning(SharedString::from(
            "Daemon is shutting down — active runs will be interrupted",
        ))
        .title("Daemon Shutting Down")
        .autohide(false)
    }
}

// ── OS-level native notifications ──────────────────────────────────

/// Single long-lived worker thread that drains a bounded channel and
/// dispatches each notification via `notify_rust`. Set up lazily on
/// first call to `send_os_notification`. Avoids spawning a new OS
/// thread per notification (which under bursty event streams could
/// exhaust thread / handle quotas) and serialises the `notify_rust`
/// call so the platform backend doesn't need to be re-initialised.
static NOTIFY_TX: std::sync::OnceLock<std::sync::mpsc::SyncSender<(String, String)>> =
    std::sync::OnceLock::new();

fn ensure_worker() -> &'static std::sync::mpsc::SyncSender<(String, String)> {
    NOTIFY_TX.get_or_init(|| {
        // Bounded so a runaway event source can't grow the queue
        // unboundedly; if the worker is wedged, we drop newest
        // (see `try_send` below) rather than memory-bloat the UI process.
        let (tx, rx) = std::sync::mpsc::sync_channel::<(String, String)>(64);
        std::thread::Builder::new()
            .name("surge-notifications".into())
            .spawn(move || {
                while let Ok((title, body)) = rx.recv() {
                    if let Err(e) = notify_rust::Notification::new()
                        .appname("Surge")
                        .summary(&title)
                        .body(&body)
                        .timeout(notify_rust::Timeout::Milliseconds(5000))
                        .show()
                    {
                        tracing::warn!("Failed to send OS notification: {e}");
                    }
                }
            })
            .expect("spawning surge-notifications worker thread");
        tx
    })
}

/// Send a native OS notification (Windows toast / macOS / Linux).
/// Hands the (title, body) pair to a single long-lived worker so the
/// caller never blocks on platform IO. If the worker queue is full we
/// drop the newest entry and log — better than blocking the UI thread.
pub fn send_os_notification(title: &str, body: &str) {
    let tx = ensure_worker();
    if let Err(std::sync::mpsc::TrySendError::Full(_)) =
        tx.try_send((title.to_string(), body.to_string()))
    {
        tracing::warn!(
            title = %title,
            "OS notification queue full; dropping notification"
        );
    }
}

/// Send OS notification for a `GlobalDaemonEvent` from the daemon
/// runtime. Mirrors the `SurgeNotification::run_*` taxonomy used for
/// in-app banners so the OS toast and the in-app card line up.
pub fn os_notify_global(event: &surge_orchestrator::engine::ipc::GlobalDaemonEvent) {
    use surge_orchestrator::engine::handle::RunOutcome;
    use surge_orchestrator::engine::ipc::GlobalDaemonEvent as G;

    let (title, body): (String, String) = match event {
        G::RunAccepted { run_id } => (
            "Run Started".into(),
            format!("Run {} accepted", run_id.short()),
        ),
        G::RunFinished { run_id, outcome } => {
            let short = run_id.short();
            match outcome {
                RunOutcome::Completed { .. } => (
                    "Run Completed".into(),
                    format!("Run {short} completed successfully"),
                ),
                RunOutcome::Failed { error } => ("Run Failed".into(), format!("{short}: {error}")),
                RunOutcome::Aborted { reason } => {
                    ("Run Aborted".into(), format!("{short}: {reason}"))
                },
                // `RunOutcome` is `#[non_exhaustive]`. Surface the new
                // variant as a generic "finished" toast until the
                // taxonomy here grows to match.
                _ => ("Run Finished".into(), format!("Run {short} finished")),
            }
        },
        G::DaemonShuttingDown => (
            "Daemon Shutting Down".into(),
            "Active runs will be interrupted".into(),
        ),
        // `GlobalDaemonEvent` is `#[non_exhaustive]`. Silently drop
        // unknown future variants here; add cases as new ones land.
        _ => return,
    };

    send_os_notification(&title, &body);
}

/// Send OS notification for a SurgeEvent.
pub fn os_notify_event(event: &surge_core::SurgeEvent) {
    use surge_core::SurgeEvent;

    let (title, body): (String, String) = match event {
        SurgeEvent::TaskStateChanged {
            task_id, new_state, ..
        } => {
            let id_short = task_id.short();
            match new_state {
                surge_core::TaskState::Completed => (
                    "Task Completed".into(),
                    format!("{id_short} finished successfully"),
                ),
                surge_core::TaskState::Failed { .. } => {
                    ("Task Failed".into(), format!("{id_short} failed"))
                },
                _ => return,
            }
        },
        SurgeEvent::GateAwaitingApproval {
            task_id, gate_name, ..
        } => {
            let id_short = task_id.short();
            (
                "Review Required".into(),
                format!("{gate_name} ({id_short}) needs your review"),
            )
        },
        SurgeEvent::AgentConnected { agent_name } => {
            ("Agent Connected".into(), format!("{agent_name} is ready"))
        },
        SurgeEvent::AgentDisconnected { agent_name } => (
            "Agent Disconnected".into(),
            format!("{agent_name} connection lost"),
        ),
        SurgeEvent::AgentRateLimited {
            agent_name,
            retry_after_secs,
        } => (
            "Rate Limit".into(),
            format!("{agent_name} rate limited — resets in {retry_after_secs}s"),
        ),
        _ => return,
    };

    send_os_notification(&title, &body);
}
