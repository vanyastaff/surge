//! `TicketStateSync` — drives `ticket_index` FSM from engine `RunHandle`
//! events and atomically queues terminal tracker comments.

use crate::intake_completion::{
    durable_outcome, format_completion, post_comment_bounded, terminal_kind,
};
use std::sync::Arc;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::intake::IntakeRepo;
use surge_persistence::intake_outbox;
use surge_persistence::runs::storage::Storage;
use surge_persistence::runs::{Clock, SystemClock};
use tracing::{info, warn};

/// Per-run subscriber that mirrors engine state into `ticket_index` and
/// enqueues terminal comments for the shared durable delivery worker.
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
    /// transitions, and enqueue terminal tracker comments. Returns when the run
    /// reaches a terminal state or the broadcast sender is dropped.
    pub async fn run(self, mut handle: RunHandle) {
        info!(task_id = %self.task_id, run_id = %handle.run_id, "TicketStateSync started");
        let mut went_active = false;
        let mut observed = None;
        let mut events_open = true;
        loop {
            tokio::select! {
                biased;
                joined = &mut handle.completion => {
                    self.on_completion(handle.run_id, joined, observed).await;
                    return;
                },
                event = handle.events.recv(), if events_open && observed.is_none() => match event {
                    Ok(EngineRunEvent::Persisted { .. }) if !went_active => {
                        if let Err(error) = self.set_active(&handle.run_id) {
                            warn!(%error, "transition to Active failed");
                        }
                        went_active = true;
                    },
                    Ok(EngineRunEvent::Terminal { outcome }) => {
                        self.on_terminal(&handle.run_id, &outcome).await;
                        observed = Some(outcome);
                    },
                    Ok(_) => {},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "ticket subscriber lagged; checking durable run history");
                        if let Some(outcome) = durable_outcome(&self.storage, handle.run_id).await {
                            self.on_terminal(&handle.run_id, &outcome).await;
                            observed = Some(outcome);
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        events_open = false;
                        if let Some(outcome) = durable_outcome(&self.storage, handle.run_id).await {
                            self.on_terminal(&handle.run_id, &outcome).await;
                            observed = Some(outcome);
                        }
                    },
                },
            }
        }
    }

    async fn on_completion(
        &self,
        run_id: surge_core::RunId,
        joined: Result<RunOutcome, tokio::task::JoinError>,
        observed: Option<RunOutcome>,
    ) {
        match joined {
            Ok(outcome) => match observed {
                Some(previous) if previous != outcome => {
                    warn!(%run_id, ?previous, ?outcome, "ticket event disagrees with joined completion");
                },
                Some(_) => {},
                None => self.on_terminal(&run_id, &outcome).await,
            },
            Err(error) => {
                warn!(%run_id, %error, "ticket execution completion failed");
                if observed.is_none()
                    && let Some(outcome) = durable_outcome(&self.storage, run_id).await
                {
                    self.on_terminal(&run_id, &outcome).await;
                }
            },
        }
    }

    fn set_active(&self, run_id: &surge_core::RunId) -> Result<bool, String> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|error| error.to_string())?;
        IntakeRepo::new(&conn)
            .mark_run_active(self.task_id.as_str(), &run_id.to_string())
            .map_err(|error| error.to_string())
    }

    async fn on_terminal(&self, run_id: &surge_core::RunId, outcome: &RunOutcome) {
        let (comment, state, _) = format_completion(outcome);
        if let Some(kind) = terminal_kind(outcome) {
            let result = self
                .storage
                .acquire_registry_conn()
                .map_err(|error| error.to_string())
                .and_then(|conn| {
                    intake_outbox::enqueue_terminal(
                        &conn,
                        self.task_id.as_str(),
                        &run_id.to_string(),
                        kind,
                        &comment,
                        SystemClock.now_ms(),
                    )
                    .map_err(|error| error.to_string())
                });
            if let Err(error) = result {
                warn!(%error, "terminal ticket/comment transaction failed");
            }
            return;
        }
        {
            let conn = match self.storage.acquire_registry_conn() {
                Ok(conn) => conn,
                Err(error) => {
                    warn!(%error, "cannot check correlated ticket");
                    return;
                },
            };
            match IntakeRepo::new(&conn).fetch(self.task_id.as_str()) {
                Ok(Some(row))
                    if row.run_id.as_deref() == Some(&run_id.to_string())
                        && matches!(
                            row.state,
                            surge_persistence::intake::TicketState::Active
                                | surge_persistence::intake::TicketState::RunStarted
                        ) => {},
                _ => return,
            }
        }
        if let Err(error) =
            post_comment_bounded(self.source.as_ref(), &self.task_id, &comment).await
        {
            warn!(%error, task_id = %self.task_id, "tracker comment on terminal failed");
        }
        info!(task_id = %self.task_id, ?state, "TicketStateSync done");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_intake::testing::MockTaskSource;
    use surge_persistence::intake::{IntakeRow, TicketState};

    /// A `TicketStateSync` over a fresh registry DB with one seeded ticket
    /// already `Active` (mirroring what the real `run()` loop does on its
    /// first `Persisted` event, before any terminal outcome arrives).
    async fn seeded_sync() -> (
        TicketStateSync,
        Arc<MockTaskSource>,
        Arc<Storage>,
        surge_core::RunId,
        crate::runtime_home_fixture::FixtureHome,
    ) {
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let task_id = TaskId::try_new("mock:test#1").unwrap();
        {
            let conn = storage.acquire_registry_conn().unwrap();
            IntakeRepo::new(&conn)
                .insert(&IntakeRow {
                    task_id: task_id.as_str().to_string(),
                    source_id: "mock:test".into(),
                    provider: "mock".into(),
                    run_id: Some(run_id.to_string()),
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
        writer.close().await.unwrap();
        (sync, source, storage, run_id, dir)
    }

    /// Task 12 M3: the behavioral point of this whole change. Before the
    /// fix, `on_terminal`'s catch-all folded a `Parked` outcome onto
    /// `Failed` (`"on_terminal: unknown RunOutcome variant; defaulting to
    /// Failed"`) — the opposite of what happened, since a parked run is
    /// working as designed and resumes on its own.
    #[tokio::test(flavor = "multi_thread")]
    async fn on_terminal_parked_leaves_ticket_state_untouched_and_posts_a_pause_comment() {
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        let wake_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:05:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        sync.on_terminal(&run_id, &RunOutcome::Parked { wake_at })
            .await;

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
        drop(conn);
        drop(sync);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn on_terminal_completed_still_transitions_as_before() {
        // Regression guard: the `Option<TicketState>` refactor must not
        // have quietly turned the three real terminal outcomes into no-ops.
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        sync.on_terminal(
            &run_id,
            &RunOutcome::Completed {
                terminal: surge_core::keys::NodeKey::try_from("end").unwrap(),
            },
        )
        .await;

        assert_eq!(source.posted_comments().await.len(), 0);
        let conn = storage.acquire_registry_conn().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM terminal_comment_outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        let row = IntakeRepo::new(&conn)
            .fetch("mock:test#1")
            .unwrap()
            .expect("ticket row must still exist");
        assert_eq!(row.state, TicketState::Completed);
        drop(conn);
        drop(sync);
        drop(storage);
        dir.close().unwrap();
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_handle_cannot_promote_or_complete_reassigned_ticket() {
        let (sync, source, storage, old_run, dir) = seeded_sync().await;
        let new_run = surge_core::RunId::new();
        let writer = storage.create_run(new_run, "/new", None).await.unwrap();
        {
            let conn = storage.acquire_registry_conn().unwrap();
            conn.execute(
                "UPDATE ticket_index SET state = 'RunStarted', run_id = ?1",
                [new_run.to_string()],
            )
            .unwrap();
        }
        assert!(!sync.set_active(&old_run).unwrap());
        sync.on_terminal(
            &old_run,
            &RunOutcome::Failed {
                error: "old failure".into(),
            },
        )
        .await;
        assert_eq!(source.posted_comments().await.len(), 0);
        let conn = storage.acquire_registry_conn().unwrap();
        let row = IntakeRepo::new(&conn)
            .fetch("mock:test#1")
            .unwrap()
            .unwrap();
        assert_eq!(row.state, TicketState::RunStarted);
        assert_eq!(row.run_id, Some(new_run.to_string()));
        writer.close().await.unwrap();
        drop(conn);
        drop(sync);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn completed_ticket_cannot_be_reactivated_or_announced_as_parked() {
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        sync.on_terminal(
            &run_id,
            &RunOutcome::Aborted {
                reason: "stopped".into(),
            },
        )
        .await;
        assert!(!sync.set_active(&run_id).unwrap());
        sync.on_terminal(
            &run_id,
            &RunOutcome::Parked {
                wake_at: chrono::Utc::now(),
            },
        )
        .await;
        assert_eq!(source.posted_comments().await.len(), 0);
        let conn = storage.acquire_registry_conn().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM terminal_comment_outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            IntakeRepo::new(&conn)
                .fetch("mock:test#1")
                .unwrap()
                .unwrap()
                .state,
            TicketState::Aborted
        );
        drop(conn);
        drop(sync);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lagged_subscriber_continues_to_a_terminal_event() {
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        for _ in 0..2 {
            tx.send(EngineRunEvent::Persisted {
                seq: 1,
                payload: Box::new(surge_core::run_event::EventPayload::RunFailed {
                    error: "dropped".into(),
                }),
            })
            .unwrap();
        }
        tx.send(EngineRunEvent::Terminal {
            outcome: RunOutcome::Aborted {
                reason: "cancelled".into(),
            },
        })
        .unwrap();
        sync.run(RunHandle {
            run_id,
            events: rx,
            completion: tokio::spawn(async {
                RunOutcome::Aborted {
                    reason: "cancelled".into(),
                }
            }),
        })
        .await;
        assert_eq!(source.posted_comments().await.len(), 0);
        let conn = storage.acquire_registry_conn().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM terminal_comment_outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            IntakeRepo::new(&conn)
                .fetch("mock:test#1")
                .unwrap()
                .unwrap()
                .state,
            TicketState::Aborted
        );
        drop(conn);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_completion_settles_with_event_sender_still_open() {
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        let (sender, receiver) = tokio::sync::broadcast::channel(8);
        let completion = tokio::spawn(std::future::pending::<RunOutcome>());
        completion.abort();
        let settled = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sync.run(RunHandle {
                run_id,
                events: receiver,
                completion,
            }),
        )
        .await;
        assert!(
            settled.is_ok(),
            "failed completion must settle without waiting for event sender closure"
        );
        assert_eq!(fetch_ticket_state(&storage), TicketState::Active);
        drop(sender);
        drop(source);
        drop(storage);
        dir.close().unwrap();
    }

    fn fetch_ticket_state(storage: &Storage) -> TicketState {
        let conn = storage.acquire_registry_conn().unwrap();
        IntakeRepo::new(&conn)
            .fetch("mock:test#1")
            .unwrap()
            .unwrap()
            .state
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn successful_completion_settles_with_event_sender_still_open() {
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        let (sender, receiver) = tokio::sync::broadcast::channel(8);
        let completion = tokio::spawn(async {
            RunOutcome::Completed {
                terminal: surge_core::keys::NodeKey::try_new("end").unwrap(),
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sync.run(RunHandle {
                run_id,
                events: receiver,
                completion,
            }),
        )
        .await
        .unwrap();
        assert_eq!(fetch_ticket_state(&storage), TicketState::Completed);
        drop(sender);
        drop(source);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closed_events_still_wait_for_actual_completion() {
        let (sync, source, storage, run_id, dir) = seeded_sync().await;
        let (sender, receiver) = tokio::sync::broadcast::channel(8);
        drop(sender);
        let (release, pending) = tokio::sync::oneshot::channel();
        let completion = tokio::spawn(async { pending.await.unwrap() });
        let mut follower = Box::pin(sync.run(RunHandle {
            run_id,
            events: receiver,
            completion,
        }));
        assert!(futures::poll!(follower.as_mut()).is_pending());
        release
            .send(RunOutcome::Aborted {
                reason: "settled".into(),
            })
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), follower.as_mut())
            .await
            .unwrap();
        drop(follower);
        assert_eq!(fetch_ticket_state(&storage), TicketState::Aborted);
        drop(source);
        drop(storage);
        dir.close().unwrap();
    }
}
