//! Signal-handler installation and graceful-drain helpers. Cancels a
//! [`CancellationToken`] on SIGTERM / SIGINT (Unix) or Ctrl+C
//! (Windows).

use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Spawn a task that listens for OS termination signals and cancels
/// `token` when one fires. Returns immediately; the task lives for
/// the daemon's lifetime.
pub fn install_signal_handlers(token: CancellationToken) {
    tokio::spawn(async move {
        wait_for_termination().await;
        tracing::info!("termination signal received; cancelling shutdown token");
        token.cancel();
    });
}

#[cfg(unix)]
async fn wait_for_termination() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM received"),
        _ = int.recv() => tracing::info!("SIGINT received"),
    }
}

#[cfg(windows)]
async fn wait_for_termination() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("Ctrl+C received");
}

/// Wait for `token` to be cancelled, then sleep up to `grace` for
/// in-flight tasks to wind down. Used by `main.rs` after the server
/// loop exits to give run forwarders time to publish final events.
pub async fn drain(token: CancellationToken, grace: Duration) {
    token.cancelled().await;
    tokio::time::sleep(grace).await;
}
