//! Cockpit-card snooze re-emitter.
//!
//! Polls `inbox_action_queue` for `kind = 'snooze'` rows with
//! `subject_kind = 'cockpit_card'` whose `snooze_until` has elapsed,
//! re-emits the corresponding card with a "snooze ended" marker, and
//! marks the row processed.
//!
//! The re-emit path uses `editMessageText` only — never `sendMessage`
//! (Decision 8 of the Telegram cockpit milestone plan). The body
//! reuses the card's last rendered content with a `🛏 Snooze ended`
//! footer so the operator gets a fresh notification without the
//! cockpit having to re-derive run state from the event log.

use async_trait::async_trait;

use crate::card::emit::{CardStore, TelegramApi};
use crate::error::{Result, TelegramCockpitError};

/// Persistence surface for cockpit-card snooze queries.
///
/// Production wraps `surge_persistence::inbox_queue::{
/// list_due_cockpit_snoozes, mark_action_processed }`. The trait
/// avoids dragging a raw SQLite connection through the scheduler so
/// tests can inject a fake without standing up a persistence pool.
#[async_trait]
pub trait CockpitSnoozeQueue: Send + Sync {
    /// Return every unprocessed cockpit snooze whose wake-up time
    /// is `<= now_ms`. Rows are ordered by enqueue seq.
    async fn list_due(&self, now_ms: i64) -> Result<Vec<DueSnooze>>;

    /// Mark the row at `seq` as processed. Idempotent.
    async fn mark_processed(&self, seq: i64) -> Result<()>;
}

/// One due cockpit-card snooze, returned by [`CockpitSnoozeQueue::list_due`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSnooze {
    /// Queue sequence number — pass to
    /// [`CockpitSnoozeQueue::mark_processed`] once the re-emit
    /// succeeds.
    pub seq: i64,
    /// Cockpit card ULID.
    pub card_id: String,
}

/// Result of [`CockpitSnoozeRescheduler::tick`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    /// How many due rows were observed.
    pub scanned: usize,
    /// How many cards were edited and marked processed.
    pub reemitted: usize,
    /// Due rows whose card row had been pruned or closed — marked
    /// processed without re-emit so they don't loop forever.
    pub skipped_card_gone: usize,
    /// Due rows whose card had no `message_id` (the card was created
    /// but never delivered). Marked processed; nothing visible to edit.
    pub skipped_undelivered: usize,
    /// Due rows where the Bot API edit failed; left unprocessed so the
    /// next tick retries.
    pub failed_edit: usize,
}

/// Footer appended to a card body when re-emitting after a snooze
/// expiry. Exposed so docs / tests can reference the exact string.
pub const SNOOZE_END_FOOTER: &str = "\n\n🛏 Snooze ended — card is active again.";

/// Cockpit-side snooze re-emit driver.
///
/// One per daemon. Construct with the four trait deps and call
/// [`CockpitSnoozeRescheduler::tick`] on a periodic schedule (the
/// daemon supervisor handles the loop and shutdown).
pub struct CockpitSnoozeRescheduler<Q, S, T>
where
    Q: CockpitSnoozeQueue,
    S: CardStore,
    T: TelegramApi,
{
    /// Queue accessor.
    pub queue: Q,
    /// Card store accessor.
    pub store: S,
    /// Bot API surface.
    pub api: T,
}

