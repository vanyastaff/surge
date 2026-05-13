//! Recovery reconciler — startup pass that walks every open card and
//! reconciles it against the current run state.
//!
//! Decisions 8 and 10 of the Telegram cockpit milestone plan: on daemon
//! restart the cockpit reads `find_open()`, derives the run state for
//! each row, and either edits the card to its terminal body and closes it
//! or leaves the row untouched for the live dispatcher to handle. **No
//! `sendMessage` is ever issued from this path** — duplicates would be
//! user-visible on a card the operator may already have responded to.

use surge_core::id::RunId;

use crate::card::emit::{CardStore, TelegramApi};
use crate::card::render::{render_completion, render_failure};
use crate::commands::status::RunSnapshotProvider;
use crate::error::Result;

/// Outcome counts from [`reconcile_open_cards`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Total open cards observed at reconcile time.
    pub total: usize,
    /// Cards whose run had terminated; edited to a terminal body and
    /// closed.
    pub closed: usize,
    /// Cards whose run was still in progress; left for the live
    /// dispatcher to update on the next tap event.
    pub skipped_running: usize,
    /// Cards whose run id could not be parsed back into a [`RunId`].
    /// Closed with no body edit so they do not stay open forever.
    pub skipped_bad_run_id: usize,
    /// Cards whose run had no snapshot (likely the run never started or
    /// was pruned). Same treatment as `skipped_bad_run_id`.
    pub skipped_unknown_run: usize,
}

