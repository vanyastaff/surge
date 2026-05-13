//! Cockpit entry point — drives the engine-tap loop and the Telegram
//! update loop until shutdown.
//!
//! Two concurrent loops cooperate:
//!
//! - **`tap_loop`** subscribes to [`Engine::subscribe_tap`] and forwards
//!   every [`RunEventTap`] through [`dispatch`] (the event-to-card
//!   mapping). On
//!   [`broadcast::error::RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
//!   it triggers a full
//!   [`reconcile_open_cards`](crate::cockpit::recover::reconcile_open_cards)
//!   pass so a slow subscriber recovers without losing card-state
//!   coherence.
//!
//! - **`update_loop`** consumes a generic `Stream<Item = teloxide::Update>`
//!   so production can plug `update_listeners::polling_default(bot)` in
//!   while tests can inject synthesized [`Update`] values for e2e
//!   coverage. Each incoming update is routed through the
//!   [`UpdateRoutes`] trait — callbacks fan to the callback handler,
//!   commands fan to the command dispatch, replies fan to the
//!   forced-reply feedback path. The seam keeps the runtime decoupled
//!   from the concrete bot-command type set so test rigs do not have
//!   to spin up `teloxide`.
//!
//! Both loops select against a [`CancellationToken`] so a single
//! shutdown signal cleanly winds down the cockpit alongside the rest of
//! the daemon.

use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use futures::stream::StreamExt;
use surge_orchestrator::engine::RunEventTap;
use teloxide::types::{Update, UpdateKind};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::card::emit::{CardStore, TelegramApi};
use crate::cockpit::dispatch::{CockpitCtx, dispatch};
use crate::cockpit::recover::reconcile_open_cards;
use crate::commands::status::RunSnapshotProvider;
use crate::error::Result;

/// Seam between the cockpit runtime and the concrete Telegram routing
/// surface.
///
/// Production wires this to the callback handler, the command
/// dispatch table, and the forced-reply path. Tests inject an in-memory
/// recorder so they can assert "given this Update sequence, here are
/// the engine calls / reconciles / replies the cockpit issued" without
/// needing teloxide or wiremock.
///
/// Every method returns `()` on success; routing errors are logged
/// inside the impl and never bubbled up to the outer loop (the outer
/// loop must survive transient routing failures — Decision 17).
#[async_trait]
pub trait UpdateRoutes: Send + Sync {
    /// Handle a `cockpit:<verb>:<card_id>` callback. `data` is the
    /// raw `callback_data` string.
    async fn handle_callback(&self, chat_id: i64, data: &str, callback_query_id: &str);

    /// Handle a slash-command message — `text` starts with `/`. The
    /// command parser lives inside the implementation; the runtime
    /// only knows that this is the entry point.
    async fn handle_command(&self, chat_id: i64, text: &str);

    /// Handle a text message that is a reply (i.e. its
    /// `reply_to_message` is set). The runtime forwards the
    /// `(chat_id, reply_to_message_id, text)` triple so the impl can
    /// look up any pending forced-reply prompt and resolve the
    /// associated edit gate.
    async fn handle_reply(
        &self,
        chat_id: i64,
        reply_to_message_id: i64,
        text: &str,
    );
}

/// Bundle of every dependency [`run_cockpit`] needs.
///
/// Built once by the daemon at startup; both loops borrow it via
/// `Arc`. Cloning a `CockpitRuntime` is cheap because every field is
/// either an `Arc` or a small typed wrapper.
pub struct CockpitRuntime<S, T, P, R>
where
    S: CardStore,
    T: TelegramApi,
    P: RunSnapshotProvider,
    R: UpdateRoutes,
{
    /// Engine-event dispatch context (event → card mapping).
    pub dispatch_ctx: CockpitCtx<S, T>,
    /// Snapshot read-API used by the lag-triggered reconcile path.
    pub snapshots: P,
    /// Update routing seam (see [`UpdateRoutes`]).
    pub routes: R,
}

