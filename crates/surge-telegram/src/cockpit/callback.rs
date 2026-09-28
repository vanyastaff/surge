//! Callback router — `cockpit:<verb>:<card_id>` → engine action.
//!
//! Three responsibilities, in order:
//!
//! 1. **Parse** the wire format (ADR 0010). Unknown verbs and malformed
//!    payloads short-circuit to [`CallbackOutcome::Unknown`] without touching
//!    the database or the engine.
//! 2. **Admit** the originating chat against the pairings allowlist
//!    (Decision 6). Unpaired chats short-circuit to
//!    [`CallbackOutcome::AdmissionDenied`].
//! 3. **Dispatch** the verb to the right engine call. Approve / Edit /
//!    Reject route to [`EngineResolver::resolve_card`]; Ack is a
//!    no-op acknowledgement. Stale taps (missing or closed card) short-
//!    circuit to [`CallbackOutcome::StaleTap`] per Decision 14.
//!
//! Both the engine and the pairings allowlist are reached through traits so
//! unit tests can substitute in-memory fakes without standing up a runtime.
//! The production update loop acknowledges the query first, then dispatches
//! here and sends the resulting success, stale, or failure message.

use async_trait::async_trait;
use serde_json::json;

use crate::card::emit::CardStore;
use crate::error::Result;

/// Verb extracted from the `cockpit:<verb>:<card_id>` callback prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackVerb {
    /// Approve a HumanGate or bootstrap-stage gate.
    Approve,
    /// Request an edit (the bot follow-up forces a reply for feedback —
    /// the reply path is handled separately).
    Edit,
    /// Reject the gate; the run is expected to terminate.
    Reject,
    /// Abort the running run.
    Abort,
    /// Snooze the card.
    Snooze,
    /// Read-only acknowledgement — completion / failure / escalation cards.
    Ack,
}

impl CallbackVerb {
    /// Wire form (lowercase short string).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Edit => "edit",
            Self::Reject => "reject",
            Self::Abort => "abort",
            Self::Snooze => "snooze",
            Self::Ack => "ack",
        }
    }
}

/// Parsed `cockpit:<verb>:<card_id>` callback payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCallback {
    /// Recognised verb.
    pub verb: CallbackVerb,
    /// Card id (ULID) extracted from the payload.
    pub card_id: String,
}

/// Errors raised by [`parse_callback_data`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackParseError {
    /// The string does not start with `cockpit:`.
    UnknownNamespace,
    /// The verb segment is missing or contains an unrecognised value.
    UnknownVerb(String),
    /// The card id segment is missing or empty.
    MissingCardId,
}

/// Parse a Telegram `callback_data` string per [ADR 0010](
/// ../../../../docs/adr/0010-telegram-callback-schema.md).
///
/// Returns [`ParsedCallback`] for `cockpit:<verb>:<card_id>` strings and
/// the matching [`CallbackParseError`] variant otherwise.
///
/// # Errors
///
/// See [`CallbackParseError`] — `UnknownNamespace`, `UnknownVerb`,
/// `MissingCardId`.
pub fn parse_callback_data(data: &str) -> std::result::Result<ParsedCallback, CallbackParseError> {
    let rest = data
        .strip_prefix("cockpit:")
        .ok_or(CallbackParseError::UnknownNamespace)?;
    let (verb_str, card_id) = rest
        .split_once(':')
        .ok_or(CallbackParseError::MissingCardId)?;
    if card_id.is_empty() {
        return Err(CallbackParseError::MissingCardId);
    }
    let verb = match verb_str {
        "approve" => CallbackVerb::Approve,
        "edit" => CallbackVerb::Edit,
        "reject" => CallbackVerb::Reject,
        "abort" => CallbackVerb::Abort,
        "snooze" => CallbackVerb::Snooze,
        "ack" => CallbackVerb::Ack,
        other => return Err(CallbackParseError::UnknownVerb(other.to_owned())),
    };
    Ok(ParsedCallback {
        verb,
        card_id: card_id.to_owned(),
    })
}

