//! Cockpit e2e contracts — exercises multi-module flows with
//! in-memory adapters.
//!
//! Each sub-test covers one acceptance criterion from the Telegram
//! cockpit milestone plan (Tasks 25-28). The tests are
//! integration-style: they wire `surge-telegram`'s callback router,
//! card store, recover module, and rate limiter together against
//! fake-but-faithful trait implementations. The wiremock-vs-real-bot
//! decision is deferred — the production update stream wiring is a
//! one-line `teloxide::update_listeners::polling_default(bot)` swap
//! once the contracts here are stable.
//!
//! What we verify, per task:
//! - **T25** `cockpit_approve_callback_resolves_gate_once`:
//!   approve verb → engine resolve called exactly once with the
//!   right outcome. (Acceptance criterion #15)
//! - **T26** `cockpit_edit_callback_seeds_edit_outcome_with_comment`:
//!   edit verb path through the resolver with the right JSON.
//!   (Acceptance criterion #4)
//! - **T27** `reconcile_does_not_send_new_messages_after_restart`:
//!   the recover module never resends — only edits or closes.
//!   (Acceptance criteria #7 + #16)
//! - **T28** `stale_taps_and_admission_denial_skip_engine`: closed
//!   cards and unpaired chats short-circuit at the callback router.
//!   (Acceptance criteria #2 + #10 + #12 sub-tests a + b — the
//!   per-chat rate-limit sub-test is covered in the `rate_limiter`
//!   unit tests because the limiter is a pure module without a
//!   cross-module seam to integrate against.)

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use surge_notify::telegram::InboxKeyboardButton;
use surge_persistence::telegram::cards::Card;
use surge_telegram::{CardEmitter, CardStore, TelegramApi, TelegramCockpitError};
use surge_telegram::cockpit::{
    Admission, CallbackCtx, CallbackOutcome, CallbackVerb, EngineResolver, ReconcileReport,
    handle_callback, reconcile_open_cards,
};

// ── Fakes ─────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeCards {
    cards: Mutex<Vec<Card>>,
    /// (card_id, new_hash) calls.
    hash_updates: Mutex<Vec<(String, String)>>,
    closes: Mutex<Vec<String>>,
}

impl FakeCards {
    fn with(card: Card) -> Self {
        Self {
            cards: Mutex::new(vec![card]),
            ..Self::default()
        }
    }
    fn closed_ids(&self) -> Vec<String> {
        self.closes.lock().unwrap().clone()
    }
}

#[async_trait]
impl CardStore for FakeCards {
    async fn upsert(
        &self,
        run_id: &str,
        node_key: &str,
        attempt_index: i64,
        kind: &str,
        chat_id: i64,
        content_hash: &str,
        now_ms: i64,
    ) -> Result<String, TelegramCockpitError> {
        let mut cards = self.cards.lock().unwrap();
        for c in cards.iter() {
            if c.run_id == run_id && c.node_key == node_key && c.attempt_index == attempt_index {
                return Ok(c.card_id.clone());
            }
        }
        let fresh = format!("CARD-{}", cards.len() + 1);
        cards.push(Card {
            card_id: fresh.clone(),
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
        Ok(fresh)
    }

    async fn find_by_id(&self, card_id: &str) -> Result<Option<Card>, TelegramCockpitError> {
        Ok(self
            .cards
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.card_id == card_id)
            .cloned())
    }

    async fn mark_message_sent(
        &self,
        card_id: &str,
        message_id: i64,
        content_hash: &str,
        _now_ms: i64,
    ) -> Result<(), TelegramCockpitError> {
        for c in self.cards.lock().unwrap().iter_mut() {
            if c.card_id == card_id {
                c.message_id = Some(message_id);
                c.content_hash = content_hash.to_owned();
                return Ok(());
            }
        }
        Err(TelegramCockpitError::CardNotFound)
    }

    async fn update_content_hash(
        &self,
        card_id: &str,
        new_hash: &str,
        _now_ms: i64,
    ) -> Result<bool, TelegramCockpitError> {
        self.hash_updates
            .lock()
            .unwrap()
            .push((card_id.to_owned(), new_hash.to_owned()));
        Ok(true)
    }

    async fn find_open(&self) -> Result<Vec<Card>, TelegramCockpitError> {
        Ok(self
            .cards
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.closed_at.is_none())
            .cloned()
            .collect())
    }

