//! `TicketStateSync` — drives `ticket_index` FSM from engine `RunHandle`
//! events and posts tracker comments.

use std::sync::Arc;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::intake::{IntakeRepo, TicketState};
use surge_persistence::runs::storage::Storage;
use tracing::{info, warn};

/// Per-run subscriber that mirrors engine state into `ticket_index` and
/// posts tracker comments on terminal events.
pub struct TicketStateSync {
    task_id: TaskId,
    storage: Arc<Storage>,
    source: Arc<dyn TaskSource>,
}

impl TicketStateSync {
    /// Construct with the originating ticket and the registered `TaskSource`.
    #[must_use]
    pub fn new(task_id: TaskId, storage: Arc<Storage>, source: Arc<dyn TaskSource>) -> Self {
        Self {
            task_id,
            storage,
            source,
        }
    }

    /// Drive the loop: consume events from `handle.events`, apply FSM
    /// transitions, and post tracker comments. Returns when the run
    /// reaches a terminal state or the broadcast sender is dropped.
    pub async fn run(self, mut handle: RunHandle) {
        info!(task_id = %self.task_id, run_id = %handle.run_id, "TicketStateSync started");
        let mut went_active = false;
        loop {
            match handle.events.recv().await {
                Ok(EngineRunEvent::Persisted { .. }) if !went_active => {
                    if let Err(e) = self.set_state(TicketState::Active).await {
                        warn!(error = %e, "transition to Active failed");
                    }
                    went_active = true;
                },
                Ok(EngineRunEvent::Terminal { outcome }) => {
                    self.on_terminal(&outcome).await;
                    return;
                },
                Ok(_) => {
                    // Unknown future variant — ignore and keep looping.
                },
                Err(_) => {
                    // Sender dropped — engine has exited. We exit silently
                    // (the engine will have written its own RunCompleted /
                    // RunFailed event already if it terminated normally).
                    return;
                },
            }
        }
    }

