# Inbox-Card Callback Handler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `▶ Start` / `⏸ Snooze 24h` / `✕ Skip` taps on inbox cards (Telegram + Desktop) into real `Engine::start_run` calls, durable `ticket_index` FSM transitions, and tracker comments — closing RFC-0010 acceptance criterion #4.

**Architecture:** Cross-process-ready transport: Telegram bot loop and Desktop receiver write `Inbox*Requested` rows to a SQLite-backed `inbox_action_queue` table; an `InboxActionConsumer` task in `surge-daemon` polls and dispatches. The `BootstrapGraphBuilder` trait abstracts user-prompt → `Graph`; today's `MinimalBootstrapGraphBuilder` produces a single-Agent graph, future RFC-0004 swaps in a multi-stage staged builder via DI. `TicketStateSync` subscribes to `RunHandle.events` and applies `ticket_index` FSM transitions + posts tracker comments. `SnoozeScheduler` re-emits snoozed cards.

**Tech Stack:** Rust 2024 (MSRV 1.85), tokio multi-thread, existing deps (`teloxide` 0.13, `notify-rust`, `rusqlite`, `chrono`, `ulid`, `async-trait`), no new workspace deps.

**Spec:** [docs/superpowers/specs/2026-05-06-inbox-callback-handler-design.md](../specs/2026-05-06-inbox-callback-handler-design.md)

**Spec deviations to call out (the plan implements these):**
- The spec §3.7 numbers the migration `0003_inbox_callback_columns.sql`, but the registry already has migrations through `0003_task_source_state.sql`. The plan uses `0004_inbox_callback_columns.sql` and `0005_inbox_queues.sql`.
- The spec §3.9 sketches a single `global_events` table with cursors per consumer. The plan splits into two purpose-specific queues (`inbox_action_queue` for incoming requests, `inbox_delivery_queue` for outgoing cards) for clearer audit and simpler indexing.
- The spec §3.10 shows new variants on a single `EventPayload` enum. The codebase has two enums: `surge-core::run_event::EventPayload` (engine-internal, run-scoped) and `surge-core::event::SurgeEvent` (cross-component, user-visible). The plan adds the new audit variants to `SurgeEvent`; the queue tables themselves are not events.

---

## File Structure

### New files

```
crates/surge-orchestrator/src/bootstrap/
├── mod.rs                                          (re-exports)
├── builder.rs                                      (BootstrapGraphBuilder trait + types)
└── minimal.rs                                      (MinimalBootstrapGraphBuilder impl)

crates/surge-daemon/src/inbox/
├── mod.rs                                          (re-exports + ActionChannel enum)
├── consumer.rs                                     (InboxActionConsumer)
├── state_sync.rs                                   (TicketStateSync)
├── snooze_scheduler.rs                             (SnoozeScheduler)
├── tg_bot.rs                                       (TgInboxBot)
└── desktop_listener.rs                             (DesktopActionListener)

crates/surge-persistence/src/runs/migrations/registry/
├── 0004_inbox_callback_columns.sql                 (add columns to ticket_index)
└── 0005_inbox_queues.sql                           (inbox_action_queue + inbox_delivery_queue)

crates/surge-daemon/tests/
└── inbox_callback_e2e.rs                           (end-to-end integration test)
```

### Modified files

```
crates/surge-orchestrator/src/lib.rs                (re-export bootstrap module)
crates/surge-orchestrator/Cargo.toml                (add surge-intake dep)
crates/surge-core/src/event.rs                     (5 new SurgeEvent variants)
crates/surge-core/src/config.rs                    (TelegramConfig, InboxConfig)
crates/surge-persistence/src/intake.rs             (new IntakeRepo methods, IntakeRow fields)
crates/surge-persistence/src/runs/migrations.rs    (add new migrations to REGISTRY_MIGRATIONS)
crates/surge-persistence/src/runs/storage.rs       (inbox queue helpers)
crates/surge-persistence/src/lib.rs                (re-export new helpers if needed)
crates/surge-notify/src/messages.rs                (InboxCardPayload: run_id → callback_token)
crates/surge-notify/src/telegram.rs                (format_inbox_card uses callback_token)
crates/surge-notify/src/desktop.rs                 (format_inbox_card_desktop uses callback_token)
crates/surge-daemon/Cargo.toml                     (add teloxide, surge-orchestrator, surge-intake)
crates/surge-daemon/src/lib.rs                     (pub mod inbox)
crates/surge-daemon/src/main.rs                    (wire inbox subsystems, change InboxCard construction)
```

---

## Phase 1 — Persistence foundation

### Task 1.1: Migration `0004_inbox_callback_columns.sql`

**Files:**
- Create: `crates/surge-persistence/src/runs/migrations/registry/0004_inbox_callback_columns.sql`
- Modify: `crates/surge-persistence/src/runs/migrations.rs:19-32`

- [ ] **Step 1: Write the migration SQL**

Create `crates/surge-persistence/src/runs/migrations/registry/0004_inbox_callback_columns.sql`:

```sql
-- 0004_inbox_callback_columns.sql
-- Adds the per-card callback_token plus Telegram message references to
-- ticket_index. callback_token is generated each time an inbox card is
-- emitted (including on snooze re-emission); cleared on Start. The
-- partial UNIQUE index allows multiple post-decision rows with NULL
-- token while preventing two open cards from colliding.

ALTER TABLE ticket_index ADD COLUMN callback_token TEXT;
ALTER TABLE ticket_index ADD COLUMN tg_chat_id INTEGER;
ALTER TABLE ticket_index ADD COLUMN tg_message_id INTEGER;

CREATE UNIQUE INDEX IF NOT EXISTS idx_ticket_index_callback_token
    ON ticket_index(callback_token)
    WHERE callback_token IS NOT NULL;
```

- [ ] **Step 2: Register the migration**

Open `crates/surge-persistence/src/runs/migrations.rs`. Append a new entry to `REGISTRY_MIGRATIONS` after the existing `registry-0003-task-source-state` entry:

```rust
pub const REGISTRY_MIGRATIONS: MigrationSet = &[
    (
        "registry-0001-initial",
        include_str!("migrations/registry/0001_initial.sql"),
    ),
    (
        "registry-0002-ticket-index",
        include_str!("migrations/registry/0002_ticket_index.sql"),
    ),
    (
        "registry-0003-task-source-state",
        include_str!("migrations/registry/0003_task_source_state.sql"),
    ),
    (
        "registry-0004-inbox-callback-columns",
        include_str!("migrations/registry/0004_inbox_callback_columns.sql"),
    ),
];
```

- [ ] **Step 3: Run existing tests**

```bash
cargo test -p surge-persistence --lib
```

Expected: PASS (existing migration round-trip tests applied; new migration auto-runs).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/migrations/registry/0004_inbox_callback_columns.sql \
        crates/surge-persistence/src/runs/migrations.rs
git commit -m "feat(persistence): migration 0004 — inbox callback_token + tg refs columns"
```

---

### Task 1.2: Migration `0005_inbox_queues.sql`

**Files:**
- Create: `crates/surge-persistence/src/runs/migrations/registry/0005_inbox_queues.sql`
- Modify: `crates/surge-persistence/src/runs/migrations.rs`

- [ ] **Step 1: Write the migration SQL**

Create `crates/surge-persistence/src/runs/migrations/registry/0005_inbox_queues.sql`:

```sql
-- 0005_inbox_queues.sql
-- Two queues that decouple the inbox-action receivers (Telegram bot,
-- Desktop listener) from the consumer (InboxActionConsumer) and the
-- delivery loop (TgInboxBot.outgoing_loop) from the router (which
-- enqueues fresh cards). Both are FIFO with monotonic seq.
--
-- inbox_action_queue: incoming requests from receivers. processed_at
-- becomes non-NULL once InboxActionConsumer commits the dispatch.
--
-- inbox_delivery_queue: outgoing card payloads. Each transport leg
-- (telegram, desktop) records its own delivery timestamp + IDs so the
-- legs run independently and can both deliver the same card.

CREATE TABLE IF NOT EXISTS inbox_action_queue (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    kind            TEXT    NOT NULL,    -- "start" | "snooze" | "skip"
    task_id         TEXT    NOT NULL,
    callback_token  TEXT    NOT NULL,
    decided_via     TEXT    NOT NULL,    -- "telegram" | "desktop"
    snooze_until    TEXT,                -- ISO-8601, only for kind="snooze"
    enqueued_at     TEXT    NOT NULL,
    processed_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_inbox_action_queue_pending
    ON inbox_action_queue(seq)
    WHERE processed_at IS NULL;

CREATE TABLE IF NOT EXISTS inbox_delivery_queue (
    seq                       INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id                   TEXT    NOT NULL,
    callback_token            TEXT    NOT NULL,
    payload_json              TEXT    NOT NULL,
    enqueued_at               TEXT    NOT NULL,
    telegram_delivered_at     TEXT,
    telegram_chat_id          INTEGER,
    telegram_message_id       INTEGER,
    desktop_delivered_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_inbox_delivery_queue_tg_pending
    ON inbox_delivery_queue(seq)
    WHERE telegram_delivered_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_inbox_delivery_queue_desktop_pending
    ON inbox_delivery_queue(seq)
    WHERE desktop_delivered_at IS NULL;
```

- [ ] **Step 2: Register the migration**

Append to `REGISTRY_MIGRATIONS` in `crates/surge-persistence/src/runs/migrations.rs`:

```rust
    (
        "registry-0005-inbox-queues",
        include_str!("migrations/registry/0005_inbox_queues.sql"),
    ),
```

- [ ] **Step 3: Verify**

```bash
cargo test -p surge-persistence --lib
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-persistence/src/runs/migrations/registry/0005_inbox_queues.sql \
        crates/surge-persistence/src/runs/migrations.rs
git commit -m "feat(persistence): migration 0005 — inbox action + delivery queues"
```

---

### Task 1.3: Extend `IntakeRow` and add `IntakeRepo` token methods

**Files:**
- Modify: `crates/surge-persistence/src/intake.rs`

- [ ] **Step 1: Write a failing test for the new fields and helpers**

Append to the `#[cfg(test)] mod repo_tests` block in `crates/surge-persistence/src/intake.rs`:

```rust
    #[test]
    fn callback_token_set_clear_lookup() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/T-1", TicketState::InboxNotified))
            .unwrap();

        repo.set_callback_token("linear:wsp1/T-1", "01HKGZTOK1").unwrap();
        let row = repo.fetch_by_callback_token("01HKGZTOK1").unwrap().unwrap();
        assert_eq!(row.task_id, "linear:wsp1/T-1");
        assert_eq!(row.callback_token.as_deref(), Some("01HKGZTOK1"));

        repo.clear_callback_token("linear:wsp1/T-1").unwrap();
        assert!(
            repo.fetch_by_callback_token("01HKGZTOK1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn callback_token_uniqueness() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/T-2", TicketState::InboxNotified))
            .unwrap();
        repo.insert(&sample_row("linear:wsp1/T-3", TicketState::InboxNotified))
            .unwrap();
        repo.set_callback_token("linear:wsp1/T-2", "01HKGZSAME").unwrap();
        let dup_err = repo.set_callback_token("linear:wsp1/T-3", "01HKGZSAME");
        assert!(dup_err.is_err(), "duplicate callback_token must fail UNIQUE");
    }

    #[test]
    fn tg_message_ref_round_trip() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/T-4", TicketState::InboxNotified))
            .unwrap();
        repo.set_tg_message_ref("linear:wsp1/T-4", -1001234567890, 4242).unwrap();
        let row = repo.fetch("linear:wsp1/T-4").unwrap().unwrap();
        assert_eq!(row.tg_chat_id, Some(-1001234567890));
        assert_eq!(row.tg_message_id, Some(4242));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p surge-persistence --lib intake::repo_tests::callback_token_set_clear_lookup
```

Expected: FAIL — `set_callback_token` and `fetch_by_callback_token` don't exist; `IntakeRow` has no `callback_token` field.

- [ ] **Step 3: Add fields to `IntakeRow`**

Modify the `IntakeRow` struct in `crates/surge-persistence/src/intake.rs`. Append three fields after `snooze_until`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeRow {
    pub task_id: String,
    pub source_id: String,
    pub provider: String,
    pub run_id: Option<String>,
    pub triage_decision: Option<String>,
    pub duplicate_of: Option<String>,
    pub priority: Option<String>,
    pub state: TicketState,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub snooze_until: Option<DateTime<Utc>>,
    /// Short ULID embedded in inbox-card callback data; lookup key for the
    /// daemon's inbox handler. NULL after Start (cleared) or before any
    /// card is emitted.
    pub callback_token: Option<String>,
    /// Telegram chat ID where the most recent inbox card was sent.
    pub tg_chat_id: Option<i64>,
    /// Telegram message ID of the most recent inbox card; used by future
    /// `editMessageReplyMarkup` to remove the keyboard after action.
    pub tg_message_id: Option<i32>,
}
```

- [ ] **Step 4: Update existing `insert` and `fetch` to handle new columns**

Replace `insert` body's SQL and params:

```rust
    pub fn insert(&self, row: &IntakeRow) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO ticket_index(\
                task_id, source_id, provider, run_id, triage_decision, duplicate_of,\
                priority, state, first_seen, last_seen, snooze_until,\
                callback_token, tg_chat_id, tg_message_id\
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                row.task_id,
                row.source_id,
                row.provider,
                row.run_id,
                row.triage_decision,
                row.duplicate_of,
                row.priority,
                row.state.as_str(),
                row.first_seen.to_rfc3339(),
                row.last_seen.to_rfc3339(),
                row.snooze_until.map(|d| d.to_rfc3339()),
                row.callback_token,
                row.tg_chat_id,
                row.tg_message_id,
            ],
        )?;
        Ok(())
    }
```

Replace `fetch` body's SELECT and the row construction. Find the existing `fetch` method and replace it:

```rust
    pub fn fetch(&self, task_id: &str) -> rusqlite::Result<Option<IntakeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, source_id, provider, run_id, triage_decision, duplicate_of, \
                    priority, state, first_seen, last_seen, snooze_until, \
                    callback_token, tg_chat_id, tg_message_id \
             FROM ticket_index WHERE task_id = ?1",
        )?;
        let mut rows = stmt.query(params![task_id])?;
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
            callback_token: r.get(11)?,
            tg_chat_id: r.get(12)?,
            tg_message_id: r.get(13)?,
        }))
    }
```

- [ ] **Step 5: Add the new helper methods**

Append to `impl<'a> IntakeRepo<'a>` in the same file:

```rust
    /// Set the callback token, replacing any prior value. UNIQUE constraint
    /// on the partial index will reject collisions.
    pub fn set_callback_token(&self, task_id: &str, token: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET callback_token = ?1 WHERE task_id = ?2",
            params![token, task_id],
        )?;
        Ok(())
    }

    /// Clear the callback token (called after Start to free the token for
    /// the partial UNIQUE index).
    pub fn clear_callback_token(&self, task_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET callback_token = NULL WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    /// Look up a ticket row by callback_token. Used by inbox-action receivers
    /// to map a callback_data string back to the ticket.
    pub fn fetch_by_callback_token(
        &self,
        token: &str,
    ) -> rusqlite::Result<Option<IntakeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id FROM ticket_index WHERE callback_token = ?1",
        )?;
        let task_id: Option<String> = stmt
            .query_row(params![token], |r| r.get::<_, String>(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        match task_id {
            Some(id) => self.fetch(&id),
            None => Ok(None),
        }
    }

    /// Persist the Telegram message reference for the most recent inbox card.
    pub fn set_tg_message_ref(
        &self,
        task_id: &str,
        chat_id: i64,
        msg_id: i32,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET tg_chat_id = ?1, tg_message_id = ?2 WHERE task_id = ?3",
            params![chat_id, msg_id, task_id],
        )?;
        Ok(())
    }
