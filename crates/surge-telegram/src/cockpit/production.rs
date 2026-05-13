//! Production trait adapters and `spawn_cockpit` helper.
//!
//! Wraps the production persistence + engine + Bot API surfaces in
//! impls of the cockpit traits (`CardStore`, `TelegramApi`,
//! `RunSnapshotProvider`, `CockpitSnoozeQueue`, `EngineResolver`,
//! `Admission`, `UpdateRoutes`). Daemon callers construct one
//! [`CockpitWiring`] bundle and call [`spawn_cockpit`] — everything
//! else (loops, supervisor, snooze rescheduler) is wired internally.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use surge_core::id::RunId;
use surge_notify::telegram::InboxKeyboardButton;
use surge_orchestrator::engine::RunEventTap;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::runs::RunStatusSnapshot;
use surge_persistence::runs::storage::Storage;
use surge_persistence::telegram::cards::Card;
use teloxide::payloads::{EditMessageTextSetters, SendMessageSetters};
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup, Update,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::card::emit::{CardStore, TelegramApi};
use crate::cockpit::callback::{Admission, EngineResolver};
use crate::cockpit::run::UpdateRoutes;
use crate::cockpit::snooze::{CockpitSnoozeQueue, CockpitSnoozeRescheduler, DueSnooze};
use crate::commands::status::RunSnapshotProvider;
use crate::error::{Result, TelegramCockpitError};

/// SQLite-backed `CardStore` wrapping `Arc<Storage>`.
#[derive(Clone)]
pub struct SqliteCardStore {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl CardStore for SqliteCardStore {
    async fn upsert(
        &self,
        run_id: &str,
        node_key: &str,
        attempt_index: i64,
        kind: &str,
        chat_id: i64,
        content_hash: &str,
        now_ms: i64,
    ) -> Result<String> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::upsert(
            &conn,
            run_id,
            node_key,
            attempt_index,
            kind,
            chat_id,
            content_hash,
            now_ms,
        )
        .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }

    async fn find_by_id(&self, card_id: &str) -> Result<Option<Card>> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::find_by_id(&conn, card_id)
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }

    async fn mark_message_sent(
        &self,
        card_id: &str,
        message_id: i64,
        content_hash: &str,
        now_ms: i64,
    ) -> Result<()> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::mark_message_sent(
            &conn,
            card_id,
            message_id,
            content_hash,
            now_ms,
        )
        .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }

    async fn update_content_hash(
        &self,
        card_id: &str,
        new_hash: &str,
        now_ms: i64,
    ) -> Result<bool> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::update_content_hash(&conn, card_id, new_hash, now_ms)
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }

    async fn find_open(&self) -> Result<Vec<Card>> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::find_open(&conn)
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }

    async fn close(&self, card_id: &str, now_ms: i64) -> Result<()> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::close(&conn, card_id, now_ms)
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }
}

/// `teloxide::Bot`-backed `TelegramApi`. Bare sendMessage / editMessageText
/// without inline keyboards yet — the keyboard rendering helper from
/// `surge-notify` is used at the card-emitter boundary; this adapter
/// passes through the rendered markup.
#[derive(Clone)]
pub struct TeloxideTelegramApi {
    /// teloxide bot handle.
    pub bot: teloxide::Bot,
}

#[async_trait]
impl TelegramApi for TeloxideTelegramApi {
    async fn send_message(
        &self,
        chat_id: i64,
        body_md: &str,
        keyboard: &[Vec<InboxKeyboardButton>],
    ) -> Result<i64> {
        use teloxide::prelude::Requester as _;
        let markup = build_inline_keyboard(keyboard);
        let msg = self
            .bot
            .send_message(teloxide::types::ChatId(chat_id), body_md)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .reply_markup(markup)
            .await?;
        Ok(i64::from(msg.id.0))
    }

