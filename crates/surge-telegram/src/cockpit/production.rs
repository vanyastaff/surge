//! Production trait adapters and `spawn_cockpit` helper.
//!
//! Wraps the production persistence + engine + Bot API surfaces in
//! impls of the cockpit traits (`CardStore`, `TelegramApi`,
//! `RunSnapshotProvider`, `CockpitSnoozeQueue`, `EngineResolver`,
//! `Admission`, `UpdateRoutes`). Daemon callers construct one
//! [`CockpitWiring`] bundle and call [`spawn_cockpit`] — everything
//! else (loops, supervisor, snooze rescheduler) is wired internally.

#[path = "outgoing_admission.rs"]
mod outgoing_admission;
#[path = "reply_admission.rs"]
mod reply_admission;

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
use crate::commands::status::{PendingRequestBatch, PendingRequestFailure, RunSnapshotProvider};
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
                            "https://example.invalid/"
                                .parse()
                                .expect("static valid url")
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

/// Read-only durable snapshots and pending-request recovery.
/// Unreadable or missing registered journals are errors, never absent runs.
#[derive(Clone)]
pub struct PersistenceSnapshots {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

impl PersistenceSnapshots {
    async fn pending_request_for_run(&self, run_id: RunId) -> Result<Option<RunEventTap>> {
        let inspection = self
            .storage
            .inspect_run(run_id)
            .await
            .map_err(|error| TelegramCockpitError::Persistence(error.to_string()))?;
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            inspection.database
        else {
            return Err(TelegramCockpitError::Persistence(
                "registered run event database is missing".into(),
            ));
        };
        pending_request(run_id, &events)
    }
}

#[async_trait]
impl RunSnapshotProvider for PersistenceSnapshots {
    async fn pending_requests(&self) -> Result<PendingRequestBatch> {
        let mut batch = PendingRequestBatch::default();
        for status in [
            surge_core::RunStatus::Running,
            surge_core::RunStatus::Bootstrapping,
        ] {
            let runs = self
                .storage
                .list_runs(surge_persistence::runs::registry::RunFilter {
                    status: Some(status),
                    ..Default::default()
                })
                .await
                .map_err(|error| TelegramCockpitError::Persistence(error.to_string()))?;
            for run in runs {
                match self.pending_request_for_run(run.id).await {
                    Ok(Some(request)) => batch.requests.push(request),
                    Ok(None) => {},
                    Err(error) => batch.failures.push(PendingRequestFailure {
                        run_id: run.id,
                        error,
                    }),
                }
            }
        }
        Ok(batch)
    }

    async fn request_settled(&self, card: &Card) -> Result<bool> {
        let run_id = card
            .run_id
            .parse::<RunId>()
            .map_err(|_| TelegramCockpitError::CardClosed)?;
        let inspection = self
            .storage
            .inspect_run(run_id)
            .await
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            inspection.database
        else {
            return Err(TelegramCockpitError::Persistence(
                "approval journal is missing".into(),
            ));
        };
        Ok(exact_request_settled(card, &events))
    }

