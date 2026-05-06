//! Storage layer for `surge-intake`'s `ticket_index` and `task_source_state` tables.
//!
//! Currently exposes `TicketState` enum + `IntakeRow` model. The repository
//! (read/write methods) is added in T2.4.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Lifecycle states of an external ticket as tracked by `surge-intake`.
///
/// See `docs/revision/rfcs/0010-issue-tracker-integration.md` data-flow section
/// for the FSM diagram. The string form (returned by `as_str` and parsed by
/// `FromStr`) is the on-disk representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TicketState {
    /// Ticket initially observed, no triage yet.
    Seen,
    /// Flagged as duplicate of another ticket during tier1 filtering.
    Tier1Dup,
    /// Triaged and assigned a decision.
    Triaged,
    /// Triaged as duplicate; canonical ticket is stored in `duplicate_of`.
    TriagedDup,
    /// Triaged as out-of-scope.
    TriagedOOS,
    /// Triaged but decision is unclear or pending clarification.
    TriagedUnclear,
    /// Triage decision communicated to inbox.
    InboxNotified,
    /// Temporarily snoozed; will resume at `snooze_until` timestamp.
    Snoozed,
    /// User explicitly skipped this ticket.
    Skipped,
    /// A run has been spawned for this ticket.
    RunStarted,
    /// Run is currently executing.
    Active,
    /// Run completed successfully.
    Completed,
    /// Run failed.
    Failed,
    /// Run was aborted by user or system.
    Aborted,
    /// Ticket has become stale (no activity for X days).
    Stale,
    /// Ticket was triaged but has become stale.
    TriageStale,
}

impl TicketState {
    /// Stable on-disk string form. Inverse of [`FromStr`].
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Seen => "Seen",
            Self::Tier1Dup => "Tier1Dup",
            Self::Triaged => "Triaged",
            Self::TriagedDup => "TriagedDup",
            Self::TriagedOOS => "TriagedOOS",
            Self::TriagedUnclear => "TriagedUnclear",
            Self::InboxNotified => "InboxNotified",
            Self::Snoozed => "Snoozed",
            Self::Skipped => "Skipped",
            Self::RunStarted => "RunStarted",
            Self::Active => "Active",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Aborted => "Aborted",
            Self::Stale => "Stale",
            Self::TriageStale => "TriageStale",
        }
    }

    /// Whether `from -> self` is a valid transition.
    ///
    /// Returns `true` iff this transition is permitted by the ticket FSM
    /// described in `docs/revision/rfcs/0010-issue-tracker-integration.md`.
    /// Used for property testing and for runtime guards in `IntakeRepo::update_state`
    /// (future enhancement).
    #[must_use]
    pub fn is_valid_transition_from(&self, from: Self) -> bool {
        use TicketState::*;
        match (from, *self) {
            // Self-transitions: always allowed (no-op updates).
            (a, b) if a == b => true,

            // Initial branching from `Seen`:
            (Seen, Tier1Dup)
            | (Seen, Triaged)
            | (Seen, TriagedDup)
            | (Seen, TriagedOOS)
            | (Seen, TriagedUnclear)
            | (Seen, Stale) => true,

            // Triage decisions can produce inbox notification or stale.
            (Triaged, InboxNotified)
            | (Triaged, Stale)
            | (TriagedDup, Stale)
            | (TriagedOOS, Stale)
            | (TriagedUnclear, Stale)
            | (TriagedUnclear, Triaged)
            | (TriagedUnclear, TriagedDup)
            | (TriagedUnclear, TriagedOOS) => true,

            // Inbox notified -> user decisions.
            (InboxNotified, Snoozed)
            | (InboxNotified, Skipped)
            | (InboxNotified, RunStarted)
            | (InboxNotified, Stale) => true,

            // Snoozed can return to inbox when timer fires, or skip/stale.
            (Snoozed, InboxNotified) | (Snoozed, Skipped) | (Snoozed, Stale) => true,

            // Run started -> active or abort/stale recovery.
            (RunStarted, Active) | (RunStarted, TriageStale) | (RunStarted, Aborted) => true,

            // Active -> terminal states.
            (Active, Completed) | (Active, Failed) | (Active, Aborted) => true,

            // Recovery: stale-triage path.
            (TriageStale, Triaged) | (TriageStale, Aborted) => true,

            // Tier1Dup is terminal-ish for this ticket but allow Aborted.
            (Tier1Dup, Aborted) => true,

            // Terminal states are sticky (no transitions allowed beyond their self-transitions above).
            // (Completed, Failed, Aborted, Skipped, Stale -> nothing else)

            // Anything else is invalid.
            _ => false,
        }
    }
}