/// Decision returned by [`handle_callback`]. The teloxide-shaped wrapper
/// translates this into `answerCallbackQuery` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    /// The verb routed to an engine call and the engine accepted it.
    Resolved {
        /// Card id the call resolved.
        card_id: String,
        /// Verb that fired.
        verb: CallbackVerb,
    },
    /// The engine accepted the decision, but the card's presentation state
    /// could not be closed. This is not a failed or retryable decision.
    ResolvedCardUpdateFailed {
        /// Card whose presentation needs reconciliation.
        card_id: String,
        /// Accepted operator decision.
        verb: CallbackVerb,
    },
    /// The verb was [`CallbackVerb::Ack`] — no engine call, just an
    /// acknowledgement.
    Acknowledged {
        /// Card id that was acknowledged.
        card_id: String,
    },
    /// The verb was [`CallbackVerb::Snooze`] or [`CallbackVerb::Abort`] —
    /// the cockpit's MVP does not wire these yet. The callback responder
    /// shows "not yet implemented" to the operator.
    NotImplemented {
        /// Verb that fired.
        verb: CallbackVerb,
    },
    /// The verb was [`CallbackVerb::Edit`]. The handler must NOT call
    /// `resolve_card` here — tapping `Edit` is only an
    /// expression of intent. The runtime is expected to send a
    /// forced-reply prompt for the operator's feedback text, store
    /// the prompt's `message_id` on the card's
    /// `pending_edit_prompt_message_id`, and resolve the gate when
    /// the reply arrives (Decision 7 of the Telegram cockpit
    /// milestone plan).
    PendingEditFeedback {
        /// Card id awaiting a forced-reply feedback message.
        card_id: String,
        /// Run id the eventual resolution will target.
        run_id: String,
    },
    /// The originating chat is not on the pairings allowlist.
    AdmissionDenied {
        /// Chat id that attempted the callback.
        chat_id: i64,
    },
    /// The card is missing or already closed (Decision 14).
    StaleTap {
        /// Card id from the payload (informational — the row may not
        /// exist).
        card_id: String,
    },
    /// The payload did not parse.
    Unknown {
        /// The error that prevented parsing.
        error: CallbackParseError,
    },
}

/// Admission check against the pairings allowlist.
///
/// Production wraps the SQLite-backed
/// `surge_persistence::telegram::pairings::is_admitted` function.
#[async_trait]
pub trait Admission: Send + Sync {
    /// Return `true` if `chat_id` is currently paired and not revoked.
    async fn is_admitted(&self, chat_id: i64) -> Result<bool>;
}

/// Card-bound resolution surface. Production validates the card's immutable
/// request sequence against its journal before calling `Engine::resolve_gate_input`.
#[async_trait]
pub trait EngineResolver: Send + Sync {
    /// Forward the operator's decision to the engine.
    ///
    /// `card` carries the original run, node and durable request sequence.
    /// `response` is the operator's decision. Implementations must never replace
    /// the captured request with a newer request during resolution.
    async fn resolve_card(
        &self,
        card: &surge_persistence::telegram::cards::Card,
        response: serde_json::Value,
    ) -> Result<()>;
}

/// All the dependencies [`handle_callback`] needs in one bundle.
pub struct CallbackCtx<S, A, E>
where
    S: CardStore,
    A: Admission,
    E: EngineResolver,
{
    /// Card store for the lookup that follows admission.
    pub store: S,
    /// Pairings allowlist.
    pub admission: A,
    /// Engine adapter used by Approve / Edit / Reject.
    pub engine: E,
}