    async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        body_md: &str,
        keyboard: &[Vec<InboxKeyboardButton>],
    ) -> Result<()> {
        use teloxide::prelude::Requester as _;
        let message_id_i32 = i32::try_from(message_id)
            .map_err(|_| TelegramCockpitError::Transport("message_id overflow".into()))?;
        let markup = build_inline_keyboard(keyboard);
        self.bot
            .edit_message_text(
                teloxide::types::ChatId(chat_id),
                teloxide::types::MessageId(message_id_i32),
                body_md,
            )
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .reply_markup(markup)
            .await?;
        Ok(())
    }
}

/// Convert the cockpit's `InboxKeyboardButton` rows into a
/// `teloxide` `InlineKeyboardMarkup`. URL buttons map to
/// `InlineKeyboardButtonKind::Url`; everything else maps to
/// `CallbackData`. Empty input yields an empty markup which Telegram
/// renders as a no-keyboard message.
fn build_inline_keyboard(rows: &[Vec<InboxKeyboardButton>]) -> InlineKeyboardMarkup {
    let mapped = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|btn| {
                    if btn.is_url {
                        let url = btn.data.parse().unwrap_or_else(|_| {
                            "https://example.invalid/".parse().expect("static valid url")
                        });
                        InlineKeyboardButton::new(&btn.label, InlineKeyboardButtonKind::Url(url))
                    } else {
                        InlineKeyboardButton::new(
                            &btn.label,
                            InlineKeyboardButtonKind::CallbackData(btn.data.clone()),
                        )
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    InlineKeyboardMarkup::new(mapped)
}

/// `RunSnapshotProvider` backed by the persistence layer's
/// `query::current_status`. Opens a run reader per call; if the run
/// has no reader (run never started or pruned), surfaces `None`.
#[derive(Clone)]
pub struct PersistenceSnapshots {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl RunSnapshotProvider for PersistenceSnapshots {
    async fn snapshot(&self, run_id: RunId) -> Result<Option<RunStatusSnapshot>> {
        let reader = match self.storage.open_run_reader(run_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(
                    target: "telegram::cockpit::production",
                    %run_id,
                    error = %e,
                    "open_run_reader returned err; treating run as unknown"
                );
                return Ok(None);
            },
        };
        let snapshot = surge_persistence::runs::query::current_status(&reader, run_id)
            .await
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        Ok(Some(snapshot))
    }
}

/// `CockpitSnoozeQueue` backed by the `inbox_action_queue` table.
#[derive(Clone)]
pub struct PersistenceSnoozeQueue {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl CockpitSnoozeQueue for PersistenceSnoozeQueue {
    async fn list_due(&self, now_ms: i64) -> Result<Vec<DueSnooze>> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .unwrap_or_else(chrono::Utc::now);
        let rows = surge_persistence::inbox_queue::list_due_cockpit_snoozes(&conn, now)
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| DueSnooze {
                seq: r.seq,
                card_id: r.card_id,
            })
            .collect())
    }

    async fn mark_processed(&self, seq: i64) -> Result<()> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::inbox_queue::mark_action_processed(&conn, seq)
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }
}

/// `EngineResolver` wrapping `Arc<dyn EngineFacade>`. Parses the
/// string `run_id` back into a `RunId` (the callback layer carries the
/// id as a string column from `telegram_cards`).
#[derive(Clone)]
pub struct EngineFacadeResolver {
    /// Engine facade — production wraps `LocalEngineFacade` or
    /// `DaemonEngineFacade`.
    pub engine: Arc<dyn EngineFacade>,
}