/// Run the cockpit until `shutdown` fires or both source streams end.
///
/// `tap_rx` is the broadcast receiver returned by
/// [`Engine::subscribe_tap`]; `update_stream` is the Telegram update
/// source — production calls
/// [`teloxide::update_listeners::polling_default`] and adapts the
/// resulting listener into a `Stream<Item = Update>`. Tests pass a
/// `futures::stream::iter` over synthesized updates.
///
/// Returns `Ok(())` on graceful shutdown. Per-iteration errors from
/// the two loops are logged and absorbed — the cockpit must keep
/// running through transient failures (Decision 17).
///
/// # Errors
///
/// Returns the underlying error only when initial subscription /
/// stream construction has already failed before any iteration ran
/// (none today; reserved for future variants).
pub async fn run_cockpit<S, T, P, R>(
    runtime: Arc<CockpitRuntime<S, T, P, R>>,
    tap_rx: broadcast::Receiver<RunEventTap>,
    update_stream: impl Stream<Item = Update> + Send + Unpin + 'static,
    shutdown: CancellationToken,
) -> Result<()>
where
    S: CardStore + 'static,
    T: TelegramApi + 'static,
    P: RunSnapshotProvider + 'static,
    R: UpdateRoutes + 'static,
{
    info!(target: "telegram::cockpit", "cockpit runtime starting");

    let tap_rt = Arc::clone(&runtime);
    let update_rt = Arc::clone(&runtime);
    let tap_shutdown = shutdown.clone();
    let update_shutdown = shutdown.clone();

    let tap_handle = tokio::spawn(async move {
        drive_tap_loop(tap_rt, tap_rx, tap_shutdown).await;
    });
    let update_handle = tokio::spawn(async move {
        drive_update_loop(update_rt, update_stream, update_shutdown).await;
    });

    // Both loops watch the same `shutdown`; awaiting them serially is
    // fine because cancellation winds both down concurrently.
    if let Err(err) = tap_handle.await {
        warn!(
            target: "telegram::cockpit",
            error = %err,
            "tap loop task ended with a join error",
        );
    }
    if let Err(err) = update_handle.await {
        warn!(
            target: "telegram::cockpit",
            error = %err,
            "update loop task ended with a join error",
        );
    }

    info!(target: "telegram::cockpit", "cockpit runtime stopped");
    Ok(())
}

/// Drive the engine-tap loop.
///
/// Exposed `pub` for tests that want to exercise the lag-triggered
/// reconcile without standing up the full [`run_cockpit`] surface.
pub async fn drive_tap_loop<S, T, P, R>(
    runtime: Arc<CockpitRuntime<S, T, P, R>>,
    mut tap_rx: broadcast::Receiver<RunEventTap>,
    shutdown: CancellationToken,
) where
    S: CardStore,
    T: TelegramApi,
    P: RunSnapshotProvider,
    R: UpdateRoutes,
{
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                debug!(target: "telegram::cockpit::tap", "shutdown signalled");
                return;
            }
            recv = tap_rx.recv() => {
                match recv {
                    Ok(tap) => handle_tap(&runtime, tap).await,
                    Err(broadcast::error::RecvError::Closed) => {
                        info!(
                            target: "telegram::cockpit::tap",
                            "tap broadcast closed; exiting loop",
                        );
                        return;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            target: "telegram::cockpit::tap",
                            skipped = skipped,
                            "tap subscriber lagged; triggering reconcile",
                        );
                        run_reconcile(&runtime).await;
                    }
                }
            }
        }
    }
}

/// Drive the Telegram-update loop.
///
/// Exposed `pub` for tests that want to feed synthesized [`Update`]
/// values without standing up [`run_cockpit`].
pub async fn drive_update_loop<S, T, P, R>(
    runtime: Arc<CockpitRuntime<S, T, P, R>>,
    mut stream: impl Stream<Item = Update> + Send + Unpin,
    shutdown: CancellationToken,
) where
    S: CardStore,
    T: TelegramApi,
    P: RunSnapshotProvider,
    R: UpdateRoutes,
{
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                debug!(target: "telegram::cockpit::update", "shutdown signalled");
                return;
            }
            next = stream.next() => {
                let Some(update) = next else {
                    info!(
                        target: "telegram::cockpit::update",
                        "update stream ended; exiting loop",
                    );
                    return;
                };
                route_update(&runtime, update).await;
            }
        }
    }
}

async fn handle_tap<S, T, P, R>(
    runtime: &Arc<CockpitRuntime<S, T, P, R>>,
    tap: RunEventTap,
) where
    S: CardStore,
    T: TelegramApi,
    P: RunSnapshotProvider,
    R: UpdateRoutes,
{
    let now_ms = chrono::Utc::now().timestamp_millis();
    match dispatch(tap, &runtime.dispatch_ctx, now_ms).await {
        Ok(outcome) => {
            debug!(
                target: "telegram::cockpit::tap",
                ?outcome,
                "tap dispatched",
            );
        },
        Err(err) => {
            warn!(
                target: "telegram::cockpit::tap",
                error = %err,
                "tap dispatch failed; cockpit continues",
            );
        },
    }
}