    async fn close(&self, card_id: &str, now_ms: i64) -> Result<(), TelegramCockpitError> {
        for c in self.cards.lock().unwrap().iter_mut() {
            if c.card_id == card_id {
                c.closed_at = Some(now_ms);
                self.closes.lock().unwrap().push(card_id.to_owned());
                return Ok(());
            }
        }
        Err(TelegramCockpitError::CardNotFound)
    }
}

#[derive(Default)]
struct FakeBot {
    sends: Mutex<u32>,
    edits: Mutex<Vec<(i64, i64, String)>>,
}

impl FakeBot {
    fn send_count(&self) -> u32 {
        *self.sends.lock().unwrap()
    }
    fn edit_count(&self) -> usize {
        self.edits.lock().unwrap().len()
    }
}

#[async_trait]
impl TelegramApi for FakeBot {
    async fn send_message(
        &self,
        _: i64,
        _: &str,
        _: &[Vec<InboxKeyboardButton>],
    ) -> Result<i64, TelegramCockpitError> {
        let mut n = self.sends.lock().unwrap();
        *n += 1;
        Ok(i64::from(*n))
    }
    async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        body_md: &str,
        _: &[Vec<InboxKeyboardButton>],
    ) -> Result<(), TelegramCockpitError> {
        self.edits
            .lock()
            .unwrap()
            .push((chat_id, message_id, body_md.to_owned()));
        Ok(())
    }
}

#[derive(Default)]
struct FakeAdmission {
    paired_chats: Mutex<Vec<i64>>,
}

impl FakeAdmission {
    fn pair(chat_id: i64) -> Self {
        Self {
            paired_chats: Mutex::new(vec![chat_id]),
        }
    }
}

#[async_trait]
impl Admission for FakeAdmission {
    async fn is_admitted(&self, chat_id: i64) -> Result<bool, TelegramCockpitError> {
        Ok(self.paired_chats.lock().unwrap().contains(&chat_id))
    }
}

#[derive(Default)]
struct FakeEngine {
    calls: Mutex<Vec<(String, Option<String>, serde_json::Value)>>,
}

impl FakeEngine {
    fn calls(&self) -> Vec<(String, Option<String>, serde_json::Value)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl EngineResolver for FakeEngine {
    async fn resolve_human_input(
        &self,
        run_id: &str,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), TelegramCockpitError> {
        self.calls
            .lock()
            .unwrap()
            .push((run_id.to_owned(), call_id, response));
        Ok(())
    }
}

struct FakeSnapshots;

#[async_trait]
impl surge_telegram::commands::RunSnapshotProvider for FakeSnapshots {
    async fn snapshot(
        &self,
        _run_id: surge_core::id::RunId,
    ) -> Result<Option<surge_persistence::runs::RunStatusSnapshot>, TelegramCockpitError> {
        // Recover module treats `None` as "unknown run"; for T27 we
        // care only that no new `sendMessage` is issued — the
        // snapshot's value is irrelevant.
        Ok(None)
    }
}

fn sample_card(card_id: &str, run_id: &str, with_message: bool) -> Card {
    Card {
        card_id: card_id.into(),
        run_id: run_id.into(),
        node_key: "approve_plan".into(),
        attempt_index: 0,
        kind: "human_gate".into(),
        chat_id: 42,
        message_id: if with_message { Some(999) } else { None },
        content_hash: "h".into(),
        pending_edit_prompt_message_id: None,
        created_at: 0,
        updated_at: 0,
        closed_at: None,
    }
}

// ── T25: bootstrap approve → engine resolve once ────────────────

#[tokio::test]
async fn t25_cockpit_approve_callback_resolves_gate_once() {
    let store = FakeCards::with(sample_card("CARD-A", "run-1", true));
    let admission = FakeAdmission::pair(42);
    let engine = FakeEngine::default();
    let ctx = CallbackCtx {
        store,
        admission,
        engine,
    };

    let outcome = handle_callback(42, "cockpit:approve:CARD-A", &ctx).await.unwrap();
    assert!(matches!(
        outcome,
        CallbackOutcome::Resolved { verb: CallbackVerb::Approve, .. },
    ));

    let calls = ctx.engine.calls();
    assert_eq!(calls.len(), 1, "engine resolve must be called exactly once");
    assert_eq!(calls[0].0, "run-1");
    assert_eq!(calls[0].2["outcome"], "approve");
    assert!(calls[0].2.get("comment").is_none());
}

// ── T26: edit → engine resolve with comment ──────────────────────

#[tokio::test]
async fn t26_cockpit_edit_callback_seeds_edit_outcome_with_comment() {
    let store = (FakeCards::with(sample_card("CARD-B", "run-2", true)));
    let ctx = CallbackCtx {
        store,
        admission: FakeAdmission::pair(42),
        engine: FakeEngine::default(),
    };

    let outcome = handle_callback(42, "cockpit:edit:CARD-B", &ctx).await.unwrap();
    assert!(matches!(
        outcome,
        CallbackOutcome::Resolved { verb: CallbackVerb::Edit, .. },
    ));
    let calls = ctx.engine.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].2["outcome"], "edit");
    // MVP: callback seeds an empty comment; `/feedback <run_id>
    // <text>` is the textual alternative.
    assert_eq!(calls[0].2["comment"], "");
}

