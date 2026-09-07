//! Read-API for a run's current status — originally the cockpit's
//! `/status` and `/runs` commands, now also `surge-daemon`'s wake scheduler
//! (Task 12 M4), which reads the same fold to find a parked run's recorded
//! worktree and its consecutive-blind-park count without a second pass over
//! the log.
//!
//! Folds a run's persisted event log down to a small snapshot. Reuses the
//! existing [`RunReader`] / `RunFilter` machinery — no new fold rules.

use std::ops::Range;
use std::path::PathBuf;

use surge_core::id::RunId;
use surge_core::run_event::{EscalationCause, EventPayload};

use crate::runs::error::StorageError;
use crate::runs::reader::{ReadEvent, RunReader};
use crate::runs::seq::EventSeq;

/// Lightweight per-run status the cockpit's `/status` command renders.
///
/// All optional fields are `None` until the corresponding event appears in
/// the log. `terminal` is `true` once a `RunCompleted` or `RunFailed` event
/// has been observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStatusSnapshot {
    /// The run this snapshot describes.
    pub run_id: RunId,
    /// Most-recent `StageEntered` node, or `None` before the first stage.
    pub active_node: Option<String>,
    /// Most-recent `OutcomeReported.outcome` string, or `None`.
    pub last_outcome: Option<String>,
    /// Most-recent stage `attempt` from `StageEntered`, or `None`.
    pub last_attempt: Option<u32>,
    /// `true` once a terminal event (`RunCompleted` or `RunFailed`) lands.
    pub terminal: bool,
    /// `true` only for `RunFailed`. Distinguishes failed-terminal from
    /// success-terminal for card rendering.
    pub failed: bool,
    /// Unix epoch ms of the `RunStarted` event, or `None` if not yet started.
    pub started_at_ms: Option<i64>,
    /// Unix epoch ms of the most recent event.
    pub last_event_at_ms: Option<i64>,
    /// `last_event_at_ms - started_at_ms`, or `None` if either is missing.
    pub elapsed_ms: Option<i64>,
    /// Total number of events in the log (== highest seq observed).
    pub event_count: u64,
    /// The worktree path recorded by the most recent `RunParked` event,
    /// cleared by `RunWokeFromPark` (Task 12 M4). `None` for a run that
    /// has never parked, or whose most recent park was already woken.
    ///
    /// This is the run's *actual* worktree, recorded at the moment it
    /// parked (`RunTaskParams::worktree_path`, threaded through
    /// `EventPayload::RunParked.worktree`) — not the
    /// `<worktrees_root>/<run_id>` path `surge-daemon`'s crash-recovery
    /// scan reconstructs (`recovery.rs`'s own doc names the limitation:
    /// a run launched with a custom `--worktree` is not resumable through
    /// that reconstruction). `surge-daemon::wake_scheduler` (Task 12 M4)
    /// reads this field so a parked run's wake probes the worktree it was
    /// actually parked from.
    pub parked_worktree: Option<PathBuf>,
    /// Count of consecutive `RunParked { basis: WakeBasis::PolicyBackoff }`
    /// events with no successful dispatch (`StageCompleted`) between them
    /// (Task 12 M4, `CapacityConfig::blind_park_limit`). Increments on each
    /// such park, resets to `0` on `StageCompleted`. A `RunParked { basis:
    /// WakeBasis::ObservedReset }` does **not** reset it — the streak this
    /// counter tracks is "blind" (no learned reset time, no real dispatch
    /// success), and a still-exhausted-but-now-observed window is neither.
    pub consecutive_blind_parks: u32,
    /// `true` once `surge-daemon::wake_scheduler` has already raised
    /// `EscalationRequested { cause: CapacityBlindParkLimitExceeded }` for
    /// the *current* blind-park streak — cleared, alongside
    /// [`Self::consecutive_blind_parks`], by the next `StageCompleted`. Lets
    /// the wake scheduler escalate once per streak instead of once per
    /// poll tick for as long as the run stays stuck.
    pub blind_park_limit_escalated: bool,
}

impl RunStatusSnapshot {
    /// Construct an empty snapshot bound to `run_id`.
    #[must_use]
    pub fn empty(run_id: RunId) -> Self {
        Self {
            run_id,
            active_node: None,
            last_outcome: None,
            last_attempt: None,
            terminal: false,
            failed: false,
            started_at_ms: None,
            last_event_at_ms: None,
            elapsed_ms: None,
            event_count: 0,
            parked_worktree: None,
            consecutive_blind_parks: 0,
            blind_park_limit_escalated: false,
        }
    }
}

