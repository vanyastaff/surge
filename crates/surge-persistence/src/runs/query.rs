//! Read-API for the cockpit's `/status` and `/runs` commands.
//!
//! Folds a run's persisted event log down to a small snapshot the bot can
//! render in a single Telegram card. Reuses the existing [`RunReader`] /
//! `RunFilter` machinery — no new fold rules.

use std::ops::Range;

use surge_core::id::RunId;
use surge_core::run_event::EventPayload;

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
}