/// Translate one Telegram callback into a cockpit decision.
///
/// `chat_id` is the originating chat. `data` is the raw `callback_data`
/// string from the Telegram update.
///
/// # Errors
///
/// Surfaces anything the underlying [`CardStore`], [`Admission`], or
/// [`EngineResolver`] return; never panics on a malformed payload.
pub async fn handle_callback<S, A, E>(
    chat_id: i64,
    data: &str,
    ctx: &CallbackCtx<S, A, E>,
) -> Result<CallbackOutcome>
where
    S: CardStore,
    A: Admission,
    E: EngineResolver,
{
    // 1. Parse the wire format.
    let parsed = match parse_callback_data(data) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(
                target: "telegram::callback",
                chat_id = %chat_id,
                error = ?err,
                data = %data,
                "callback rejected — parse failure",
            );
            return Ok(CallbackOutcome::Unknown { error: err });
        },
    };

    // 2. Admission check.
    if !ctx.admission.is_admitted(chat_id).await? {
        tracing::info!(
            target: "telegram::auth",
            chat_id = %chat_id,
            verb = %parsed.verb.as_str(),
            "admission denied",
        );
        return Ok(CallbackOutcome::AdmissionDenied { chat_id });
    }

    // 3. Card lookup. Missing or closed ⇒ stale tap (Decision 14).
    let card = ctx.store.find_by_id(&parsed.card_id).await?;
    let card = match card {
        Some(c) if c.closed_at.is_none() => c,
        Some(_) | None => {
            tracing::info!(
                target: "telegram::callback",
                chat_id = %chat_id,
                verb = %parsed.verb.as_str(),
                card_id = %parsed.card_id,
                "stale tap",
            );
            return Ok(CallbackOutcome::StaleTap {
                card_id: parsed.card_id,
            });
        },
    };

    if card.chat_id != chat_id {
        return Ok(CallbackOutcome::AdmissionDenied { chat_id });
    }
    // Exact request validation happens at the resolver immediately before mutation.
    let outcome = match parsed.verb {
        CallbackVerb::Approve => resolve(&parsed, &card, &ctx.engine, "approve", None).await,
        CallbackVerb::Reject => resolve(&parsed, &card, &ctx.engine, "reject", None).await,
        CallbackVerb::Edit => {
            // Decision 7: the engine MUST NOT be resolved here.
            // Tapping `Edit` is only an expression of intent —
            // resolving immediately with an empty comment would
            // close the gate before the operator's feedback can be
            // collected, and the subsequent reply text would have
            // nowhere to land. The runtime owns the forced-reply
            // prompt + reply correlation; we surface
            // `PendingEditFeedback` so it knows to start that flow.
            tracing::info!(
                target: "telegram::callback",
                chat_id = %chat_id,
                verb = "edit",
                card_id = %parsed.card_id,
                run_id = %card.run_id,
                "edit intent received; awaiting forced-reply feedback"
            );
            Ok(CallbackOutcome::PendingEditFeedback {
                card_id: parsed.card_id,
                run_id: card.run_id.clone(),
            })
        },
        CallbackVerb::Ack => {
            tracing::info!(
                target: "telegram::callback",
                chat_id = %chat_id,
                verb = "ack",
                card_id = %parsed.card_id,
                "acknowledged",
            );
            Ok(CallbackOutcome::Acknowledged {
                card_id: parsed.card_id,
            })
        },
        CallbackVerb::Snooze | CallbackVerb::Abort => {
            tracing::info!(
                target: "telegram::callback",
                chat_id = %chat_id,
                verb = %parsed.verb.as_str(),
                card_id = %parsed.card_id,
                "verb deferred — not yet wired",
            );
            Ok(CallbackOutcome::NotImplemented { verb: parsed.verb })
        },
    };
    match outcome {
        Ok(resolved @ CallbackOutcome::Resolved { .. }) => {
            if let Err(error) = ctx
                .store
                .close(&card.card_id, chrono::Utc::now().timestamp_millis())
                .await
            {
                tracing::warn!(%error, card_id = %card.card_id, "decision accepted but card close failed");
                return Ok(CallbackOutcome::ResolvedCardUpdateFailed {
                    card_id: card.card_id,
                    verb: parsed.verb,
                });
            }
            Ok(resolved)
        },
        Err(
            crate::error::TelegramCockpitError::CardClosed
            | crate::error::TelegramCockpitError::EngineResolve(
                surge_orchestrator::engine::EngineError::StaleGateRequest,
            ),
        ) => Ok(CallbackOutcome::StaleTap {
            card_id: card.card_id,
        }),
        other => other,
    }
}

