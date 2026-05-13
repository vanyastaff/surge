//! Card emitter — the seam between a [`RenderedCard`] and the Telegram Bot
//! API.
//!
//! `emit()` reads the persisted card row and decides between three paths:
//!
//! 1. No `message_id` yet → `sendMessage`, then record `message_id` and the
//!    rendered `content_hash`.
//! 2. `message_id` present and `content_hash` differs → `editMessageText`,
//!    then update the stored hash.
//! 3. `message_id` present and `content_hash` matches → no-op (no Bot API
//!    call, no DB write). Decision 8 in ADR 0011.
//!
//! The implementation is parameterised over [`TelegramApi`] and [`CardStore`]
//! traits so unit tests can exercise every branch without standing up a
//! Telegram mock or a SQLite pool. The production impls of those traits
//! land alongside the bot loop in later phases.

use async_trait::async_trait;
use surge_notify::telegram::InboxKeyboardButton;
use surge_persistence::telegram::cards::Card;

use crate::card::render::RenderedCard;
use crate::error::{Result, TelegramCockpitError};

/// Outcome of a single [`CardEmitter::emit`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitOutcome {
    /// The card had no `message_id`; a fresh `sendMessage` was issued and
    /// the returned `message_id` was recorded.
    Sent {
        /// Telegram message id assigned by the Bot API.
        message_id: i64,
    },
    /// The card already had a `message_id` and the rendered `content_hash`
    /// differed; an `editMessageText` was issued and the stored hash was
    /// refreshed.
    Edited,
    /// The card already had a `message_id` and the rendered `content_hash`
    /// matched; nothing was sent and nothing was written.
    NoOp,
}

/// Abstract Telegram Bot API surface used by the emitter.
///
/// Production wraps `teloxide::Bot`; tests substitute a fake. Only the two
/// methods the emitter actually invokes are exposed; future bot interactions
/// (callback answers, command replies) live behind their own seams.
#[async_trait]
pub trait TelegramApi: Send + Sync {
    /// Send a fresh Markdown-formatted message with an optional inline
    /// keyboard. Returns the Telegram `message_id` of the created message.
    ///
    /// # Errors
    ///
    /// Returns [`TelegramCockpitError::Transport`] (or
    /// [`TelegramCockpitError::RateLimited`]) on a Bot API failure.
    async fn send_message(
        &self,
        chat_id: i64,
        body_md: &str,
        keyboard: &[Vec<InboxKeyboardButton>],
    ) -> Result<i64>;

    /// Replace the body and keyboard of an existing message.
    ///
    /// # Errors
    ///
    /// Returns [`TelegramCockpitError::Transport`] (or
    /// [`TelegramCockpitError::RateLimited`]) on a Bot API failure. The
    /// caller is expected to treat Telegram's "message is not modified"
    /// response as success at the impl layer.
    async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        body_md: &str,
        keyboard: &[Vec<InboxKeyboardButton>],
    ) -> Result<()>;
}

/// Abstract cards-table surface used by the emitter.
///
/// Production wraps the `surge-persistence::telegram::cards` functions
/// against a `Pool<SqliteConnectionManager>`; tests substitute an in-memory
/// fake.
#[async_trait]
pub trait CardStore: Send + Sync {
    /// Upsert a card row keyed by `(run_id, node_key, attempt_index)` and
    /// return the canonical `card_id` (fresh ULID on a new row, the existing
    /// ULID on a re-emit for the same triple). See
    /// `surge_persistence::telegram::cards::upsert` for the production
    /// semantics this trait abstracts.
    #[allow(clippy::too_many_arguments)]
    async fn upsert(
        &self,
        run_id: &str,
        node_key: &str,
        attempt_index: i64,
        kind: &str,
        chat_id: i64,
        content_hash: &str,
        now_ms: i64,
    ) -> Result<String>;

    /// Fetch a card by primary key. Returns `Ok(None)` when no row matches.
    async fn find_by_id(&self, card_id: &str) -> Result<Option<Card>>;

    /// Record the Telegram `message_id` and the final `content_hash` after
    /// the initial `sendMessage` succeeded.
    async fn mark_message_sent(
        &self,
        card_id: &str,
        message_id: i64,
        content_hash: &str,
        now_ms: i64,
    ) -> Result<()>;

