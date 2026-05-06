# RFC-0010 Plan-C Polish · Run Completion → Tracker Comment

**Goal:** Wire engine run-finish events to tracker `post_comment` calls so that each tracker-originated run posts a "✅ run completed" / "❌ Run failed: <reason>" / "Run aborted: <reason>" comment back to the originating ticket and transitions `ticket_index.state` Active → terminal.

**Architecture:** Lift `BroadcastRegistry` ownership from `run_server` into `surge-daemon` `main.rs` via a new entry point `run_with_registry`. In `main.rs`, subscribe to `GlobalDaemonEvent` and spawn a tokio task that, on each `RunFinished`, looks up the ticket via a new `IntakeRepo::lookup_ticket_by_run_id`, formats the comment, calls the matching `TaskSource::post_comment`, and updates the FSM. New integration test directly drives the same consumer using a synthetic `RunFinished` and a `MockTaskSource`.

**Tech Stack:** `tokio::sync::broadcast` (already used by registry), `rusqlite` (existing), `surge-intake::testing::MockTaskSource` (existing).

---

## File structure

### Created
- `docs/revision/plans/2026-05-06-rfc-0010-plan-c-polish-run-completion.md` — this plan
- `crates/surge-daemon/tests/daemon_run_completion_comment.rs` — integration test

### Modified
- `crates/surge-persistence/src/intake.rs` — add `IntakeRepo::lookup_ticket_by_run_id`
- `crates/surge-daemon/src/server.rs` — add `pub async fn run_with_registry(cfg, facade, broadcast, shutdown)`; refactor existing `run` to delegate
- `crates/surge-daemon/src/lib.rs` — re-export `run_with_registry`
- `crates/surge-daemon/src/main.rs` — create `BroadcastRegistry` early, subscribe global, spawn run-completion consumer, pass registry into `run_with_registry`

---

## Task 1 — `IntakeRepo::lookup_ticket_by_run_id`

**Files:**
- Modify: [crates/surge-persistence/src/intake.rs](crates/surge-persistence/src/intake.rs)

- [ ] **Step 1: Write the failing test**

Add inside the existing `mod repo_tests`:

```rust
#[test]
fn lookup_ticket_by_run_id_returns_row() {
    let conn = db_with_schema();
    conn.execute("INSERT INTO runs(id) VALUES ('run_xyz')", []).unwrap();
    let repo = IntakeRepo::new(&conn);
    let mut row = sample_row("linear:wsp1/ABC-9", TicketState::Active);
    row.run_id = Some("run_xyz".into());
    repo.insert(&row).unwrap();

    let fetched = repo.lookup_ticket_by_run_id("run_xyz").unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.task_id, "linear:wsp1/ABC-9");
    assert_eq!(fetched.state, TicketState::Active);
}

#[test]
fn lookup_ticket_by_run_id_returns_none_when_absent() {
    let conn = db_with_schema();
    let repo = IntakeRepo::new(&conn);
    let res = repo.lookup_ticket_by_run_id("does_not_exist").unwrap();
    assert!(res.is_none());
}

#[test]
fn lookup_ticket_by_run_id_skips_rows_without_run_id() {
    let conn = db_with_schema();
    let repo = IntakeRepo::new(&conn);
    repo.insert(&sample_row("linear:wsp1/ABC-10", TicketState::Seen))
        .unwrap();
    let res = repo.lookup_ticket_by_run_id("anything").unwrap();
    assert!(res.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails to compile**

```
cargo test -p surge-persistence --lib intake::repo_tests::lookup_ticket_by_run_id_returns_row
```

Expected: compile error — method does not exist.

- [ ] **Step 3: Add the method**

Inside `impl<'a> IntakeRepo<'a>`, after `lookup_active_run`:

```rust
/// Reverse lookup of `lookup_active_run`: returns the ticket row whose
/// `run_id` matches, regardless of state. Used when an engine run finishes
/// and we need to find the originating ticket (if any) so we can post
/// a tracker comment + update the ticket FSM.
///
/// Returns `Ok(None)` when no row has this `run_id` (e.g., the run was not
/// tracker-originated). Returns the full row on hit.
pub fn lookup_ticket_by_run_id(&self, run_id: &str) -> rusqlite::Result<Option<IntakeRow>> {
    let mut stmt = self.conn.prepare(
        "SELECT task_id, source_id, provider, run_id, triage_decision, duplicate_of, \
                priority, state, first_seen, last_seen, snooze_until \
         FROM ticket_index WHERE run_id = ?1",
    )?;
    let mut rows = stmt.query(params![run_id])?;
    let Some(r) = rows.next()? else {
        return Ok(None);
    };

    let state_str: String = r.get(7)?;
    let state: TicketState = state_str.parse().map_err(|e: String| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, e.into())
    })?;
    let first_seen: String = r.get(8)?;
    let last_seen: String = r.get(9)?;
    let snooze_until: Option<String> = r.get(10)?;

    Ok(Some(IntakeRow {
        task_id: r.get(0)?,
        source_id: r.get(1)?,
        provider: r.get(2)?,
        run_id: r.get(3)?,
        triage_decision: r.get(4)?,
        duplicate_of: r.get(5)?,
        priority: r.get(6)?,
        state,
        first_seen: DateTime::parse_from_rfc3339(&first_seen)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    e.to_string().into(),
                )
            })?
            .with_timezone(&Utc),
        last_seen: DateTime::parse_from_rfc3339(&last_seen)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    9,
                    rusqlite::types::Type::Text,
                    e.to_string().into(),
                )
            })?
            .with_timezone(&Utc),
        snooze_until: snooze_until
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    e.to_string().into(),
                )
            })?,
    }))
}
```

- [ ] **Step 4: Run tests**

```
cargo test -p surge-persistence --lib intake::repo_tests
```

Expected: all repo tests pass (existing 4 + new 3 = 7 passing).

- [ ] **Step 5: Commit**

```
git add crates/surge-persistence/src/intake.rs
git commit -m "feat(persistence): IntakeRepo::lookup_ticket_by_run_id reverse lookup"
```

---

## Task 2 — Lift `BroadcastRegistry` ownership to caller

**Files:**
- Modify: [crates/surge-daemon/src/server.rs](crates/surge-daemon/src/server.rs)
- Modify: [crates/surge-daemon/src/lib.rs](crates/surge-daemon/src/lib.rs)

- [ ] **Step 1: Refactor server.rs**

Inside `crates/surge-daemon/src/server.rs`, replace the existing `pub async fn run(...)` with two functions:

```rust
/// Wires together the engine facade, admission, broadcast registry,
/// and the IPC listener. Creates a fresh `BroadcastRegistry` internally.
/// For callers that need to subscribe to global events themselves, use
/// [`run_with_registry`] instead.
pub async fn run(
    cfg: ServerConfig,
    facade: Arc<dyn EngineFacade>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let broadcast = Arc::new(BroadcastRegistry::new());
    run_with_registry(cfg, facade, broadcast, shutdown).await
}