/// Build the JSON response for an Approve / Edit / Reject verb and forward
/// it to the engine adapter.
async fn resolve<E: EngineResolver>(
    parsed: &ParsedCallback,
    card: &surge_persistence::telegram::cards::Card,
    engine: &E,
    outcome: &str,
    comment: Option<String>,
) -> Result<CallbackOutcome> {
    let response = match comment {
        Some(c) => json!({ "outcome": outcome, "comment": c }),
        None => json!({ "outcome": outcome }),
    };
    engine.resolve_card(card, response).await?;
    tracing::info!(
        target: "telegram::callback",
        verb = %parsed.verb.as_str(),
        card_id = %parsed.card_id,
        run_id = %card.run_id,
        "resolved gate",
    );
    Ok(CallbackOutcome::Resolved {
        card_id: parsed.card_id.clone(),
        verb: parsed.verb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::card::emit::CardStore;
    use surge_notify::telegram::InboxKeyboardButton; // unused but pulls trait
    use surge_persistence::telegram::cards::Card;

    const SAMPLE_CARD_ID: &str = "01HKGZTOKABCDEFGHJKMNPQRST";

    #[test]
    fn parse_accepts_every_known_verb() {
        for verb_str in ["approve", "edit", "reject", "abort", "snooze", "ack"] {
            let data = format!("cockpit:{verb_str}:{SAMPLE_CARD_ID}");
            let parsed = parse_callback_data(&data).unwrap();
            assert_eq!(parsed.card_id, SAMPLE_CARD_ID);
            assert_eq!(parsed.verb.as_str(), verb_str);
        }
    }

    #[test]
    fn parse_rejects_unknown_namespace() {
        let err = parse_callback_data("inbox:start:foo").unwrap_err();
        assert_eq!(err, CallbackParseError::UnknownNamespace);
    }

    #[test]
    fn parse_rejects_unknown_verb() {
        let err = parse_callback_data("cockpit:resurrect:01HK").unwrap_err();
        assert!(matches!(err, CallbackParseError::UnknownVerb(v) if v == "resurrect"));
    }

    #[test]
    fn parse_rejects_missing_card_id_segment() {
        let err = parse_callback_data("cockpit:approve").unwrap_err();
        assert_eq!(err, CallbackParseError::MissingCardId);
    }

    #[test]
    fn parse_rejects_empty_card_id() {
        let err = parse_callback_data("cockpit:approve:").unwrap_err();
        assert_eq!(err, CallbackParseError::MissingCardId);
    }

    #[test]
    fn parse_callback_data_round_trip() {
        let data = format!("cockpit:approve:{SAMPLE_CARD_ID}");
        let parsed = parse_callback_data(&data).unwrap();
        assert_eq!(parsed.verb, CallbackVerb::Approve);
    }

    // ── Fakes ──

    #[derive(Default)]
    struct FakeAdmission {
        allow: Mutex<bool>,
    }

    impl FakeAdmission {
        fn allowing() -> Self {
            Self {
                allow: Mutex::new(true),
            }
        }
        fn denying() -> Self {
            Self {
                allow: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl Admission for FakeAdmission {
        async fn is_admitted(&self, _chat_id: i64) -> Result<bool> {
            Ok(*self.allow.lock().unwrap())
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
        async fn resolve_card(&self, card: &Card, response: serde_json::Value) -> Result<()> {
            self.calls.lock().unwrap().push((
                card.run_id.clone(),
                Some(card.card_id.clone()),
                response,
            ));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        cards: Mutex<Vec<Card>>,
    }

    impl FakeStore {
        fn insert(&self, card: Card) {
            self.cards.lock().unwrap().push(card);
        }
    }

    #[async_trait]
    impl CardStore for FakeStore {
        async fn upsert(
            &self,
            _run_id: &str,
            _node_key: &str,
            _attempt_index: i64,
            _kind: &str,
            _chat_id: i64,
            _content_hash: &str,
            _now_ms: i64,
        ) -> Result<String> {
            unreachable!("callback tests do not exercise upsert")
        }

        async fn find_by_id(&self, card_id: &str) -> Result<Option<Card>> {
            let cards = self.cards.lock().unwrap();
            Ok(cards.iter().find(|c| c.card_id == card_id).cloned())
        }

        async fn mark_message_sent(
            &self,
            _card_id: &str,
            _message_id: i64,
            _content_hash: &str,
            _now_ms: i64,
        ) -> Result<()> {
            unreachable!("callback tests do not exercise mark_message_sent")
        }

        async fn update_content_hash(
            &self,
            _card_id: &str,
            _new_hash: &str,
            _now_ms: i64,
        ) -> Result<bool> {
            unreachable!("callback tests do not exercise update_content_hash")
        }

        async fn find_open(&self) -> Result<Vec<Card>> {
            unreachable!("callback tests do not exercise find_open")
        }

        async fn close(&self, card_id: &str, now_ms: i64) -> Result<()> {
            let mut cards = self.cards.lock().unwrap();
            let card = cards
                .iter_mut()
                .find(|card| card.card_id == card_id)
                .unwrap();
            card.closed_at = Some(now_ms);
            Ok(())
        }
    }

    fn open_card(card_id: &str, run_id: &str) -> Card {
        Card {
            card_id: card_id.to_owned(),
            run_id: run_id.to_owned(),
            node_key: "approve_plan".to_owned(),
            attempt_index: 0,
            kind: "human_gate".to_owned(),
            chat_id: 42,
            message_id: Some(555),
            content_hash: "hash".to_owned(),
            pending_edit_prompt_message_id: None,
            created_at: 1_000,
            updated_at: 1_000,
            closed_at: None,
        }
    }

    fn ctx(
        store: FakeStore,
        admission: FakeAdmission,
        engine: FakeEngine,
    ) -> CallbackCtx<FakeStore, FakeAdmission, FakeEngine> {
        CallbackCtx {
            store,
            admission,
            engine,
        }
    }

    #[tokio::test]
    async fn unparseable_callback_returns_unknown_without_touching_anything() {
        let ctx = ctx(
            FakeStore::default(),
            FakeAdmission::denying(),
            FakeEngine::default(),
        );
        let outcome = handle_callback(42, "garbage", &ctx).await.unwrap();
        assert!(matches!(outcome, CallbackOutcome::Unknown { .. }));
        // Engine must not have been called.
        assert!(ctx.engine.calls().is_empty());
    }

    #[tokio::test]
    async fn unpaired_chat_short_circuits_at_admission() {
        let store = FakeStore::default();
        store.insert(open_card(SAMPLE_CARD_ID, "run-1"));
        let ctx = ctx(store, FakeAdmission::denying(), FakeEngine::default());

        let data = format!("cockpit:approve:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(99, &data, &ctx).await.unwrap();

        assert!(matches!(
            outcome,
            CallbackOutcome::AdmissionDenied { chat_id: 99 }
        ));
        assert!(ctx.engine.calls().is_empty());
    }

    #[tokio::test]
    async fn missing_card_is_a_stale_tap() {
        let ctx = ctx(
            FakeStore::default(),
            FakeAdmission::allowing(),
            FakeEngine::default(),
        );
        let data = format!("cockpit:approve:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &data, &ctx).await.unwrap();
        assert!(matches!(outcome, CallbackOutcome::StaleTap { .. }));
        assert!(ctx.engine.calls().is_empty());
    }

    #[tokio::test]
    async fn closed_card_is_a_stale_tap() {
        let store = FakeStore::default();
        let mut closed = open_card(SAMPLE_CARD_ID, "run-1");
        closed.closed_at = Some(2_000);
        store.insert(closed);
        let ctx = ctx(store, FakeAdmission::allowing(), FakeEngine::default());

        let data = format!("cockpit:approve:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &data, &ctx).await.unwrap();
        assert!(matches!(outcome, CallbackOutcome::StaleTap { .. }));
        assert!(ctx.engine.calls().is_empty());
    }

    #[tokio::test]
    async fn approve_verb_calls_engine_with_outcome_approve() {
        let store = FakeStore::default();
        store.insert(open_card(SAMPLE_CARD_ID, "run-XYZ"));
        let ctx = ctx(store, FakeAdmission::allowing(), FakeEngine::default());

        let data = format!("cockpit:approve:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &data, &ctx).await.unwrap();

        assert!(matches!(
            outcome,
            CallbackOutcome::Resolved {
                verb: CallbackVerb::Approve,
                ..
            }
        ));
        let calls = ctx.engine.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "run-XYZ");
        assert_eq!(calls[0].1.as_deref(), Some(SAMPLE_CARD_ID));
        assert_eq!(calls[0].2["outcome"], "approve");
        // No `comment` key on approve.
        assert!(calls[0].2.get("comment").is_none());
    }

    #[tokio::test]
    async fn reject_verb_calls_engine_with_outcome_reject() {
        let store = FakeStore::default();
        store.insert(open_card(SAMPLE_CARD_ID, "run-1"));
        let ctx = ctx(store, FakeAdmission::allowing(), FakeEngine::default());
        let data = format!("cockpit:reject:{SAMPLE_CARD_ID}");
        let _ = handle_callback(42, &data, &ctx).await.unwrap();
        assert_eq!(ctx.engine.calls()[0].2["outcome"], "reject");
    }

    #[tokio::test]
    async fn edit_verb_defers_resolution_pending_forced_reply() {
        let store = FakeStore::default();
        store.insert(open_card(SAMPLE_CARD_ID, "run-XYZ"));
        let ctx = ctx(store, FakeAdmission::allowing(), FakeEngine::default());
        let data = format!("cockpit:edit:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &data, &ctx).await.unwrap();
        match outcome {
            CallbackOutcome::PendingEditFeedback { card_id, run_id } => {
                assert_eq!(card_id, SAMPLE_CARD_ID);
                assert_eq!(run_id, "run-XYZ");
            },
            other => panic!("expected PendingEditFeedback, got {other:?}"),
        }
        // Crucially: engine MUST NOT have been called — resolution
        // is deferred until the forced-reply feedback arrives.
        assert!(ctx.engine.calls().is_empty());
    }

    #[tokio::test]
    async fn ack_verb_acknowledges_without_engine_call() {
        let store = FakeStore::default();
        store.insert(open_card(SAMPLE_CARD_ID, "run-1"));
        let ctx = ctx(store, FakeAdmission::allowing(), FakeEngine::default());
        let data = format!("cockpit:ack:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &data, &ctx).await.unwrap();
        assert!(matches!(outcome, CallbackOutcome::Acknowledged { .. }));
        assert!(ctx.engine.calls().is_empty());
    }

    #[tokio::test]
    async fn snooze_and_abort_return_not_implemented_in_mvp() {
        let store = FakeStore::default();
        store.insert(open_card(SAMPLE_CARD_ID, "run-1"));
        let ctx = ctx(store, FakeAdmission::allowing(), FakeEngine::default());

        let snooze = format!("cockpit:snooze:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &snooze, &ctx).await.unwrap();
        assert!(matches!(
            outcome,
            CallbackOutcome::NotImplemented {
                verb: CallbackVerb::Snooze
            }
        ));

        let abort = format!("cockpit:abort:{SAMPLE_CARD_ID}");
        let outcome = handle_callback(42, &abort, &ctx).await.unwrap();
        assert!(matches!(
            outcome,
            CallbackOutcome::NotImplemented {
                verb: CallbackVerb::Abort
            }
        ));

        assert!(ctx.engine.calls().is_empty());
    }

    // Silence unused-import warning — `InboxKeyboardButton` is brought in
    // by the parent module's re-exports indirectly via `CardStore` and is
    // unused in tests but kept for symmetry.
    #[allow(dead_code)]
    fn _kb_ref(_: &InboxKeyboardButton) {}
}
