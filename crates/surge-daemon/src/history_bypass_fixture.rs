use std::sync::Arc;

use chrono::Utc;
use rusqlite::Connection;
use surge_core::RunId;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use surge_persistence::runs::Storage;
use tokio::sync::Mutex;

mod producer {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/failed_settlement.rs"
    ));
}

pub(crate) struct Fixture {
    pub storage: Arc<Storage>,
    pub run: RunId,
    worktree: tempfile::TempDir,
    home: crate::runtime_home_fixture::FixtureHome,
}

impl Fixture {
    pub async fn new() -> Self {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let run = producer::produce(home.path(), worktree.path()).await;
        let storage = Storage::open(home.path()).await.unwrap();
        Self {
            storage,
            run,
            worktree,
            home,
        }
    }

    pub fn registry(&self, task: &str) -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
            .unwrap();
        for migration in [
            include_str!(
                "../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql"
            ),
            include_str!(
                "../../surge-persistence/src/runs/migrations/registry/0004_inbox_callback_columns.sql"
            ),
            include_str!(
                "../../surge-persistence/src/runs/migrations/registry/0013_intake_emit_log.sql"
            ),
            include_str!(
                "../../surge-persistence/src/runs/migrations/registry/0020_terminal_comment_outbox.sql"
            ),
        ] {
            conn.execute_batch(migration).unwrap();
        }
        let run = self.run.to_string();
        conn.execute("INSERT INTO runs(id) VALUES (?1)", [&run])
            .unwrap();
        IntakeRepo::new(&conn)
            .insert(&IntakeRow {
                task_id: task.into(),
                source_id: "mock:test".into(),
                provider: "mock".into(),
                run_id: Some(run),
                triage_decision: Some("enqueued".into()),
                duplicate_of: None,
                priority: Some("medium".into()),
                state: TicketState::Active,
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                snooze_until: None,
                callback_token: None,
                tg_chat_id: None,
                tg_message_id: None,
            })
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    pub fn close(self) {
        let Self {
            storage,
            worktree,
            home,
            ..
        } = self;
        drop(storage);
        worktree.close().unwrap();
        home.close().unwrap();
    }
}

pub(crate) fn close_registry(conn: Arc<Mutex<Connection>>) {
    Arc::try_unwrap(conn)
        .expect("reconciliation released registry owners")
        .into_inner()
        .close()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_completed_run_reports_exhausted_forwarder_pool() {
    Fixture::new().await.close();
}
