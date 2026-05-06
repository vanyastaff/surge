//! Storage layer for `surge-intake`'s `ticket_index` and `task_source_state` tables.
//!
//! Currently exposes `TicketState` enum + `IntakeRow` model. The repository
//! (read/write methods) is added in T2.4.

use chrono::{DateTime, Utc};
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
    /// Internal task ID assigned to this ticket.
    pub task_id: String,
    /// External ticket identifier (e.g., GitHub issue #, Jira key).
    pub source_id: String,
    /// Provider name (e.g., "github", "jira", "email").
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