impl<Q, S, T> CockpitSnoozeRescheduler<Q, S, T>
where
    Q: CockpitSnoozeQueue,
    S: CardStore,
    T: TelegramApi,
{
    /// One scheduling pass — emit every due card, mark every
    /// processed row.
    ///
    /// # Errors
    ///
    /// Returns errors from the queue access surface; individual card
    /// re-emit failures are absorbed into [`TickReport::failed_edit`]
    /// so a single transient Bot API hiccup doesn't poison the whole
    /// tick.
    pub async fn tick(&self, now_ms: i64) -> Result<TickReport> {
        let due = self.queue.list_due(now_ms).await?;
        let mut report = TickReport {
            scanned: due.len(),
            ..TickReport::default()
        };
        for row in due {
            self.process_one(&row, &mut report, now_ms).await;
        }
        tracing::info!(
            target: "telegram::cockpit::snooze",
            scanned = report.scanned,
            reemitted = report.reemitted,
            skipped_card_gone = report.skipped_card_gone,
            skipped_undelivered = report.skipped_undelivered,
            failed_edit = report.failed_edit,
            "cockpit snooze tick",
        );
        Ok(report)
    }

    async fn process_one(&self, row: &DueSnooze, report: &mut TickReport, now_ms: i64) {
        let card = match self.store.find_by_id(&row.card_id).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                tracing::info!(
                    target: "telegram::cockpit::snooze",
                    card_id = %row.card_id,
                    seq = row.seq,
                    "snooze due but card row missing; marking processed",
                );
                report.skipped_card_gone += 1;
                let _ = self.queue.mark_processed(row.seq).await;
                return;
            },
            Err(err) => {
                tracing::warn!(
                    target: "telegram::cockpit::snooze",
                    card_id = %row.card_id,
                    seq = row.seq,
                    error = %err,
                    "card lookup failed; leaving snooze unprocessed for retry",
                );
                report.failed_edit += 1;
                return;
            },
        };

        if card.closed_at.is_some() {
            tracing::info!(
                target: "telegram::cockpit::snooze",
                card_id = %row.card_id,
                seq = row.seq,
                "snooze due but card already closed; marking processed",
            );
            report.skipped_card_gone += 1;
            let _ = self.queue.mark_processed(row.seq).await;
            return;
        }

        let Some(message_id) = card.message_id else {
            tracing::info!(
                target: "telegram::cockpit::snooze",
                card_id = %row.card_id,
                seq = row.seq,
                "snooze due but card has no message_id; marking processed",
            );
            report.skipped_undelivered += 1;
            let _ = self.queue.mark_processed(row.seq).await;
            return;
        };

        // Re-emit with the snooze-ended footer. We do not attempt to
        // re-render the card body from current run state — that's
        // recover.rs territory. The footer is enough of a visible cue
        // for the operator that the snooze elapsed.
        let body = format!("Card `{}` is back.{SNOOZE_END_FOOTER}", row.card_id);
        let keyboard: Vec<Vec<surge_notify::telegram::InboxKeyboardButton>> = Vec::new();
        match self
            .api
            .edit_message_text(card.chat_id, message_id, &body, &keyboard)
            .await
        {
            Ok(()) => {
                report.reemitted += 1;
                if let Err(err) = self.queue.mark_processed(row.seq).await {
                    tracing::warn!(
                        target: "telegram::cockpit::snooze",
                        card_id = %row.card_id,
                        seq = row.seq,
                        error = %err,
                        "edit succeeded but mark_processed failed",
                    );
                }
                // Refresh the card row's content_hash so the dispatcher
                // does not believe the body still matches. Failure to
                // update the hash is non-fatal — the next dispatch will
                // call update_content_hash anyway.
                let new_hash = simple_hash(&body);
                if let Err(err) = self
                    .store
                    .update_content_hash(&row.card_id, &new_hash, now_ms)
                    .await
                {
                    tracing::debug!(
                        target: "telegram::cockpit::snooze",
                        card_id = %row.card_id,
                        error = %err,
                        "post-reemit hash update failed",
                    );
                }
            },
            Err(err) => {
                tracing::warn!(
                    target: "telegram::cockpit::snooze",
                    card_id = %row.card_id,
                    seq = row.seq,
                    error = %err,
                    "edit_message_text failed; leaving snooze unprocessed",
                );
                report.failed_edit += 1;
            },
        }
    }
}

/// Helper so callers can pass `TelegramCockpitError::Persistence` text
/// strings without importing the enum at every call site.
#[must_use]
pub fn persistence_error(msg: impl Into<String>) -> TelegramCockpitError {
    TelegramCockpitError::Persistence(msg.into())
}