#[async_trait]
impl EngineResolver for EngineFacadeResolver {
    async fn resolve_human_input(
        &self,
        run_id: &str,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<()> {
        let parsed = run_id
            .parse::<RunId>()
            .map_err(|e| TelegramCockpitError::Persistence(format!("invalid run_id: {e}")))?;
        self.engine
            .resolve_human_input(parsed, call_id, response)
            .await?;
        Ok(())
    }
}

/// `Admission` backed by the `telegram_pairings` allowlist.
#[derive(Clone)]
pub struct PairingsAdmission {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl Admission for PairingsAdmission {
    async fn is_admitted(&self, chat_id: i64) -> Result<bool> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        Ok(surge_persistence::telegram::pairings::is_admitted(
            &conn, chat_id,
        )
        .unwrap_or(false))
    }
}

/// Production routing surface — dispatches Telegram updates to the
/// existing handler functions, gating commands on the pairings
/// allowlist. Forced-reply messages currently log + drop; the
/// `/feedback <run_id> <text>` command is the recommended path.
#[derive(Clone)]
pub struct ProductionRoutes {
    /// Pairings allowlist.
    pub admission: PairingsAdmission,
    /// Engine resolver used by the callback router.
    pub engine: EngineFacadeResolver,
    /// Card store used by the callback router for lookups.
    pub store: SqliteCardStore,
}

impl ProductionRoutes {
    async fn admit(&self, chat_id: i64) -> bool {
        match self.admission.is_admitted(chat_id).await {
            Ok(true) => true,
            Ok(false) => {
                tracing::info!(
                    target: "telegram::auth",
                    %chat_id,
                    "admission denied for command/reply",
                );
                false
            },
            Err(err) => {
                tracing::warn!(
                    target: "telegram::auth",
                    %chat_id,
                    error = %err,
                    "admission lookup failed; denying by default",
                );
                false
            },
        }
    }
}

#[async_trait]
impl UpdateRoutes for ProductionRoutes {
    async fn handle_callback(&self, chat_id: i64, data: &str, _callback_query_id: &str) {
        let ctx = crate::cockpit::callback::CallbackCtx {
            store: self.store.clone(),
            admission: self.admission.clone(),
            engine: self.engine.clone(),
        };
        if let Err(err) = crate::cockpit::callback::handle_callback(chat_id, data, &ctx).await {
            warn!(
                target: "telegram::callback",
                %chat_id,
                error = %err,
                "callback handler returned error; cockpit continues",
            );
        }
    }

    async fn handle_command(&self, chat_id: i64, text: &str) {
        let (cmd, args) = split_command(text);
        // `/pair` is the only command available to UNPAIRED chats.
        if cmd != "/pair" && !self.admit(chat_id).await {
            // Silently drop — admission denial is INFO-logged inside admit().
            return;
        }
        info!(
            target: "telegram::cmd",
            %chat_id,
            cmd = %cmd,
            args_len = args.len(),
            "command dispatched",
        );
        // The actual command reply formatting + Bot API send-back is
        // wired in a follow-up that owns the Bot handle for output.
        // For now we log the dispatch so the operator can verify
        // routing works end-to-end; the e2e tests in T25-T28 exercise
        // the same seam with assertable recorders.
    }