    async fn snapshot(&self, run_id: RunId) -> Result<Option<RunStatusSnapshot>> {
        let inspection = self
            .storage
            .inspect_run(run_id)
            .await
            .map_err(|error| TelegramCockpitError::Persistence(error.to_string()))?;
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            inspection.database
        else {
            if inspection.registry.is_some() || inspection.run_directory_present {
                return Ok(Some(
                    surge_persistence::runs::query::aggregate_status_with_registry(
                        run_id,
                        &[],
                        inspection.registry.map(|row| row.status),
                    ),
                ));
            }
            return Ok(None);
        };
        let mut snapshot = surge_persistence::runs::query::aggregate_status_with_registry(
            run_id,
            &events,
            inspection.registry.map(|row| row.status),
        );
        // Only durable terminal evidence retires request cards; a crash does not.
        snapshot.terminal = matches!(
            snapshot.display,
            surge_core::run_display::RunDisplayState::Done(_)
        );
        snapshot.failed = matches!(
            snapshot.display,
            surge_core::run_display::RunDisplayState::Done(surge_core::TerminalReason::Failed)
        );
        Ok(Some(snapshot))
    }
}

/// A newer request alone is never evidence that the card's request was settled.
fn exact_request_settled(card: &Card, events: &[surge_persistence::runs::ReadEvent]) -> bool {
    use surge_core::EventPayload;
    let Some(source) = events
        .iter()
        .find(|event| i64::try_from(event.seq.0).ok() == Some(card.attempt_index))
    else {
        return false;
    };
    let EventPayload::HumanInputRequested {
        node,
        session: None,
        call_id: Some(request),
        ..
    } = &source.payload.payload
    else {
        return false;
    };
    if node.as_str() != card.node_key
        || surge_core::id::GateRequestId::from_event_call_id(request).is_none()
    {
        return false;
    }
    events
        .iter()
        .filter(|event| event.seq.0 > source.seq.0)
        .any(|event| match &event.payload.payload {
            EventPayload::HumanInputResolved {
                node: resolved_node,
                call_id,
                ..
            }
            | EventPayload::HumanInputTimedOut {
                node: resolved_node,
                call_id,
                ..
            } => resolved_node == node && call_id.as_ref() == Some(request),
            _ => false,
        })
}

/// Fold one read-only snapshot. A request is actionable only at the current node.
fn pending_request(
    run_id: RunId,
    events: &[surge_persistence::runs::ReadEvent],
) -> Result<Option<RunEventTap>> {
    let log: Vec<_> = events
        .iter()
        .map(|event| surge_core::run_event::RunEvent {
            run_id,
            seq: event.seq.0,
            timestamp: chrono::DateTime::from_timestamp_millis(event.timestamp_ms)
                .unwrap_or_default(),
            payload: event.payload.payload.clone(),
        })
        .collect();
    let state = surge_core::run_state::fold(&log)
        .map_err(|error| TelegramCockpitError::Persistence(error.to_string()))?;
    let surge_core::run_state::RunState::Pipeline {
        cursor,
        graph,
        pending_human_input: Some(pending),
        parked: None,
        ..
    } = state
    else {
        return Ok(None);
    };
    if cursor.node != pending.node || graph.find_node(&pending.node).is_none() {
        return Ok(None);
    }
    let Some(event) = events
        .iter()
        .find(|event| event.seq.0 == pending.requested_seq)
    else {
        return Ok(None);
    };
    match &event.payload.payload {
        surge_core::EventPayload::HumanInputRequested {
            session: None,
            call_id: Some(id),
            ..
        } if surge_core::id::GateRequestId::from_event_call_id(id).is_some() => {},
        _ => return Ok(None), // Legacy unbound requests are readable, never actionable.
    }
    Ok(Some(RunEventTap {
        run_id,
        event: event.clone(),
    }))
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

/// Shared polling routes tracker inbox callbacks to their owning subsystem.
#[async_trait]
pub trait InboxCallbacks: Send + Sync {
    /// Validate the originating chat and enqueue the existing inbox action.
    async fn handle(&self, chat_id: i64, data: &str) -> Result<String>;
}

/// `EngineResolver` wrapping `Arc<dyn EngineFacade>`. Parses the
/// string `run_id` back into a `RunId` (the callback layer carries the
/// id as a string column from `telegram_cards`).
#[derive(Clone)]
pub struct EngineFacadeResolver {
    /// Read-only evidence used to bind a card to its original request.
    pub storage: Arc<Storage>,
    /// Engine facade — production wraps `LocalEngineFacade` or
    /// `DaemonEngineFacade`.
    pub engine: Arc<dyn EngineFacade>,
}

#[async_trait]
impl EngineResolver for EngineFacadeResolver {
    async fn resolve_card(&self, card: &Card, response: serde_json::Value) -> Result<()> {
        let (run_id, node, request_id) = self.card_request(card).await?;
        self.engine
            .resolve_gate_input(run_id, node, request_id, response)
            .await?;
        Ok(())
    }
}

impl EngineFacadeResolver {
    pub(super) async fn card_request(
        &self,
        card: &Card,
    ) -> Result<(
        RunId,
        surge_core::keys::NodeKey,
        surge_core::id::GateRequestId,
    )> {
        let run_id = card
            .run_id
            .parse::<RunId>()
            .map_err(|_| TelegramCockpitError::CardClosed)?;
        let inspection = self
            .storage
            .inspect_run(run_id)
            .await
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            inspection.database
        else {
            return Err(TelegramCockpitError::Persistence(
                "approval journal is missing".into(),
            ));
        };
        // Select the source event by the card's immutable sequence first. Current
        // state only validates that exact request; it never supplies a replacement.
        let request = events
            .iter()
            .find(|event| i64::try_from(event.seq.0).ok() == Some(card.attempt_index))
            .ok_or(TelegramCockpitError::CardClosed)?;
        let current = pending_request(run_id, &events)?.ok_or(TelegramCockpitError::CardClosed)?;
        if card.closed_at.is_some() || current.event.seq != request.seq {
            return Err(TelegramCockpitError::CardClosed);
        }
        let surge_core::EventPayload::HumanInputRequested {
            node,
            session: None,
            call_id: Some(call_id),
            ..
        } = request.payload.payload.clone()
        else {
            return Err(TelegramCockpitError::CardClosed);
        };
        if node.as_str() != card.node_key {
            return Err(TelegramCockpitError::CardClosed);
        }
        let request_id = surge_core::id::GateRequestId::from_event_call_id(&call_id)
            .ok_or(TelegramCockpitError::CardClosed)?;
        Ok((run_id, node, request_id))
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
        surge_persistence::telegram::pairings::is_admitted(&conn, chat_id)
            .map_err(|error| TelegramCockpitError::Persistence(error.to_string()))
    }
}

/// `PairingTokenConsumer` backed by `surge_persistence::telegram::pairing`.
#[derive(Clone)]
pub struct PersistencePairingConsumer {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl crate::commands::PairingTokenConsumer for PersistencePairingConsumer {
    async fn pair_with_token(&self, token: &str, chat_id: i64, now_ms: i64) -> Result<String> {
        use surge_persistence::telegram::pairing::{PairingError, pair_with_token};
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        match pair_with_token(&conn, token, chat_id, now_ms) {
            Ok(label) => Ok(label),
            Err(PairingError::WrongChat | PairingError::InvalidTarget) => {
                Err(TelegramCockpitError::PairingTargetMismatch)
            },
            Err(PairingError::LegacyUnbound) => Err(TelegramCockpitError::PairingLegacyCode),
            Err(PairingError::NotFound) => Err(TelegramCockpitError::PairingTokenInvalid),
            Err(PairingError::Expired) | Err(PairingError::AlreadyConsumed) => {
                Err(TelegramCockpitError::PairingTokenExpired)
            },
            Err(other) => Err(TelegramCockpitError::Persistence(other.to_string())),
        }
    }
}

/// `RunListProvider` backed by `surge_persistence::runs::registry::list_runs`.
#[derive(Clone)]
pub struct PersistenceRunList {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl crate::commands::RunListProvider for PersistenceRunList {
    async fn list_recent(&self, limit: u32) -> Result<Vec<crate::commands::RunRow>> {
        let filter = surge_persistence::runs::registry::RunFilter {
            limit: Some(limit as usize),
            ..Default::default()
        };
        let summaries = self
            .storage
            .list_runs(filter)
            .await
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        Ok(summaries
            .into_iter()
            .map(|s| crate::commands::RunRow {
                run_id: s.id.to_string(),
                status: format!("{:?}", s.status),
                started_at_ms: s.started_at_ms,
            })
            .collect())
    }
}

/// `RunAborter` backed by `EngineFacade::stop_run`.
#[derive(Clone)]
pub struct EngineFacadeAborter {
    /// Engine facade.
    pub engine: Arc<dyn EngineFacade>,
}

#[async_trait]
impl crate::commands::RunAborter for EngineFacadeAborter {
    async fn abort_run(&self, run_id: &str, reason: &str) -> Result<()> {
        let parsed = run_id
            .parse::<RunId>()
            .map_err(|e| TelegramCockpitError::Persistence(format!("invalid run_id: {e}")))?;
        self.engine
            .stop_run(parsed, reason.to_owned())
            .await
            .map_err(TelegramCockpitError::EngineResolve)
    }
}

/// `RunStarter` stub — full archetype resolution + start_run wiring is
/// tracked in a follow-up that requires plumbing `ArchetypeRegistry`
/// through `CockpitWiring`. For now this surfaces a recoverable
/// "deferred" reply so operators see the gap explicitly.
#[derive(Clone)]
pub struct DeferredRunStarter;

#[async_trait]
impl crate::commands::RunStarter for DeferredRunStarter {
    async fn start_run(&self, _archetype_or_path: &str) -> Result<String> {
        Err(TelegramCockpitError::Persistence(
            "/run via Telegram is not yet wired — use `surge engine run` from the CLI for now"
                .into(),
        ))
    }
}

/// `CockpitSnoozeWriter` backed by `inbox_queue::append_cockpit_snooze`.
#[derive(Clone)]
pub struct PersistenceCockpitSnoozeWriter {
    /// Shared storage handle.
    pub storage: Arc<Storage>,
}

#[async_trait]
impl crate::commands::CockpitSnoozeWriter for PersistenceCockpitSnoozeWriter {
    async fn snooze(&self, card_id: &str, wake_at_ms: i64) -> Result<()> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        let wake_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(wake_at_ms)
            .unwrap_or_else(chrono::Utc::now);
        surge_persistence::inbox_queue::append_cockpit_snooze(&conn, card_id, "telegram", wake_at)
            .map(|_| ())
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))
    }
}

/// Production routing surface — dispatches Telegram updates to the
/// existing handler functions, gating commands on the pairings
/// allowlist. Forced-reply messages currently log + drop; the
/// `/feedback <run_id> <text>` command is the recommended path.
#[derive(Clone)]
pub struct ProductionRoutes {
    /// Existing tracker-inbox action owner; provided by the daemon.
    pub inbox: Option<Arc<dyn InboxCallbacks>>,
    /// Pairings allowlist.
    pub admission: PairingsAdmission,
    /// Engine resolver used by the callback router.
    pub engine: EngineFacadeResolver,
    /// Card store used by the callback router for lookups.
    pub store: SqliteCardStore,
    /// teloxide bot handle for sending text replies to commands.
    pub bot: teloxide::Bot,
    /// Pairing token consumer (for `/pair`).
    pub pairing_consumer: PersistencePairingConsumer,
    /// Run-status snapshot provider (for `/status`).
    pub snapshots: PersistenceSnapshots,
    /// Run list provider (for `/runs`).
    pub run_list: PersistenceRunList,
    /// Run aborter (for `/abort`).
    pub run_aborter: EngineFacadeAborter,
    /// Run starter (for `/run` — currently a stub; see [`DeferredRunStarter`]).
    pub run_starter: DeferredRunStarter,
    /// Cockpit snooze writer (for `/snooze` as a reply).
    pub snooze_writer: PersistenceCockpitSnoozeWriter,
}

impl ProductionRoutes {
    pub(super) async fn admit(&self, chat_id: i64) -> bool {
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
    async fn handle_callback(&self, chat_id: i64, data: &str, callback_query_id: &str) {
        self.dispatch_callback(chat_id, data, callback_query_id)
            .await;
    }

    async fn handle_command(&self, chat_id: i64, text: &str) {
        let (cmd_raw, args) = split_command(text);
        // Strip an optional `@botname` suffix (group commands), then
        // also collapse trailing whitespace.
        let cmd = match cmd_raw.find('@') {
            Some(i) => &cmd_raw[..i],
            None => cmd_raw,
        };

        // `/pair` is the only command available to UNPAIRED chats.
        if cmd != "/pair" && !self.admit(chat_id).await {
            // Silently drop — admission denial is INFO-logged inside admit().
            return;
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let reply_res: crate::error::Result<crate::commands::CommandReply> = match cmd {
            "/pair" => {
                crate::commands::handle_pair(chat_id, args, &self.pairing_consumer, now_ms).await
            },
            "/status" => crate::commands::handle_status(chat_id, args, &self.snapshots).await,
            "/runs" => crate::commands::handle_runs(chat_id, args, &self.run_list).await,
            "/run" => crate::commands::handle_run(chat_id, args, &self.run_starter).await,
            "/abort" => crate::commands::handle_abort(chat_id, args, &self.run_aborter).await,
            "/feedback" => Ok(crate::commands::handle_feedback()),
            "/snooze" => Ok(crate::commands::CommandReply::new(
                "ℹ `/snooze <duration>` works only as a *reply* to a cockpit card — reply to the card you want to defer.",
            )),
            other => Ok(crate::commands::CommandReply::new(format!(
                "❓ Unknown command `{other}`. Available: /pair, /status, /runs, /run, /abort, /feedback, /snooze (reply)."
            ))),
        };

        match reply_res {
            Ok(reply) => {
                if cmd == "/pair" {
                    if let Err(error) =
                        reply_admission::send_pairing_reply(&self.bot, chat_id, &reply.text).await
                    {
                        warn!(%chat_id, %error, "pairing reply failed");
                    }
                } else {
                    self.send_reply(chat_id, &reply.text).await;
                }
            },
            Err(err) => {
                warn!(
                    target: "telegram::cmd",
                    %chat_id,
                    cmd = %cmd,
                    error = %err,
                    "command handler failed; sending generic error reply"
                );
                let text = format!("❌ Internal error while handling `{cmd}`. Check daemon logs.");
                if cmd == "/pair" {
                    if let Err(error) =
                        reply_admission::send_pairing_reply(&self.bot, chat_id, &text).await
                    {
                        warn!(%chat_id, %error, "pairing error reply failed");
                    }
                } else {
                    self.send_reply(chat_id, &text).await;
                }
            },
        }
    }

    async fn handle_reply(&self, chat_id: i64, reply_to_message_id: i64, text: &str) {
        self.dispatch_edit_reply(chat_id, reply_to_message_id, text)
            .await;
    }
}

impl ProductionRoutes {
    /// Send a plain-text reply to the originating chat. No `parse_mode`
    /// is set (text rendering uses literal Markdown that we keep as-is;
    /// MarkdownV2 would require pervasive escaping of operator content).
    pub(super) async fn send_reply(&self, chat_id: i64, text: &str) {
        if let Err(err) =
            reply_admission::send_reply(&self.bot, &self.admission, chat_id, text).await
        {
            warn!(
                target: "telegram::cmd::reply",
                %chat_id,
                error = %err,
                "bot.send_message reply failed",
            );
        }
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
    /// Optional tracker inbox owner sharing this bot's sole update stream.
    pub inbox: Option<Arc<dyn InboxCallbacks>>,
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
        inbox,
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
    let bot_for_routes = bot.clone();
    let bot_api = outgoing_admission::AdmittedTelegramApi {
        api: TeloxideTelegramApi { bot },
        admission: PairingsAdmission {
            storage: Arc::clone(&storage),
        },
    };
    let snapshots = PersistenceSnapshots {
        storage: Arc::clone(&storage),
    };
    let snooze_queue = PersistenceSnoozeQueue {
        storage: Arc::clone(&storage),
    };
    let admission = PairingsAdmission {
        storage: Arc::clone(&storage),
    };
    let engine_resolver = EngineFacadeResolver {
        storage: Arc::clone(&storage),
        engine: Arc::clone(&engine),
    };
    let pairing_consumer = PersistencePairingConsumer {
        storage: Arc::clone(&storage),
    };
    let run_list = PersistenceRunList {
        storage: Arc::clone(&storage),
    };
    let run_aborter = EngineFacadeAborter {
        engine: Arc::clone(&engine),
    };
    let snooze_writer = PersistenceCockpitSnoozeWriter {
        storage: Arc::clone(&storage),
    };

    let emitter = crate::card::emit::CardEmitter::new(card_store.clone(), bot_api.clone());
    let dispatch_ctx = crate::cockpit::dispatch::CockpitCtx {
        emitter,
        admin_chat_id,
    };
    let routes = ProductionRoutes {
        inbox,
        admission,
        engine: engine_resolver,
        store: card_store.clone(),
        bot: bot_for_routes,
        pairing_consumer,
        snapshots: snapshots.clone(),
        run_list,
        run_aborter,
        run_starter: DeferredRunStarter,
        snooze_writer,
    };
    let runtime = Arc::new(crate::cockpit::run::CockpitRuntime {
        dispatch_ctx,
        snapshots,
        routes,
    });

    let rt_runtime = Arc::clone(&runtime);
    let rt_shutdown = shutdown.clone();
    let runtime_handle = tokio::spawn(async move {
        if let Err(err) =
            crate::cockpit::run::run_cockpit(rt_runtime, tap_rx, updates, rt_shutdown).await
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