/// Like [`run`], but accepts a pre-built [`BroadcastRegistry`] so the
/// caller can `subscribe_global()` for daemon-internal listeners (e.g.,
/// run-completion → tracker comment hook).
pub async fn run_with_registry(
    cfg: ServerConfig,
    facade: Arc<dyn EngineFacade>,
    broadcast: Arc<BroadcastRegistry>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    use interprocess::local_socket::ListenerOptions;

    let admission = Arc::new(AdmissionController::new(cfg.max_active, cfg.max_queue));
    let pending_starts: PendingStarts = Arc::new(Mutex::new(HashMap::new()));

    // F2: Unlink any stale socket file from a previous unclean exit.
    // On Windows, the named pipe doesn't live on the filesystem so this is a no-op.
    #[cfg(unix)]
    {
        if cfg.socket_path.exists() {
            let _ = std::fs::remove_file(&cfg.socket_path);
        }
    }

    // ...rest of original `run` body unchanged, minus the local
    // `let broadcast = Arc::new(BroadcastRegistry::new());` line.
}
```

(Concretely: cut the `let broadcast = …` line, rename the function, and add the thin `run` wrapper above.)

- [ ] **Step 2: Update `lib.rs` re-export**

In `crates/surge-daemon/src/lib.rs`:

```rust
pub use server::{ServerConfig, run as run_server, run_with_registry};
```

- [ ] **Step 3: Build**

```
cargo build -p surge-daemon
```

Expected: success. Existing tests still link.

- [ ] **Step 4: Run all existing daemon tests**

```
cargo test -p surge-daemon
```

Expected: all green (no signature changes seen by tests).

- [ ] **Step 5: Commit**

```
git add crates/surge-daemon/src/server.rs crates/surge-daemon/src/lib.rs
git commit -m "refactor(daemon): expose run_with_registry for in-process subscribers"
```

---

## Task 3 — Run-completion consumer in `main.rs`

**Files:**
- Modify: [crates/surge-daemon/src/main.rs](crates/surge-daemon/src/main.rs)

- [ ] **Step 1: Construct registry early & subscribe before server spawn**

Near the top of the async block in `main` — right after `notifier` is built and before the `Engine::new_with_mcp` call (line ~118), add:

```rust
let broadcast_registry = Arc::new(surge_daemon::broadcast::BroadcastRegistry::new());
let global_rx = broadcast_registry.subscribe_global();
```

- [ ] **Step 2: Pass registry into the source-router consumer**

Modify `spawn_task_router` to accept the same `Arc<BroadcastRegistry>` plus a `tokio::sync::broadcast::Receiver<GlobalDaemonEvent>` and spawn a third task: the run-completion consumer.

Replace the call site `spawn_task_router(sources, source_map, Arc::clone(&notifier), Arc::clone(&storage)).await;` with a new signature that also forwards the source map + connection. Concretely, refactor `spawn_task_router` to extract a shared `(source_map_arc, conn_arc)` tuple and use it for both the existing router-output consumer and the new run-completion consumer.

The body of the new tokio task lives in a new helper:

```rust
fn spawn_run_completion_consumer(
    mut rx: tokio::sync::broadcast::Receiver<surge_orchestrator::engine::ipc::GlobalDaemonEvent>,
    source_map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<TokioMutex<rusqlite::Connection>>,
) {
    use surge_orchestrator::engine::handle::RunOutcome;
    use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
    use surge_persistence::intake::{IntakeRepo, TicketState};

    tokio::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("run-completion consumer: broadcast closed; exiting");
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "run-completion consumer lagged; some RunFinished events dropped");
                    continue;
                }
            };
            let GlobalDaemonEvent::RunFinished { run_id, outcome } = ev else {
                continue;
            };

            // Look up ticket by run_id.
            let row = {
                let guard = conn.lock().await;
                IntakeRepo::new(&*guard).lookup_ticket_by_run_id(&run_id.to_string())
            };
            let row = match row {
                Ok(Some(r)) => r,
                Ok(None) => continue, // not a tracker-originated run
                Err(e) => {
                    warn!(error = %e, %run_id, "lookup_ticket_by_run_id failed");
                    continue;
                }
            };

            // Resolve the source.
            let Some(source) = source_map.get(&row.source_id) else {
                warn!(
                    source_id = %row.source_id,
                    task_id = %row.task_id,
                    "no source registered; cannot post completion comment"
                );
                continue;
            };

            // Format body + target FSM state.
            let (body, new_state, purpose) = match &outcome {
                RunOutcome::Completed { terminal } => (
                    format!("✅ Surge run completed (terminal node: `{terminal}`)."),
                    TicketState::Completed,
                    "run_completed",
                ),
                RunOutcome::Failed { error } => (
                    format!("❌ Run failed: {error}"),
                    TicketState::Failed,
                    "run_failed",
                ),
                RunOutcome::Aborted { reason } => (
                    format!("Run aborted: {reason}"),
                    TicketState::Aborted,
                    "run_aborted",
                ),
            };

            // Post comment via TaskSource. Best-effort: log on failure.
            let task_id = match surge_intake::types::TaskId::try_new(row.task_id.clone()) {
                Ok(id) => id,
                Err(e) => {
                    warn!(error = %e, task_id = %row.task_id, "invalid task_id; skipping comment post");
                    continue;
                }
            };
            match source.post_comment(&task_id, &body).await {
                Ok(()) => {
                    info!(
                        task_id = %row.task_id,
                        purpose = %purpose,
                        "posted run-completion comment"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        task_id = %row.task_id,
                        "failed to post run-completion comment"
                    );
                }
            }

            // Transition ticket FSM.
            let guard = conn.lock().await;
            if let Err(e) = IntakeRepo::new(&*guard).update_state(&row.task_id, new_state) {
                warn!(error = %e, task_id = %row.task_id, "failed to update ticket state");
            }
        }
    });
}
```

- [ ] **Step 3: Refactor `spawn_task_router` to share `source_map` + `conn` with the new consumer**

Change the function so it builds `source_map_arc` and `conn_arc` once and returns them, then call the new `spawn_run_completion_consumer` from the call site:

```rust
let (source_map_arc, conn_arc) = match spawn_task_router(sources, source_map, Arc::clone(&notifier), Arc::clone(&storage)).await {
    Some(pair) => pair,
    None => {
        info!("intake disabled; skipping run-completion consumer");
        // pass through without a completion consumer
        // (no panic; daemon still serves engine RPC)
        let server_handle = tokio::spawn({...existing block...});
        // …existing flow…
        return server_handle;
    }
};
spawn_run_completion_consumer(global_rx, source_map_arc, conn_arc);
```

This is illustrative — write the smallest restructure that lets `main` get hold of `source_map_arc` and `conn_arc` after `spawn_task_router` returns. The cleanest shape: change `spawn_task_router`'s return type from `()` to `Option<(Arc<HashMap<...>>, Arc<TokioMutex<Connection>>)>`. `None` means intake disabled (no sources or DB open failed) — caller skips the run-completion consumer in that case.

- [ ] **Step 4: Switch `run_server` call to `run_with_registry`**

In the server-spawn block:

```rust
let server_handle = tokio::spawn({
    let facade = facade.clone();
    let shutdown_for_server = shutdown.clone();
    let shutdown_for_cancel = shutdown.clone();
    let broadcast = Arc::clone(&broadcast_registry);
    async move {
        if let Err(e) = surge_daemon::run_with_registry(server_cfg, facade, broadcast, shutdown_for_server).await {
            tracing::error!(err = %e, "server exited with error; cancelling shutdown token");
            shutdown_for_cancel.cancel();
        }
    }
});
```

- [ ] **Step 5: Build**

```
cargo build -p surge-daemon
```

Expected: success.

- [ ] **Step 6: Commit**

```
git add crates/surge-daemon/src/main.rs
git commit -m "feat(daemon): post tracker comment on engine run completion"
```

---

## Task 4 — Integration test: synthetic `RunFinished` → tracker comment

**Files:**
- Create: [crates/surge-daemon/tests/daemon_run_completion_comment.rs](crates/surge-daemon/tests/daemon_run_completion_comment.rs)

This test does NOT spin up the full daemon binary. It directly exercises the run-completion consumer logic: given a populated `ticket_index` row + a `MockTaskSource`, when a `GlobalDaemonEvent::RunFinished` is published, the comment is posted and the FSM transitions.

To do that we need to extract the consumer into a callable helper. The cleanest factoring: move `spawn_run_completion_consumer` from `main.rs` into a new `pub(crate)` (or `pub`) function in a new module `crates/surge-daemon/src/intake_completion.rs`. The test then calls that function directly with a fresh broadcast channel and an in-memory SQLite `Connection`.

- [ ] **Step 1: Move `spawn_run_completion_consumer` to a new lib module**

Create `crates/surge-daemon/src/intake_completion.rs` containing the function (taking `Arc<HashMap<String, Arc<dyn TaskSource>>>`, `Arc<TokioMutex<rusqlite::Connection>>`, and the `broadcast::Receiver<GlobalDaemonEvent>`). Export from `lib.rs` as `pub mod intake_completion;`. Update `main.rs` to call `surge_daemon::intake_completion::spawn(...)` instead of the inline helper.

- [ ] **Step 2: Write the failing test**

Create `crates/surge-daemon/tests/daemon_run_completion_comment.rs`:

```rust
//! Integration test for RFC-0010 acceptance criterion #5: a tracker-originated
//! run that completes successfully posts a "✅" comment to the originating
//! ticket; failure posts "❌ Run failed: <reason>"; abort posts "Run aborted:
//! <reason>". The ticket FSM transitions to the matching terminal state.
//!
//! Drives the consumer directly, without spinning up the daemon binary.

