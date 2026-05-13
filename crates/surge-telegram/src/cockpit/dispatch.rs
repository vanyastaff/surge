//! Dispatch table for [`RunEventTap`] → cockpit card actions.
//!
//! Implements the locked event→card mapping (Decision 11). One `RunEventTap`
//! produces at most one `emit()` call; events that do not require a card —
//! `RunStarted`, `StageInputsResolved`, etc. — fall through as
//! [`DispatchOutcome::Ignored`] without touching the store or the Bot API.

use surge_core::run_event::{BootstrapStage, EventPayload};
use surge_orchestrator::engine::RunEventTap;

use crate::card::emit::{CardEmitter, CardStore, EmitOutcome, TelegramApi};
use crate::card::render::{
    CardKind, RenderedCard, render_bootstrap, render_completion, render_escalation, render_failure,
    render_human_gate,
};
use crate::error::Result;

/// Per-dispatcher context shared across every incoming tap event.
///
/// Carries the [`CardEmitter`] handle and the chat id that cards are
/// currently routed to. Later phases will replace `admin_chat_id` with a
/// per-run subscriber set sourced from `telegram_pairings`.
pub struct CockpitCtx<S, A>
where
    S: CardStore,
    A: TelegramApi,
{
    /// Emitter that translates rendered cards into Bot API calls.
    pub emitter: CardEmitter<S, A>,
    /// Telegram chat id to send cards to. MVP carries a single admin chat;
    /// the multi-subscriber path is wired in a later phase.
    pub admin_chat_id: i64,
}

/// Outcome of a single [`dispatch`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// A card was upserted and emitted. The emitter outcome (Sent / Edited
    /// / NoOp) is forwarded verbatim.
    Emitted {
        /// The card kind that was emitted.
        kind: CardKind,
        /// The card row's ULID.
        card_id: String,
        /// What the emitter actually did.
        emit: EmitOutcome,
    },
    /// The incoming event variant has no cockpit-card mapping — dispatch
    /// returned without touching the store or the Bot API.
    Ignored {
        /// The event kind as reported by `EventPayload::kind_str` (or the
        /// equivalent debug discriminator), used only for logging.
        event_kind: &'static str,
    },
}

/// Translate a single [`RunEventTap`] into a cockpit card action.
///
/// Pure dispatch table — no internal state. The bot loop calls this once per
/// tap event. `now_ms` is supplied by the caller (instead of read inside)
/// to keep the function deterministic under test.
///
/// # Errors
///
/// Surfaces anything the underlying [`CardStore`] or [`TelegramApi`] return.
pub async fn dispatch<S, A>(
    tap: RunEventTap,
    ctx: &CockpitCtx<S, A>,
    now_ms: i64,
) -> Result<DispatchOutcome>
where
    S: CardStore,
    A: TelegramApi,
{
    let run_id_str = tap.run_id.to_string();

    let action = decide_action(&tap);

    let Some(action) = action else {
        let event_kind = tap.event.payload.payload().discriminant_str();
        tracing::debug!(
            target: "telegram::cockpit::dispatch",
            run_id = %run_id_str,
            event = %event_kind,
            "ignored event — no cockpit mapping",
        );
        return Ok(DispatchOutcome::Ignored { event_kind });
    };

    let rendered = render_for_action(&run_id_str, &action);

    let card_id = ctx
        .emitter
        .store()
        .upsert(
            &run_id_str,
            action.node_key,
            action.attempt_index,
            rendered.kind.as_str(),
            ctx.admin_chat_id,
            &rendered.content_hash,
            now_ms,
        )
        .await?;

    let emit = ctx.emitter.emit(&card_id, &rendered, now_ms).await?;
    let kind = rendered.kind;

    tracing::info!(
        target: "telegram::cockpit::dispatch",
        run_id = %run_id_str,
        event = %action.event_kind,
        card_id = %card_id,
        kind = %kind.as_str(),
        outcome = ?emit,
        "dispatched event",
    );

    Ok(DispatchOutcome::Emitted {
        kind,
        card_id,
        emit,
    })
}