```

- [ ] **Step 6: Update `sample_row` test helper**

Find `fn sample_row(task_id: &str, state: TicketState) -> IntakeRow` and add the three new fields to its initializer:

```rust
    fn sample_row(task_id: &str, state: TicketState) -> IntakeRow {
        IntakeRow {
            task_id: task_id.into(),
            source_id: "linear:wsp1".into(),
            provider: "linear".into(),
            run_id: None,
            triage_decision: None,
            duplicate_of: None,
            priority: None,
            state,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            snooze_until: None,
            callback_token: None,
            tg_chat_id: None,
            tg_message_id: None,
        }
    }
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p surge-persistence --lib intake
```

Expected: all `intake::repo_tests::*` tests PASS, including the three new ones.

- [ ] **Step 8: Commit**

```bash
git add crates/surge-persistence/src/intake.rs
git commit -m "feat(persistence): IntakeRow callback_token + tg_chat_id/msg_id + repo helpers"
```

---

### Task 1.4: Add `IntakeRepo::update_state_validated`

**Files:**
- Modify: `crates/surge-persistence/src/intake.rs`

- [ ] **Step 1: Write a failing test**

Append to `#[cfg(test)] mod repo_tests` in `crates/surge-persistence/src/intake.rs`:

```rust
    #[test]
    fn update_state_validated_accepts_valid_transition() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/V-1", TicketState::InboxNotified))
            .unwrap();
        repo.update_state_validated("linear:wsp1/V-1", TicketState::RunStarted)
            .expect("InboxNotified -> RunStarted is valid");
        assert_eq!(
            repo.fetch("linear:wsp1/V-1").unwrap().unwrap().state,
            TicketState::RunStarted
        );
    }

    #[test]
    fn update_state_validated_rejects_invalid_transition() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/V-2", TicketState::Skipped))
            .unwrap();
        // Skipped is terminal; any non-self transition is invalid.
        let err = repo
            .update_state_validated("linear:wsp1/V-2", TicketState::Active)
            .unwrap_err();
        match err {
            IntakeError::InvalidTransition { from, to } => {
                assert_eq!(from, TicketState::Skipped);
                assert_eq!(to, TicketState::Active);
            }
            other => panic!("expected InvalidTransition, got {other:?}"),
        }
        // State must be unchanged.
        assert_eq!(
            repo.fetch("linear:wsp1/V-2").unwrap().unwrap().state,
            TicketState::Skipped
        );
    }

    #[test]
    fn update_state_validated_errors_when_row_missing() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        let err = repo
            .update_state_validated("linear:wsp1/missing", TicketState::Active)
            .unwrap_err();
        assert!(matches!(err, IntakeError::NotFound { .. }));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p surge-persistence --lib intake::repo_tests::update_state_validated_accepts_valid_transition
```

Expected: FAIL — `update_state_validated` and `IntakeError` don't exist.

- [ ] **Step 3: Add `IntakeError` enum**

In `crates/surge-persistence/src/intake.rs`, near the top of the file (after the existing `use` statements), add:

```rust
/// Errors raised by `IntakeRepo` mutating helpers.
#[derive(Debug, thiserror::Error)]
pub enum IntakeError {
    /// The requested transition violates the FSM defined in
    /// `TicketState::is_valid_transition_from`.
    #[error("invalid ticket state transition {from:?} -> {to:?}")]
    InvalidTransition { from: TicketState, to: TicketState },
    /// No row with the given `task_id` exists.
    #[error("ticket_index row not found: {task_id}")]
    NotFound { task_id: String },
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
```

- [ ] **Step 4: Add `update_state_validated`**

Append to `impl<'a> IntakeRepo<'a>`:

```rust
    /// Validated state transition. Errors if the row is missing or if
    /// `to.is_valid_transition_from(current)` returns false; the on-disk
    /// state is unchanged on error.
    ///
    /// This is the only mutator the inbox subsystem uses; raw `update_state`
    /// remains for crash-recovery and tests.
    pub fn update_state_validated(
        &self,
        task_id: &str,
        to: TicketState,
    ) -> Result<(), IntakeError> {
        let current = self
            .fetch(task_id)?
            .ok_or_else(|| IntakeError::NotFound {
                task_id: task_id.into(),
            })?;
        if !to.is_valid_transition_from(current.state) {
            return Err(IntakeError::InvalidTransition {
                from: current.state,
                to,
            });
        }
        self.update_state(task_id, to)?;
        Ok(())
    }
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p surge-persistence --lib intake::repo_tests::update_state_validated
```

Expected: PASS for all three new tests; existing tests still PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-persistence/src/intake.rs
git commit -m "feat(persistence): IntakeRepo::update_state_validated with FSM enforcement"
```

---

### Task 1.5: Add `IntakeRepo` snooze + run_id setters and `fetch_due_snoozed`

**Files:**
- Modify: `crates/surge-persistence/src/intake.rs`

- [ ] **Step 1: Write failing tests**

Append to `mod repo_tests`:

```rust
    #[test]
    fn snooze_until_set_clear_round_trip() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/S-1", TicketState::InboxNotified))
            .unwrap();
        let until = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        repo.set_snooze_until("linear:wsp1/S-1", until).unwrap();
        let row = repo.fetch("linear:wsp1/S-1").unwrap().unwrap();
        assert_eq!(row.snooze_until, Some(until));
        repo.clear_snooze_until("linear:wsp1/S-1").unwrap();
        assert!(repo.fetch("linear:wsp1/S-1").unwrap().unwrap().snooze_until.is_none());
    }

    #[test]
    fn set_run_id_persists() {
        let conn = db_with_schema();
        // Pre-create the run row to satisfy the FK.
        conn.execute("INSERT INTO runs(id) VALUES ('01ABCRUNID0001')", [])
            .unwrap();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/R-1", TicketState::InboxNotified))
            .unwrap();
        repo.set_run_id("linear:wsp1/R-1", "01ABCRUNID0001".into()).unwrap();
        let row = repo.fetch("linear:wsp1/R-1").unwrap().unwrap();
        assert_eq!(row.run_id.as_deref(), Some("01ABCRUNID0001"));
    }

    #[test]
    fn fetch_due_snoozed_returns_only_due_rows() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        let past = Utc::now() - chrono::Duration::hours(1);
        let future = Utc::now() + chrono::Duration::hours(1);

        let mut due_row = sample_row("linear:wsp1/D-1", TicketState::Snoozed);
        due_row.snooze_until = Some(past);
        repo.insert(&due_row).unwrap();

        let mut not_yet_row = sample_row("linear:wsp1/D-2", TicketState::Snoozed);
        not_yet_row.snooze_until = Some(future);
        repo.insert(&not_yet_row).unwrap();

        // Wrong state: skipped tickets must not be returned even if snooze_until is past.
        let mut skipped_row = sample_row("linear:wsp1/D-3", TicketState::Skipped);
        skipped_row.snooze_until = Some(past);
        repo.insert(&skipped_row).unwrap();

        let due = repo.fetch_due_snoozed(Utc::now()).unwrap();
        let ids: Vec<&str> = due.iter().map(|r| r.task_id.as_str()).collect();
        assert_eq!(ids, vec!["linear:wsp1/D-1"]);
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p surge-persistence --lib intake::repo_tests::snooze_until_set_clear_round_trip
```

Expected: FAIL — methods don't exist.

- [ ] **Step 3: Implement the methods**

Append to `impl<'a> IntakeRepo<'a>`:

```rust
    /// Set the `snooze_until` timestamp for a ticket.
    pub fn set_snooze_until(
        &self,
        task_id: &str,
        until: DateTime<Utc>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET snooze_until = ?1 WHERE task_id = ?2",
            params![until.to_rfc3339(), task_id],
        )?;
        Ok(())
    }

    /// Clear the `snooze_until` timestamp.
    pub fn clear_snooze_until(&self, task_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET snooze_until = NULL WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    /// Set the run_id (run row must already exist due to FK).
    pub fn set_run_id(&self, task_id: &str, run_id: String) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET run_id = ?1 WHERE task_id = ?2",
            params![run_id, task_id],
        )?;
        Ok(())
    }

    /// Return all rows with state='Snoozed' AND snooze_until <= now.
    /// Caller is responsible for the state transition + snooze_until clear.
    pub fn fetch_due_snoozed(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<IntakeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id FROM ticket_index \
             WHERE state = 'Snoozed' AND snooze_until IS NOT NULL AND snooze_until <= ?1 \
             ORDER BY snooze_until ASC",
        )?;
        let mut out = Vec::new();
        let mut rows = stmt.query(params![now.to_rfc3339()])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            if let Some(row) = self.fetch(&id)? {
                out.push(row);
            }
        }
        Ok(out)
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p surge-persistence --lib intake
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/intake.rs
git commit -m "feat(persistence): IntakeRepo set_run_id + snooze helpers + fetch_due_snoozed"
```

---

### Task 1.6: `Storage` inbox-queue helpers

**Files:**
- Create: `crates/surge-persistence/src/runs/inbox_queue.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs`
- Modify: `crates/surge-persistence/src/runs/storage.rs`

- [ ] **Step 1: Create the inbox-queue module**

Create `crates/surge-persistence/src/runs/inbox_queue.rs`:

```rust
//! Inbox action + delivery queues for the surge-daemon inbox subsystem.
//!
//! The receivers (`TgInboxBot`, `DesktopActionListener`) write to
//! `inbox_action_queue`; `InboxActionConsumer` polls and processes.
//!
//! The router writes to `inbox_delivery_queue` when a `RouterOutput::Triage`
//! event needs an inbox card; the bot's outgoing loop and the desktop
//! deliverer both read from it independently.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

/// One row of the `inbox_action_queue` table — a pending request from a
/// receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxActionRow {
    pub seq: i64,
    pub kind: InboxActionKind,
    pub task_id: String,
    pub callback_token: String,
    pub decided_via: String,
    pub snooze_until: Option<DateTime<Utc>>,
    pub enqueued_at: DateTime<Utc>,
}

/// Action kind on `inbox_action_queue.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxActionKind {
    Start,
    Snooze,
    Skip,
}

impl InboxActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Snooze => "snooze",
            Self::Skip => "skip",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "start" => Some(Self::Start),
            "snooze" => Some(Self::Snooze),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

/// One row of the `inbox_delivery_queue` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxDeliveryRow {
    pub seq: i64,
    pub task_id: String,
    pub callback_token: String,
    pub payload_json: String,
    pub enqueued_at: DateTime<Utc>,
    pub telegram_delivered_at: Option<DateTime<Utc>>,
    pub telegram_chat_id: Option<i64>,
    pub telegram_message_id: Option<i32>,
    pub desktop_delivered_at: Option<DateTime<Utc>>,
}

/// Append a new action row. Returns the assigned seq.
pub fn append_action(
    conn: &Connection,
    kind: InboxActionKind,
    task_id: &str,
    callback_token: &str,
    decided_via: &str,
    snooze_until: Option<DateTime<Utc>>,
) -> rusqlite::Result<i64> {
    let now = Utc::now();
    conn.execute(
        "INSERT INTO inbox_action_queue \
            (kind, task_id, callback_token, decided_via, snooze_until, enqueued_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            kind.as_str(),
            task_id,
            callback_token,
            decided_via,
            snooze_until.map(|d| d.to_rfc3339()),
            now.to_rfc3339(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Read pending action rows (processed_at IS NULL), ordered by seq.
pub fn list_pending_actions(conn: &Connection) -> rusqlite::Result<Vec<InboxActionRow>> {
    let mut stmt = conn.prepare(
        "SELECT seq, kind, task_id, callback_token, decided_via, snooze_until, enqueued_at \
         FROM inbox_action_queue WHERE processed_at IS NULL ORDER BY seq ASC",
    )?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let kind_str: String = r.get(1)?;
        let kind = InboxActionKind::parse(&kind_str).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown inbox action kind: {kind_str}").into(),
            )
        })?;
        let snooze_until_str: Option<String> = r.get(5)?;
        let enqueued_at_str: String = r.get(6)?;
        out.push(InboxActionRow {
            seq: r.get(0)?,
            kind,
            task_id: r.get(2)?,
            callback_token: r.get(3)?,
            decided_via: r.get(4)?,
            snooze_until: snooze_until_str
                .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?,
            enqueued_at: DateTime::parse_from_rfc3339(&enqueued_at_str)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?
                .with_timezone(&Utc),
        });
    }
    Ok(out)
}

/// Mark a row processed (idempotent — safe to call twice).
pub fn mark_action_processed(conn: &Connection, seq: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbox_action_queue SET processed_at = ?1 WHERE seq = ?2 AND processed_at IS NULL",
        params![Utc::now().to_rfc3339(), seq],
    )?;
    Ok(())
}