/// Aggregate a per-event status snapshot from an ordered slice of
/// [`ReadEvent`]s.
///
/// Pure function — does no I/O. Lifted out of [`current_status`] so the
/// aggregation logic is unit-testable against synthetic event slices.
#[must_use]
pub fn aggregate_status(run_id: RunId, events: &[ReadEvent]) -> RunStatusSnapshot {
    let mut snap = RunStatusSnapshot::empty(run_id);
    snap.event_count = events.len() as u64;

    for ev in events {
        snap.last_event_at_ms = Some(ev.timestamp_ms);
        match ev.payload.payload() {
            EventPayload::RunStarted { .. } => {
                snap.started_at_ms = Some(ev.timestamp_ms);
            },
            EventPayload::StageEntered { node, attempt, .. } => {
                snap.active_node = Some(node.as_str().to_owned());
                snap.last_attempt = Some(*attempt);
            },
            EventPayload::OutcomeReported { outcome, .. } => {
                snap.last_outcome = Some(outcome.as_str().to_owned());
            },
            EventPayload::RunCompleted { .. } => {
                snap.terminal = true;
            },
            EventPayload::RunFailed { .. } => {
                snap.terminal = true;
                snap.failed = true;
            },
            EventPayload::RunParked {
                worktree, basis, ..
            } => {
                snap.parked_worktree = Some(worktree.clone());
                if matches!(basis, surge_core::capacity::WakeBasis::PolicyBackoff) {
                    snap.consecutive_blind_parks = snap.consecutive_blind_parks.saturating_add(1);
                }
            },
            EventPayload::RunWokeFromPark { .. } => {
                snap.parked_worktree = None;
            },
            EventPayload::StageCompleted { .. } => {
                snap.consecutive_blind_parks = 0;
                snap.blind_park_limit_escalated = false;
            },
            EventPayload::EscalationRequested {
                cause: EscalationCause::CapacityBlindParkLimitExceeded,
                ..
            } => {
                snap.blind_park_limit_escalated = true;
            },
            _ => {},
        }
    }

    if let (Some(start), Some(end)) = (snap.started_at_ms, snap.last_event_at_ms) {
        snap.elapsed_ms = Some(end.saturating_sub(start));
    }
    snap
}