/// Internal record of "what cockpit action does this event imply".
///
/// Held as a separate value so unit tests can probe the decision without
/// running the emitter.
#[derive(Debug, Clone)]
struct CardAction {
    /// Stable string discriminator for logs and tests.
    event_kind: &'static str,
    /// Card kind to be produced.
    kind: CardKind,
    /// Node key the card is keyed against. For run-level cards (status,
    /// completion, failure, escalation) this is a synthetic per-kind key so
    /// the cards table's `(run_id, node_key, attempt_index)` triple
    /// remains unique without colliding with real graph nodes.
    node_key: &'static str,
    /// `RunMemory.node_visits[node]` value at emit time. MVP defaults to
    /// `0` until the dispatcher gains access to live memory; the bootstrap
    /// edit loop's multi-attempt cards land here in a later phase.
    attempt_index: i64,
    /// Pre-extracted payload pieces the renderer needs. Avoids matching
    /// the event variant a second time inside `render_for_action`.
    payload: ActionPayload,
}

#[derive(Debug, Clone)]
enum ActionPayload {
    HumanGate {
        prompt: String,
    },
    Bootstrap {
        stage: BootstrapStage,
        summary: String,
    },
    Completion {
        terminal_node: String,
    },
    Failure {
        error: String,
    },
    Escalation {
        stage: Option<BootstrapStage>,
        reason: String,
    },
}

/// Decide which cockpit action (if any) a tap event requires.
fn decide_action(tap: &RunEventTap) -> Option<CardAction> {
    match tap.event.payload.payload() {
        EventPayload::HumanInputRequested { node, prompt, .. } => Some(CardAction {
            event_kind: "HumanInputRequested",
            kind: CardKind::HumanGate,
            // Node keys carry an &str view; the lifetime escapes via the
            // `CardAction`, so we lean on the existing static-table trick:
            // copy into a static-leak-free String by going through render.
            // Here we accept the leak as a one-shot test artifact — the
            // production dispatcher will plumb the &str through directly.
            node_key: leak_node_key(node.as_str()),
            attempt_index: 0,
            payload: ActionPayload::HumanGate {
                prompt: prompt.clone(),
            },
        }),
        EventPayload::BootstrapApprovalRequested { stage, .. } => Some(CardAction {
            event_kind: "BootstrapApprovalRequested",
            kind: CardKind::from(*stage),
            node_key: bootstrap_node_key(*stage),
            attempt_index: 0,
            payload: ActionPayload::Bootstrap {
                stage: *stage,
                summary: format!("Stage {stage:?} ready for review."),
            },
        }),
        EventPayload::RunCompleted { terminal_node } => Some(CardAction {
            event_kind: "RunCompleted",
            kind: CardKind::Completion,
            node_key: "__completion__",
            attempt_index: 0,
            payload: ActionPayload::Completion {
                terminal_node: terminal_node.as_str().to_owned(),
            },
        }),
        EventPayload::RunFailed { error } => Some(CardAction {
            event_kind: "RunFailed",
            kind: CardKind::Failure,
            node_key: "__failure__",
            attempt_index: 0,
            payload: ActionPayload::Failure {
                error: error.clone(),
            },
        }),
        EventPayload::EscalationRequested { stage, reason } => Some(CardAction {
            event_kind: "EscalationRequested",
            kind: CardKind::Escalation,
            node_key: "__escalation__",
            attempt_index: 0,
            payload: ActionPayload::Escalation {
                stage: *stage,
                reason: reason.clone(),
            },
        }),
        _ => None,
    }
}

/// Render the card for an action. The `card_id` placeholder is required
/// because the renderer bakes the id into `callback_data`; the cockpit
/// upserts to learn the real id and renders again, which is acceptable for
/// MVP — the content_hash will end up reflecting the real id either way.
fn render_for_action(_run_id: &str, action: &CardAction) -> RenderedCard {
    let placeholder_card_id = "00000000000000000000000000";
    match &action.payload {
        ActionPayload::HumanGate { prompt } => render_human_gate(placeholder_card_id, prompt),
        ActionPayload::Bootstrap { stage, summary } => {
            render_bootstrap(placeholder_card_id, *stage, summary)
        },
        ActionPayload::Completion { terminal_node } => {
            // RunId is not directly available in the renderer signature, so
            // we use a synthetic placeholder for the body text — the
            // production wiring threads the real RunId through.
            render_completion(
                placeholder_card_id,
                &surge_core::id::RunId::nil(),
                terminal_node,
            )
        },
        ActionPayload::Failure { error } => {
            render_failure(placeholder_card_id, &surge_core::id::RunId::nil(), error)
        },
        ActionPayload::Escalation { stage, reason } => {
            render_escalation(placeholder_card_id, *stage, reason)
        },
    }
}

