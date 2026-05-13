//! `TaskRouter` — multiplex events from multiple `TaskSource`s into a single
//! channel, applying Tier-1 PreFilter on each event.
//!
//! Lives in `surge-intake` to keep storage + dedup + multiplexing close together.
//! `surge-daemon` instantiates this with the configured sources at startup.

use crate::Result;
use crate::dedup::Tier1PreFilter;
use crate::source::TaskSource;
use crate::types::{TaskEvent, TaskEventKind, Tier1Decision};
use futures::stream::{StreamExt, select_all};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

/// Output of the router for downstream consumers.
///
/// New-task events flow through Tier-1 PreFilter and emerge as
/// [`RouterOutput::Triage`] or [`RouterOutput::EarlyDuplicate`].
/// External-update events (status / labels / closed) bypass dedup and
/// surface as [`RouterOutput::ExternalUpdate`] so the daemon can
/// reflect them into the `ticket_index` FSM without paying the
/// triage-author LLM cost.
#[derive(Debug)]
#[non_exhaustive]
pub enum RouterOutput {
    /// Event should be triaged (Tier-1 said pass).
    Triage { event: TaskEvent },
    /// Tier-1 dedup said this is an early duplicate of an active run;
    /// downstream may post a comment to that effect.
    EarlyDuplicate { event: TaskEvent, run_id: String },
    /// Non-`NewTask` event — status change, label change, or close —
    /// forwarded directly without dedup. Downstream maps the event
    /// into a `ticket_index` state transition (and, where applicable,
    /// an `EngineFacade::stop_run` call for `Active` runs).
    ExternalUpdate { event: TaskEvent },
}

/// Multiplexer over a fixed set of `TaskSource`s.
///
/// Borrows a shared `Connection` (wrapped in `Arc<Mutex<...>>`) and forwards
/// every incoming event through Tier-1 PreFilter into the provided mpsc
/// channel. Source errors are logged at WARN; they do not stop the router.
pub struct TaskRouter {
    sources: Vec<Arc<dyn TaskSource>>,
    conn: Arc<Mutex<Connection>>,
    out_tx: mpsc::Sender<RouterOutput>,
}

impl TaskRouter {
    /// Construct a new router with the given sources, shared connection, and output channel.
    pub fn new(
        sources: Vec<Arc<dyn TaskSource>>,
        conn: Arc<Mutex<Connection>>,
        out_tx: mpsc::Sender<RouterOutput>,
    ) -> Self {
        Self {
            sources,
            conn,
            out_tx,
        }
    }