/// Append a delivery row for an outgoing inbox card.
pub fn append_delivery(
    conn: &Connection,
    task_id: &str,
    callback_token: &str,
    payload_json: &str,
) -> rusqlite::Result<i64> {
    let now = Utc::now();
    conn.execute(
        "INSERT INTO inbox_delivery_queue \
            (task_id, callback_token, payload_json, enqueued_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, callback_token, payload_json, now.to_rfc3339()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Pending Telegram deliveries (telegram_delivered_at IS NULL).
pub fn list_pending_telegram_deliveries(
    conn: &Connection,
) -> rusqlite::Result<Vec<InboxDeliveryRow>> {
    list_deliveries(conn, "telegram_delivered_at IS NULL")
}

/// Pending Desktop deliveries (desktop_delivered_at IS NULL).
pub fn list_pending_desktop_deliveries(
    conn: &Connection,
) -> rusqlite::Result<Vec<InboxDeliveryRow>> {
    list_deliveries(conn, "desktop_delivered_at IS NULL")
}

fn list_deliveries(
    conn: &Connection,
    where_clause: &str,
) -> rusqlite::Result<Vec<InboxDeliveryRow>> {
    let sql = format!(
        "SELECT seq, task_id, callback_token, payload_json, enqueued_at, \
                telegram_delivered_at, telegram_chat_id, telegram_message_id, \
                desktop_delivered_at \
         FROM inbox_delivery_queue WHERE {where_clause} ORDER BY seq ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let enqueued_at_str: String = r.get(4)?;
        let tg_at_str: Option<String> = r.get(5)?;
        let dt_at_str: Option<String> = r.get(8)?;
        out.push(InboxDeliveryRow {
            seq: r.get(0)?,
            task_id: r.get(1)?,
            callback_token: r.get(2)?,
            payload_json: r.get(3)?,
            enqueued_at: DateTime::parse_from_rfc3339(&enqueued_at_str)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?
                .with_timezone(&Utc),
            telegram_delivered_at: tg_at_str
                .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?,
            telegram_chat_id: r.get(6)?,
            telegram_message_id: r.get(7)?,
            desktop_delivered_at: dt_at_str
                .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?,
        });
    }
    Ok(out)
}

/// Record successful Telegram delivery.
pub fn record_telegram_delivered(
    conn: &Connection,
    seq: i64,
    chat_id: i64,
    message_id: i32,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbox_delivery_queue SET telegram_delivered_at = ?1, \
            telegram_chat_id = ?2, telegram_message_id = ?3 \
         WHERE seq = ?4 AND telegram_delivered_at IS NULL",
        params![Utc::now().to_rfc3339(), chat_id, message_id, seq],
    )?;
    Ok(())
}

/// Record successful Desktop delivery.
pub fn record_desktop_delivered(conn: &Connection, seq: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbox_delivery_queue SET desktop_delivered_at = ?1 \
         WHERE seq = ?2 AND desktop_delivered_at IS NULL",
        params![Utc::now().to_rfc3339(), seq],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
            .unwrap();
        let m1 = include_str!("migrations/registry/0002_ticket_index.sql");
        conn.execute_batch(m1).unwrap();
        let m2 = include_str!("migrations/registry/0004_inbox_callback_columns.sql");
        conn.execute_batch(m2).unwrap();
        let m3 = include_str!("migrations/registry/0005_inbox_queues.sql");
        conn.execute_batch(m3).unwrap();
        conn
    }

    #[test]
    fn append_then_list_then_mark_processed() {
        let conn = db();
        let seq = append_action(&conn, InboxActionKind::Start, "linear:t/T-1", "tok1", "telegram", None).unwrap();
        assert!(seq > 0);
        let pending = list_pending_actions(&conn).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, InboxActionKind::Start);
        assert_eq!(pending[0].callback_token, "tok1");
        mark_action_processed(&conn, seq).unwrap();
        assert_eq!(list_pending_actions(&conn).unwrap().len(), 0);
    }

    #[test]
    fn snooze_action_carries_until_timestamp() {
        let conn = db();
        let until = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        append_action(&conn, InboxActionKind::Snooze, "linear:t/T-2", "tok2", "desktop", Some(until)).unwrap();
        let pending = list_pending_actions(&conn).unwrap();
        assert_eq!(pending[0].snooze_until, Some(until));
    }

    #[test]
    fn delivery_legs_independent() {
        let conn = db();
        let seq = append_delivery(&conn, "linear:t/D-1", "tok-d", r#"{"x":1}"#).unwrap();
        assert_eq!(list_pending_telegram_deliveries(&conn).unwrap().len(), 1);
        assert_eq!(list_pending_desktop_deliveries(&conn).unwrap().len(), 1);

        record_telegram_delivered(&conn, seq, 12345, 6789).unwrap();
        assert_eq!(list_pending_telegram_deliveries(&conn).unwrap().len(), 0);
        assert_eq!(list_pending_desktop_deliveries(&conn).unwrap().len(), 1);

        record_desktop_delivered(&conn, seq).unwrap();
        assert_eq!(list_pending_desktop_deliveries(&conn).unwrap().len(), 0);
    }

    #[test]
    fn idempotent_marking() {
        let conn = db();
        let seq = append_action(&conn, InboxActionKind::Skip, "linear:t/T-3", "tok3", "telegram", None).unwrap();
        mark_action_processed(&conn, seq).unwrap();
        // Second call is a no-op (the WHERE clause filters it out).
        mark_action_processed(&conn, seq).unwrap();
    }
}
```

- [ ] **Step 2: Wire the new module into `runs/mod.rs`**

Open `crates/surge-persistence/src/runs/mod.rs`. Add `pub mod inbox_queue;` (alphabetical placement; near other `pub mod` declarations).

- [ ] **Step 3: Re-export from `lib.rs`**

Open `crates/surge-persistence/src/lib.rs`. Add a re-export so callers don't have to spell `runs::inbox_queue`:

```rust
pub use runs::inbox_queue;
```

(Place near the existing `pub use` statements.)

- [ ] **Step 4: Add `Storage` accessor for the registry connection**

The bot loop and consumer need a way to acquire a connection from the registry pool. Add to `crates/surge-persistence/src/runs/storage.rs` `impl Storage`:

```rust
    /// Acquire a registry-pool connection. Used by inbox subsystems that
    /// share the registry DB. The caller holds the connection for the
    /// duration of one logical operation; do not hold it across awaits.
    pub fn acquire_registry_conn(
        &self,
    ) -> Result<r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>, r2d2::Error> {
        self.registry_pool.get()
    }
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p surge-persistence --lib runs::inbox_queue
```

Expected: all four tests PASS.

- [ ] **Step 6: Build the workspace**

```bash
cargo build --workspace
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-persistence/src/runs/inbox_queue.rs \
        crates/surge-persistence/src/runs/mod.rs \
        crates/surge-persistence/src/runs/storage.rs \
        crates/surge-persistence/src/lib.rs
git commit -m "feat(persistence): inbox_queue module — action + delivery queues"
```

---

## Phase 2 — Core types + events

### Task 2.1: Add `Inbox*Requested` and `InboxRunStarted` variants to `SurgeEvent`

**Files:**
- Modify: `crates/surge-core/src/event.rs`

- [ ] **Step 1: Write a failing test**

Append to the existing `mod` containing `tracker_roundtrip` tests (search for `// ── RFC-0010 tracker variant tests ──`):

```rust
    #[test]
    fn inbox_run_start_requested_round_trip() {
        let event = SurgeEvent::InboxRunStartRequested {
            task_id: "linear:wsp1/T-1".into(),
            callback_token: "01HKGZTOK1".into(),
            decided_via: "telegram".into(),
        };
        let rt = tracker_roundtrip(&event);
        match rt {
            SurgeEvent::InboxRunStartRequested { task_id, callback_token, decided_via } => {
                assert_eq!(task_id, "linear:wsp1/T-1");
                assert_eq!(callback_token, "01HKGZTOK1");
                assert_eq!(decided_via, "telegram");
            }
            other => panic!("expected InboxRunStartRequested, got {other:?}"),
        }
    }

    #[test]
    fn inbox_snooze_requested_round_trip() {
        let event = SurgeEvent::InboxSnoozeRequested {
            task_id: "linear:wsp1/T-2".into(),
            callback_token: "01HKGZTOK2".into(),
            until_rfc3339: "2030-01-01T00:00:00Z".into(),
            decided_via: "desktop".into(),
        };
        let rt = tracker_roundtrip(&event);
        assert!(matches!(rt, SurgeEvent::InboxSnoozeRequested { .. }));
    }

    #[test]
    fn inbox_skip_requested_round_trip() {
        let event = SurgeEvent::InboxSkipRequested {
            task_id: "linear:wsp1/T-3".into(),
            callback_token: "01HKGZTOK3".into(),
            decided_via: "telegram".into(),
        };
        let rt = tracker_roundtrip(&event);
        assert!(matches!(rt, SurgeEvent::InboxSkipRequested { .. }));
    }

    #[test]
    fn inbox_run_started_round_trip() {
        let event = SurgeEvent::InboxRunStarted {
            task_id: "linear:wsp1/T-4".into(),
            run_id: "01ABCRUN0001".into(),
        };
        let rt = tracker_roundtrip(&event);
        assert!(matches!(rt, SurgeEvent::InboxRunStarted { .. }));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p surge-core --lib event::tests::inbox_run_start_requested_round_trip
```

Expected: FAIL — variants don't exist.

- [ ] **Step 3: Add the new variants**

Open `crates/surge-core/src/event.rs`. Find the `// === RFC-0010 tracker integration ===` section and append (within the `SurgeEvent` enum definition, after `UserMentionReceived`):

```rust
    /// User tapped Start on an inbox card; queued for InboxActionConsumer.
    InboxRunStartRequested {
        task_id: String,
        callback_token: String,
        decided_via: String, // "telegram"|"desktop"
    },

    /// User tapped Snooze on an inbox card.
    InboxSnoozeRequested {
        task_id: String,
        callback_token: String,
        /// RFC-3339 absolute timestamp until which the card is snoozed.
        until_rfc3339: String,
        decided_via: String,
    },

    /// User tapped Skip on an inbox card.
    InboxSkipRequested {
        task_id: String,
        callback_token: String,
        decided_via: String,
    },

    /// Inbox-initiated run was successfully started by `Engine::start_run`.
    InboxRunStarted {
        task_id: String,
        run_id: String,
    },
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p surge-core --lib event::tests
```

Expected: all PASS, including the four new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-core/src/event.rs
git commit -m "feat(core): SurgeEvent inbox-action variants (Start/Snooze/Skip Requested + RunStarted)"
```

---

### Task 2.2: Add `TelegramConfig` and `InboxConfig` to `SurgeConfig`

**Files:**
- Modify: `crates/surge-core/src/config.rs`

- [ ] **Step 1: Write a failing test**

Append to the `#[cfg(test)] mod` block at the bottom of `crates/surge-core/src/config.rs`:

```rust
    #[test]
    fn telegram_and_inbox_config_round_trip_toml() {
        let toml_str = r#"
[telegram]
chat_id_env = "SURGE_TELEGRAM_CHAT_ID"
bot_token_env = "SURGE_TELEGRAM_BOT_TOKEN"

[inbox]
snooze_poll_interval_seconds = 600
delivery_channels = ["telegram", "desktop"]
"#;
        let cfg: SurgeConfig = toml::from_str(toml_str).expect("must parse");
        let telegram = cfg.telegram.expect("telegram section");
        assert_eq!(telegram.chat_id_env.as_deref(), Some("SURGE_TELEGRAM_CHAT_ID"));
        assert_eq!(telegram.bot_token_env.as_deref(), Some("SURGE_TELEGRAM_BOT_TOKEN"));
        assert_eq!(telegram.chat_id, None);
        let inbox = cfg.inbox;
        assert_eq!(inbox.snooze_poll_interval.as_secs(), 600);
        assert_eq!(inbox.delivery_channels, vec!["telegram".to_string(), "desktop".to_string()]);
    }

    #[test]
    fn telegram_and_inbox_config_default_when_absent() {
        let cfg: SurgeConfig = toml::from_str("").unwrap();
        assert!(cfg.telegram.is_none());
        assert_eq!(cfg.inbox.snooze_poll_interval.as_secs(), 300);
        assert!(cfg.inbox.delivery_channels.is_empty());
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test -p surge-core --lib config::tests::telegram_and_inbox_config_round_trip_toml
```

Expected: FAIL — fields don't exist.

- [ ] **Step 3: Add the config types**

In `crates/surge-core/src/config.rs`, append (after the existing config types and before the test module):

```rust
/// Optional Telegram bot configuration.
///
/// `chat_id_env` and `bot_token_env` are the names of the environment
/// variables to read for the chat ID and bot token respectively. Direct
/// `chat_id` is allowed for tests / local dev only — secrets never go in
/// `surge.toml` (per RFC-0010 pattern).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramConfig {
    #[serde(default)]
    pub chat_id_env: Option<String>,
    #[serde(default)]
    pub bot_token_env: Option<String>,
    #[serde(default)]
    pub chat_id: Option<i64>,
}

/// Inbox subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxConfig {
    /// How often the snooze scheduler polls for due cards.
    #[serde(
        rename = "snooze_poll_interval_seconds",
        default = "default_snooze_poll_interval_secs",
        with = "duration_seconds_field"
    )]
    pub snooze_poll_interval: std::time::Duration,
    /// Which channels deliver inbox cards. Empty == "all configured".
    #[serde(default)]
    pub delivery_channels: Vec<String>,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            snooze_poll_interval: std::time::Duration::from_secs(300),
            delivery_channels: Vec::new(),
        }
    }
}

const fn default_snooze_poll_interval_secs() -> std::time::Duration {
    std::time::Duration::from_secs(300)
}

mod duration_seconds_field {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}
```

- [ ] **Step 4: Add fields to `SurgeConfig`**

Find the `SurgeConfig` struct definition. Append two fields:

```rust
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,

    #[serde(default)]
    pub inbox: InboxConfig,
```

If `SurgeConfig` already has a `Default` impl that lists each field, add `telegram: None` and `inbox: InboxConfig::default()`.

- [ ] **Step 5: Run tests**

```bash
cargo test -p surge-core --lib config::tests
```

Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-core/src/config.rs
git commit -m "feat(core): SurgeConfig telegram + inbox sections"
```

---

## Phase 3 — `BootstrapGraphBuilder` trait + `MinimalBootstrapGraphBuilder`

### Task 3.1: Create the `bootstrap` module skeleton

**Files:**
- Create: `crates/surge-orchestrator/src/bootstrap/mod.rs`
- Create: `crates/surge-orchestrator/src/bootstrap/builder.rs`
- Modify: `crates/surge-orchestrator/src/lib.rs`
- Modify: `crates/surge-orchestrator/Cargo.toml`

- [ ] **Step 1: Add `surge-intake` dep to `surge-orchestrator/Cargo.toml`**

Open `crates/surge-orchestrator/Cargo.toml`. Under `[dependencies]`, add (alphabetical placement):

```toml
surge-intake = { workspace = true }
```

- [ ] **Step 2: Create `bootstrap/mod.rs`**

Create `crates/surge-orchestrator/src/bootstrap/mod.rs`:

```rust
//! Bootstrap-graph construction.
//!
//! The `BootstrapGraphBuilder` trait abstracts how a user prompt becomes the
//! initial `Graph` for an `Engine::start_run` invocation. Today's
//! `MinimalBootstrapGraphBuilder` produces a single-Agent graph; future
//! `StagedBootstrapGraphBuilder` (RFC-0004) will produce the 6-node prelude
//! Description Author → Approve → Roadmap Planner → Approve → Flow Generator
//! → Approve.

mod builder;
mod minimal;

pub use builder::{
    BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt,
};
pub use minimal::MinimalBootstrapGraphBuilder;
```

- [ ] **Step 3: Create `bootstrap/builder.rs`**

Create `crates/surge-orchestrator/src/bootstrap/builder.rs`:

```rust
//! `BootstrapGraphBuilder` trait + shared types.

use async_trait::async_trait;
use std::path::PathBuf;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_intake::types::Priority;
use thiserror::Error;

/// Build the initial `Graph` for a user-initiated run.
///
/// Implementations are stateless and shareable behind `Arc<dyn ...>`. The
/// daemon's inbox consumer holds one and invokes `build` on each Start tap.
#[async_trait]
pub trait BootstrapGraphBuilder: Send + Sync {
    async fn build(
        &self,
        run_id: RunId,
        prompt: BootstrapPrompt,
        worktree: PathBuf,
    ) -> Result<Graph, BootstrapBuildError>;
}

/// Free-text prompt + structured ticket metadata.
///
/// `MinimalBootstrapGraphBuilder` reads only `description`. Future
/// `StagedBootstrapGraphBuilder` will read all fields to populate the
/// Description Author preamble.
#[derive(Debug, Clone)]
pub struct BootstrapPrompt {
    pub title: String,
    pub description: String,
    pub tracker_url: Option<String>,
    pub priority: Option<Priority>,
    pub labels: Vec<String>,
}