impl FromStr for TicketState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Seen" => Ok(Self::Seen),
            "Tier1Dup" => Ok(Self::Tier1Dup),
            "Triaged" => Ok(Self::Triaged),
            "TriagedDup" => Ok(Self::TriagedDup),
            "TriagedOOS" => Ok(Self::TriagedOOS),
            "TriagedUnclear" => Ok(Self::TriagedUnclear),
            "InboxNotified" => Ok(Self::InboxNotified),
            "Snoozed" => Ok(Self::Snoozed),
            "Skipped" => Ok(Self::Skipped),
            "RunStarted" => Ok(Self::RunStarted),
            "Active" => Ok(Self::Active),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Aborted" => Ok(Self::Aborted),
            "Stale" => Ok(Self::Stale),
            "TriageStale" => Ok(Self::TriageStale),
            other => Err(format!("unknown TicketState: {other}")),
        }
    }
}

/// One row of the `ticket_index` table.
///
/// Maps an external ticket (from an issue tracker, email, etc.) to an internal
/// task run and tracks its lifecycle state and metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeRow {
    /// External ticket identifier in `provider:scope#id` form (PK of `ticket_index`),
    /// e.g. `"github_issues:user/repo#1234"` or `"linear:wsp_acme/ABC-42"`.
    pub task_id: String,
    /// Configured source instance id (e.g. `"linear-acme"`, `"github-myapp"`).
    pub source_id: String,
    /// Provider name (e.g., `"linear"`, `"github_issues"`).
    pub provider: String,
    /// Associated run ID if this ticket spawned a `surge-run`.
    pub run_id: Option<String>,
    /// Triage decision (e.g., "implement", "wontfix").
    pub triage_decision: Option<String>,
    /// If triaged as duplicate, the task_id of the canonical ticket.
    pub duplicate_of: Option<String>,
    /// Priority level from triage.
    pub priority: Option<String>,
    /// Current lifecycle state.
    pub state: TicketState,
    /// Timestamp when this ticket was first observed.
    pub first_seen: DateTime<Utc>,
    /// Timestamp of most recent activity.
    pub last_seen: DateTime<Utc>,
    /// If state is Snoozed, resume processing at this time.
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

/// Read/write helpers for the `ticket_index` table.
///
/// Borrows a `&Connection` rather than owning it so the caller (e.g. the
/// surge-daemon TaskRouter) can hold the connection in a `Mutex` and lock
/// per call.
pub struct IntakeRepo<'a> {
    conn: &'a Connection,
}

impl<'a> IntakeRepo<'a> {
    /// Construct a repository borrowing the given connection.
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a new row. Fails if the `task_id` already exists (PK conflict).
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