    /// Drive the router until all source streams finish or the output channel closes.
    /// In production the streams are infinite (polling loops) and this method runs
    /// until the daemon shuts down.
    pub async fn run(self) -> Result<()> {
        let streams = self
            .sources
            .iter()
            .map(|s| s.watch_for_tasks())
            .collect::<Vec<_>>();
        let mut multiplex = select_all(streams);

        while let Some(item) = multiplex.next().await {
            let event = match item {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "task source emitted error; continuing");
                    continue;
                },
            };
            let Some(out) = self.classify_event(event).await else {
                continue;
            };
            if self.out_tx.send(out).await.is_err() {
                info!("router output channel closed; stopping");
                return Ok(());
            }
        }

        Ok(())
    }

    /// Decide which [`RouterOutput`] variant an incoming event maps to.
    ///
    /// `NewTask` events flow through Tier-1 PreFilter and emerge as
    /// `Triage` or `EarlyDuplicate`. All other event kinds bypass dedup
    /// and surface as `ExternalUpdate`. Returns `None` only when dedup
    /// itself errored — the caller drops the event and continues.
    async fn classify_event(&self, event: TaskEvent) -> Option<RouterOutput> {
        if !matches!(event.kind, TaskEventKind::NewTask) {
            return Some(RouterOutput::ExternalUpdate { event });
        }
        let decision = {
            let conn = self.conn.lock().await;
            let pre = Tier1PreFilter::new(&conn);
            pre.check(&event)
        };
        match decision {
            Ok(Tier1Decision::Pass) => Some(RouterOutput::Triage { event }),
            Ok(Tier1Decision::EarlyDuplicate { run_id }) => {
                Some(RouterOutput::EarlyDuplicate { event, run_id })
            },
            Err(e) => {
                warn!(
                    error = %e,
                    task_id = %event.task_id,
                    "Tier-1 dedup failed; skipping event"
                );
                None
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockTaskSource;
    use crate::types::{TaskEvent, TaskEventKind, TaskId};
    use chrono::Utc;
    use rusqlite::Connection;
    use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
            .unwrap();
        let sql = include_str!(
            "../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql"
        );
        conn.execute_batch(sql).unwrap();
        let sql4 = include_str!(
            "../../surge-persistence/src/runs/migrations/registry/0004_inbox_callback_columns.sql"
        );
        conn.execute_batch(sql4).unwrap();
        conn
    }

    fn ev(task_id: &str) -> TaskEvent {
        TaskEvent {
            source_id: "mock:t".into(),
            task_id: TaskId::try_new(task_id).unwrap(),
            kind: TaskEventKind::NewTask,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn passes_through_new_event_as_triage() {
        let conn = Arc::new(Mutex::new(db()));
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#1")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src], Arc::clone(&conn), tx);

        let handle = tokio::spawn(router.run());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("did not receive within 1s")
            .expect("channel closed");
        match received {
            RouterOutput::Triage { event } => assert_eq!(event.task_id.as_str(), "mock:t#1"),
            other => panic!("expected Triage, got {other:?}"),
        }
        // Drop receiver so router exits cleanly.
        drop(rx);
        let _ = handle.await;
    }

    fn closed_event(task_id: &str) -> TaskEvent {
        TaskEvent {
            source_id: "mock:t".into(),
            task_id: TaskId::try_new(task_id).unwrap(),
            kind: TaskEventKind::TaskClosed,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    fn labels_changed_event(task_id: &str, added: Vec<String>) -> TaskEvent {
        TaskEvent {
            source_id: "mock:t".into(),
            task_id: TaskId::try_new(task_id).unwrap(),
            kind: TaskEventKind::LabelsChanged {
                added,
                removed: vec![],
            },
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn task_closed_emits_external_update_without_dedup() {
        let conn = Arc::new(Mutex::new(db()));
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(closed_event("mock:t#42")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src], Arc::clone(&conn), tx);

        let handle = tokio::spawn(router.run());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("did not receive within 1s")
            .expect("channel closed");
        match received {
            RouterOutput::ExternalUpdate { event } => {
                assert_eq!(event.task_id.as_str(), "mock:t#42");
                assert!(matches!(event.kind, TaskEventKind::TaskClosed));
            },
            other => panic!("expected ExternalUpdate, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn labels_changed_emits_external_update() {
        let conn = Arc::new(Mutex::new(db()));
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(labels_changed_event(
            "mock:t#7",
            vec!["surge:disabled".into()],
        ))
        .await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src], Arc::clone(&conn), tx);

        let handle = tokio::spawn(router.run());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match received {
            RouterOutput::ExternalUpdate { event } => match event.kind {
                TaskEventKind::LabelsChanged { added, .. } => {
                    assert_eq!(added, vec!["surge:disabled".to_string()]);
                },
                other => panic!("expected LabelsChanged, got {other:?}"),
            },
            other => panic!("expected ExternalUpdate, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn emits_early_duplicate_when_active_run_exists() {
        let conn = Arc::new(Mutex::new(db()));
        // Pre-seed an active run for task mock:t#9.
        {
            let c = conn.lock().await;
            c.execute("INSERT INTO runs(id) VALUES ('run_active')", [])
                .unwrap();
            IntakeRepo::new(&c)
                .insert(&IntakeRow {
                    task_id: "mock:t#9".into(),
                    source_id: "mock:t".into(),
                    provider: "mock".into(),
                    run_id: Some("run_active".into()),
                    triage_decision: None,
                    duplicate_of: None,
                    priority: None,
                    state: TicketState::Active,
                    first_seen: Utc::now(),
                    last_seen: Utc::now(),
                    snooze_until: None,
                    callback_token: None,
                    tg_chat_id: None,
                    tg_message_id: None,
                })
                .unwrap();
        }
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#9")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src], Arc::clone(&conn), tx);

        let handle = tokio::spawn(router.run());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match received {
            RouterOutput::EarlyDuplicate { run_id, .. } => assert_eq!(run_id, "run_active"),
            other => panic!("expected EarlyDuplicate, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }
}