#[derive(Debug, Error)]
pub enum BootstrapBuildError {
    #[error("invalid prompt: {0}")]
    InvalidPrompt(String),
    #[error("graph construction failed: {0}")]
    GraphBuild(String),
    #[error("profile not available: {0}")]
    ProfileMissing(String),
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Open `crates/surge-orchestrator/src/lib.rs`. After the existing `pub mod triage;` line (or alphabetically appropriate location), add:

```rust
pub mod bootstrap;
```

- [ ] **Step 5: Build**

```bash
cargo build -p surge-orchestrator
```

Expected: clean (no `MinimalBootstrapGraphBuilder` impl yet — the `mod minimal` line in `mod.rs` will fail until Task 3.2; **temporarily comment that line out** for this task only, or include this task and 3.2 in one commit).

If you choose to keep them separate, modify `bootstrap/mod.rs` for this task to:
```rust
mod builder;
// mod minimal;   // added in Task 3.2

pub use builder::{
    BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt,
};
// pub use minimal::MinimalBootstrapGraphBuilder;
```
and uncomment in Task 3.2.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/Cargo.toml \
        crates/surge-orchestrator/src/bootstrap/mod.rs \
        crates/surge-orchestrator/src/bootstrap/builder.rs \
        crates/surge-orchestrator/src/lib.rs
git commit -m "feat(orchestrator): BootstrapGraphBuilder trait + types"
```

---

### Task 3.2: Implement `MinimalBootstrapGraphBuilder`

**Files:**
- Create: `crates/surge-orchestrator/src/bootstrap/minimal.rs`
- Modify: `crates/surge-orchestrator/src/bootstrap/mod.rs` (un-comment if you split)

- [ ] **Step 1: Inspect existing graph types**

Run a quick inspection to confirm the exact shape of `Graph`, `Node`, `Edge`, and `NodeKind::Agent`:

```bash
grep -n "pub struct Graph\|pub enum NodeKind\|pub struct Node\b" crates/surge-core/src/graph/*.rs
```

Note the field names: at the time of writing, `Graph` has `nodes: Vec<Node>`, `edges: Vec<Edge>`, `entry: NodeKey`. `Node` has `key: NodeKey`, `kind: NodeKind`, `config: NodeConfig`. `NodeKind::Agent` carries an `AgentConfig`. Adapt the implementation below if any of these names changed.

- [ ] **Step 2: Write failing tests**

Create `crates/surge-orchestrator/src/bootstrap/minimal.rs` and write the test module first (TDD):

```rust
//! `MinimalBootstrapGraphBuilder` — produces a single-Agent graph for
//! today's inbox-card pipeline. Replaced by `StagedBootstrapGraphBuilder`
//! when RFC-0004 lands.

use crate::bootstrap::builder::{
    BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt,
};
use async_trait::async_trait;
use std::path::PathBuf;
use surge_core::graph::Graph;
use surge_core::id::RunId;

/// Single-stage Agent bootstrap.
#[derive(Debug, Clone, Default)]
pub struct MinimalBootstrapGraphBuilder;

impl MinimalBootstrapGraphBuilder {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BootstrapGraphBuilder for MinimalBootstrapGraphBuilder {
    async fn build(
        &self,
        _run_id: RunId,
        prompt: BootstrapPrompt,
        _worktree: PathBuf,
    ) -> Result<Graph, BootstrapBuildError> {
        if prompt.description.trim().is_empty() {
            return Err(BootstrapBuildError::InvalidPrompt(
                "description must not be empty".into(),
            ));
        }
        let prompt_text = render_prompt(&prompt);
        build_single_agent_graph(&prompt_text)
            .map_err(BootstrapBuildError::GraphBuild)
    }
}

fn render_prompt(prompt: &BootstrapPrompt) -> String {
    let mut s = String::new();
    s.push_str("You are working on this ticket.\n\n");
    s.push_str(&format!("Title: {}\n", prompt.title));
    if let Some(url) = &prompt.tracker_url {
        s.push_str(&format!("URL: {url}\n"));
    }
    if !prompt.labels.is_empty() {
        s.push_str(&format!("Labels: {}\n", prompt.labels.join(", ")));
    }
    s.push_str("\nDescription:\n");
    s.push_str(&prompt.description);
    s.push_str(
        "\n\nImplement the request directly in this worktree. Run tests \
         before reporting done. If the request is ambiguous, escalate.",
    );
    s
}

/// Build the actual Graph value.
///
/// Adapt the body of this function to match the exact constructors that
/// `surge-core::graph` exposes at impl time. The shape is invariant:
/// one Agent node + two Terminal nodes + two edges.
fn build_single_agent_graph(prompt_text: &str) -> Result<Graph, String> {
    use surge_core::graph::{Edge, Graph, Node, NodeConfig, NodeKey, NodeKind, OutcomeKey, PortRef, TerminalKind};

    // Adapt names below to whatever is in your tree:
    //   - NodeKey::try_new
    //   - NodeKind::Agent { ... } / NodeKind::Terminal { ... }
    //   - NodeConfig holding the Agent + Terminal payloads
    //   - Edge::new(from: PortRef, to: NodeKey)
    let agent_key = NodeKey::try_new("bootstrap-agent")
        .map_err(|e| format!("agent NodeKey: {e}"))?;
    let success_key = NodeKey::try_new("terminal-success")
        .map_err(|e| format!("success NodeKey: {e}"))?;
    let failure_key = NodeKey::try_new("terminal-failure")
        .map_err(|e| format!("failure NodeKey: {e}"))?;

    let agent_node = Node::new_agent_with_prompt(
        agent_key.clone(),
        /* profile */ "implementer",
        /* prompt override */ prompt_text,
        /* declared outcomes */ vec![
            OutcomeKey::try_new("done").unwrap(),
            OutcomeKey::try_new("blocked").unwrap(),
        ],
    )
    .map_err(|e| format!("agent Node: {e}"))?;

    let success_node = Node::new_terminal(
        success_key.clone(),
        TerminalKind::Success,
    )
    .map_err(|e| format!("success Node: {e}"))?;

    let failure_node = Node::new_terminal(
        failure_key.clone(),
        TerminalKind::Failure { exit_code: 1 },
    )
    .map_err(|e| format!("failure Node: {e}"))?;

    let edges = vec![
        Edge::new(
            PortRef {
                node: agent_key.clone(),
                outcome: OutcomeKey::try_new("done").unwrap(),
            },
            success_key.clone(),
        ),
        Edge::new(
            PortRef {
                node: agent_key.clone(),
                outcome: OutcomeKey::try_new("blocked").unwrap(),
            },
            failure_key.clone(),
        ),
    ];

    Graph::new(
        /* entry */ agent_key,
        /* nodes */ vec![agent_node, success_node, failure_node],
        /* edges */ edges,
    )
    .map_err(|e| format!("Graph::new: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::RunId;

    fn sample_prompt() -> BootstrapPrompt {
        BootstrapPrompt {
            title: "Fix parser panic".into(),
            description: "Stack overflow on deep nesting in parse_object.".into(),
            tracker_url: Some("https://github.com/o/r/issues/42".into()),
            priority: None,
            labels: vec!["surge:enabled".into(), "bug".into()],
        }
    }

    #[tokio::test]
    async fn produces_graph_for_valid_prompt() {
        let builder = MinimalBootstrapGraphBuilder::new();
        let graph = builder
            .build(RunId::new(), sample_prompt(), std::env::temp_dir())
            .await
            .expect("builds");
        // The graph has at least one node and is internally consistent.
        // (Structural validation is performed by the engine on start_run.)
        assert!(!graph.nodes().is_empty());
    }

    #[tokio::test]
    async fn rejects_empty_description() {
        let builder = MinimalBootstrapGraphBuilder::new();
        let mut p = sample_prompt();
        p.description = "   ".into();
        let err = builder
            .build(RunId::new(), p, std::env::temp_dir())
            .await
            .unwrap_err();
        assert!(matches!(err, BootstrapBuildError::InvalidPrompt(_)));
    }

    #[test]
    fn render_prompt_includes_title_url_labels_description() {
        let p = sample_prompt();
        let rendered = render_prompt(&p);
        assert!(rendered.contains("Fix parser panic"));
        assert!(rendered.contains("https://github.com/o/r/issues/42"));
        assert!(rendered.contains("surge:enabled"));
        assert!(rendered.contains("Stack overflow"));
        assert!(rendered.contains("Implement the request"));
    }
}
```

- [ ] **Step 3: Run tests to confirm shape**

```bash
cargo test -p surge-orchestrator --lib bootstrap::minimal::tests
```

Expected: COMPILATION ERROR — the `Node::new_agent_with_prompt`, `Edge::new(...)`, `Graph::new(...)`, `TerminalKind`, `PortRef`, etc. constructor names may differ from your actual `surge-core::graph` API.

If the names differ, adapt `build_single_agent_graph` to the actual API. The body MUST produce a valid `Graph` containing:
1. One `NodeKind::Agent` node (using profile `implementer`) with a prompt override containing `prompt_text`. Declared outcomes: `done`, `blocked`.
2. One `NodeKind::Terminal { TerminalKind::Success }` node.
3. One `NodeKind::Terminal { TerminalKind::Failure { exit_code: 1 } }` node.
4. Two edges: `agent.done → success`, `agent.blocked → failure`.
5. Entry = the agent node.

If your `Node` constructors take a `NodeConfig` value rather than per-kind helpers, build the `NodeConfig::Agent(AgentConfig { ... })` directly. Always run a structural validator (e.g., `surge_orchestrator::engine::validate::validate_for_m6`) before returning to catch shape errors early — but only if it's already on `pub` use; otherwise rely on the caller's validation in `Engine::start_run`.

- [ ] **Step 4: Run tests until green**

```bash
cargo test -p surge-orchestrator --lib bootstrap::minimal::tests
```

Expected: all three tests PASS.

- [ ] **Step 5: Re-enable `mod minimal` in `bootstrap/mod.rs`** (if you split Task 3.1 / 3.2)

In `crates/surge-orchestrator/src/bootstrap/mod.rs`, ensure both lines are uncommented:

```rust
mod builder;
mod minimal;

pub use builder::{
    BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt,
};
pub use minimal::MinimalBootstrapGraphBuilder;
```

- [ ] **Step 6: Workspace build**

```bash
cargo build --workspace
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-orchestrator/src/bootstrap/minimal.rs \
        crates/surge-orchestrator/src/bootstrap/mod.rs
git commit -m "feat(orchestrator): MinimalBootstrapGraphBuilder — single-Agent bootstrap"
```

---

## Phase 4 — `InboxCardPayload` schema migration

### Task 4.1: Replace `InboxCardPayload.run_id` with `callback_token`

**Files:**
- Modify: `crates/surge-notify/src/messages.rs`
- Modify: `crates/surge-notify/src/telegram.rs`
- Modify: `crates/surge-notify/src/desktop.rs`

- [ ] **Step 1: Update the type and round-trip test**

Open `crates/surge-notify/src/messages.rs`. Modify `InboxCardPayload`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCardPayload {
    pub task_id: TaskId,
    pub source_id: String,
    pub provider: String,
    pub title: String,
    pub summary: String,
    pub priority: Priority,
    pub task_url: String,
    /// Short ULID — embedded in callback_data of inline buttons. The
    /// daemon resolves this to a `task_id` via
    /// `IntakeRepo::fetch_by_callback_token`. Replaces the prior `run_id`
    /// field (the actual `RunId` is generated only when Engine::start_run
    /// runs; pre-creating one violated the FK on `ticket_index.run_id`).
    pub callback_token: String,
}
```

Update the `inbox_card_tests::round_trip_inbox_card` test at the bottom of the file:

```rust
        let payload = InboxCardPayload {
            task_id: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            title: "Fix parser".into(),
            summary: "panic on nested".into(),
            priority: Priority::High,
            task_url: "https://github.com/user/repo/issues/1".into(),
            callback_token: "01HKGZTOKABC".into(),
        };
        let msg = NotifyMessage::InboxCard(payload.clone());
        let s = serde_json::to_string(&msg).unwrap();
        let back: NotifyMessage = serde_json::from_str(&s).unwrap();
        match back {
            NotifyMessage::InboxCard(p) => {
                assert_eq!(p.task_id, payload.task_id);
                assert_eq!(p.callback_token, "01HKGZTOKABC");
                assert_eq!(p.priority, Priority::High);
            }
        }
```

- [ ] **Step 2: Update Telegram formatter**

Open `crates/surge-notify/src/telegram.rs`. In `format_inbox_card`, replace the three `payload.run_id` references in the keyboard construction:

```rust
    let keyboard = vec![
        vec![
            InboxKeyboardButton::callback(
                "▶ Start",
                format!("inbox:start:{}", payload.callback_token),
            ),
            InboxKeyboardButton::callback(
                "⏸ Snooze 24h",
                format!("inbox:snooze:{}", payload.callback_token),
            ),
            InboxKeyboardButton::callback(
                "✕ Skip",
                format!("inbox:skip:{}", payload.callback_token),
            ),
        ],
        vec![InboxKeyboardButton::url(
            "View ticket ↗",
            payload.task_url.clone(),
        )],
    ];
```

Update the test fixture `sample_payload()`:

```rust
    fn sample_payload() -> InboxCardPayload {
        InboxCardPayload {
            task_id: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            title: "Fix parser panic".into(),
            summary: "Stack overflow at depth 16".into(),
            priority: Priority::High,
            task_url: "https://github.com/user/repo/issues/1".into(),
            callback_token: "01HKGZTOKABC".into(),
        }
    }
```

Update the `keyboard_layout_has_start_snooze_skip_and_view_url` test's expected callback_data values:

```rust
        assert_eq!(row0[0].data, "inbox:start:01HKGZTOKABC");
        assert_eq!(row0[1].data, "inbox:snooze:01HKGZTOKABC");
        assert_eq!(row0[2].data, "inbox:skip:01HKGZTOKABC");
```

- [ ] **Step 3: Update Desktop formatter**

Open `crates/surge-notify/src/desktop.rs`. In `format_inbox_card_desktop`, replace the `run_id` reference:

```rust
    let token = payload.callback_token.clone();
    let actions = vec![
        (format!("inbox:start:{token}"), "Start".to_string()),
        (format!("inbox:snooze:{token}"), "Snooze 24h".to_string()),
        (format!("inbox:skip:{token}"), "Skip".to_string()),
    ];
```

Update the test fixture and assertions:

```rust
    fn sample_payload() -> InboxCardPayload {
        InboxCardPayload {
            task_id: TaskId::try_new("linear:wsp/A-1").unwrap(),
            source_id: "linear:wsp".into(),
            provider: "linear".into(),
            title: "Add tracing to auth".into(),
            summary: "ad-hoc".into(),
            priority: Priority::Medium,
            task_url: "https://linear.app/wsp/issue/A-1".into(),
            callback_token: "tok_x".into(),
        }
    }
```

In `three_actions_in_correct_order`:

```rust
        assert_eq!(r.actions[0].0, "inbox:start:tok_x");
        assert_eq!(r.actions[1].0, "inbox:snooze:tok_x");
        assert_eq!(r.actions[2].0, "inbox:skip:tok_x");
```

- [ ] **Step 4: Build and run tests**

```bash
cargo build -p surge-notify
cargo test -p surge-notify --lib
```

Expected: all PASS.

- [ ] **Step 5: Update the daemon's existing InboxCardPayload construction**

Open `crates/surge-daemon/src/main.rs`. Find the `RouterOutput::Triage` arm (around line 311). The current code generates `run_id_str = ulid::Ulid::new().to_string()` and stores it as `run_id`. Replace with:

```rust
                surge_intake::router::RouterOutput::Triage { event } => {
                    let title = event
                        .raw_payload
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("New ticket")
                        .to_string();
                    let task_url = event
                        .raw_payload
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let provider = event
                        .task_id
                        .as_str()
                        .split(':')
                        .next()
                        .unwrap_or("unknown")
                        .to_string();
                    let callback_token = ulid::Ulid::new().to_string();
                    let payload = surge_notify::messages::InboxCardPayload {
                        task_id: event.task_id.clone(),
                        source_id: event.source_id.clone(),
                        provider: provider.clone(),
                        title: title.clone(),
                        summary: String::new(),
                        priority: surge_intake::types::Priority::Medium,
                        task_url,
                        callback_token: callback_token.clone(),
                    };
                    // The full enqueue + state update logic is wired in Phase 9.
                    // For this step we keep the existing render-and-deliver-via-Desktop
                    // path so daemon still compiles between phases.
                    let rendered_desktop =
                        surge_notify::desktop::format_inbox_card_desktop(&payload);
                    let rendered = surge_notify::RenderedNotification {
                        severity: surge_core::notify_config::NotifySeverity::Info,
                        title: rendered_desktop.title.clone(),
                        body: rendered_desktop.body.clone(),
                        artifact_paths: vec![],
                    };
                    let run_id = match callback_token.parse::<surge_core::id::RunId>() {
                        Ok(id) => id,
                        Err(_) => {
                            // ULID-string differs in checksum from RunId's expected
                            // form; use a fresh RunId for the delivery context.
                            surge_core::id::RunId::new()
                        }
                    };
                    let node_key = match surge_core::keys::NodeKey::try_new("intake") {
                        Ok(key) => key,
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to construct intake NodeKey; skipping delivery");
                            continue;
                        },
                    };
                    let channel = surge_core::notify_config::NotifyChannel::Desktop;
                    let ctx = surge_notify::NotifyDeliveryContext {
                        run_id,
                        node: &node_key,
                    };
                    match notifier.deliver(&ctx, &channel, &rendered).await {
                        Ok(()) => tracing::info!(task_id = %event.task_id, "InboxCard delivered to Desktop"),
                        Err(surge_notify::NotifyError::ChannelNotConfigured) => {
                            tracing::debug!(task_id = %event.task_id, "Desktop channel not configured; skipping");
                        }
                        Err(e) => tracing::warn!(error = %e, task_id = %event.task_id, "InboxCard delivery to Desktop failed"),
                    }
                },
```

- [ ] **Step 6: Build the workspace**

```bash
cargo build --workspace
cargo test --workspace --lib
```

Expected: clean. Existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-notify/ crates/surge-daemon/src/main.rs
git commit -m "feat(notify): InboxCardPayload run_id → callback_token + formatter updates"
```

---

## Phase 5 — Bot loop transport

### Task 5.1: `inbox` module skeleton in `surge-daemon`

**Files:**
- Create: `crates/surge-daemon/src/inbox/mod.rs`
- Modify: `crates/surge-daemon/src/lib.rs`
- Modify: `crates/surge-daemon/Cargo.toml`

- [ ] **Step 1: Add deps**

Open `crates/surge-daemon/Cargo.toml`. Under `[dependencies]`, ensure these are present (alphabetical):

```toml
async-trait = { workspace = true }
chrono = { workspace = true }
surge-orchestrator = { workspace = true }
teloxide = { version = "0.13", default-features = false, features = ["macros", "ctrlc_handler"] }
ulid = { workspace = true }
```

(`surge-intake`, `surge-notify`, `surge-persistence` are already there.)

- [ ] **Step 2: Create the module skeleton**

Create `crates/surge-daemon/src/inbox/mod.rs`:

```rust
//! Inbox-action subsystem.
//!
//! Receivers (Telegram bot loop, Desktop action listener) write requests to
//! `inbox_action_queue`; `InboxActionConsumer` polls and dispatches.
//! Outgoing inbox cards are rendered by `TgInboxBot::outgoing_loop` from
//! `inbox_delivery_queue`. `TicketStateSync` follows engine events for
//! inbox-initiated runs. `SnoozeScheduler` re-emits cards when their
//! snooze_until expires.

pub mod consumer;
pub mod desktop_listener;
pub mod snooze_scheduler;
pub mod state_sync;
pub mod tg_bot;

/// Channel through which an inbox decision was received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionChannel {
    Telegram,
    Desktop,
}