    // `on_terminal` (the other FSM-transition method on this type) awaits
    // real I/O (`self.source.post_comment(...)`); `async fn` here keeps both
    // transition methods uniform rather than exposing which one happens to
    // await nothing today.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "matches `on_terminal`'s async signature on this type; `std::future::ready`/`async move` alternatives would make this eagerly evaluate the registry write instead of lazily-until-polled like its sibling"
    )]
    async fn set_state(&self, to: TicketState) -> Result<(), String> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| e.to_string())?;
        IntakeRepo::new(&conn)
            .update_state_validated(self.task_id.as_str(), to)
            .map_err(|e| e.to_string())
    }

    async fn on_terminal(&self, outcome: &RunOutcome) {
        let (state, comment): (Option<TicketState>, String) = match outcome {
            RunOutcome::Completed { .. } => (
                Some(TicketState::Completed),
                "✅ Surge run complete.".to_string(),
            ),
            RunOutcome::Failed { error } => (
                Some(TicketState::Failed),
                format!("❌ Surge run failed: {error}"),
            ),
            RunOutcome::Aborted { reason } => (
                Some(TicketState::Aborted),
                format!("Surge run aborted: {reason}"),
            ),
            // Task 12 M3 (R37/R37.1): a parked run is not finished — it is
            // paused on a provider rate-limit window and resumes on its
            // own (Task 12 M4's wake scheduler; today, a manual
            // `Engine::resume_run`). Reporting `Failed` here would tell the
            // human the opposite of what happened: the system is working
            // as designed, not broken.
            //
            // No `TicketState` variant represents "waiting, will resume on
            // its own" today. `TicketState::Snoozed` is *not* it — that is
            // a different mechanism entirely (`SnoozeScheduler`, driven by
            // a ticket-level `snooze_until` an operator/automation set),
            // not `RunStatus::Parked`/`wake_at`; the Task 12 plan names
            // this exact non-overlap explicitly ("same shape, different
            // subject — do not merge on the second occurrence"). Reusing
            // it here would misrepresent this ticket the same way
            // collapsing to `Failed` does. Until a real variant exists —
            // an M4 decision, not this call site's — leaving the ticket's
            // state untouched (it stays whatever it already was, typically
            // `Active`) is the honest choice: true but incomplete beats
            // false.
            RunOutcome::Parked { wake_at } => (
                None,
                format!(
                    "⏸ Surge run paused until {wake_at} (provider rate limit reached); it \
                     will resume automatically once the window resets."
                ),
            ),
            // Policy, stated once, shared with `intake_completion.rs`'s
            // identical wildcard (Task 12 M3 review, "two tails now
            // diverge"): a genuinely unrecognized future `RunOutcome`
            // variant is treated the same way `Parked` is above — leave
            // the ticket's state untouched rather than guess a terminal
            // one. Guessing `Failed` (this arm's previous choice) or
            // `Aborted` (the sibling file's) is a coin flip on a fact
            // neither file has any way to know, and the `Parked` fix two
            // arms up exists specifically because a wrong guess reads as
            // an active lie, not a gap. `warn!`, not `info!`, because
            // unlike a known, named `Parked` pause, this case really is
            // unexpected — an operator should notice a build shipped a
            // `RunOutcome` variant this code does not recognize yet.
            _ => {
                warn!(
                    task_id = %self.task_id,
                    "on_terminal: unrecognized RunOutcome variant; leaving ticket state \
                     untouched rather than guessing a terminal one"
                );
                (
                    None,
                    "Surge run ended with an outcome this build does not recognize yet."
                        .to_string(),
                )
            },
        };
        if let Some(state) = state
            && let Err(e) = self.set_state(state).await
        {
            warn!(error = %e, ?state, "transition to terminal state failed");
        }
        if let Err(e) = self.source.post_comment(&self.task_id, &comment).await {
            warn!(error = %e, task_id = %self.task_id, "tracker comment on terminal failed");
        }
        info!(task_id = %self.task_id, ?state, "TicketStateSync done");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_intake::testing::MockTaskSource;
    use surge_persistence::intake::IntakeRow;

    /// A `TicketStateSync` over a fresh registry DB with one seeded ticket
    /// already `Active` (mirroring what the real `run()` loop does on its
    /// first `Persisted` event, before any terminal outcome arrives).
    async fn seeded_sync() -> (TicketStateSync, Arc<MockTaskSource>, Arc<Storage>) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let task_id = TaskId::try_new("mock:test#1").unwrap();
        {
            let conn = storage.acquire_registry_conn().unwrap();
            IntakeRepo::new(&conn)
                .insert(&IntakeRow {
                    task_id: task_id.as_str().to_string(),
                    source_id: "mock:test".into(),
                    provider: "mock".into(),
                    // `None`, not a foreign-keyed `runs.id` — this test
                    // drives `on_terminal` directly and never looks the
                    // ticket up by run_id, so there is nothing to satisfy
                    // the `runs` table's foreign key for.
                    run_id: None,
                    triage_decision: Some("enqueued".into()),
                    duplicate_of: None,
                    priority: Some("medium".into()),
                    state: TicketState::Active,
                    first_seen: chrono::Utc::now(),
                    last_seen: chrono::Utc::now(),
                    snooze_until: None,
                    callback_token: None,
                    tg_chat_id: None,
                    tg_message_id: None,
                })
                .unwrap();
        }
        let source = Arc::new(MockTaskSource::new("mock:test", "mock"));
        let sync = TicketStateSync::new(
            task_id,
            storage.clone(),
            source.clone() as Arc<dyn TaskSource>,
        );
        (sync, source, storage)
    }

    /// Task 12 M3: the behavioral point of this whole change. Before the
    /// fix, `on_terminal`'s catch-all folded a `Parked` outcome onto
    /// `Failed` (`"on_terminal: unknown RunOutcome variant; defaulting to
    /// Failed"`) — the opposite of what happened, since a parked run is
    /// working as designed and resumes on its own.
    #[tokio::test(flavor = "multi_thread")]
    async fn on_terminal_parked_leaves_ticket_state_untouched_and_posts_a_pause_comment() {
        let (sync, source, storage) = seeded_sync().await;
        let wake_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:05:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        sync.on_terminal(&RunOutcome::Parked { wake_at }).await;

        let comments = source.posted_comments().await;
        assert_eq!(comments.len(), 1, "expected exactly one comment");
        let body = &comments[0].1;
        assert!(
            body.contains("2026-01-01"),
            "comment must name the wake time, got: {body}"
        );
        let lower = body.to_lowercase();
        assert!(
            !lower.contains("fail") && !lower.contains("abort"),
            "comment must not read as a failure, got: {body}"
        );

        let conn = storage.acquire_registry_conn().unwrap();
        let row = IntakeRepo::new(&conn)
            .fetch("mock:test#1")
            .unwrap()
            .expect("ticket row must still exist");
        assert_eq!(
            row.state,
            TicketState::Active,
            "a parked run must not transition the ticket FSM — it is not finished, and no \
             TicketState represents \"paused, resumes on its own\" today"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn on_terminal_completed_still_transitions_as_before() {
        // Regression guard: the `Option<TicketState>` refactor must not
        // have quietly turned the three real terminal outcomes into no-ops.
        let (sync, source, storage) = seeded_sync().await;
        sync.on_terminal(&RunOutcome::Completed {
            terminal: surge_core::keys::NodeKey::try_from("end").unwrap(),
        })
        .await;

        assert_eq!(source.posted_comments().await.len(), 1);
        let conn = storage.acquire_registry_conn().unwrap();
        let row = IntakeRepo::new(&conn)
            .fetch("mock:test#1")
            .unwrap()
            .expect("ticket row must still exist");
        assert_eq!(row.state, TicketState::Completed);
    }
}