    /// Update only the `last_seen` timestamp on an existing row.
    pub fn upsert_last_seen(
        &self,
        task_id: &str,
        last_seen: DateTime<Utc>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET last_seen = ?1 WHERE task_id = ?2",
            params![last_seen.to_rfc3339(), task_id],
        )?;
        Ok(())
    }

    /// Update the lifecycle `state` of an existing row.
    pub fn update_state(&self, task_id: &str, state: TicketState) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET state = ?1 WHERE task_id = ?2",
            params![state.as_str(), task_id],
        )?;
        Ok(())
    }

    /// Fetch one row by `task_id`. Returns `Ok(None)` if absent.
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

    /// Returns the `run_id` of an active duplicate run for the given task,
    /// if one exists. Used by Tier-1 PreFilter.
    pub fn lookup_active_run(&self, task_id: &str) -> rusqlite::Result<Option<String>> {
        let row = self.conn.query_row(
            "SELECT run_id FROM ticket_index \
             WHERE task_id = ?1 \
               AND run_id IS NOT NULL \
               AND state NOT IN ('Completed','Failed','Aborted','Skipped','Stale','TriagedDup','TriagedOOS')",
            params![task_id],
            |r| r.get::<_, Option<String>>(0),
        );
        match row {
            Ok(opt) => Ok(opt),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_state_round_trip() {
        for s in [
            TicketState::Seen,
            TicketState::Triaged,
            TicketState::Active,
            TicketState::Completed,
            TicketState::TriageStale,
        ] {
            let str_form = s.as_str();
            let back: TicketState = str_form.parse().unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn ticket_state_unknown_errors() {
        let err = TicketState::from_str("Garbage").unwrap_err();
        assert!(err.contains("Garbage"));
    }

    #[test]
    fn ticket_state_all_variants_have_distinct_strings() {
        // Sanity check: as_str() is bijective with FromStr — reject collisions.
        let all = [
            TicketState::Seen,
            TicketState::Tier1Dup,
            TicketState::Triaged,
            TicketState::TriagedDup,
            TicketState::TriagedOOS,
            TicketState::TriagedUnclear,
            TicketState::InboxNotified,
            TicketState::Snoozed,
            TicketState::Skipped,
            TicketState::RunStarted,
            TicketState::Active,
            TicketState::Completed,
            TicketState::Failed,
            TicketState::Aborted,
            TicketState::Stale,
            TicketState::TriageStale,
        ];
        let mut strs: Vec<&'static str> = all.iter().map(|s| s.as_str()).collect();
        strs.sort();
        let original_len = strs.len();
        strs.dedup();
        assert_eq!(
            strs.len(),
            original_len,
            "TicketState::as_str strings must be distinct"
        );
    }
}

#[cfg(test)]
mod repo_tests {
    use super::*;
    use rusqlite::Connection;

    fn db_with_schema() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
            .unwrap();
        let m2 = include_str!("runs/migrations/registry/0002_ticket_index.sql");
        conn.execute_batch(m2).unwrap();
        let m4 = include_str!("runs/migrations/registry/0004_inbox_callback_columns.sql");
        conn.execute_batch(m4).unwrap();
        conn
    }

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

    #[test]
    fn insert_then_fetch_roundtrip() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        let row = sample_row("linear:wsp1/ABC-1", TicketState::Seen);
        repo.insert(&row).unwrap();
        let fetched = repo.fetch("linear:wsp1/ABC-1").unwrap().unwrap();
        assert_eq!(fetched.state, TicketState::Seen);
        assert_eq!(fetched.task_id, "linear:wsp1/ABC-1");
    }

    #[test]
    fn lookup_active_run_returns_none_when_no_run_id() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/ABC-2", TicketState::Seen))
            .unwrap();
        let res = repo.lookup_active_run("linear:wsp1/ABC-2").unwrap();
        assert_eq!(res, None);
    }

    #[test]
    fn lookup_active_run_returns_run_id_when_active() {
        let conn = db_with_schema();
        conn.execute("INSERT INTO runs(id) VALUES ('run_abc')", [])
            .unwrap();
        let repo = IntakeRepo::new(&conn);
        let mut row = sample_row("linear:wsp1/ABC-3", TicketState::Active);
        row.run_id = Some("run_abc".into());
        repo.insert(&row).unwrap();
        let res = repo.lookup_active_run("linear:wsp1/ABC-3").unwrap();
        assert_eq!(res, Some("run_abc".into()));
    }

    #[test]
    fn lookup_active_run_excludes_completed() {
        let conn = db_with_schema();
        conn.execute("INSERT INTO runs(id) VALUES ('run_done')", [])
            .unwrap();
        let repo = IntakeRepo::new(&conn);
        let mut row = sample_row("linear:wsp1/ABC-4", TicketState::Completed);
        row.run_id = Some("run_done".into());
        repo.insert(&row).unwrap();
        let res = repo.lookup_active_run("linear:wsp1/ABC-4").unwrap();
        assert_eq!(res, None);
    }

    #[test]
    fn update_state_changes_row() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/ABC-5", TicketState::Seen))
            .unwrap();
        repo.update_state("linear:wsp1/ABC-5", TicketState::Triaged)
            .unwrap();
        let fetched = repo.fetch("linear:wsp1/ABC-5").unwrap().unwrap();
        assert_eq!(fetched.state, TicketState::Triaged);
    }

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
}