impl ActionChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Desktop => "desktop",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "telegram" => Some(Self::Telegram),
            "desktop" => Some(Self::Desktop),
            _ => None,
        }
    }
}
```

- [ ] **Step 3: Re-export from `lib.rs`**

Open `crates/surge-daemon/src/lib.rs`. Add (alphabetical with other `pub mod`):

```rust
pub mod inbox;
```

- [ ] **Step 4: Stub each submodule so compilation works**

Each of `consumer.rs`, `desktop_listener.rs`, `snooze_scheduler.rs`, `state_sync.rs`, `tg_bot.rs` is filled out in subsequent tasks. To unblock this task, create empty stubs:

`crates/surge-daemon/src/inbox/consumer.rs`:
```rust
//! Inbox action consumer (Phase 6).
```

`crates/surge-daemon/src/inbox/desktop_listener.rs`:
```rust
//! Desktop action listener (Phase 5.5).
```

`crates/surge-daemon/src/inbox/snooze_scheduler.rs`:
```rust
//! Snooze re-emission scheduler (Phase 8).
```

`crates/surge-daemon/src/inbox/state_sync.rs`:
```rust
//! Ticket-state sync (Phase 7).
```

`crates/surge-daemon/src/inbox/tg_bot.rs`:
```rust
//! Telegram bot loop (Phase 5.2-5.4).
```

- [ ] **Step 5: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-daemon/Cargo.toml crates/surge-daemon/src/lib.rs crates/surge-daemon/src/inbox/
git commit -m "feat(daemon): inbox module skeleton + ActionChannel enum + teloxide dep"
```

---

### Task 5.2: `TgInboxBot::on_callback` — write request to queue

**Files:**
- Modify: `crates/surge-daemon/src/inbox/tg_bot.rs`

- [ ] **Step 1: Replace stub with the bot struct + callback handler**

Replace `crates/surge-daemon/src/inbox/tg_bot.rs` contents with:

```rust
//! Telegram bot loop for inbox cards: outgoing delivery + incoming callbacks.

use crate::inbox::ActionChannel;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use surge_persistence::inbox_queue::{self, InboxActionKind};
use surge_persistence::runs::Storage;
use teloxide::dispatching::dialogue::GetChatId;
use teloxide::prelude::*;
use teloxide::types::CallbackQuery;
use teloxide::utils::command::BotCommands;
use teloxide::Bot;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Telegram inbox bot — runs the long-poll dispatcher + outgoing-delivery loop.
pub struct TgInboxBot {
    bot: Bot,
    chat_id: ChatId,
    storage: Arc<Storage>,
}

impl TgInboxBot {
    pub fn new(bot: Bot, chat_id: ChatId, storage: Arc<Storage>) -> Self {
        Self {
            bot,
            chat_id,
            storage,
        }
    }

    /// Drive both legs (outgoing + incoming) until cancellation.
    pub async fn run(self, shutdown: CancellationToken) {
        let outgoing = {
            let bot = self.bot.clone();
            let storage = Arc::clone(&self.storage);
            let shutdown = shutdown.clone();
            tokio::spawn(outgoing_loop(bot, self.chat_id, storage, shutdown))
        };
        let incoming = {
            let bot = self.bot.clone();
            let storage = Arc::clone(&self.storage);
            tokio::spawn(incoming_loop(bot, storage))
        };
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("TgInboxBot: shutdown signalled");
            }
            _ = outgoing => {}
            _ = incoming => {}
        }
    }
}

async fn outgoing_loop(
    _bot: Bot,
    _chat_id: ChatId,
    _storage: Arc<Storage>,
    _shutdown: CancellationToken,
) {
    // Implemented in Task 5.3.
}

async fn incoming_loop(bot: Bot, storage: Arc<Storage>) {
    let handler = Update::filter_callback_query().endpoint(on_callback);
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![storage])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn on_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<Storage>,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    match parse_callback_data(data) {
        Some((action, token)) => {
            match handle_action(&storage, action, token, ActionChannel::Telegram).await {
                Ok(()) => {
                    let _ = bot.answer_callback_query(q.id.clone()).text("Recorded").await;
                }
                Err(CallbackHandleError::TokenNotFound) => {
                    let _ = bot.answer_callback_query(q.id.clone()).text("Card expired").await;
                }
                Err(CallbackHandleError::Persistence(e)) => {
                    warn!(error = %e, "inbox callback persistence error");
                    let _ = bot
                        .answer_callback_query(q.id.clone())
                        .text("Internal error — see daemon logs")
                        .await;
                }
            }
        }
        None => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("Invalid action")
                .await;
        }
    }
    Ok(())
}

/// Parse `inbox:<action>:<token>` callback strings.
pub(crate) fn parse_callback_data(s: &str) -> Option<(InboxActionKind, &str)> {
    let mut parts = s.splitn(3, ':');
    if parts.next()? != "inbox" {
        return None;
    }
    let action = InboxActionKind::parse(parts.next()?)?;
    let token = parts.next()?;
    if token.is_empty() {
        return None;
    }
    Some((action, token))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CallbackHandleError {
    #[error("callback token not found")]
    TokenNotFound,
    #[error("persistence error: {0}")]
    Persistence(String),
}

/// Verify the token resolves to a ticket, then enqueue the action.
///
/// Shared by Telegram and Desktop receivers.
pub(crate) async fn handle_action(
    storage: &Storage,
    action: InboxActionKind,
    token: &str,
    via: ActionChannel,
) -> Result<(), CallbackHandleError> {
    // Resolve token → task_id.
    let task_id = {
        let conn = storage
            .acquire_registry_conn()
            .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?;
        let repo = surge_persistence::intake::IntakeRepo::new(&conn);
        repo.fetch_by_callback_token(token)
            .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?
            .map(|row| row.task_id)
    };
    let task_id = match task_id {
        Some(id) => id,
        None => return Err(CallbackHandleError::TokenNotFound),
    };

    // Enqueue.
    let snooze_until = match action {
        InboxActionKind::Snooze => Some(Utc::now() + ChronoDuration::hours(24)),
        _ => None,
    };
    let conn = storage
        .acquire_registry_conn()
        .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?;
    inbox_queue::append_action(
        &conn,
        action,
        &task_id,
        token,
        via.as_str(),
        snooze_until,
    )
    .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?;
    info!(task_id = %task_id, action = action.as_str(), via = via.as_str(), "inbox action enqueued");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_data_valid_start() {
        let (kind, token) = parse_callback_data("inbox:start:01HKGZ").unwrap();
        assert_eq!(kind, InboxActionKind::Start);
        assert_eq!(token, "01HKGZ");
    }

    #[test]
    fn parse_callback_data_valid_snooze_skip() {
        assert_eq!(parse_callback_data("inbox:snooze:t").unwrap().0, InboxActionKind::Snooze);
        assert_eq!(parse_callback_data("inbox:skip:t").unwrap().0, InboxActionKind::Skip);
    }

    #[test]
    fn parse_callback_data_invalid_prefix() {
        assert!(parse_callback_data("approval:start:t").is_none());
    }

    #[test]
    fn parse_callback_data_invalid_action() {
        assert!(parse_callback_data("inbox:meow:t").is_none());
    }

    #[test]
    fn parse_callback_data_empty_token() {
        assert!(parse_callback_data("inbox:start:").is_none());
    }
}
```

- [ ] **Step 2: Run unit tests**

```bash
cargo test -p surge-daemon --lib inbox::tg_bot::tests
```

Expected: all five PASS.

- [ ] **Step 3: Build the workspace**

```bash
cargo build --workspace
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/inbox/tg_bot.rs
git commit -m "feat(daemon): TgInboxBot callback parser + handle_action enqueue"
```

---

### Task 5.3: `TgInboxBot::outgoing_loop` — render + send pending inbox cards

**Files:**
- Modify: `crates/surge-daemon/src/inbox/tg_bot.rs`

- [ ] **Step 1: Implement `outgoing_loop`**

Replace the placeholder `outgoing_loop` body in `crates/surge-daemon/src/inbox/tg_bot.rs`:

```rust
async fn outgoing_loop(
    bot: Bot,
    chat_id: ChatId,
    storage: Arc<Storage>,
    shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = interval.tick() => {}
        }
        if let Err(e) = tick_outgoing(&bot, chat_id, &storage).await {
            warn!(error = %e, "TgInboxBot outgoing tick failed");
        }
    }
}

async fn tick_outgoing(
    bot: &Bot,
    chat_id: ChatId,
    storage: &Storage,
) -> Result<(), String> {
    use surge_notify::messages::InboxCardPayload;
    use surge_notify::telegram::format_inbox_card;
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let pending = {
        let conn = storage
            .acquire_registry_conn()
            .map_err(|e| e.to_string())?;
        inbox_queue::list_pending_telegram_deliveries(&conn).map_err(|e| e.to_string())?
    };
    for row in pending {
        let payload: InboxCardPayload = match serde_json::from_str(&row.payload_json) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, seq = row.seq, "failed to parse delivery payload; skipping");
                continue;
            }
        };
        let rendered = format_inbox_card(&payload);
        let kb_rows: Vec<Vec<InlineKeyboardButton>> = rendered
            .keyboard
            .iter()
            .map(|row| {
                row.iter()
                    .map(|btn| {
                        if btn.is_url {
                            InlineKeyboardButton::url(
                                btn.label.clone(),
                                btn.data.parse().unwrap_or_else(|_| {
                                    // Fall back to a placeholder; teloxide URL parse is strict.
                                    "https://example.invalid/".parse().unwrap()
                                }),
                            )
                        } else {
                            InlineKeyboardButton::callback(btn.label.clone(), btn.data.clone())
                        }
                    })
                    .collect()
            })
            .collect();
        let kb = InlineKeyboardMarkup::new(kb_rows);
        match bot
            .send_message(chat_id, rendered.body)
            .reply_markup(kb)
            .await
        {
            Ok(msg) => {
                let conn = storage
                    .acquire_registry_conn()
                    .map_err(|e| e.to_string())?;
                inbox_queue::record_telegram_delivered(
                    &conn,
                    row.seq,
                    chat_id.0,
                    msg.id.0,
                )
                .map_err(|e| e.to_string())?;
                let repo = surge_persistence::intake::IntakeRepo::new(&conn);
                let _ = repo.set_tg_message_ref(&row.task_id, chat_id.0, msg.id.0);
                info!(
                    task_id = %row.task_id,
                    seq = row.seq,
                    "InboxCard delivered to Telegram"
                );
            }
            Err(e) => {
                warn!(error = %e, task_id = %row.task_id, "Telegram send failed; will retry");
                // Don't mark as delivered — next tick retries.
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean. (No new tests yet — outgoing-loop is integration-level.)

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/src/inbox/tg_bot.rs
git commit -m "feat(daemon): TgInboxBot outgoing_loop — render + send pending inbox cards"
```

---

### Task 5.4: `enqueue_inbox_card` helper

**Files:**
- Create: helpers in `crates/surge-daemon/src/inbox/mod.rs`

- [ ] **Step 1: Add the helper**

Append to `crates/surge-daemon/src/inbox/mod.rs`:

```rust
use std::sync::Arc;
use surge_notify::messages::InboxCardPayload;
use surge_persistence::inbox_queue;
use surge_persistence::intake::{IntakeRepo, IntakeRow};
use surge_persistence::runs::Storage;

/// Enqueue an inbox card for delivery and persist the callback_token on
/// the existing `ticket_index` row. Idempotent at the row level: if the
/// row doesn't exist yet, it's inserted with state=`InboxNotified`.
pub async fn enqueue_inbox_card(
    storage: &Arc<Storage>,
    payload: &InboxCardPayload,
) -> Result<(), String> {
    let conn = storage.acquire_registry_conn().map_err(|e| e.to_string())?;
    let repo = IntakeRepo::new(&conn);

    let existing = repo.fetch(payload.task_id.as_str()).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    if existing.is_none() {
        let row = IntakeRow {
            task_id: payload.task_id.as_str().into(),
            source_id: payload.source_id.clone(),
            provider: payload.provider.clone(),
            run_id: None,
            triage_decision: None,
            duplicate_of: None,
            priority: Some(payload.priority.label().into()),
            state: surge_persistence::intake::TicketState::InboxNotified,
            first_seen: now,
            last_seen: now,
            snooze_until: None,
            callback_token: Some(payload.callback_token.clone()),
            tg_chat_id: None,
            tg_message_id: None,
        };
        repo.insert(&row).map_err(|e| e.to_string())?;
    } else {
        repo.set_callback_token(payload.task_id.as_str(), &payload.callback_token)
            .map_err(|e| e.to_string())?;
    }

    let payload_json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    inbox_queue::append_delivery(
        &conn,
        payload.task_id.as_str(),
        &payload.callback_token,
        &payload_json,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 2: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/src/inbox/mod.rs
git commit -m "feat(daemon): inbox::enqueue_inbox_card helper"
```