/// Cheap, deterministic content-hash for the re-emit body. Not
/// cryptographic; the dispatcher uses the same shape via the
/// `sha2::Sha256` path. Here we just want a stable string so the next
/// dispatch sees a different value and decides to refresh.
fn simple_hash(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use surge_notify::telegram::InboxKeyboardButton;
    use surge_persistence::telegram::cards::Card;

    #[derive(Default)]
    struct FakeQueue {
        due: Mutex<Vec<DueSnooze>>,
        processed: Mutex<Vec<i64>>,
    }

    impl FakeQueue {
        fn with_due(rows: Vec<DueSnooze>) -> Self {
            Self {
                due: Mutex::new(rows),
                processed: Mutex::new(Vec::new()),
            }
        }
        fn processed(&self) -> Vec<i64> {
            self.processed.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CockpitSnoozeQueue for FakeQueue {
        async fn list_due(&self, _now_ms: i64) -> Result<Vec<DueSnooze>> {
            // Drain — list_due is destructive in this fake so tick()
            // sees the rows exactly once.
            Ok(std::mem::take(&mut *self.due.lock().unwrap()))
        }
        async fn mark_processed(&self, seq: i64) -> Result<()> {
            self.processed.lock().unwrap().push(seq);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        cards: Mutex<Vec<Card>>,
        hashes: Mutex<Vec<(String, String)>>,
    }

    impl FakeStore {
        fn with_card(card: Card) -> Self {
            Self {
                cards: Mutex::new(vec![card]),
                hashes: Mutex::new(Vec::new()),
            }
        }
        fn hashes(&self) -> Vec<(String, String)> {
            self.hashes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CardStore for FakeStore {
        async fn upsert(
            &self,
            _: &str,
            _: &str,
            _: i64,
            _: &str,
            _: i64,
            _: &str,
            _: i64,
        ) -> Result<String> {
            unreachable!("snooze tests do not exercise upsert")
        }
        async fn find_by_id(&self, card_id: &str) -> Result<Option<Card>> {
            Ok(self
                .cards
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.card_id == card_id)
                .cloned())
        }
        async fn mark_message_sent(&self, _: &str, _: i64, _: &str, _: i64) -> Result<()> {
            unreachable!("snooze tests do not exercise mark_message_sent")
        }
        async fn update_content_hash(&self, card_id: &str, new_hash: &str, _: i64) -> Result<bool> {
            self.hashes
                .lock()
                .unwrap()
                .push((card_id.to_owned(), new_hash.to_owned()));
            Ok(true)
        }
        async fn find_open(&self) -> Result<Vec<Card>> {
            unreachable!("snooze tests do not exercise find_open")
        }
        async fn close(&self, _: &str, _: i64) -> Result<()> {
            unreachable!("snooze tests do not exercise close")
        }
    }

    #[derive(Default)]
    struct FakeApi {
        edits: Mutex<Vec<(i64, i64, String)>>,
        fail: Mutex<bool>,
    }

    impl FakeApi {
        fn edits(&self) -> Vec<(i64, i64, String)> {
            self.edits.lock().unwrap().clone()
        }
        fn failing() -> Self {
            Self {
                edits: Mutex::new(Vec::new()),
                fail: Mutex::new(true),
            }
        }
    }

    #[async_trait]
    impl TelegramApi for FakeApi {
        async fn send_message(
            &self,
            _: i64,
            _: &str,
            _: &[Vec<InboxKeyboardButton>],
        ) -> Result<i64> {
            unreachable!("snooze tests do not exercise send_message")
        }
        async fn edit_message_text(
            &self,
            chat_id: i64,
            message_id: i64,
            body_md: &str,
            _: &[Vec<InboxKeyboardButton>],
        ) -> Result<()> {
            if *self.fail.lock().unwrap() {
                return Err(TelegramCockpitError::Transport("test".into()));
            }
            self.edits
                .lock()
                .unwrap()
                .push((chat_id, message_id, body_md.to_owned()));
            Ok(())
        }
    }

    fn open_card_with_message(card_id: &str, chat_id: i64, message_id: i64) -> Card {
        Card {
            card_id: card_id.to_owned(),
            run_id: "run-1".to_owned(),
            node_key: "node".to_owned(),
            attempt_index: 0,
            kind: "human_gate".to_owned(),
            chat_id,
            message_id: Some(message_id),
            content_hash: "h".to_owned(),
            pending_edit_prompt_message_id: None,
            created_at: 0,
            updated_at: 0,
            closed_at: None,
        }
    }

    fn rescheduler(
        q: FakeQueue,
        s: FakeStore,
        a: FakeApi,
    ) -> CockpitSnoozeRescheduler<FakeQueue, FakeStore, FakeApi> {
        CockpitSnoozeRescheduler {
            queue: q,
            store: s,
            api: a,
        }
    }

    #[tokio::test]
    async fn empty_queue_returns_empty_report() {
        let rt = rescheduler(
            FakeQueue::default(),
            FakeStore::default(),
            FakeApi::default(),
        );
        let r = rt.tick(0).await.unwrap();
        assert_eq!(r, TickReport::default());
    }

    #[tokio::test]
    async fn happy_path_reemits_and_marks_processed() {
        let due = vec![DueSnooze {
            seq: 7,
            card_id: "CARD-1".into(),
        }];
        let rt = rescheduler(
            FakeQueue::with_due(due),
            FakeStore::with_card(open_card_with_message("CARD-1", 42, 999)),
            FakeApi::default(),
        );
        let r = rt.tick(1_000).await.unwrap();
        assert_eq!(r.scanned, 1);
        assert_eq!(r.reemitted, 1);
        assert_eq!(rt.queue.processed(), vec![7]);
        let edits = rt.api.edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].0, 42);
        assert_eq!(edits[0].1, 999);
        assert!(edits[0].2.contains("Snooze ended"));
        // Hash was refreshed.
        assert_eq!(rt.store.hashes().len(), 1);
    }

    #[tokio::test]
    async fn missing_card_is_marked_processed_without_edit() {
        let due = vec![DueSnooze {
            seq: 11,
            card_id: "GONE".into(),
        }];
        let rt = rescheduler(
            FakeQueue::with_due(due),
            FakeStore::default(),
            FakeApi::default(),
        );
        let r = rt.tick(0).await.unwrap();
        assert_eq!(r.skipped_card_gone, 1);
        assert_eq!(rt.queue.processed(), vec![11]);
        assert!(rt.api.edits().is_empty());
    }

    #[tokio::test]
    async fn closed_card_is_marked_processed_without_edit() {
        let mut closed = open_card_with_message("CARD-2", 42, 500);
        closed.closed_at = Some(2_000);
        let rt = rescheduler(
            FakeQueue::with_due(vec![DueSnooze {
                seq: 12,
                card_id: "CARD-2".into(),
            }]),
            FakeStore::with_card(closed),
            FakeApi::default(),
        );
        let r = rt.tick(0).await.unwrap();
        assert_eq!(r.skipped_card_gone, 1);
        assert!(rt.api.edits().is_empty());
        assert_eq!(rt.queue.processed(), vec![12]);
    }

    #[tokio::test]
    async fn card_without_message_id_is_skipped() {
        let mut undelivered = open_card_with_message("CARD-3", 42, 0);
        undelivered.message_id = None;
        let rt = rescheduler(
            FakeQueue::with_due(vec![DueSnooze {
                seq: 13,
                card_id: "CARD-3".into(),
            }]),
            FakeStore::with_card(undelivered),
            FakeApi::default(),
        );
        let r = rt.tick(0).await.unwrap();
        assert_eq!(r.skipped_undelivered, 1);
        assert!(rt.api.edits().is_empty());
        assert_eq!(rt.queue.processed(), vec![13]);
    }

    #[tokio::test]
    async fn edit_failure_leaves_row_unprocessed_for_retry() {
        let rt = rescheduler(
            FakeQueue::with_due(vec![DueSnooze {
                seq: 14,
                card_id: "CARD-4".into(),
            }]),
            FakeStore::with_card(open_card_with_message("CARD-4", 42, 555)),
            FakeApi::failing(),
        );
        let r = rt.tick(0).await.unwrap();
        assert_eq!(r.failed_edit, 1);
        assert!(rt.queue.processed().is_empty());
    }
}