use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_daemon::intake_completion;
use surge_intake::TaskSource;
use surge_intake::testing::MockTaskSource;
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use tokio::sync::{Mutex as TokioMutex, broadcast};

fn db_with_schema() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);").unwrap();
    let sql = include_str!(
        "../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql"
    );
    conn.execute_batch(sql).unwrap();
    conn
}

fn seed_ticket(conn: &Connection, task_id: &str, run_id: &str) {
    conn.execute("INSERT INTO runs(id) VALUES (?1)", [run_id]).unwrap();
    let repo = IntakeRepo::new(conn);
    repo.insert(&IntakeRow {
        task_id: task_id.into(),
        source_id: "mock:test".into(),
        provider: "mock".into(),
        run_id: Some(run_id.into()),
        triage_decision: Some("enqueued".into()),
        duplicate_of: None,
        priority: Some("medium".into()),
        state: TicketState::Active,
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        snooze_until: None,
    })
    .unwrap();
}

fn make_setup() -> (
    Arc<MockTaskSource>,
    Arc<HashMap<String, Arc<dyn TaskSource>>>,
    Arc<TokioMutex<Connection>>,
    broadcast::Sender<GlobalDaemonEvent>,
    broadcast::Receiver<GlobalDaemonEvent>,
) {
    let src = Arc::new(MockTaskSource::new("mock:test", "mock"));
    let mut map: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
    map.insert("mock:test".into(), Arc::clone(&src) as Arc<dyn TaskSource>);
    let map = Arc::new(map);
    let conn = Arc::new(TokioMutex::new(db_with_schema()));
    let (tx, rx) = broadcast::channel(8);
    (src, map, conn, tx, rx)
}