// ── T27: reconcile never sends new messages ─────────────────────

#[tokio::test]
async fn t27_reconcile_does_not_send_new_messages_after_restart() {
    let store = FakeCards::with(sample_card("CARD-C", "run-3", true));
    let bot = FakeBot::default();
    let snapshots = FakeSnapshots;
    let report: ReconcileReport = reconcile_open_cards(&store, &snapshots, &bot, 1_000)
        .await
        .unwrap();
    assert_eq!(report.total, 1);
    assert_eq!(bot.send_count(), 0, "reconcile must NEVER call send_message");
    // The recover module closes cards whose run is unknown (our snapshot
    // fake returns None for every run id), so the open card is closed.
    assert_eq!(store.closed_ids(), vec!["CARD-C"]);

    // Restart-style second pass: no cards remain open ⇒ no new sends or
    // edits.
    let report2 = reconcile_open_cards(&store, &snapshots, &bot, 2_000)
        .await
        .unwrap();
    assert_eq!(report2.total, 0);
    assert_eq!(bot.send_count(), 0);
}

// ── T28a: stale tap on closed card ──────────────────────────────

#[tokio::test]
async fn t28a_stale_tap_on_closed_card_does_not_call_engine() {
    let mut closed = sample_card("CARD-D", "run-4", true);
    closed.closed_at = Some(2_000);
    let store = FakeCards::with(closed);
    let ctx = CallbackCtx {
        store,
        admission: FakeAdmission::pair(42),
        engine: FakeEngine::default(),
    };
    let outcome = handle_callback(42, "cockpit:approve:CARD-D", &ctx).await.unwrap();
    assert!(matches!(outcome, CallbackOutcome::StaleTap { .. }));
    assert!(ctx.engine.calls().is_empty());
}

// ── T28b: unpaired chat denied at admission ─────────────────────

#[tokio::test]
async fn t28b_unpaired_chat_is_denied_at_admission() {
    let store = (FakeCards::with(sample_card("CARD-E", "run-5", true)));
    let ctx = CallbackCtx {
        store,
        // Note: chat 99 is NOT in the paired list (which holds 42 only).
        admission: FakeAdmission::pair(42),
        engine: FakeEngine::default(),
    };
    let outcome = handle_callback(99, "cockpit:approve:CARD-E", &ctx).await.unwrap();
    assert!(matches!(outcome, CallbackOutcome::AdmissionDenied { chat_id: 99 }));
    assert!(ctx.engine.calls().is_empty());
}

// ── T28c: stale tap on missing card ─────────────────────────────

#[tokio::test]
async fn t28c_missing_card_id_returns_stale_tap() {
    let store = FakeCards::default();
    let ctx = CallbackCtx {
        store,
        admission: FakeAdmission::pair(42),
        engine: FakeEngine::default(),
    };
    let outcome = handle_callback(42, "cockpit:approve:NOPE", &ctx).await.unwrap();
    assert!(matches!(outcome, CallbackOutcome::StaleTap { .. }));
    assert!(ctx.engine.calls().is_empty());
}

// ── Bonus: emitter idempotency under hash-match no-op ───────────

#[tokio::test]
async fn emitter_noop_when_hash_matches_existing_content() {
    let mut card = sample_card("CARD-IDEMP", "run-9", true);
    card.content_hash = "same".into();
    let store = FakeCards::with(card);
    let bot = FakeBot::default();
    let emitter = CardEmitter::new(store, bot);
    let rendered = surge_telegram::card::render::RenderedCard {
        kind: surge_telegram::CardKind::HumanGate,
        body_md: "irrelevant".into(),
        keyboard: vec![],
        content_hash: "same".into(),
    };
    let outcome = emitter.emit("CARD-IDEMP", &rendered, 1_000).await.unwrap();
    assert_eq!(outcome, surge_telegram::EmitOutcome::NoOp);
    assert_eq!(emitter.api().send_count(), 0);
    assert_eq!(emitter.api().edit_count(), 0);
}