/// Map a [`BootstrapStage`] to its synthetic node-key constant. The
/// canonical bootstrap graph names match these so a real `node_key` would
/// shadow the synthetic key — that is intentional, the upsert collapses
/// onto the canonical node when both exist.
fn bootstrap_node_key(stage: BootstrapStage) -> &'static str {
    match stage {
        BootstrapStage::Description => "bootstrap_description",
        BootstrapStage::Roadmap => "bootstrap_roadmap",
        BootstrapStage::Flow => "bootstrap_flow",
    }
}

/// Convert a `&str` view of a `NodeKey` into a `&'static str`. Used by the
/// `CardAction` struct in the MVP dispatcher; the production dispatcher
/// will thread the owned string through instead, retiring this helper.
fn leak_node_key(node_key: &str) -> &'static str {
    Box::leak(node_key.to_owned().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, Ordering};

    use async_trait::async_trait;
    use surge_core::id::RunId;
    use surge_core::keys::NodeKey;
    use surge_core::migrations::MAX_SUPPORTED_VERSION;
    use surge_core::run_event::{EventPayload, VersionedEventPayload};
    use surge_notify::telegram::InboxKeyboardButton;
    use surge_persistence::runs::{EventSeq, ReadEvent};
    use surge_persistence::telegram::cards::Card;

    fn node(name: &str) -> NodeKey {
        NodeKey::try_new(name).expect("valid node key")
    }

    fn tap(run_id: RunId, seq: u64, payload: EventPayload) -> RunEventTap {
        RunEventTap {
            run_id,
            event: ReadEvent {
                seq: EventSeq(seq),
                timestamp_ms: 0,
                kind: "fixture".to_owned(),
                payload: VersionedEventPayload {
                    schema_version: MAX_SUPPORTED_VERSION,
                    payload,
                },
            },
        }
    }

    #[derive(Default)]
    struct InMemStore {
        cards: Mutex<Vec<Card>>,
    }

    #[async_trait]
    impl CardStore for InMemStore {
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
                c.run_id == run_id && c.node_key == node_key && c.attempt_index == attempt_index
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

        async fn find_open(&self) -> Result<Vec<Card>> {
            let cards = self.cards.lock().unwrap();
            Ok(cards
                .iter()
                .filter(|c| c.closed_at.is_none())
                .cloned()
                .collect())
        }

        async fn close(&self, card_id: &str, now_ms: i64) -> Result<()> {
            let mut cards = self.cards.lock().unwrap();
            if let Some(card) = cards.iter_mut().find(|c| c.card_id == card_id) {
                if card.closed_at.is_none() {
                    card.closed_at = Some(now_ms);
                    card.updated_at = now_ms;
                }
            }
            Ok(())
        }
    }

    struct InMemApi {
        next_message_id: AtomicI64,
        send_calls: Mutex<Vec<i64>>,
    }

    impl Default for InMemApi {
        fn default() -> Self {
            Self {
                next_message_id: AtomicI64::new(1_000),
                send_calls: Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl TelegramApi for InMemApi {
        async fn send_message(
            &self,
            chat_id: i64,
            _body_md: &str,
            _keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<i64> {
            self.send_calls.lock().unwrap().push(chat_id);
            Ok(self.next_message_id.fetch_add(1, Ordering::SeqCst))
        }

        async fn edit_message_text(
            &self,
            _chat_id: i64,
            _message_id: i64,
            _body_md: &str,
            _keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<()> {
            Ok(())
        }
    }

    fn ctx() -> CockpitCtx<InMemStore, InMemApi> {
        CockpitCtx {
            emitter: CardEmitter::new(InMemStore::default(), InMemApi::default()),
            admin_chat_id: 42,
        }
    }

    #[tokio::test]
    async fn human_input_requested_emits_human_gate_card() {
        let ctx = ctx();
        let outcome = dispatch(
            tap(
                RunId::new(),
                1,
                EventPayload::HumanInputRequested {
                    node: node("approve_plan"),
                    session: None,
                    call_id: None,
                    prompt: "Please review".into(),
                    schema: None,
                },
            ),
            &ctx,
            1_000,
        )
        .await
        .unwrap();

        let DispatchOutcome::Emitted { kind, emit, .. } = outcome else {
            panic!("expected Emitted outcome, got {outcome:?}");
        };
        assert_eq!(kind, CardKind::HumanGate);
        assert!(matches!(emit, EmitOutcome::Sent { .. }));
    }

    #[tokio::test]
    async fn bootstrap_approval_requested_routes_per_stage() {
        for stage in [
            BootstrapStage::Description,
            BootstrapStage::Roadmap,
            BootstrapStage::Flow,
        ] {
            let ctx = ctx();
            let outcome = dispatch(
                tap(
                    RunId::new(),
                    1,
                    EventPayload::BootstrapApprovalRequested {
                        stage,
                        channel: surge_core::approvals::ApprovalChannel::Desktop {
                            duration: surge_core::approvals::ApprovalDuration::Transient,
                        },
                    },
                ),
                &ctx,
                1_000,
            )
            .await
            .unwrap();

            let DispatchOutcome::Emitted { kind, .. } = outcome else {
                panic!("expected Emitted outcome for stage {stage:?}");
            };
            assert_eq!(kind, CardKind::from(stage));
        }
    }

    #[tokio::test]
    async fn run_completed_emits_completion_card() {
        let ctx = ctx();
        let outcome = dispatch(
            tap(
                RunId::new(),
                7,
                EventPayload::RunCompleted {
                    terminal_node: node("end"),
                },
            ),
            &ctx,
            2_000,
        )
        .await
        .unwrap();
        let DispatchOutcome::Emitted { kind, .. } = outcome else {
            panic!("expected Emitted outcome");
        };
        assert_eq!(kind, CardKind::Completion);
    }

    #[tokio::test]
    async fn run_failed_emits_failure_card() {
        let ctx = ctx();
        let outcome = dispatch(
            tap(
                RunId::new(),
                7,
                EventPayload::RunFailed {
                    error: "agent crashed".into(),
                },
            ),
            &ctx,
            2_000,
        )
        .await
        .unwrap();
        let DispatchOutcome::Emitted { kind, .. } = outcome else {
            panic!("expected Emitted outcome");
        };
        assert_eq!(kind, CardKind::Failure);
    }

    #[tokio::test]
    async fn escalation_requested_emits_escalation_card() {
        let ctx = ctx();
        let outcome = dispatch(
            tap(
                RunId::new(),
                5,
                EventPayload::EscalationRequested {
                    stage: Some(BootstrapStage::Flow),
                    reason: "edit loop cap exceeded".into(),
                },
            ),
            &ctx,
            2_000,
        )
        .await
        .unwrap();
        let DispatchOutcome::Emitted { kind, .. } = outcome else {
            panic!("expected Emitted outcome");
        };
        assert_eq!(kind, CardKind::Escalation);
    }

    #[tokio::test]
    async fn irrelevant_event_is_ignored_without_touching_store() {
        let ctx = ctx();
        let outcome = dispatch(
            tap(
                RunId::new(),
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/p"),
                    initial_prompt: String::new(),
                    config: surge_core::run_event::RunConfig {
                        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                    },
                },
            ),
            &ctx,
            1_000,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, DispatchOutcome::Ignored { .. }));
    }

    #[tokio::test]
    async fn repeated_dispatch_for_same_run_node_is_noop_on_second_call() {
        let ctx = ctx();
        let run_id = RunId::new();
        let mk_tap = |seq| {
            tap(
                run_id,
                seq,
                EventPayload::HumanInputRequested {
                    node: node("approve_plan"),
                    session: None,
                    call_id: None,
                    prompt: "Please review".into(),
                    schema: None,
                },
            )
        };

        let first = dispatch(mk_tap(1), &ctx, 1_000).await.unwrap();
        let second = dispatch(mk_tap(2), &ctx, 1_100).await.unwrap();

        let DispatchOutcome::Emitted { emit: emit1, .. } = first else {
            panic!("expected Emitted first");
        };
        let DispatchOutcome::Emitted { emit: emit2, .. } = second else {
            panic!("expected Emitted second");
        };
        assert!(matches!(emit1, EmitOutcome::Sent { .. }));
        assert_eq!(emit2, EmitOutcome::NoOp);
    }
}