#[tokio::test]
async fn run_completed_posts_success_comment_and_transitions_state() {
    let (src, map, conn, tx, rx) = make_setup();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#1", "01HXYZRUNCOMPLETED01");
    }

    intake_completion::spawn(rx, map, Arc::clone(&conn));

    let run_id = "01HXYZRUNCOMPLETED01".parse::<RunId>().unwrap();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    // Allow the consumer to process.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1, "expected exactly one comment");
    assert!(comments[0].1.starts_with("✅"), "got: {}", comments[0].1);

    let guard = conn.lock().await;
    let row = IntakeRepo::new(&*guard)
        .lookup_ticket_by_run_id("01HXYZRUNCOMPLETED01")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Completed);
}

#[tokio::test]
async fn run_failed_posts_failure_comment_and_transitions_state() {
    let (src, map, conn, tx, rx) = make_setup();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#2", "01HXYZRUNFAILED02000");
    }

    intake_completion::spawn(rx, map, Arc::clone(&conn));

    let run_id = "01HXYZRUNFAILED02000".parse::<RunId>().unwrap();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Failed {
            error: "graph validation error".into(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].1.starts_with("❌ Run failed:"), "got: {}", comments[0].1);
    assert!(comments[0].1.contains("graph validation error"));

    let guard = conn.lock().await;
    let row = IntakeRepo::new(&*guard)
        .lookup_ticket_by_run_id("01HXYZRUNFAILED02000")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Failed);
}

#[tokio::test]
async fn run_aborted_posts_abort_comment_and_transitions_state() {
    let (src, map, conn, tx, rx) = make_setup();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#3", "01HXYZRUNABORTED03000");
    }

    intake_completion::spawn(rx, map, Arc::clone(&conn));

    let run_id = "01HXYZRUNABORTED03000".parse::<RunId>().unwrap();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Aborted {
            reason: "user pressed Stop".into(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].1.starts_with("Run aborted:"));
    assert!(comments[0].1.contains("user pressed Stop"));

    let guard = conn.lock().await;
    let row = IntakeRepo::new(&*guard)
        .lookup_ticket_by_run_id("01HXYZRUNABORTED03000")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Aborted);
}

#[tokio::test]
async fn run_finished_with_no_matching_ticket_is_a_no_op() {
    let (src, map, conn, tx, rx) = make_setup();
    // Note: no `seed_ticket` — DB is empty.
    intake_completion::spawn(rx, map, Arc::clone(&conn));

    let run_id = RunId::new();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(src.posted_comments().await.is_empty());
}

