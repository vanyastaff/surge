//! `TaskRouter` — multiplex events from multiple `TaskSource`s into a single
//! channel, applying Tier-1 PreFilter on each event.
//!
//! Lives in `surge-intake` to keep storage + dedup + multiplexing close together.
//! `surge-daemon` instantiates this with the configured sources at startup.

use crate::Result;
use crate::dedup::Tier1PreFilter;
use crate::source::TaskSource;
use crate::types::{TaskEvent, Tier1Decision};
use futures::stream::{StreamExt, select_all};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

/// Output of the router for downstream consumers (Triage Author dispatcher).
#[derive(Debug)]
pub enum RouterOutput {
    /// Event should be triaged (Tier-1 said pass).
    Triage { event: TaskEvent },
    /// Tier-1 dedup said this is an early duplicate of an active run;
    /// downstream may post a comment to that effect.
    EarlyDuplicate { event: TaskEvent, run_id: String },
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
            match item {
                Ok(event) => {
                    let decision_result = {
                        let conn = self.conn.lock().await;
                        let pre = Tier1PreFilter::new(&conn);
                        pre.check(&event)
                    };
                    let decision = match decision_result {
                        Ok(d) => d,
                        Err(e) => {
                            warn!(
                                error = %e,
                                task_id = %event.task_id,
                                "Tier-1 dedup failed; skipping event"
                            );
                            continue;
                        },
                    };

                    let out = match decision {
                        Tier1Decision::Pass => RouterOutput::Triage { event },
                        Tier1Decision::EarlyDuplicate { run_id } => {
                            RouterOutput::EarlyDuplicate { event, run_id }
                        },
                    };

                    if self.out_tx.send(out).await.is_err() {
                        info!("router output channel closed; stopping");
                        return Ok(());
                    }
                },
                Err(e) => {
                    warn!(error = %e, "task source emitted error; continuing");
                },
            }
        }

        Ok(())
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