/// Read the full event log for `run_id` and fold it into a
/// [`RunStatusSnapshot`].
///
/// This is the entry point the cockpit's `/status` command handler calls.
/// Reading every event is acceptable because (a) per-run logs are small in
/// practice and (b) the read cost is bounded by SQL polling already used by
/// `subscribe_events`.
///
/// # Errors
///
/// Returns [`StorageError`] if the underlying SQL read fails.
pub async fn current_status(
    reader: &RunReader,
    run_id: RunId,
) -> Result<RunStatusSnapshot, StorageError> {
    tracing::debug!(
        target: "persistence::runs::query",
        %run_id,
        "current_status read"
    );
    let events = reader
        .read_events(Range {
            start: EventSeq(0),
            end: EventSeq(u64::MAX),
        })
        .await?;
    Ok(aggregate_status(run_id, &events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::budget::BudgetGuard;
    use surge_core::capacity::WakeBasis;
    use surge_core::keys::{NodeKey, OutcomeKey};
    use surge_core::migrations::MAX_SUPPORTED_VERSION;
    use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
    use surge_core::sandbox::SandboxMode;

    fn run_config() -> RunConfig {
        RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: Vec::new(),
            budget: BudgetGuard::default(),
        }
    }

    fn event(seq: u64, timestamp_ms: i64, payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms,
            kind: "fixture".to_owned(),
            payload: VersionedEventPayload {
                schema_version: MAX_SUPPORTED_VERSION,
                payload,
            },
        }
    }

    fn node(name: &str) -> NodeKey {
        NodeKey::try_new(name).expect("valid node key")
    }

    fn outcome(name: &str) -> OutcomeKey {
        OutcomeKey::try_new(name).expect("valid outcome key")
    }

    #[test]
    fn empty_event_list_yields_empty_snapshot() {
        let run_id = RunId::new();
        let snap = aggregate_status(run_id, &[]);
        assert_eq!(snap, RunStatusSnapshot::empty(run_id));
    }

    #[test]
    fn started_event_populates_start_time_only() {
        let run_id = RunId::new();
        let events = [event(
            1,
            1_000,
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: PathBuf::from("/p"),
                initial_prompt: String::new(),
                config: run_config(),
            },
        )];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.started_at_ms, Some(1_000));
        assert_eq!(snap.last_event_at_ms, Some(1_000));
        assert_eq!(snap.elapsed_ms, Some(0));
        assert!(snap.active_node.is_none());
        assert!(snap.last_outcome.is_none());
        assert!(!snap.terminal);
        assert!(!snap.failed);
        assert_eq!(snap.event_count, 1);
    }

    #[test]
    fn stage_entered_updates_active_node_and_attempt() {
        let run_id = RunId::new();
        let events = [
            event(
                1,
                1_000,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/p"),
                    initial_prompt: String::new(),
                    config: run_config(),
                },
            ),
            event(
                2,
                1_100,
                EventPayload::StageEntered {
                    node: node("first"),
                    attempt: 1,
                },
            ),
            event(
                3,
                1_200,
                EventPayload::StageEntered {
                    node: node("second"),
                    attempt: 2,
                },
            ),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.active_node.as_deref(), Some("second"));
        assert_eq!(snap.last_attempt, Some(2));
        assert_eq!(snap.elapsed_ms, Some(200));
    }

    #[test]
    fn outcome_reported_updates_last_outcome() {
        let run_id = RunId::new();
        let events = [event(
            1,
            1_000,
            EventPayload::OutcomeReported {
                node: node("agent"),
                outcome: outcome("approve"),
                summary: "ok".into(),
            },
        )];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.last_outcome.as_deref(), Some("approve"));
    }

    #[test]
    fn run_completed_marks_terminal_not_failed() {
        let run_id = RunId::new();
        let events = [
            event(
                1,
                1_000,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/p"),
                    initial_prompt: String::new(),
                    config: run_config(),
                },
            ),
            event(
                2,
                1_500,
                EventPayload::RunCompleted {
                    terminal_node: node("end"),
                },
            ),
        ];
        let snap = aggregate_status(run_id, &events);
        assert!(snap.terminal);
        assert!(!snap.failed);
        assert_eq!(snap.elapsed_ms, Some(500));
    }

    #[test]
    fn run_failed_marks_terminal_and_failed() {
        let run_id = RunId::new();
        let events = [
            event(
                1,
                1_000,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/p"),
                    initial_prompt: String::new(),
                    config: run_config(),
                },
            ),
            event(
                2,
                1_800,
                EventPayload::RunFailed {
                    error: "boom".into(),
                },
            ),
        ];
        let snap = aggregate_status(run_id, &events);
        assert!(snap.terminal);
        assert!(snap.failed);
        assert_eq!(snap.elapsed_ms, Some(800));
    }

    /// Task 12 M4: `parked_worktree` is `surge-daemon::wake_scheduler`'s
    /// only door onto the worktree a parked run actually recorded — must
    /// be set from `RunParked.worktree`, not left `None`.
    #[test]
    fn run_parked_records_the_worktree() {
        let run_id = RunId::new();
        let worktree = PathBuf::from("/wt/actual");
        let events = [event(
            1,
            1_000,
            EventPayload::RunParked {
                wake_at: chrono::Utc::now(),
                runtime: Some("claude-acp".into()),
                worktree: worktree.clone(),
                basis: WakeBasis::PolicyBackoff,
                reason: "exhausted".into(),
            },
        )];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.parked_worktree, Some(worktree));
    }

    /// The other half: `RunWokeFromPark` must clear `parked_worktree`, or
    /// a wake scheduler that re-reads the snapshot after resuming would
    /// keep seeing a worktree for a run that is no longer parked.
    #[test]
    fn run_woke_from_park_clears_the_recorded_worktree() {
        let run_id = RunId::new();
        let events = [
            event(
                1,
                1_000,
                EventPayload::RunParked {
                    wake_at: chrono::Utc::now(),
                    runtime: Some("claude-acp".into()),
                    worktree: PathBuf::from("/wt/actual"),
                    basis: WakeBasis::PolicyBackoff,
                    reason: "exhausted".into(),
                },
            ),
            event(2, 2_000, EventPayload::RunWokeFromPark {}),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(
            snap.parked_worktree, None,
            "RunWokeFromPark must clear parked_worktree"
        );
    }

    /// A second park after a wake must record the (possibly different)
    /// worktree from that second `RunParked`, not stay cleared or stick to
    /// the first one — `parked_worktree` tracks the *most recent* park.
    #[test]
    fn a_second_park_after_a_wake_records_its_own_worktree() {
        let run_id = RunId::new();
        let second_worktree = PathBuf::from("/wt/second");
        let events = [
            event(
                1,
                1_000,
                EventPayload::RunParked {
                    wake_at: chrono::Utc::now(),
                    runtime: Some("claude-acp".into()),
                    worktree: PathBuf::from("/wt/first"),
                    basis: WakeBasis::PolicyBackoff,
                    reason: "exhausted".into(),
                },
            ),
            event(2, 2_000, EventPayload::RunWokeFromPark {}),
            event(
                3,
                3_000,
                EventPayload::RunParked {
                    wake_at: chrono::Utc::now(),
                    runtime: Some("claude-acp".into()),
                    worktree: second_worktree.clone(),
                    basis: WakeBasis::PolicyBackoff,
                    reason: "exhausted again".into(),
                },
            ),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.parked_worktree, Some(second_worktree));
    }

    // ── `consecutive_blind_parks` / `blind_park_limit_escalated` (Task 12 M4) ──

    fn blind_park(seq: u64, ts: i64) -> ReadEvent {
        event(
            seq,
            ts,
            EventPayload::RunParked {
                wake_at: chrono::Utc::now(),
                runtime: Some("claude-acp".into()),
                worktree: PathBuf::from("/wt"),
                basis: WakeBasis::PolicyBackoff,
                reason: "blind backoff".into(),
            },
        )
    }

    fn observed_reset_park(seq: u64, ts: i64) -> ReadEvent {
        event(
            seq,
            ts,
            EventPayload::RunParked {
                wake_at: chrono::Utc::now(),
                runtime: Some("claude-acp".into()),
                worktree: PathBuf::from("/wt"),
                basis: WakeBasis::ObservedReset,
                reason: "observed reset".into(),
            },
        )
    }

    fn stage_completed(seq: u64, ts: i64) -> ReadEvent {
        event(
            seq,
            ts,
            EventPayload::StageCompleted {
                node: NodeKey::try_from("agent_1").unwrap(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
        )
    }

    fn blind_park_escalation(seq: u64, ts: i64) -> ReadEvent {
        event(
            seq,
            ts,
            EventPayload::EscalationRequested {
                stage: None,
                reason: "5 consecutive blind parks".into(),
                cause: EscalationCause::CapacityBlindParkLimitExceeded,
            },
        )
    }

    #[test]
    fn consecutive_blind_parks_counts_policy_backoff_parks() {
        let run_id = RunId::new();
        let events = [
            blind_park(1, 1_000),
            blind_park(2, 2_000),
            blind_park(3, 3_000),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.consecutive_blind_parks, 3);
    }

    #[test]
    fn observed_reset_park_does_not_increment_the_blind_streak() {
        let run_id = RunId::new();
        let events = [blind_park(1, 1_000), observed_reset_park(2, 2_000)];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(
            snap.consecutive_blind_parks, 1,
            "an ObservedReset-basis park is not a blind park — it must not add to the count"
        );
    }

    #[test]
    fn stage_completed_resets_the_blind_streak_and_the_escalation_flag() {
        let run_id = RunId::new();
        let events = [
            blind_park(1, 1_000),
            blind_park(2, 2_000),
            blind_park_escalation(3, 2_500),
            stage_completed(4, 3_000),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.consecutive_blind_parks, 0);
        assert!(!snap.blind_park_limit_escalated);
    }

    #[test]
    fn a_fresh_streak_after_reset_counts_from_zero_and_can_escalate_again() {
        let run_id = RunId::new();
        let events = [
            blind_park(1, 1_000),
            blind_park(2, 2_000),
            blind_park_escalation(3, 2_500),
            stage_completed(4, 3_000),
            blind_park(5, 4_000),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.consecutive_blind_parks, 1);
        assert!(
            !snap.blind_park_limit_escalated,
            "a new streak after a reset must be eligible to escalate again"
        );
    }

    #[test]
    fn blind_park_limit_escalated_flag_is_set_by_the_escalation_event() {
        let run_id = RunId::new();
        let events = [
            blind_park(1, 1_000),
            blind_park(2, 2_000),
            blind_park_escalation(3, 2_500),
        ];
        let snap = aggregate_status(run_id, &events);
        assert_eq!(snap.consecutive_blind_parks, 2);
        assert!(snap.blind_park_limit_escalated);
    }
}