#[cfg(test)]
mod fsm_proptests {
    use super::*;
    use proptest::prelude::*;

    /// All TicketState variants, in declaration order, for proptest::prop_oneof.
    fn any_state() -> impl Strategy<Value = TicketState> {
        prop_oneof![
            Just(TicketState::Seen),
            Just(TicketState::Tier1Dup),
            Just(TicketState::Triaged),
            Just(TicketState::TriagedDup),
            Just(TicketState::TriagedOOS),
            Just(TicketState::TriagedUnclear),
            Just(TicketState::InboxNotified),
            Just(TicketState::Snoozed),
            Just(TicketState::Skipped),
            Just(TicketState::RunStarted),
            Just(TicketState::Active),
            Just(TicketState::Completed),
            Just(TicketState::Failed),
            Just(TicketState::Aborted),
            Just(TicketState::Stale),
            Just(TicketState::TriageStale),
        ]
    }

    proptest! {
        /// Walking a chain of valid transitions never lands in an invalid state.
        ///
        /// Generates a sequence of (from, to) candidate pairs; for each, if
        /// `to.is_valid_transition_from(from)` returns true, advance to `to`,
        /// otherwise stay in `from`. The walk's final state is always one of
        /// the 16 declared variants.
        #[test]
        fn walk_terminates_in_a_valid_state(
            start in any_state(),
            steps in proptest::collection::vec(any_state(), 0..50),
        ) {
            let mut current = start;
            for candidate in steps {
                if candidate.is_valid_transition_from(current) {
                    current = candidate;
                }
            }
            // Round-trip the final state through string form to confirm
            // the FSM walk produced a value that survives serialization.
            let s = current.as_str();
            let back: TicketState = s.parse().unwrap();
            prop_assert_eq!(back, current);
        }

        /// Self-transitions are always valid (idempotent updates).
        #[test]
        fn self_transition_always_valid(s in any_state()) {
            prop_assert!(s.is_valid_transition_from(s));
        }

        /// Round-trip `as_str` <-> `FromStr` for every randomly-chosen state.
        #[test]
        fn as_str_round_trips_for_all_variants(s in any_state()) {
            let str_form = s.as_str();
            let back: TicketState = str_form.parse().expect("FromStr");
            prop_assert_eq!(back, s);
        }
    }

    /// Hand-coded "happy path" scenario: Seen -> Triaged -> InboxNotified -> RunStarted -> Active -> Completed
    #[test]
    fn happy_path_is_valid() {
        let chain = [
            TicketState::Seen,
            TicketState::Triaged,
            TicketState::InboxNotified,
            TicketState::RunStarted,
            TicketState::Active,
            TicketState::Completed,
        ];
        for window in chain.windows(2) {
            let [from, to] = [window[0], window[1]];
            assert!(
                to.is_valid_transition_from(from),
                "happy path step {from:?} -> {to:?} should be valid"
            );
        }
    }

    /// Hand-coded invalid: Seen -> Active (skips bootstrap entirely)
    #[test]
    fn cannot_skip_bootstrap_states() {
        assert!(
            !TicketState::Active.is_valid_transition_from(TicketState::Seen),
            "Seen -> Active must be invalid"
        );
        assert!(
            !TicketState::Completed.is_valid_transition_from(TicketState::Seen),
            "Seen -> Completed must be invalid"
        );
    }

    /// Terminal states are sticky.
    #[test]
    fn terminals_are_sticky() {
        for terminal in [
            TicketState::Completed,
            TicketState::Failed,
            TicketState::Aborted,
            TicketState::Skipped,
        ] {
            for next in [
                TicketState::Triaged,
                TicketState::Active,
                TicketState::InboxNotified,
            ] {
                assert!(
                    !next.is_valid_transition_from(terminal),
                    "{terminal:?} -> {next:?} must be invalid (terminal is sticky)"
                );
            }
        }
    }
}
