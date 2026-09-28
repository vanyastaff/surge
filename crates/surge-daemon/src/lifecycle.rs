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

/// Wait for shutdown and then for observed task completion, bounded by `grace`.
/// Returns false when the deadline expires before all owners have settled.
pub async fn drain_until<F, Fut>(token: CancellationToken, grace: Duration, mut settled: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    token.cancelled().await;
    tokio::time::timeout(grace, async {
        loop {
            if settled().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settled_shutdown_does_not_wait_for_full_grace() {
        let token = CancellationToken::new();
        token.cancel();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            drain_until(token, Duration::from_secs(30), || async { true }),
        )
        .await;
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn unsettled_shutdown_is_bounded() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(!drain_until(token, Duration::from_millis(20), || async { false }).await);
    }

    #[tokio::test]
    async fn completion_does_not_exit_before_shutdown_signal() {
        let token = CancellationToken::new();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                drain_until(token, Duration::from_secs(30), || async { true }),
            )
            .await
            .is_err()
        );
    }
}
