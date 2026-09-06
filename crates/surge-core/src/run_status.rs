//! High-level lifecycle status for a `Run`, persisted in the registry DB.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Coarse lifecycle status of a run, suitable for cross-run queries and CLI listings.
///
/// Distinct from [`RunState`](crate::run_state::RunState), which is the full
/// state machine derived from the event log. `RunStatus` is the durable
/// "what should the operator know about this run right now" string stored in
/// the registry DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run created; bootstrap stages (description, roadmap, flow) still in progress.
    Bootstrapping,
    /// Bootstrap complete; pipeline executor active.
    Running,
    /// Pipeline reached a successful terminal node.
    Completed,
    /// Pipeline reached a terminal failure (RunFailed event).
    Failed,
    /// User or system aborted the run (RunAborted event).
    Aborted,
    /// Daemon process recorded as running but no longer alive (stale-pid detection).
    Crashed,
    /// Pipeline execution is paused waiting for a provider rate-limit
    /// window to reset (Task 12, R37/R37.1) — recorded by `RunParked`,
    /// cleared by `RunWokeFromPark`. Deliberately **not** terminal: the run
    /// resumes on its own once `wake_at` passes, with no further operator
    /// action required, unlike [`Self::Failed`]/[`Self::Aborted`]/
    /// [`Self::Crashed`].
    Parked,
}

impl RunStatus {
    /// Stable string form used in the registry DB `status` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrapping => "bootstrapping",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Crashed => "crashed",
            Self::Parked => "parked",
        }
    }

    /// True if the run is in a terminal state (no further events expected).
    ///
    /// [`Self::Parked`] deliberately is **not** enumerated here — the
    /// `matches!` above simply does not name it, so it falls through to
    /// `false` without any edit to this method (Task 12, §1(16)): a parked
    /// run is paused, not finished, and further events (`RunWokeFromPark`,
    /// then resumed pipeline execution) are expected once `wake_at` passes.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Aborted | Self::Crashed
        )
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown status string.
#[derive(Debug, Clone, thiserror::Error)]
#[error("unknown RunStatus: {0:?}")]
pub struct ParseRunStatusError(pub String);

impl FromStr for RunStatus {
    type Err = ParseRunStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "bootstrapping" => Self::Bootstrapping,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "aborted" => Self::Aborted,
            "crashed" => Self::Crashed,
            "parked" => Self::Parked,
            other => return Err(ParseRunStatusError(other.to_string())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_string_form() {
        for s in [
            RunStatus::Bootstrapping,
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
            RunStatus::Crashed,
            RunStatus::Parked,
        ] {
            assert_eq!(s.as_str().parse::<RunStatus>().unwrap(), s);
        }
    }

    #[test]
    fn unknown_string_is_error() {
        assert!("nonsense".parse::<RunStatus>().is_err());
    }

    #[test]
    fn terminal_classification() {
        assert!(!RunStatus::Bootstrapping.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
        assert!(RunStatus::Completed.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Aborted.is_terminal());
        assert!(RunStatus::Crashed.is_terminal());
    }

    #[test]
    fn parked_is_not_terminal() {
        // Task 12 (§1(16)): pinned as intent, not accident — `Parked` simply
        // is not named in `is_terminal`'s `matches!`, so this already holds
        // without any change to that method; this test exists so a future
        // edit to `is_terminal` cannot silently start treating a parked run
        // as finished without failing a test.
        assert!(!RunStatus::Parked.is_terminal());
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(RunStatus::Crashed.to_string(), "crashed");
    }
}