---

### Task 5.5: `DesktopActionListener` — forward notify-rust actions

**Files:**
- Modify: `crates/surge-daemon/src/inbox/desktop_listener.rs`

- [ ] **Step 1: Replace stub**

Replace `crates/surge-daemon/src/inbox/desktop_listener.rs`:

```rust
//! Desktop action listener.
//!
//! Spawns `notify-rust::Notification::show()` per pending desktop card and
//! waits on `wait_for_action` in a blocking task; the chosen action is
//! forwarded into `inbox_action_queue` via `tg_bot::handle_action`.

use crate::inbox::{tg_bot, ActionChannel};
use std::sync::Arc;
use std::time::Duration;
use surge_persistence::inbox_queue;
use surge_persistence::runs::Storage;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub struct DesktopActionListener {
    storage: Arc<Storage>,
    poll_interval: Duration,
}

impl DesktopActionListener {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            poll_interval: Duration::from_millis(500),
        }
    }

    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "desktop listener tick failed");
            }
        }
    }

    async fn tick(&self) -> Result<(), String> {
        let pending = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            inbox_queue::list_pending_desktop_deliveries(&conn).map_err(|e| e.to_string())?
        };
        for row in pending {
            // Mark as delivered immediately to avoid duplicates if notify-rust
            // takes a long time. wait_for_action runs in a blocking thread.
            {
                let conn = self
                    .storage
                    .acquire_registry_conn()
                    .map_err(|e| e.to_string())?;
                inbox_queue::record_desktop_delivered(&conn, row.seq)
                    .map_err(|e| e.to_string())?;
            }

            let payload: surge_notify::messages::InboxCardPayload =
                match serde_json::from_str(&row.payload_json) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(error = %e, seq = row.seq, "desktop payload parse failed; skipping");
                        continue;
                    }
                };
            let rendered = surge_notify::desktop::format_inbox_card_desktop(&payload);
            let token = payload.callback_token.clone();
            let storage = Arc::clone(&self.storage);
            tokio::task::spawn_blocking(move || {
                let mut n = notify_rust::Notification::new();
                n.summary(&rendered.title)
                    .body(&rendered.body);
                for (action_id, label) in &rendered.actions {
                    n.action(action_id, label);
                }
                let handle = match n.show() {
                    Ok(h) => h,
                    Err(e) => {
                        warn!(error = %e, "notify-rust show failed");
                        return;
                    }
                };
                handle.wait_for_action(|action_id| {
                    let action_kind =
                        match parse_desktop_action_id(action_id) {
                            Some(k) => k,
                            None => {
                                debug!(action_id, "ignored desktop action (dismiss/expired)");
                                return;
                            }
                        };
                    // Bridge into async via a dedicated handle: we're in a
                    // blocking thread, so use tokio::runtime::Handle::current.
                    let storage = Arc::clone(&storage);
                    let token = token.clone();
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        rt.spawn(async move {
                            if let Err(e) = tg_bot::handle_action(
                                &storage,
                                action_kind,
                                &token,
                                ActionChannel::Desktop,
                            )
                            .await
                            {
                                warn!(error = ?e, "desktop action enqueue failed");
                            }
                        });
                    } else {
                        warn!("no tokio runtime available for desktop action; lost");
                    }
                });
                info!(token = %payload.callback_token, "desktop card dismissed/answered");
            });
        }
        Ok(())
    }
}

fn parse_desktop_action_id(s: &str) -> Option<surge_persistence::inbox_queue::InboxActionKind> {
    // Action IDs from desktop formatter are "inbox:start:<token>" etc.
    let mut parts = s.splitn(3, ':');
    if parts.next()? != "inbox" {
        return None;
    }
    surge_persistence::inbox_queue::InboxActionKind::parse(parts.next()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_action_id_parses_three_kinds() {
        assert_eq!(
            parse_desktop_action_id("inbox:start:tok").unwrap(),
            surge_persistence::inbox_queue::InboxActionKind::Start
        );
        assert_eq!(
            parse_desktop_action_id("inbox:snooze:tok").unwrap(),
            surge_persistence::inbox_queue::InboxActionKind::Snooze
        );
        assert_eq!(
            parse_desktop_action_id("inbox:skip:tok").unwrap(),
            surge_persistence::inbox_queue::InboxActionKind::Skip
        );
    }

    #[test]
    fn desktop_action_id_rejects_dismiss_and_garbage() {
        assert!(parse_desktop_action_id("__closed").is_none());
        assert!(parse_desktop_action_id("inbox:meow:tok").is_none());
        assert!(parse_desktop_action_id("approval:start:tok").is_none());
    }
}
```

Make `tg_bot::handle_action` and `tg_bot::CallbackHandleError` `pub(crate)` so the desktop listener can use them. They're already declared `pub(crate)` per Task 5.2.

- [ ] **Step 2: Run tests**

```bash
cargo test -p surge-daemon --lib inbox::desktop_listener::tests
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/src/inbox/desktop_listener.rs
git commit -m "feat(daemon): DesktopActionListener — wait_for_action → inbox_action_queue"
```

---

## Phase 6 — `InboxActionConsumer`

### Task 6.1: Consumer struct + `tick` skeleton

**Files:**
- Modify: `crates/surge-daemon/src/inbox/consumer.rs`

- [ ] **Step 1: Replace stub with the consumer struct**

Replace `crates/surge-daemon/src/inbox/consumer.rs`:

```rust
//! `InboxActionConsumer` — polls `inbox_action_queue` and dispatches.

use crate::inbox::state_sync::TicketStateSync;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::TaskSource;
use surge_intake::types::{Priority, TaskId};
use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, BootstrapPrompt};
use surge_orchestrator::engine::config::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::inbox_queue::{self, InboxActionKind, InboxActionRow};
use surge_persistence::intake::{IntakeError, IntakeRepo, TicketState};
use surge_persistence::runs::Storage;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct InboxActionConsumer {
    pub storage: Arc<Storage>,
    pub bootstrap: Arc<dyn BootstrapGraphBuilder>,
    pub engine: Arc<dyn EngineFacade>,
    pub sources: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    pub worktrees_root: PathBuf,
    pub poll_interval: Duration,
}

impl InboxActionConsumer {
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "InboxActionConsumer tick failed");
            }
        }
    }

    async fn tick(&self) -> Result<(), String> {
        let pending = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            inbox_queue::list_pending_actions(&conn).map_err(|e| e.to_string())?
        };
        for row in pending {
            let result = match row.kind {
                InboxActionKind::Start => self.handle_start(&row).await,
                InboxActionKind::Snooze => self.handle_snooze(&row).await,
                InboxActionKind::Skip => self.handle_skip(&row).await,
            };
            if let Err(e) = result {
                warn!(
                    error = %e,
                    seq = row.seq,
                    kind = row.kind.as_str(),
                    task_id = %row.task_id,
                    "inbox action handler error"
                );
            }
            // Advance cursor regardless: failed actions are surfaced via logs +
            // SurgeEvent persistence; cursor never blocks on transient errors.
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            inbox_queue::mark_action_processed(&conn, row.seq).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // Phase 6.2-6.4 fill these in.
    async fn handle_start(&self, _row: &InboxActionRow) -> Result<(), String> {
        Err("not implemented".into())
    }
    async fn handle_snooze(&self, _row: &InboxActionRow) -> Result<(), String> {
        Err("not implemented".into())
    }
    async fn handle_skip(&self, _row: &InboxActionRow) -> Result<(), String> {
        Err("not implemented".into())
    }
}
```

- [ ] **Step 2: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean (state_sync stub doesn't expose `TicketStateSync` yet — temporarily comment the `use crate::inbox::state_sync::TicketStateSync;` line and re-enable in Task 7.1).

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/src/inbox/consumer.rs
git commit -m "feat(daemon): InboxActionConsumer skeleton with poll loop"
```

---

### Task 6.2: `handle_start` — provision worktree, build graph, start_run

**Files:**
- Modify: `crates/surge-daemon/src/inbox/consumer.rs`

- [ ] **Step 1: Implement the handler**

In `crates/surge-daemon/src/inbox/consumer.rs`, replace `handle_start`:

```rust
    async fn handle_start(&self, row: &InboxActionRow) -> Result<(), String> {
        // Resolve ticket row.
        let ticket_row = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_by_callback_token(&row.callback_token)
                .map_err(|e| e.to_string())?
        };
        let ticket_row = match ticket_row {
            Some(r) => r,
            None => {
                info!(token = %row.callback_token, "Start: callback token not found; ignoring");
                return Ok(());
            }
        };
        // Idempotency: state must still be awaiting decision.
        if !matches!(
            ticket_row.state,
            TicketState::InboxNotified | TicketState::Snoozed
        ) {
            info!(
                state = ?ticket_row.state,
                task_id = %ticket_row.task_id,
                "Start: ticket no longer awaiting decision; ignoring"
            );
            return Ok(());
        }

        // Resolve TaskSource.
        let source = self
            .sources
            .get(&ticket_row.source_id)
            .ok_or_else(|| format!("source {} not registered", ticket_row.source_id))?;
        let task_id = TaskId::try_new(ticket_row.task_id.clone())
            .map_err(|e| format!("task_id: {e}"))?;
        let details = source
            .fetch_task(&task_id)
            .await
            .map_err(|e| format!("fetch_task: {e}"))?;

        // Provision worktree.
        let run_id = surge_core::id::RunId::new();
        let worktree = self.worktrees_root.join(run_id.to_string());
        std::fs::create_dir_all(&worktree).map_err(|e| format!("worktree mkdir: {e}"))?;

        // Build graph.
        let prompt = BootstrapPrompt {
            title: details.title.clone(),
            description: details.description.clone(),
            tracker_url: Some(details.url.clone()),
            priority: ticket_row.priority.as_deref().and_then(parse_priority_str),
            labels: details.labels.clone(),
        };
        let graph = self
            .bootstrap
            .build(run_id, prompt, worktree.clone())
            .await
            .map_err(|e| format!("bootstrap.build: {e}"))?;

        // Start the run.
        let handle = self
            .engine
            .start_run(run_id, graph, worktree, EngineRunConfig::default())
            .await
            .map_err(|e| format!("engine.start_run: {e}"))?;

        // Update ticket_index: state=RunStarted, run_id set, callback_token cleared.
        {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            let repo = IntakeRepo::new(&conn);
            repo.set_run_id(&ticket_row.task_id, run_id.to_string())
                .map_err(|e| e.to_string())?;
            match repo.update_state_validated(&ticket_row.task_id, TicketState::RunStarted) {
                Ok(()) => {}
                Err(IntakeError::InvalidTransition { from, to }) => {
                    warn!(
                        ?from,
                        ?to,
                        task_id = %ticket_row.task_id,
                        "Start: state transition rejected; assuming concurrent action"
                    );
                    return Ok(());
                }
                Err(e) => return Err(e.to_string()),
            }
            repo.clear_callback_token(&ticket_row.task_id)
                .map_err(|e| e.to_string())?;
        }

        // Post tracker comment.
        let comment = format!(
            "Surge run #{} started — see {} for progress.",
            run_id.short(),
            row.decided_via,
        );
        if let Err(e) = source.post_comment(&task_id, &comment).await {
            warn!(error = %e, task_id = %task_id, "tracker comment on Start failed");
        }

        // Spawn TicketStateSync to follow the run.
        let sync = TicketStateSync::new(
            task_id.clone(),
            Arc::clone(&self.storage),
            Arc::clone(source),
        );
        tokio::spawn(sync.run(handle));

        info!(task_id = %task_id, run_id = %run_id, "inbox Start dispatched");
        Ok(())
    }

fn parse_priority_str(s: &str) -> Option<Priority> {
    match s {
        "urgent" => Some(Priority::Urgent),
        "high" => Some(Priority::High),
        "medium" => Some(Priority::Medium),
        "low" => Some(Priority::Low),
        _ => None,
    }
}
```

(Place `parse_priority_str` outside the `impl` block, at the file's top level.)

- [ ] **Step 2: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean (after Task 7.1 re-enables `TicketStateSync`; if running this task alone, temporarily comment the `tokio::spawn(sync.run(handle))` line and the `use` until then).

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/src/inbox/consumer.rs
git commit -m "feat(daemon): InboxActionConsumer.handle_start — bootstrap + start_run + comment"
```

---

### Task 6.3: `handle_snooze` and `handle_skip`

**Files:**
- Modify: `crates/surge-daemon/src/inbox/consumer.rs`

- [ ] **Step 1: Implement `handle_snooze`**

Replace the stub:

```rust
    async fn handle_snooze(&self, row: &InboxActionRow) -> Result<(), String> {
        let ticket_row = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_by_callback_token(&row.callback_token)
                .map_err(|e| e.to_string())?
        };
        let ticket_row = match ticket_row {
            Some(r) => r,
            None => return Ok(()),
        };
        if !matches!(ticket_row.state, TicketState::InboxNotified) {
            return Ok(());
        }
        let until = match row.snooze_until {
            Some(u) => u,
            None => return Err("snooze action without snooze_until".into()),
        };
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| e.to_string())?;
        let repo = IntakeRepo::new(&conn);
        match repo.update_state_validated(&ticket_row.task_id, TicketState::Snoozed) {
            Ok(()) => {}
            Err(IntakeError::InvalidTransition { .. }) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        repo.set_snooze_until(&ticket_row.task_id, until)
            .map_err(|e| e.to_string())?;
        info!(task_id = %ticket_row.task_id, ?until, "inbox Snooze applied");
        Ok(())
    }
```

- [ ] **Step 2: Implement `handle_skip`**

Replace the stub:

```rust
    async fn handle_skip(&self, row: &InboxActionRow) -> Result<(), String> {
        let ticket_row = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_by_callback_token(&row.callback_token)
                .map_err(|e| e.to_string())?
        };
        let ticket_row = match ticket_row {
            Some(r) => r,
            None => return Ok(()),
        };
        if !matches!(
            ticket_row.state,
            TicketState::InboxNotified | TicketState::Snoozed
        ) {
            return Ok(());
        }
        let source = self
            .sources
            .get(&ticket_row.source_id)
            .ok_or_else(|| format!("source {} not registered", ticket_row.source_id))?;

        {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            let repo = IntakeRepo::new(&conn);
            match repo.update_state_validated(&ticket_row.task_id, TicketState::Skipped) {
                Ok(()) => {}
                Err(IntakeError::InvalidTransition { .. }) => return Ok(()),
                Err(e) => return Err(e.to_string()),
            }
            // Clear token + tg refs (no longer actionable).
            repo.clear_callback_token(&ticket_row.task_id)
                .map_err(|e| e.to_string())?;
        }

        let task_id = TaskId::try_new(ticket_row.task_id.clone())
            .map_err(|e| format!("task_id: {e}"))?;
        if let Err(e) = source.set_label(&task_id, "surge:skipped", true).await {
            warn!(error = %e, task_id = %task_id, "set_label surge:skipped failed");
        }
        if let Err(e) = source
            .post_comment(&task_id, "Surge: ticket skipped by user.")
            .await
        {
            warn!(error = %e, task_id = %task_id, "tracker comment on Skip failed");
        }
        info!(task_id = %task_id, "inbox Skip applied");
        Ok(())
    }
```

- [ ] **Step 3: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/inbox/consumer.rs
git commit -m "feat(daemon): InboxActionConsumer.handle_snooze + handle_skip"
```

---

## Phase 7 — `TicketStateSync`

### Task 7.1: `TicketStateSync` follows `RunHandle.events`

**Files:**
- Modify: `crates/surge-daemon/src/inbox/state_sync.rs`

- [ ] **Step 1: Replace stub**

Replace `crates/surge-daemon/src/inbox/state_sync.rs`:

```rust
//! `TicketStateSync` — drives ticket_index FSM from engine RunHandle events
//! and posts tracker comments.