/// Walk every open card and reconcile it against the persisted run state.
///
/// `snapshots` is the read-API the cockpit's `/status` command also uses;
/// `api` is the Bot API surface (only `edit_message_text` is invoked from
/// this path); `store` finds open cards and applies the close-or-edit
/// outcome.
///
/// # Errors
///
/// Surfaces anything the underlying [`CardStore`], [`TelegramApi`], or
/// [`RunSnapshotProvider`] return.
pub async fn reconcile_open_cards<S, A, P>(
    store: &S,
    snapshots: &P,
    api: &A,
    now_ms: i64,
) -> Result<ReconcileReport>
where
    S: CardStore,
    A: TelegramApi,
    P: RunSnapshotProvider,
{
    let open = store.find_open().await?;
    let mut report = ReconcileReport {
        total: open.len(),
        ..ReconcileReport::default()
    };

    for card in open {
        let run_id = match card.run_id.parse::<RunId>() {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(
                    target: "telegram::recover",
                    card_id = %card.card_id,
                    run_id = %card.run_id,
                    error = %err,
                    "cannot parse run_id — closing card without edit",
                );
                store.close(&card.card_id, now_ms).await?;
                report.skipped_bad_run_id += 1;
                continue;
            },
        };

        let Some(snapshot) = snapshots.snapshot(run_id).await? else {
            tracing::info!(
                target: "telegram::recover",
                card_id = %card.card_id,
                %run_id,
                "no snapshot — closing card without edit",
            );
            store.close(&card.card_id, now_ms).await?;
            report.skipped_unknown_run += 1;
            continue;
        };

        if !snapshot.terminal {
            tracing::debug!(
                target: "telegram::recover",
                card_id = %card.card_id,
                %run_id,
                "card still pending — leaving for live dispatcher",
            );
            report.skipped_running += 1;
            continue;
        }

        let rendered = if snapshot.failed {
            let reason = snapshot
                .last_outcome
                .clone()
                .unwrap_or_else(|| "(no error)".to_owned());
            render_failure(&card.card_id, &run_id, &reason)
        } else {
            let terminal_node = snapshot
                .active_node
                .clone()
                .unwrap_or_else(|| "end".to_owned());
            render_completion(&card.card_id, &run_id, &terminal_node)
        };

        if let Some(message_id) = card.message_id {
            api.edit_message_text(
                card.chat_id,
                message_id,
                &rendered.body_md,
                &rendered.keyboard,
            )
            .await?;
        }
        store.close(&card.card_id, now_ms).await?;
        report.closed += 1;

        tracing::info!(
            target: "telegram::recover",
            card_id = %card.card_id,
            %run_id,
            failed = snapshot.failed,
            "card reconciled and closed",
        );
    }

    tracing::info!(
        target: "telegram::recover",
        total = report.total,
        closed = report.closed,
        skipped_running = report.skipped_running,
        skipped_bad_run_id = report.skipped_bad_run_id,
        skipped_unknown_run = report.skipped_unknown_run,
        "reconcile complete",
    );

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use surge_notify::telegram::InboxKeyboardButton;
    use surge_persistence::runs::RunStatusSnapshot;
    use surge_persistence::telegram::cards::Card;

    use crate::commands::status::RunSnapshotProvider;

    /// In-memory `CardStore` backed by a `Vec<Card>`.
    #[derive(Default)]
    struct InMemStore {
        cards: Mutex<Vec<Card>>,
    }

    impl InMemStore {
        fn insert(&self, card: Card) {
            self.cards.lock().unwrap().push(card);
        }
        fn snapshot(&self) -> Vec<Card> {
            self.cards.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CardStore for InMemStore {
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
            unreachable!("recover tests do not exercise upsert")
        }

        async fn find_by_id(&self, _card_id: &str) -> Result<Option<Card>> {
            unreachable!("recover tests do not exercise find_by_id")
        }

        async fn mark_message_sent(
            &self,
            _card_id: &str,
            _message_id: i64,
            _content_hash: &str,
            _now_ms: i64,
        ) -> Result<()> {
            unreachable!("recover tests do not exercise mark_message_sent")
        }

        async fn update_content_hash(
            &self,
            _card_id: &str,
            _new_hash: &str,
            _now_ms: i64,
        ) -> Result<bool> {
            unreachable!("recover tests do not exercise update_content_hash")
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

    /// Records every Bot API call made on the recover path.
    #[derive(Default)]
    struct RecordingApi {
        send_calls: Mutex<Vec<()>>,
        edit_calls: Mutex<Vec<(i64, i64)>>,
    }

    impl RecordingApi {
        fn send_count(&self) -> usize {
            self.send_calls.lock().unwrap().len()
        }
        fn edit_count(&self) -> usize {
            self.edit_calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl TelegramApi for RecordingApi {
        async fn send_message(
            &self,
            _chat_id: i64,
            _body_md: &str,
            _keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<i64> {
            self.send_calls.lock().unwrap().push(());
            Ok(0)
        }

        async fn edit_message_text(
            &self,
            chat_id: i64,
            message_id: i64,
            _body_md: &str,
            _keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<()> {
            self.edit_calls.lock().unwrap().push((chat_id, message_id));
            Ok(())
        }
    }

    /// `RunSnapshotProvider` impl backed by a `HashMap<RunId, snapshot>`.
    /// `None` entries explicitly model "run not found".
    #[derive(Default)]
    struct FakeSnapshots {
        map: Mutex<HashMap<RunId, Option<RunStatusSnapshot>>>,
    }

    impl FakeSnapshots {
        fn set(&self, run_id: RunId, snap: Option<RunStatusSnapshot>) {
            self.map.lock().unwrap().insert(run_id, snap);
        }
    }

    #[async_trait]
    impl RunSnapshotProvider for FakeSnapshots {
        async fn snapshot(&self, run_id: RunId) -> Result<Option<RunStatusSnapshot>> {
            Ok(self.map.lock().unwrap().get(&run_id).cloned().flatten())
        }
    }

    fn open_card(card_id: &str, run_id_str: &str, message_id: Option<i64>) -> Card {
        Card {
            card_id: card_id.to_owned(),
            run_id: run_id_str.to_owned(),
            node_key: "approve_plan".to_owned(),
            attempt_index: 0,
            kind: "human_gate".to_owned(),
            chat_id: 42,
            message_id,
            content_hash: "hash".to_owned(),
            pending_edit_prompt_message_id: None,
            created_at: 1_000,
            updated_at: 1_000,
            closed_at: None,
        }
    }

    #[tokio::test]
    async fn empty_store_produces_zero_counts() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();
        let report = reconcile_open_cards(&store, &snaps, &api, 2_000)
            .await
            .unwrap();
        assert_eq!(report, ReconcileReport::default());
        assert_eq!(api.send_count(), 0);
        assert_eq!(api.edit_count(), 0);
    }

    #[tokio::test]
    async fn completed_run_card_gets_edited_and_closed() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();

        let run_id = RunId::new();
        store.insert(open_card("card-1", &run_id.to_string(), Some(555)));

        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.terminal = true;
        snap.active_node = Some("end".into());
        snaps.set(run_id, Some(snap));

        let report = reconcile_open_cards(&store, &snaps, &api, 3_000)
            .await
            .unwrap();
        assert_eq!(report.closed, 1);
        assert_eq!(report.skipped_running, 0);
        assert_eq!(api.edit_count(), 1);
        // CRITICAL: never sendMessage on resume.
        assert_eq!(api.send_count(), 0);

        let cards_after = store.snapshot();
        assert_eq!(cards_after[0].closed_at, Some(3_000));
    }

    #[tokio::test]
    async fn failed_run_card_gets_failure_body_and_closed() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();

        let run_id = RunId::new();
        store.insert(open_card("card-1", &run_id.to_string(), Some(555)));

        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.terminal = true;
        snap.failed = true;
        snap.last_outcome = Some("agent_crashed".into());
        snaps.set(run_id, Some(snap));

        let report = reconcile_open_cards(&store, &snaps, &api, 3_000)
            .await
            .unwrap();
        assert_eq!(report.closed, 1);
        assert_eq!(api.edit_count(), 1);
        assert_eq!(api.send_count(), 0);
    }

    #[tokio::test]
    async fn running_run_card_is_left_alone() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();

        let run_id = RunId::new();
        store.insert(open_card("card-1", &run_id.to_string(), Some(555)));

        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.active_node = Some("approve_plan".into());
        snap.terminal = false;
        snaps.set(run_id, Some(snap));

        let report = reconcile_open_cards(&store, &snaps, &api, 3_000)
            .await
            .unwrap();
        assert_eq!(report.skipped_running, 1);
        assert_eq!(report.closed, 0);
        assert_eq!(api.send_count(), 0);
        assert_eq!(api.edit_count(), 0);

        // Card row must still be open.
        let cards_after = store.snapshot();
        assert!(cards_after[0].closed_at.is_none());
    }

    #[tokio::test]
    async fn unparseable_run_id_closes_card_without_edit() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();
        store.insert(open_card("card-1", "garbage", Some(555)));

        let report = reconcile_open_cards(&store, &snaps, &api, 3_000)
            .await
            .unwrap();
        assert_eq!(report.skipped_bad_run_id, 1);
        assert_eq!(api.edit_count(), 0);
        assert_eq!(api.send_count(), 0);

        let cards_after = store.snapshot();
        assert_eq!(cards_after[0].closed_at, Some(3_000));
    }

    #[tokio::test]
    async fn unknown_run_closes_card_without_edit() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();

        let run_id = RunId::new();
        store.insert(open_card("card-1", &run_id.to_string(), Some(555)));
        snaps.set(run_id, None);

        let report = reconcile_open_cards(&store, &snaps, &api, 3_000)
            .await
            .unwrap();
        assert_eq!(report.skipped_unknown_run, 1);
        assert_eq!(api.edit_count(), 0);
        assert_eq!(api.send_count(), 0);

        let cards_after = store.snapshot();
        assert_eq!(cards_after[0].closed_at, Some(3_000));
    }

    #[tokio::test]
    async fn card_without_message_id_still_closes_but_skips_edit() {
        // Edge case: a card was upsert'd but the initial sendMessage never
        // succeeded (daemon crashed between rows). On resume we should
        // close it without trying to edit a message that doesn't exist.
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();

        let run_id = RunId::new();
        store.insert(open_card("card-1", &run_id.to_string(), None));

        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.terminal = true;
        snaps.set(run_id, Some(snap));

        let report = reconcile_open_cards(&store, &snaps, &api, 3_000)
            .await
            .unwrap();
        assert_eq!(report.closed, 1);
        assert_eq!(api.edit_count(), 0);
        assert_eq!(api.send_count(), 0);
    }

    #[tokio::test]
    async fn multiple_cards_aggregate_into_one_report() {
        let store = InMemStore::default();
        let snaps = FakeSnapshots::default();
        let api = RecordingApi::default();

        let r_done = RunId::new();
        let r_failed = RunId::new();
        let r_running = RunId::new();

        store.insert(open_card("c-done", &r_done.to_string(), Some(1)));
        store.insert(open_card("c-failed", &r_failed.to_string(), Some(2)));
        store.insert(open_card("c-running", &r_running.to_string(), Some(3)));

        let mut s_done = RunStatusSnapshot::empty(r_done);
        s_done.terminal = true;
        snaps.set(r_done, Some(s_done));

        let mut s_failed = RunStatusSnapshot::empty(r_failed);
        s_failed.terminal = true;
        s_failed.failed = true;
        snaps.set(r_failed, Some(s_failed));

        let s_running = RunStatusSnapshot::empty(r_running);
        snaps.set(r_running, Some(s_running));

        let report = reconcile_open_cards(&store, &snaps, &api, 4_000)
            .await
            .unwrap();
        assert_eq!(report.total, 3);
        assert_eq!(report.closed, 2);
        assert_eq!(report.skipped_running, 1);
        assert_eq!(api.edit_count(), 2);
        assert_eq!(api.send_count(), 0);
    }
}