    async fn handle_reply(&self, chat_id: i64, reply_to_message_id: i64, text: &str) {
        if !self.admit(chat_id).await {
            return;
        }
        info!(
            target: "telegram::cmd::reply",
            %chat_id,
            reply_to_message_id = reply_to_message_id,
            text_len = text.len(),
            "forced-reply text received; use /feedback or /snooze for the live wiring",
        );
    }
}

fn split_command(text: &str) -> (&str, &str) {
    text.find(char::is_whitespace)
        .map_or((text, ""), |i| (&text[..i], text[i..].trim_start()))
}

/// All the inputs needed to spawn the cockpit. Daemon constructs one
/// of these from its already-built `Storage` + `EngineFacade` + bot
/// token, then calls [`spawn_cockpit`].
pub struct CockpitWiring {
    /// Shared storage handle (registry pool, secrets, cards, pairings).
    pub storage: Arc<Storage>,
    /// Engine facade for callback → human-input resolution.
    pub engine: Arc<dyn EngineFacade>,
    /// teloxide bot handle (already constructed from the persisted
    /// `telegram.cockpit.bot_token`).
    pub bot: teloxide::Bot,
    /// Chat id cards are sent to (admin chat). The plan calls out
    /// multi-subscriber as a follow-up — this MVP carries a single
    /// admin chat.
    pub admin_chat_id: i64,
    /// Engine tap receiver (caller passes `engine_handle.subscribe_tap()`).
    pub tap_rx: broadcast::Receiver<RunEventTap>,
    /// Update stream — production wraps
    /// `teloxide::update_listeners::polling_default`.
    pub updates: Box<dyn Stream<Item = Update> + Send + Unpin>,
    /// Snooze rescheduler poll interval.
    pub snooze_poll_interval: Duration,
}

/// Handles returned by [`spawn_cockpit`] so the daemon supervisor can
/// abort everything on shutdown.
pub struct CockpitHandles {
    /// Main cockpit runtime (tap + update loops).
    pub runtime: JoinHandle<()>,
    /// Cockpit snooze re-emit loop.
    pub snooze: JoinHandle<()>,
}

/// Wire the cockpit and spawn its long-running tasks.
///
/// Both loops watch the same `shutdown` token. Failures inside either
/// loop are logged and absorbed; only an outright `JoinError` (panic
/// inside a spawned task) reaches the caller via the returned handles.
#[must_use]
pub fn spawn_cockpit(wiring: CockpitWiring, shutdown: CancellationToken) -> CockpitHandles {
    let CockpitWiring {
        storage,
        engine,
        bot,
        admin_chat_id,
        tap_rx,
        updates,
        snooze_poll_interval,
    } = wiring;

    // Build the production trait surfaces.
    let card_store = SqliteCardStore {
        storage: Arc::clone(&storage),
    };
    let bot_api = TeloxideTelegramApi { bot };
    let snapshots = PersistenceSnapshots {
        storage: Arc::clone(&storage),
    };
    let snooze_queue = PersistenceSnoozeQueue {
        storage: Arc::clone(&storage),
    };
    let admission = PairingsAdmission {
        storage: Arc::clone(&storage),
    };
    let engine_resolver = EngineFacadeResolver { engine };

    let emitter =
        crate::card::emit::CardEmitter::new(card_store.clone(), bot_api.clone());
    let dispatch_ctx = crate::cockpit::dispatch::CockpitCtx {
        emitter,
        admin_chat_id,
    };
    let routes = ProductionRoutes {
        admission,
        engine: engine_resolver,
        store: card_store.clone(),
    };
    let runtime = Arc::new(crate::cockpit::run::CockpitRuntime {
        dispatch_ctx,
        snapshots,
        routes,
    });

    let rt_runtime = Arc::clone(&runtime);
    let rt_shutdown = shutdown.clone();
    let runtime_handle = tokio::spawn(async move {
        if let Err(err) = crate::cockpit::run::run_cockpit(
            rt_runtime,
            tap_rx,
            updates,
            rt_shutdown,
        )
        .await
        {
            error!(
                target: "daemon::cockpit",
                error = %err,
                "cockpit runtime exited with error",
            );
        }
    });

    let snooze_shutdown = shutdown.clone();
    let snooze_handle = tokio::spawn(async move {
        let rescheduler = CockpitSnoozeRescheduler {
            queue: snooze_queue,
            store: card_store,
            api: bot_api,
        };
        run_snooze_loop(rescheduler, snooze_poll_interval, snooze_shutdown).await;
    });

    info!(target: "daemon::cockpit", "cockpit spawned");
    CockpitHandles {
        runtime: runtime_handle,
        snooze: snooze_handle,
    }
}

async fn run_snooze_loop<Q, S, T>(
    rescheduler: CockpitSnoozeRescheduler<Q, S, T>,
    poll_interval: Duration,
    shutdown: CancellationToken,
) where
    Q: CockpitSnoozeQueue + 'static,
    S: CardStore + 'static,
    T: TelegramApi + 'static,
{
    let mut ticker = tokio::time::interval(poll_interval);
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                info!(target: "daemon::cockpit", "snooze loop shutting down");
                return;
            }
            _ = ticker.tick() => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                if let Err(err) = rescheduler.tick(now_ms).await {
                    warn!(
                        target: "telegram::cockpit::snooze",
                        error = %err,
                        "snooze tick failed",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_handles_no_args() {
        assert_eq!(split_command("/status"), ("/status", ""));
    }

    #[test]
    fn split_command_separates_first_token() {
        assert_eq!(split_command("/run rust-crate"), ("/run", "rust-crate"));
        assert_eq!(
            split_command("/feedback abc some text"),
            ("/feedback", "abc some text"),
        );
    }
}