async fn run_reconcile<S, T, P, R>(runtime: &Arc<CockpitRuntime<S, T, P, R>>)
where
    S: CardStore,
    T: TelegramApi,
    P: RunSnapshotProvider,
    R: UpdateRoutes,
{
    let now_ms = chrono::Utc::now().timestamp_millis();
    let store = runtime.dispatch_ctx.emitter.store();
    let api = runtime.dispatch_ctx.emitter.api();
    match reconcile_open_cards(store, &runtime.snapshots, api, now_ms).await {
        Ok(report) => {
            info!(
                target: "telegram::cockpit::recover",
                total = report.total,
                closed = report.closed,
                skipped_running = report.skipped_running,
                skipped_bad_run_id = report.skipped_bad_run_id,
                skipped_unknown_run = report.skipped_unknown_run,
                "reconciled open cards after tap lag",
            );
        },
        Err(err) => {
            warn!(
                target: "telegram::cockpit::recover",
                error = %err,
                "reconcile after tap lag failed",
            );
        },
    }
}

async fn route_update<S, T, P, R>(
    runtime: &Arc<CockpitRuntime<S, T, P, R>>,
    update: Update,
) where
    S: CardStore,
    T: TelegramApi,
    P: RunSnapshotProvider,
    R: UpdateRoutes,
{
    match update.kind {
        UpdateKind::CallbackQuery(query) => {
            let Some(data) = query.data else {
                debug!(
                    target: "telegram::cockpit::update",
                    "callback query without data field; ignoring",
                );
                return;
            };
            let chat_id = query
                .message
                .as_ref()
                .map(teloxide::types::MaybeInaccessibleMessage::chat)
                .map(|c| c.id.0)
                .unwrap_or(query.from.id.0 as i64);
            runtime
                .routes
                .handle_callback(chat_id, &data, query.id.as_str())
                .await;
        },
        UpdateKind::Message(message) => {
            let chat_id = message.chat.id.0;
            let Some(text) = message.text() else {
                debug!(
                    target: "telegram::cockpit::update",
                    chat_id = chat_id,
                    "message without text; ignoring",
                );
                return;
            };
            if let Some(reply_to) = message.reply_to_message() {
                runtime
                    .routes
                    .handle_reply(chat_id, reply_to.id.0.into(), text)
                    .await;
            } else if text.starts_with('/') {
                runtime.routes.handle_command(chat_id, text).await;
            } else {
                debug!(
                    target: "telegram::cockpit::update",
                    chat_id = chat_id,
                    "non-command non-reply text message; ignoring",
                );
            }
        },
        other => {
            debug!(
                target: "telegram::cockpit::update",
                kind = ?std::mem::discriminant(&other),
                "unhandled update kind",
            );
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecorderRoutes {
        callbacks: Mutex<Vec<(i64, String, String)>>,
        commands: Mutex<Vec<(i64, String)>>,
        replies: Mutex<Vec<(i64, i64, String)>>,
    }

    impl RecorderRoutes {
        fn callbacks(&self) -> Vec<(i64, String, String)> {
            self.callbacks.lock().unwrap().clone()
        }
        fn commands(&self) -> Vec<(i64, String)> {
            self.commands.lock().unwrap().clone()
        }
        fn replies(&self) -> Vec<(i64, i64, String)> {
            self.replies.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl UpdateRoutes for RecorderRoutes {
        async fn handle_callback(&self, chat_id: i64, data: &str, callback_query_id: &str) {
            self.callbacks.lock().unwrap().push((
                chat_id,
                data.to_owned(),
                callback_query_id.to_owned(),
            ));
        }
        async fn handle_command(&self, chat_id: i64, text: &str) {
            self.commands
                .lock()
                .unwrap()
                .push((chat_id, text.to_owned()));
        }
        async fn handle_reply(
            &self,
            chat_id: i64,
            reply_to_message_id: i64,
            text: &str,
        ) {
            self.replies
                .lock()
                .unwrap()
                .push((chat_id, reply_to_message_id, text.to_owned()));
        }
    }

    #[test]
    fn recorder_routes_capture_all_three_paths() {
        // Compile-time check the trait shape matches usage; a real
        // smoke test for routing lives next to the daemon wiring once
        // T22 lands.
        let r = RecorderRoutes::default();
        assert!(r.callbacks().is_empty());
        assert!(r.commands().is_empty());
        assert!(r.replies().is_empty());
    }
}