#[tokio::test]
async fn post_comment_failure_still_transitions_state() {
    let (src, map, conn, tx, rx) = make_setup();
    src.arm_post_comment_failure().await;
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#5", "01HXYZRUNPOSTFAIL5000");
    }

    intake_completion::spawn(rx, map, Arc::clone(&conn));

    let run_id = "01HXYZRUNPOSTFAIL5000".parse::<RunId>().unwrap();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // post_comment failed → no comment recorded.
    assert!(src.posted_comments().await.is_empty());

    // FSM still transitioned (best-effort comment, but state is authoritative).
    let guard = conn.lock().await;
    let row = IntakeRepo::new(&*guard)
        .lookup_ticket_by_run_id("01HXYZRUNPOSTFAIL5000")
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Completed);
}
```

(The exact `RunId` literal must be a valid 26-char ULID. Use `RunId::new().to_string()` to avoid hand-coded literals if the parser is strict — see step 3 below.)

- [ ] **Step 2.1: Confirm `RunId` parsing format**

```
grep -n "impl FromStr for RunId\|impl FromStr for SpecId" crates/surge-core/src/id.rs
```

If `RunId::from_str` requires a valid 26-char ULID, replace the literal hand-coded run IDs above with `let run_id = RunId::new(); let run_id_str = run_id.to_string();` and seed the ticket with that string.

- [ ] **Step 3: Run the test**

```
cargo test -p surge-daemon --test daemon_run_completion_comment
```

Expected: 5 passed.

- [ ] **Step 4: Commit**

```
git add crates/surge-daemon/src/intake_completion.rs crates/surge-daemon/src/lib.rs \
        crates/surge-daemon/tests/daemon_run_completion_comment.rs
git commit -m "test(daemon): run-completion consumer posts tracker comment + transitions FSM"
```

---

## Task 5 — Workspace verification

- [ ] **Step 1: Build**

```
cargo build --workspace
```

Expected: success.

- [ ] **Step 2: Run all workspace tests**

```
cargo test --workspace
```

Expected: green. (No regressions in existing daemon tests since `run_server` keeps the same signature.)

- [ ] **Step 3: Lint**

```
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Format check**

```
cargo fmt --all -- --check
```

Expected: clean.

- [ ] **Step 5: No further commit needed unless step 3/4 surface fixes.**

---

## Self-review

**Spec coverage:**

- RFC-0010 § Sync rules — `Run::Completed` / `Run::Failed` / `Run::Aborted` rows of the table: each handled in Task 3's match arm with the spec's body text adapted (see "Phrasing" note).
- RFC-0010 acceptance criterion #5: covered by the integration test (Task 4) for all three outcomes.
- Tier-1 EarlyDuplicate path (existing): unaffected — that codepath posts comments via `source.post_comment` already; the run-completion hook is independent.

**Phrasing note:** The RFC table example shows `"✅ PR #47 merged"` because most successful runs end in PR composition. The engine's `RunOutcome::Completed` does NOT yet carry PR metadata (only the terminal `NodeKey`), so this plan posts `"✅ Surge run completed (terminal node: …)."` instead. Threading PR info into `RunOutcome` is a separate enhancement and out of scope for this plan.

**Placeholder scan:** The plan has no `TBD` / `TODO` / `implement later` markers. The "RunId parsing" check in Task 4 step 2.1 is a defensive note, not a placeholder.

**Type consistency:**

- `IntakeRepo::lookup_ticket_by_run_id` returns `rusqlite::Result<Option<IntakeRow>>` — matches sibling `lookup_active_run` shape (`Option<String>`) but returns the full row because the consumer needs `task_id`, `source_id`, and `state` for follow-up work.
- `intake_completion::spawn` argument order: `(rx, source_map, conn)` matches `spawn_task_router`'s output ordering.
- `GlobalDaemonEvent::RunFinished` field names (`run_id`, `outcome`) match the spec in `surge-orchestrator/src/engine/ipc.rs`.
- `RunOutcome::Completed.terminal: NodeKey` formats via its `Display` impl into the comment body — confirm `NodeKey: Display` exists (it does; see `surge-core/src/keys.rs`).

**Out of scope:**

- Threading PR number / URL into `RunOutcome::Completed` (would change engine API; ticket out a follow-up).
- Emitting `SurgeEvent::TrackerCommentPosted` to a durable event log — currently the daemon's tracker codepath uses `tracing` for observability; introducing a tracker-event sink is a separate plan.
- Recovery on daemon restart: a row left in `Active` state because the daemon crashed mid-comment-post is handled by the existing `crash recovery` rules in RFC-0010 § Crash recovery, not by this plan.