use std::sync::Arc;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::intake::{IntakeRepo, TicketState};
use surge_persistence::runs::Storage;
use tracing::{info, warn};

pub struct TicketStateSync {
    task_id: TaskId,
    storage: Arc<Storage>,
    source: Arc<dyn TaskSource>,
}

impl TicketStateSync {
    pub fn new(task_id: TaskId, storage: Arc<Storage>, source: Arc<dyn TaskSource>) -> Self {
        Self {
            task_id,
            storage,
            source,
        }
    }

    pub async fn run(self, mut handle: RunHandle) {
        info!(task_id = %self.task_id, run_id = %handle.run_id, "TicketStateSync started");
        // Drive InboxNotified/RunStarted -> Active on the first persisted event.
        let mut went_active = false;
        loop {
            match handle.events.recv().await {
                Ok(EngineRunEvent::Persisted { .. }) => {
                    if !went_active {
                        if let Err(e) = self.set_state(TicketState::Active).await {
                            warn!(error = %e, "transition to Active failed");
                        }
                        went_active = true;
                    }
                }
                Ok(EngineRunEvent::Terminal(outcome)) => {
                    self.on_terminal(&outcome).await;
                    return;
                }
                Err(_) => {
                    // Sender dropped — either run completed without a Terminal event
                    // (shouldn't happen) or the engine shut down. We exit silently.
                    return;
                }
            }
        }
    }

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
        let (state, comment): (TicketState, String) = match outcome {
            RunOutcome::Completed { .. } => (
                TicketState::Completed,
                "✅ Surge run complete.".into(),
            ),
            RunOutcome::Failed { error } => (
                TicketState::Failed,
                format!("❌ Surge run failed: {error}"),
            ),
            RunOutcome::Aborted { reason } => (
                TicketState::Aborted,
                format!("Surge run aborted: {reason}"),
            ),
        };
        if let Err(e) = self.set_state(state).await {
            warn!(error = %e, ?state, "transition to terminal state failed");
        }
        if let Err(e) = self.source.post_comment(&self.task_id, &comment).await {
            warn!(error = %e, task_id = %self.task_id, "tracker comment on terminal failed");
        }
        info!(task_id = %self.task_id, ?state, "TicketStateSync done");
    }
}
```

- [ ] **Step 2: Re-enable the import in `consumer.rs`**

If you commented out `use crate::inbox::state_sync::TicketStateSync;` in Task 6.1/6.2, restore it now. Also restore the `tokio::spawn(sync.run(handle));` call in `handle_start`.

- [ ] **Step 3: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/inbox/state_sync.rs crates/surge-daemon/src/inbox/consumer.rs
git commit -m "feat(daemon): TicketStateSync — RunHandle events drive ticket_index FSM"
```

---

## Phase 8 — `SnoozeScheduler`

### Task 8.1: Re-emit due snoozed cards

**Files:**
- Modify: `crates/surge-daemon/src/inbox/snooze_scheduler.rs`

- [ ] **Step 1: Replace stub**

Replace `crates/surge-daemon/src/inbox/snooze_scheduler.rs`:

```rust
//! `SnoozeScheduler` — periodically re-emits snoozed inbox cards once their
//! `snooze_until` has elapsed.

use crate::inbox::enqueue_inbox_card;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::types::{Priority, TaskId};
use surge_persistence::intake::{IntakeRepo, TicketState};
use surge_persistence::runs::Storage;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct SnoozeScheduler {
    pub storage: Arc<Storage>,
    pub poll_interval: Duration,
}

impl SnoozeScheduler {
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "snooze scheduler tick failed");
            }
        }
    }

    async fn tick(&self) -> Result<(), String> {
        let now = Utc::now();
        let due = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_due_snoozed(now)
                .map_err(|e| e.to_string())?
        };
        for row in due {
            // Validate state in case we raced (defensive).
            if row.state != TicketState::Snoozed {
                continue;
            }
            let new_token = ulid::Ulid::new().to_string();
            {
                let conn = self
                    .storage
                    .acquire_registry_conn()
                    .map_err(|e| e.to_string())?;
                let repo = IntakeRepo::new(&conn);
                if let Err(e) = repo.update_state_validated(&row.task_id, TicketState::InboxNotified) {
                    warn!(error = %e, task_id = %row.task_id, "snooze re-emit transition failed");
                    continue;
                }
                repo.set_callback_token(&row.task_id, &new_token)
                    .map_err(|e| e.to_string())?;
                repo.clear_snooze_until(&row.task_id)
                    .map_err(|e| e.to_string())?;
            }

            // Build a fresh InboxCardPayload from the row data.
            let task_id = match TaskId::try_new(row.task_id.clone()) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let priority = row
                .priority
                .as_deref()
                .and_then(crate::inbox::consumer_helpers::parse_priority_str)
                .unwrap_or(Priority::Medium);
            let payload = surge_notify::messages::InboxCardPayload {
                task_id,
                source_id: row.source_id.clone(),
                provider: row.provider.clone(),
                title: format!("(snoozed re-emission) {}", row.task_id),
                summary: String::new(),
                priority,
                task_url: String::new(),
                callback_token: new_token,
            };
            if let Err(e) = enqueue_inbox_card(&self.storage, &payload).await {
                warn!(error = %e, task_id = %row.task_id, "snooze re-emit enqueue failed");
            } else {
                info!(task_id = %row.task_id, "snoozed card re-emitted");
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add the helper module referenced above**

The snooze scheduler builds a payload but `parse_priority_str` was added inside `consumer.rs`. Promote it to a small helper module so both can use it. Add to `crates/surge-daemon/src/inbox/mod.rs`:

```rust
pub(crate) mod consumer_helpers {
    use surge_intake::types::Priority;

    pub fn parse_priority_str(s: &str) -> Option<Priority> {
        match s {
            "urgent" => Some(Priority::Urgent),
            "high" => Some(Priority::High),
            "medium" => Some(Priority::Medium),
            "low" => Some(Priority::Low),
            _ => None,
        }
    }
}
```

In `crates/surge-daemon/src/inbox/consumer.rs`, replace the inline `fn parse_priority_str` with `use crate::inbox::consumer_helpers::parse_priority_str;`.

- [ ] **Step 3: Build**

```bash
cargo build -p surge-daemon
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/inbox/
git commit -m "feat(daemon): SnoozeScheduler re-emits due snoozed cards with fresh tokens"
```

---

## Phase 9 — Daemon `main.rs` wiring

### Task 9.1: Spawn the inbox subsystems on startup

**Files:**
- Modify: `crates/surge-daemon/src/main.rs`

- [ ] **Step 1: Build the source registry as Arc<HashMap>**

Look at the existing `source_map` building loop (around line 145-208). After the loop completes, wrap it:

```rust
        let source_registry: Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>> =
            Arc::new(source_map);
```

Pass `source_registry` (instead of `source_map`) into `spawn_task_router` (the consumer side already uses `Arc::clone(&source_map_for_consumer)` — adapt its construction).

- [ ] **Step 2: Spawn inbox subsystems after `spawn_task_router`**

After the existing `spawn_task_router(...)` call (around line 211), add:

```rust
        // Spawn inbox subsystems (consumer + snooze scheduler + receivers).
        spawn_inbox_subsystems(
            Arc::clone(&storage),
            Arc::clone(&source_registry),
            Arc::clone(&facade),
            &config,
            shutdown.clone(),
        ).await;
```

- [ ] **Step 3: Implement `spawn_inbox_subsystems`**

Append at the bottom of `crates/surge-daemon/src/main.rs`:

```rust
async fn spawn_inbox_subsystems(
    storage: Arc<surge_persistence::runs::Storage>,
    sources: Arc<std::collections::HashMap<String, Arc<dyn TaskSource>>>,
    engine: Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
    config: &surge_core::config::SurgeConfig,
    shutdown: CancellationToken,
) {
    use surge_daemon::inbox::{
        consumer::InboxActionConsumer, desktop_listener::DesktopActionListener,
        snooze_scheduler::SnoozeScheduler, tg_bot::TgInboxBot,
    };
    use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, MinimalBootstrapGraphBuilder};

    let bootstrap: Arc<dyn BootstrapGraphBuilder> =
        Arc::new(MinimalBootstrapGraphBuilder::new());
    let worktrees_root = surge_runs_dir().join("worktrees");
    let _ = std::fs::create_dir_all(&worktrees_root);

    // Consumer.
    let consumer = InboxActionConsumer {
        storage: Arc::clone(&storage),
        bootstrap: Arc::clone(&bootstrap),
        engine: Arc::clone(&engine),
        sources: Arc::clone(&sources),
        worktrees_root,
        poll_interval: std::time::Duration::from_millis(500),
    };
    let shutdown_for_consumer = shutdown.clone();
    tokio::spawn(consumer.run(shutdown_for_consumer));

    // Snooze scheduler.
    let scheduler = SnoozeScheduler {
        storage: Arc::clone(&storage),
        poll_interval: config.inbox.snooze_poll_interval,
    };
    let shutdown_for_scheduler = shutdown.clone();
    tokio::spawn(scheduler.run(shutdown_for_scheduler));

    // Desktop listener.
    let desktop = DesktopActionListener::new(Arc::clone(&storage));
    let shutdown_for_desktop = shutdown.clone();
    tokio::spawn(desktop.run(shutdown_for_desktop));

    // Telegram bot (only if config provides a chat ID + token).
    if let Some(tg_cfg) = config.telegram.as_ref() {
        let chat_id = tg_cfg.chat_id.or_else(|| {
            tg_cfg
                .chat_id_env
                .as_deref()
                .and_then(|env| std::env::var(env).ok().and_then(|s| s.parse::<i64>().ok()))
        });
        let token = tg_cfg
            .bot_token_env
            .as_deref()
            .and_then(|env| std::env::var(env).ok());
        match (chat_id, token) {
            (Some(chat_id), Some(token)) => {
                let bot = teloxide::Bot::new(token);
                let tg = TgInboxBot::new(bot, teloxide::types::ChatId(chat_id), Arc::clone(&storage));
                let shutdown_for_tg = shutdown.clone();
                tokio::spawn(tg.run(shutdown_for_tg));
            }
            _ => {
                tracing::warn!(
                    "telegram config present but chat_id or bot_token missing — TgInboxBot not spawned"
                );
            }
        }
    } else {
        tracing::info!("no [telegram] config — TgInboxBot skipped");
    }
}
```

- [ ] **Step 4: Update `RouterOutput::Triage` arm to enqueue properly**

The Phase 4.1 update kept the existing render-and-deliver path. Now replace it with the queue-based delivery. In the consumer task spawned by `spawn_task_router`, find the `RouterOutput::Triage` arm (around line 311 from Phase 4.1's edit) and replace its body:

```rust
                surge_intake::router::RouterOutput::Triage { event } => {
                    let title = event
                        .raw_payload
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("New ticket")
                        .to_string();
                    let task_url = event
                        .raw_payload
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let provider = event
                        .task_id
                        .as_str()
                        .split(':')
                        .next()
                        .unwrap_or("unknown")
                        .to_string();
                    let callback_token = ulid::Ulid::new().to_string();
                    let payload = surge_notify::messages::InboxCardPayload {
                        task_id: event.task_id.clone(),
                        source_id: event.source_id.clone(),
                        provider,
                        title,
                        summary: String::new(),
                        priority: surge_intake::types::Priority::Medium,
                        task_url,
                        callback_token,
                    };
                    if let Err(e) = surge_daemon::inbox::enqueue_inbox_card(&storage_for_router, &payload).await {
                        tracing::warn!(error = %e, task_id = %event.task_id, "failed to enqueue inbox card");
                    } else {
                        tracing::info!(task_id = %event.task_id, "inbox card enqueued");
                    }
                },
```

The unused `notifier` parameter on `spawn_task_router` can stay (delivery now goes through the queue + bot/desktop listeners). The `let storage_for_router = Arc::clone(&storage);` capture is needed inside the spawned consumer.

- [ ] **Step 5: Build the workspace**

```bash
cargo build --workspace
```

Expected: clean.

- [ ] **Step 6: Run all tests**

```bash
cargo test --workspace --lib
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-daemon/src/main.rs
git commit -m "feat(daemon): wire inbox subsystems (consumer, scheduler, TG bot, desktop listener)"
```

---

## Phase 10 — Integration test

### Task 10.1: End-to-end mock pipeline

**Files:**
- Create: `crates/surge-daemon/tests/inbox_callback_e2e.rs`

- [ ] **Step 1: Write the test file**

Create `crates/surge-daemon/tests/inbox_callback_e2e.rs`:

```rust
//! End-to-end test for the inbox-action subsystem.
//!
//! Drives a `MockTaskSource` → `TaskRouter` → `InboxActionConsumer`
//! pipeline with a mocked engine, asserting `ticket_index` state
//! transitions and tracker comments for each of Start / Snooze / Skip
//! plus idempotency.

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_core::id::RunId;
use surge_daemon::inbox::consumer::InboxActionConsumer;
use surge_intake::TaskSource;
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{Priority, TaskId};
use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, MinimalBootstrapGraphBuilder};
use surge_orchestrator::engine::config::EngineRunConfig;
use surge_orchestrator::engine::error::EngineError;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome, RunSummary};
use surge_persistence::inbox_queue::{self, InboxActionKind};
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use surge_persistence::runs::Storage;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

// === Helpers ============================================================

async fn build_storage() -> (Arc<Storage>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let storage = Storage::open(tmp.path().join("home")).await.unwrap();
    (storage, tmp)
}

fn insert_ticket(
    storage: &Storage,
    task_id: &str,
    callback_token: &str,
) -> IntakeRow {
    let conn = storage.acquire_registry_conn().unwrap();
    let repo = IntakeRepo::new(&conn);
    let row = IntakeRow {
        task_id: task_id.into(),
        source_id: "mock:t".into(),
        provider: "mock".into(),
        run_id: None,
        triage_decision: None,
        duplicate_of: None,
        priority: Some("medium".into()),
        state: TicketState::InboxNotified,
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        snooze_until: None,
        callback_token: Some(callback_token.into()),
        tg_chat_id: None,
        tg_message_id: None,
    };
    repo.insert(&row).unwrap();
    row
}

fn fetch_state(storage: &Storage, task_id: &str) -> TicketState {
    let conn = storage.acquire_registry_conn().unwrap();
    IntakeRepo::new(&conn)
        .fetch(task_id)
        .unwrap()
        .unwrap()
        .state
}

// === Mock engine =======================================================

#[derive(Debug)]
enum MockEngineBehavior {
    SucceedsThenCompletes,
    Errors(EngineError),
}

#[derive(Debug, Default, Clone)]
struct MockEngineState {
    pub start_calls: Arc<std::sync::Mutex<Vec<RunId>>>,
}

struct MockEngineFacade {
    state: MockEngineState,
    behavior: tokio::sync::Mutex<MockEngineBehavior>,
}

impl MockEngineFacade {
    fn new(behavior: MockEngineBehavior) -> (Arc<Self>, MockEngineState) {
        let state = MockEngineState::default();
        let f = Arc::new(Self {
            state: state.clone(),
            behavior: tokio::sync::Mutex::new(behavior),
        });
        (f, state)
    }
}