    /// Conditional `content_hash` update — returns `true` when the stored
    /// value actually changed.
    async fn update_content_hash(
        &self,
        card_id: &str,
        new_hash: &str,
        now_ms: i64,
    ) -> Result<bool>;
}

/// Emitter that turns a [`RenderedCard`] into the correct Bot API call plus
/// store update.
pub struct CardEmitter<S, A>
where
    S: CardStore,
    A: TelegramApi,
{
    store: S,
    api: A,
}

impl<S, A> CardEmitter<S, A>
where
    S: CardStore,
    A: TelegramApi,
{
    /// Construct a new emitter that owns its [`CardStore`] and [`TelegramApi`]
    /// handles.
    pub fn new(store: S, api: A) -> Self {
        Self { store, api }
    }

    /// Borrow the underlying card store. Useful for tests that need to
    /// assert post-conditions on the fake.
    #[must_use]
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Borrow the underlying Bot API handle. Useful for tests that need to
    /// assert what calls were made.
    #[must_use]
    pub fn api(&self) -> &A {
        &self.api
    }

    /// Run the emitter for `card_id` against `rendered`. See the three
    /// branches in [`EmitOutcome`] for what each path does.
    ///
    /// # Errors
    ///
    /// - [`TelegramCockpitError::CardNotFound`] when no row matches `card_id`.
    /// - [`TelegramCockpitError::CardClosed`] when the row has `closed_at`
    ///   set; the cockpit's callback handler — not the emitter — owns the
    ///   stale-tap responder.
    /// - Anything the underlying [`TelegramApi`] or [`CardStore`] return.
    pub async fn emit(
        &self,
        card_id: &str,
        rendered: &RenderedCard,
        now_ms: i64,
    ) -> Result<EmitOutcome> {
        let Some(card) = self.store.find_by_id(card_id).await? else {
            tracing::warn!(
                target: "telegram::card::emit",
                card_id = %card_id,
                "emit aborted — card not found",
            );
            return Err(TelegramCockpitError::CardNotFound);
        };

        if card.closed_at.is_some() {
            tracing::warn!(
                target: "telegram::card::emit",
                card_id = %card_id,
                kind = %rendered.kind.as_str(),
                "emit aborted — card already closed",
            );
            return Err(TelegramCockpitError::CardClosed);
        }

        match card.message_id {
            None => {
                let message_id = self
                    .api
                    .send_message(card.chat_id, &rendered.body_md, &rendered.keyboard)
                    .await?;
                self.store
                    .mark_message_sent(card_id, message_id, &rendered.content_hash, now_ms)
                    .await?;
                tracing::info!(
                    target: "telegram::card::emit",
                    card_id = %card_id,
                    kind = %rendered.kind.as_str(),
                    chat_id = %card.chat_id,
                    message_id = %message_id,
                    "sent card",
                );
                Ok(EmitOutcome::Sent { message_id })
            },
            Some(_message_id) if card.content_hash == rendered.content_hash => {
                tracing::debug!(
                    target: "telegram::card::emit",
                    card_id = %card_id,
                    kind = %rendered.kind.as_str(),
                    "no-op — content_hash unchanged",
                );
                Ok(EmitOutcome::NoOp)
            },
            Some(message_id) => {
                self.api
                    .edit_message_text(
                        card.chat_id,
                        message_id,
                        &rendered.body_md,
                        &rendered.keyboard,
                    )
                    .await?;
                self.store
                    .update_content_hash(card_id, &rendered.content_hash, now_ms)
                    .await?;
                tracing::info!(
                    target: "telegram::card::emit",
                    card_id = %card_id,
                    kind = %rendered.kind.as_str(),
                    chat_id = %card.chat_id,
                    message_id = %message_id,
                    "edited card",
                );
                Ok(EmitOutcome::Edited)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, Ordering};
    use surge_core::run_event::BootstrapStage;

    use crate::card::render::{render_bootstrap, render_human_gate};

    const SAMPLE_CARD_ID: &str = "01HKGZTOKABCDEFGHJKMNPQRST";

    /// Fake card store. `cards` is the source of truth; the helpers expose
    /// the mutation history for assertions.
    #[derive(Default)]
    struct FakeStore {
        cards: Mutex<Vec<Card>>,
        mark_sent_calls: Mutex<Vec<(String, i64, String, i64)>>,
        update_hash_calls: Mutex<Vec<(String, String, i64)>>,
    }

    impl FakeStore {
        fn insert(&self, card: Card) {
            self.cards.lock().unwrap().push(card);
        }
        fn mark_sent_calls(&self) -> Vec<(String, i64, String, i64)> {
            self.mark_sent_calls.lock().unwrap().clone()
        }
        fn update_hash_calls(&self) -> Vec<(String, String, i64)> {
            self.update_hash_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CardStore for FakeStore {
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
            let mut cards = self.cards.lock().unwrap();
            if let Some(existing) = cards.iter().find(|c| {
                c.run_id == run_id
                    && c.node_key == node_key
                    && c.attempt_index == attempt_index
            }) {
                return Ok(existing.card_id.clone());
            }
            let card_id = ulid::Ulid::new().to_string();
            cards.push(Card {
                card_id: card_id.clone(),
                run_id: run_id.to_owned(),
                node_key: node_key.to_owned(),
                attempt_index,
                kind: kind.to_owned(),
                chat_id,
                message_id: None,
                content_hash: content_hash.to_owned(),
                pending_edit_prompt_message_id: None,
                created_at: now_ms,
                updated_at: now_ms,
                closed_at: None,
            });
            Ok(card_id)
        }

        async fn find_by_id(&self, card_id: &str) -> Result<Option<Card>> {
            let cards = self.cards.lock().unwrap();
            Ok(cards.iter().find(|c| c.card_id == card_id).cloned())
        }

        async fn mark_message_sent(
            &self,
            card_id: &str,
            message_id: i64,
            content_hash: &str,
            now_ms: i64,
        ) -> Result<()> {
            self.mark_sent_calls.lock().unwrap().push((
                card_id.to_owned(),
                message_id,
                content_hash.to_owned(),
                now_ms,
            ));
            let mut cards = self.cards.lock().unwrap();
            if let Some(card) = cards.iter_mut().find(|c| c.card_id == card_id) {
                card.message_id = Some(message_id);
                card.content_hash = content_hash.to_owned();
                card.updated_at = now_ms;
            }
            Ok(())
        }

        async fn update_content_hash(
            &self,
            card_id: &str,
            new_hash: &str,
            now_ms: i64,
        ) -> Result<bool> {
            self.update_hash_calls.lock().unwrap().push((
                card_id.to_owned(),
                new_hash.to_owned(),
                now_ms,
            ));
            let mut cards = self.cards.lock().unwrap();
            if let Some(card) = cards.iter_mut().find(|c| c.card_id == card_id) {
                if card.content_hash == new_hash {
                    return Ok(false);
                }
                card.content_hash = new_hash.to_owned();
                card.updated_at = now_ms;
                return Ok(true);
            }
            Ok(false)
        }
    }

    /// Fake Bot API. `next_message_id` allows tests to control the assigned
    /// message id without coordinating across calls.
    struct FakeApi {
        next_message_id: AtomicI64,
        send_calls: Mutex<Vec<(i64, String, usize)>>,
        edit_calls: Mutex<Vec<(i64, i64, String, usize)>>,
    }

    impl FakeApi {
        fn new(start_message_id: i64) -> Self {
            Self {
                next_message_id: AtomicI64::new(start_message_id),
                send_calls: Mutex::default(),
                edit_calls: Mutex::default(),
            }
        }
        fn send_calls(&self) -> Vec<(i64, String, usize)> {
            self.send_calls.lock().unwrap().clone()
        }
        fn edit_calls(&self) -> Vec<(i64, i64, String, usize)> {
            self.edit_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl TelegramApi for FakeApi {
        async fn send_message(
            &self,
            chat_id: i64,
            body_md: &str,
            keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<i64> {
            self.send_calls.lock().unwrap().push((
                chat_id,
                body_md.to_owned(),
                keyboard.iter().map(Vec::len).sum(),
            ));
            Ok(self.next_message_id.fetch_add(1, Ordering::SeqCst))
        }

        async fn edit_message_text(
            &self,
            chat_id: i64,
            message_id: i64,
            body_md: &str,
            keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<()> {
            self.edit_calls.lock().unwrap().push((
                chat_id,
                message_id,
                body_md.to_owned(),
                keyboard.iter().map(Vec::len).sum(),
            ));
            Ok(())
        }
    }

    fn pristine_card(card_id: &str, chat_id: i64, content_hash: &str) -> Card {
        Card {
            card_id: card_id.to_owned(),
            run_id: "run-1".to_owned(),
            node_key: "approve_plan".to_owned(),
            attempt_index: 0,
            kind: "human_gate".to_owned(),
            chat_id,
            message_id: None,
            content_hash: content_hash.to_owned(),
            pending_edit_prompt_message_id: None,
            created_at: 1_000,
            updated_at: 1_000,
            closed_at: None,
        }
    }

    #[tokio::test]
    async fn fresh_card_routes_through_send_message() {
        let store = FakeStore::default();
        let api = FakeApi::new(9876);
        let rendered = render_human_gate(SAMPLE_CARD_ID, "Approve the plan?");
        store.insert(pristine_card(SAMPLE_CARD_ID, 42, &rendered.content_hash));

        let emitter = CardEmitter::new(store, api);
        let outcome = emitter.emit(SAMPLE_CARD_ID, &rendered, 1_500).await.unwrap();

        assert_eq!(outcome, EmitOutcome::Sent { message_id: 9876 });
        let sent = emitter.api().send_calls();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 42);
        let marks = emitter.store().mark_sent_calls();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].1, 9876);
        assert_eq!(marks[0].2, rendered.content_hash);
        assert_eq!(marks[0].3, 1_500);
        // edit must not have fired.
        assert!(emitter.api().edit_calls().is_empty());
    }

    #[tokio::test]
    async fn unchanged_content_hash_is_a_noop() {
        let store = FakeStore::default();
        let api = FakeApi::new(0);
        let rendered = render_human_gate(SAMPLE_CARD_ID, "Approve the plan?");
        let mut card = pristine_card(SAMPLE_CARD_ID, 42, &rendered.content_hash);
        card.message_id = Some(555);
        store.insert(card);

        let emitter = CardEmitter::new(store, api);
        let outcome = emitter.emit(SAMPLE_CARD_ID, &rendered, 2_000).await.unwrap();

        assert_eq!(outcome, EmitOutcome::NoOp);
        assert!(emitter.api().send_calls().is_empty());
        assert!(emitter.api().edit_calls().is_empty());
        assert!(emitter.store().update_hash_calls().is_empty());
    }

    #[tokio::test]
    async fn changed_content_hash_drives_edit_message_text() {
        let store = FakeStore::default();
        let api = FakeApi::new(0);
        let old = render_human_gate(SAMPLE_CARD_ID, "Original");
        let new = render_human_gate(SAMPLE_CARD_ID, "Updated");
        assert_ne!(old.content_hash, new.content_hash);

        let mut card = pristine_card(SAMPLE_CARD_ID, 42, &old.content_hash);
        card.message_id = Some(555);
        store.insert(card);

        let emitter = CardEmitter::new(store, api);
        let outcome = emitter.emit(SAMPLE_CARD_ID, &new, 3_000).await.unwrap();

        assert_eq!(outcome, EmitOutcome::Edited);
        let edits = emitter.api().edit_calls();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].0, 42);
        assert_eq!(edits[0].1, 555);
        let updates = emitter.store().update_hash_calls();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].1, new.content_hash);
        assert!(emitter.api().send_calls().is_empty());
    }

    #[tokio::test]
    async fn closed_card_returns_card_closed_error() {
        let store = FakeStore::default();
        let api = FakeApi::new(0);
        let rendered = render_bootstrap(SAMPLE_CARD_ID, BootstrapStage::Description, "p");
        let mut card = pristine_card(SAMPLE_CARD_ID, 42, &rendered.content_hash);
        card.message_id = Some(555);
        card.closed_at = Some(1_800);
        store.insert(card);

        let emitter = CardEmitter::new(store, api);
        let err = emitter
            .emit(SAMPLE_CARD_ID, &rendered, 2_000)
            .await
            .unwrap_err();

        assert!(matches!(err, TelegramCockpitError::CardClosed));
        assert!(emitter.api().send_calls().is_empty());
        assert!(emitter.api().edit_calls().is_empty());
    }

    #[tokio::test]
    async fn missing_card_returns_card_not_found() {
        let store = FakeStore::default();
        let api = FakeApi::new(0);
        let rendered = render_human_gate(SAMPLE_CARD_ID, "x");

        let emitter = CardEmitter::new(store, api);
        let err = emitter
            .emit("does-not-exist", &rendered, 1_500)
            .await
            .unwrap_err();
        assert!(matches!(err, TelegramCockpitError::CardNotFound));
    }
}