#[async_trait]
impl EngineFacade for MockEngineFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        self.state.start_calls.lock().unwrap().push(run_id);
        let mut behavior = self.behavior.lock().await;
        match &*behavior {
            MockEngineBehavior::SucceedsThenCompletes => {
                let (tx, rx) = broadcast::channel(8);
                // Emit one Persisted (so TicketStateSync flips to Active),
                // then a Terminal::Completed.
                let tx_for_task = tx.clone();
                let completion = tokio::spawn(async move {
                    use surge_core::keys::NodeKey;
                    use surge_core::run_event::EventPayload;
                    let _ = tx_for_task.send(EngineRunEvent::Persisted {
                        seq: 0,
                        payload: EventPayload::RunStarted {
                            run_id_str: run_id.to_string(),
                            graph_hash: String::new(),
                            run_config: surge_core::run_event::RunConfig {
                                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                                auto_pr: false,
                                mcp_servers: vec![],
                            },
                        },
                    });
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = tx_for_task.send(EngineRunEvent::Terminal(
                        RunOutcome::Completed {
                            terminal: NodeKey::try_new("terminal-success").unwrap(),
                        },
                    ));
                    RunOutcome::Completed {
                        terminal: NodeKey::try_new("terminal-success").unwrap(),
                    }
                });
                Ok(RunHandle {
                    run_id,
                    events: rx,
                    completion,
                })
            }
            MockEngineBehavior::Errors(err) => Err(err.clone()),
        }
    }

    async fn resume_run(
        &self,
        _run_id: RunId,
        _worktree: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        unimplemented!("not used by inbox tests")
    }
    async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Ok(())
    }
    async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Ok(())
    }
    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(vec![])
    }
}

// EngineError must be Clone for the Errors variant. The crate's enum may
// not be Clone today; for the test we Box+rebuild via Display:
impl Clone for MockEngineBehavior {
    fn clone(&self) -> Self {
        match self {
            Self::SucceedsThenCompletes => Self::SucceedsThenCompletes,
            Self::Errors(e) => Self::Errors(EngineError::Internal(e.to_string())),
        }
    }
}

// === Test scaffold =====================================================

async fn make_consumer(
    storage: Arc<Storage>,
    engine: Arc<dyn EngineFacade>,
    source: Arc<MockTaskSource>,
) -> (InboxActionConsumer, Arc<MockTaskSource>) {
    let mut sources: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
    sources.insert("mock:t".into(), Arc::clone(&source) as Arc<dyn TaskSource>);
    let bootstrap: Arc<dyn BootstrapGraphBuilder> = Arc::new(MinimalBootstrapGraphBuilder::new());
    (
        InboxActionConsumer {
            storage,
            bootstrap,
            engine,
            sources: Arc::new(sources),
            worktrees_root: std::env::temp_dir().join("inbox_test_worktrees"),
            poll_interval: Duration::from_millis(50),
        },
        source,
    )
}

// === Scenarios =========================================================

#[tokio::test]
async fn scenario_a_start_happy_path() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#1", "tok_start_1");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    mock.preload_task(
        TaskId::try_new("mock:t#1").unwrap(),
        "Title".into(),
        "Body".into(),
        "https://mock.invalid/1".into(),
        vec![],
    )
    .await;
    let (engine, engine_state) = MockEngineFacade::new(MockEngineBehavior::SucceedsThenCompletes);
    let (consumer, mock) =
        make_consumer(Arc::clone(&storage), engine.clone() as Arc<dyn EngineFacade>, mock).await;

    // Enqueue Start action.
    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#1",
            "tok_start_1",
            "telegram",
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let consumer_handle = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.cancel();
    let _ = consumer_handle.await;

    // Assertions:
    // 1. engine.start_run was called.
    assert_eq!(engine_state.start_calls.lock().unwrap().len(), 1);
    // 2. ticket_index reached Completed (via TicketStateSync after engine emits terminal).
    //    Allow extra time for async sync.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(fetch_state(&storage, "mock:t#1"), TicketState::Completed);
    // 3. Tracker comments contain start + completion.
    let comments = mock.posted_comments().await;
    assert!(comments.iter().any(|(_, body)| body.starts_with("Surge run #")));
    assert!(comments.iter().any(|(_, body)| body.contains("✅")));
    // 4. callback_token cleared.
    let conn = storage.acquire_registry_conn().unwrap();
    let row = IntakeRepo::new(&conn).fetch("mock:t#1").unwrap().unwrap();
    assert_eq!(row.callback_token, None);
}

#[tokio::test]
async fn scenario_b_snooze_then_re_emit() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#2", "tok_snooze_2");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    let (engine, _) = MockEngineFacade::new(MockEngineBehavior::SucceedsThenCompletes);
    let (consumer, _) = make_consumer(Arc::clone(&storage), engine as Arc<dyn EngineFacade>, mock).await;

    // Enqueue Snooze with snooze_until = past so re-emission is immediate.
    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Snooze,
            "mock:t#2",
            "tok_snooze_2",
            "telegram",
            Some(Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let consumer_handle = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // After consumer ticks, state should be Snoozed.
    assert_eq!(fetch_state(&storage, "mock:t#2"), TicketState::Snoozed);

    shutdown.cancel();
    let _ = consumer_handle.await;

    // Snooze scheduler tick (manual trigger via direct tick logic):
    use surge_daemon::inbox::snooze_scheduler::SnoozeScheduler;
    let scheduler = SnoozeScheduler {
        storage: Arc::clone(&storage),
        poll_interval: Duration::from_millis(50),
    };
    let sched_shutdown = CancellationToken::new();
    let sched_handle = tokio::spawn(scheduler.run(sched_shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    sched_shutdown.cancel();
    let _ = sched_handle.await;

    // After re-emit: state back to InboxNotified, callback_token regenerated.
    let conn = storage.acquire_registry_conn().unwrap();
    let row = IntakeRepo::new(&conn).fetch("mock:t#2").unwrap().unwrap();
    assert_eq!(row.state, TicketState::InboxNotified);
    assert!(row.callback_token.is_some());
    assert_ne!(row.callback_token.as_deref(), Some("tok_snooze_2"));
    // A delivery row exists for the re-emission.
    let deliveries = inbox_queue::list_pending_telegram_deliveries(&conn).unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].task_id, "mock:t#2");
}

#[tokio::test]
async fn scenario_c_skip_sets_label_and_state() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#3", "tok_skip_3");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    let (engine, _) = MockEngineFacade::new(MockEngineBehavior::SucceedsThenCompletes);
    let (consumer, mock) =
        make_consumer(Arc::clone(&storage), engine as Arc<dyn EngineFacade>, mock).await;

    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Skip,
            "mock:t#3",
            "tok_skip_3",
            "telegram",
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let h = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    shutdown.cancel();
    let _ = h.await;

    assert_eq!(fetch_state(&storage, "mock:t#3"), TicketState::Skipped);
    let labels = mock.set_labels().await;
    assert!(labels
        .iter()
        .any(|(_, label, present)| label == "surge:skipped" && *present));
}

#[tokio::test]
async fn scenario_d_idempotent_double_start() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#4", "tok_dbl_4");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    mock.preload_task(
        TaskId::try_new("mock:t#4").unwrap(),
        "T".into(),
        "B".into(),
        "https://mock.invalid/4".into(),
        vec![],
    )
    .await;
    let (engine, engine_state) =
        MockEngineFacade::new(MockEngineBehavior::SucceedsThenCompletes);
    let (consumer, _) =
        make_consumer(Arc::clone(&storage), engine.clone() as Arc<dyn EngineFacade>, mock).await;

    // Enqueue Start TWICE for the same token.
    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#4",
            "tok_dbl_4",
            "telegram",
            None,
        )
        .unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#4",
            "tok_dbl_4",
            "telegram",
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let h = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.cancel();
    let _ = h.await;

    // Engine.start_run called exactly once.
    assert_eq!(engine_state.start_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn scenario_e_engine_failure_keeps_state_inbox_notified() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#5", "tok_fail_5");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    mock.preload_task(
        TaskId::try_new("mock:t#5").unwrap(),
        "T".into(),
        "B".into(),
        "https://mock.invalid/5".into(),
        vec![],
    )
    .await;
    let (engine, engine_state) =
        MockEngineFacade::new(MockEngineBehavior::Errors(EngineError::Internal(
            "simulated".into(),
        )));
    let (consumer, _) =
        make_consumer(Arc::clone(&storage), engine.clone() as Arc<dyn EngineFacade>, mock).await;

    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#5",
            "tok_fail_5",
            "telegram",
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let h = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown.cancel();
    let _ = h.await;

    // Engine called once.
    assert_eq!(engine_state.start_calls.lock().unwrap().len(), 1);
    // State remained InboxNotified (no transition on engine failure).
    assert_eq!(fetch_state(&storage, "mock:t#5"), TicketState::InboxNotified);
}
```

- [ ] **Step 2: Verify `MockTaskSource` exposes the methods used**

The test uses `mock.preload_task(...)`, `mock.posted_comments()`, `mock.set_labels()`. Check `crates/surge-intake/src/testing.rs` for the exact method names and adapt the test calls if names differ:

```bash
grep -n "pub fn\|pub async fn" crates/surge-intake/src/testing.rs
```

If `preload_task` doesn't exist but a similar mechanism does (e.g., `add_task` or `with_open_task`), use that. If `posted_comments` returns a different accessor, adapt. The test must end with a function on `MockTaskSource` that returns recorded `(TaskId, body)` comment tuples and `(TaskId, label, present)` label tuples.

If `MockTaskSource` doesn't expose these accessors, add them (small API extension, kept under `#[cfg(test)]` or as `pub` for test helpers).

- [ ] **Step 3: Run the tests one at a time**

```bash
cargo test -p surge-daemon --test inbox_callback_e2e scenario_a_start_happy_path -- --nocapture
```

Expected: PASS. If failures, the typical issues are:
- Mock engine event broadcast — adjust the timing in `tokio::time::sleep`.
- `RunHandle.completion` requires `JoinHandle<RunOutcome>`; the test creates one with `tokio::spawn(... return outcome)`.
- `EngineError::Internal` must be a real variant — check `crates/surge-orchestrator/src/engine/error.rs` for the actual variants and adapt.

- [ ] **Step 4: Run all five scenarios**

```bash
cargo test -p surge-daemon --test inbox_callback_e2e
```

Expected: 5 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-daemon/tests/inbox_callback_e2e.rs
git commit -m "test(daemon): inbox callback e2e — Start/Snooze/Skip/idempotent/engine-fail"
```

---

## Phase 11 — Workspace gate

### Task 11.1: Workspace verification

**Files:** none

- [ ] **Step 1: Format check**

```bash
cargo fmt --all -- --check
```

Expected: clean. If anything failed, run `cargo fmt --all` and re-stage.

- [ ] **Step 2: Build**

```bash
cargo build --workspace --all-targets
```

Expected: clean.

- [ ] **Step 3: Clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Test**

```bash
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 5: Commit if any fixes were applied**

If steps 1-4 required fixups (e.g., new clippy lints, missing `#[must_use]`), commit them with a `chore: cargo fmt + clippy fixes` message.

---

### Task 11.2: Document completion in roadmap

**Files:**
- Modify: `docs/03-ROADMAP.md`

- [ ] **Step 1: Update the polish section**

Open `docs/03-ROADMAP.md`. Find the section `## RFC-0010 — Plan-C-polish ✅ (5 of 6)` and update it:

- Change the heading to `## RFC-0010 — Plan-C-polish ✅ (6 of 6)` if all polish items are now done; otherwise add a new bullet under "Remaining":

```markdown
- [x] **Inbox-card callback handler** — Telegram + Desktop taps on Start/Snooze/Skip drive `Engine::start_run`, FSM transitions, tracker comments. `BootstrapGraphBuilder` trait + `MinimalBootstrapGraphBuilder` (single-Agent graph; RFC-0004 will replace with Staged). Closes RFC-0010 acceptance #4. Plan: `docs/superpowers/plans/2026-05-06-inbox-callback-handler.md`. Spec: `docs/superpowers/specs/2026-05-06-inbox-callback-handler-design.md`.
```

Also update the closing line if needed:

```markdown
- **Bootstrap-flow handoff after Start tap (#4)** — ✅ shipped via Plan-C-polish (`MinimalBootstrapGraphBuilder`); RFC-0004 will graduate to multi-stage Description→Roadmap→Flow chain.
```

- [ ] **Step 2: Commit**

```bash
git add docs/03-ROADMAP.md
git commit -m "docs(rfc-0010): inbox callback handler complete — closes acceptance #4"
```

---

## Plan self-review

**Spec coverage:**

| Spec section | Implementing task |
|---|---|
| §3.1 BootstrapGraphBuilder trait | Task 3.1 |
| §3.1.1 MinimalBootstrapGraphBuilder | Task 3.2 |
| §3.2 InboxActionConsumer skeleton + tick | Task 6.1 |
| §3.2.1 Start handler | Task 6.2 |
| §3.2.2 Snooze handler | Task 6.3 |
| §3.2.3 Skip handler | Task 6.3 |
| §3.3 TicketStateSync | Task 7.1 |
| §3.4 TgInboxBot (callback + outgoing) | Tasks 5.2, 5.3 |
| §3.5 Desktop receiver | Task 5.5 |
| §3.6 SnoozeScheduler | Task 8.1 |
| §3.7 Migration 0004 (callback + tg refs) | Task 1.1 |
| §3.8 New IntakeRepo methods | Tasks 1.3, 1.4, 1.5 |
| §3.9 Storage extensions (split into 2 queue tables in plan) | Tasks 1.2, 1.6 |
| §3.10 New event variants | Task 2.1 |
| §3.11 InboxCardPayload schema change | Task 4.1 |
| §4 Data flow Start tap end-to-end | Verified by Task 10.1 scenario A |
| §5 Error handling matrix | Covered piecewise in Tasks 5.2, 6.2, 6.3, 7.1 |
| §5.1 Idempotency | Task 10.1 scenario D |
| §5.2 Crash recovery | Inherited from `inbox_action_queue` design (cursor-on-disk) — testing deferred to a future task |
| §6 Testing breakdown | Task 10.1 scenarios A-E |
| §7 Configuration | Task 2.2 |
| §8 File changes | All Phase 9 wiring |
| §9 Acceptance criteria 1-7 | Task 10.1 |
| §9 Acceptance criteria 8 (workspace clean) | Task 11.1 |
| §9 Acceptance criteria 9-10 (DI swap, module curation) | Verified in Task 5.1 (no `surge-orchestrator` import in `tg_bot.rs`) and Task 9.1 (DI in `spawn_inbox_subsystems`) |

**Placeholder scan:** No `TBD`, `TODO`, or "implement later" in the plan body. Two cases where I refer to "see Phase X" — those are forward references to the same plan, not unresolved work.

**Type consistency:**
- `BootstrapPrompt` defined in Task 3.1; consumed in Task 3.2 and Task 6.2 — fields match (`title`, `description`, `tracker_url`, `priority`, `labels`).
- `InboxActionKind` defined in Task 1.6 (`Start | Snooze | Skip`); used in Tasks 5.2, 5.5, 6.1 — matching variant set.
- `ActionChannel` defined in Task 5.1 (`Telegram | Desktop`); used in Tasks 5.2, 5.5 — matching.
- `IntakeError` defined in Task 1.4 (`InvalidTransition | NotFound | Sqlite`); pattern-matched in Task 6.2, 6.3 (`InvalidTransition` arm).
- `InboxCardPayload.callback_token` introduced in Task 4.1; consumed in Task 5.4 (`enqueue_inbox_card`) and Task 8.1 (`SnoozeScheduler`) — name consistent.

**Crash recovery test:** the spec §5.2 calls for a recovery test on daemon restart. The plan does not include it as a task — the in-memory `tempfile`-backed Storage in Task 10.1's scenarios doesn't naturally model SIGKILL. Adding a recovery test is feasible but deferred (it would re-open Storage from disk between scenarios). Calling this out as a follow-up rather than a missing requirement.
