//! Message types for notification dispatching.
//!
//! Defines `NotifyMessage` enum and payload structs that channel
//! formatters (Telegram, Desktop, etc.) pattern-match and render.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::deliverer::RenderedNotification;
use serde::{Deserialize, Serialize};
use surge_core::RunId;
use surge_core::notify_config::NotifySeverity;
use surge_core::roadmap_patch::{
    OperatorConflictChoice, RoadmapPatchId, RoadmapPatchStatus, RoadmapPatchTarget,
};
use surge_intake::types::{Priority, TaskId};

/// Payload for the `InboxCard` notification variant.
///
/// Surfaces a freshly-detected task from a `TaskSource` to the user via
/// notification channels (Telegram, Desktop). The user can then choose
/// to start the run, snooze, or skip via channel-specific UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCardPayload {
    /// Identifier of the originating ticket (e.g. `"linear:wsp_acme/ABC-42"`).
    pub task_id: TaskId,
    /// Source instance id (e.g. `"linear:wsp_acme"`).
    pub source_id: String,
    /// Provider tag (`"linear"`, `"github_issues"`, ...).
    pub provider: String,
    /// Ticket title.
    pub title: String,
    /// Short summary (typically Triage Author's `inbox_summary.md`, 3-5 lines).
    pub summary: String,
    /// Priority assigned by Triage Author.
    pub priority: Priority,
    /// Tracker URL of the originating ticket (deep-link).
    pub task_url: String,
    /// Short ULID — embedded in `callback_data` of inline buttons. The
    /// daemon resolves this to a `task_id` via
    /// `IntakeRepo::fetch_by_callback_token`. Replaces the prior `run_id`
    /// field (the actual `RunId` is generated only when `Engine::start_run`
    /// runs; pre-creating one violated the FK on `ticket_index.run_id`).
    pub callback_token: String,
}

/// Lifecycle event type for roadmap amendment notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapAmendmentNotificationKind {
    /// A patch needs human approval.
    ApprovalRequested,
    /// A patch was applied to a roadmap/flow.
    PatchApplied,
    /// An active runner observed or picked up an amendment.
    RunnerPickup,
    /// A follow-up run was materialized or started.
    FollowUpRunCreated,
    /// Applying the patch hit a conflict that needs operator input.
    ConflictDetected,
    /// A patch was rejected.
    PatchRejected,
}

impl RoadmapAmendmentNotificationKind {
    /// Stable `snake_case` label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApprovalRequested => "approval_requested",
            Self::PatchApplied => "patch_applied",
            Self::RunnerPickup => "runner_pickup",
            Self::FollowUpRunCreated => "follow_up_run_created",
            Self::ConflictDetected => "conflict_detected",
            Self::PatchRejected => "patch_rejected",
        }
    }
}

/// Structured notification payload for roadmap amendment lifecycle events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoadmapAmendmentNotificationPayload {
    /// Notification lifecycle kind.
    pub kind: RoadmapAmendmentNotificationKind,
    /// Patch this notification concerns.
    pub patch_id: RoadmapPatchId,
    /// Target roadmap/run for the patch.
    pub target: RoadmapPatchTarget,
    /// Parent or active run id, when applicable.
    pub run_id: Option<RunId>,
    /// Follow-up run id, when one was created.
    pub follow_up_run_id: Option<RunId>,
    /// Current patch lifecycle status, when known.
    pub status: Option<RoadmapPatchStatus>,
    /// Short human-readable summary.
    pub summary: String,
    /// Additional human-readable detail.
    pub detail: Option<String>,
    /// Stable conflict codes or labels.
    #[serde(default)]
    pub conflict_codes: Vec<String>,
    /// Operator choices available for conflicts.
    #[serde(default)]
    pub conflict_choices: Vec<OperatorConflictChoice>,
    /// Artifact paths worth linking in rendered channels.
    #[serde(default)]
    pub artifact_paths: Vec<PathBuf>,
}

impl RoadmapAmendmentNotificationPayload {
    /// Build a payload with the required lifecycle fields.
    #[must_use]
    pub fn new(
        kind: RoadmapAmendmentNotificationKind,
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            patch_id,
            target,
            run_id: None,
            follow_up_run_id: None,
            status: None,
            summary: summary.into(),
            detail: None,
            conflict_codes: Vec::new(),
            conflict_choices: Vec::new(),
            artifact_paths: Vec::new(),
        }
    }
}

/// Notification message type.
///
/// Used by channel-specific formatters to render payloads for delivery
/// (Telegram, Desktop, Email, Slack, Webhook).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotifyMessage {
    /// An inbox card — a freshly-detected task waiting for user action.
    InboxCard(InboxCardPayload),
    /// Roadmap amendment lifecycle message.
    RoadmapAmendment(Box<RoadmapAmendmentNotificationPayload>),
}

impl NotifyMessage {
    /// Render this structured message to the channel-neutral notification body.
    #[must_use]
    pub fn render(&self) -> RenderedNotification {
        match self {
            Self::InboxCard(payload) => render_inbox_card(payload),
            Self::RoadmapAmendment(payload) => render_roadmap_amendment(payload),
        }
    }
}

fn render_inbox_card(payload: &InboxCardPayload) -> RenderedNotification {
    RenderedNotification {
        severity: NotifySeverity::Info,
        title: format!("Inbox: {}", payload.title),
        body: format!("{}\n{}", payload.summary, payload.task_url),
        artifact_paths: Vec::new(),
    }
}

fn render_roadmap_amendment(payload: &RoadmapAmendmentNotificationPayload) -> RenderedNotification {
    let severity = match payload.kind {
        RoadmapAmendmentNotificationKind::ConflictDetected
        | RoadmapAmendmentNotificationKind::PatchRejected => NotifySeverity::Warn,
        RoadmapAmendmentNotificationKind::ApprovalRequested
        | RoadmapAmendmentNotificationKind::PatchApplied
        | RoadmapAmendmentNotificationKind::RunnerPickup
        | RoadmapAmendmentNotificationKind::FollowUpRunCreated => NotifySeverity::Info,
    };
    let mut body = format!(
        "{}\npatch_id={}\ntarget={}",
        payload.summary,
        payload.patch_id,
        amendment_target_label(&payload.target)
    );
    if let Some(detail) = &payload.detail {
        body.push('\n');
        body.push_str(detail);
    }
    if let Some(run_id) = payload.run_id {
        let _ = write!(body, "\nrun_id={run_id}");
    }
    if let Some(run_id) = payload.follow_up_run_id {
        let _ = write!(body, "\nfollowup_run_id={run_id}");
    }
    if !payload.conflict_codes.is_empty() {
        let _ = write!(body, "\nconflicts={}", payload.conflict_codes.join(", "));
    }
    RenderedNotification {
        severity,
        title: format!("Roadmap amendment: {}", payload.kind.as_str()),
        body,
        artifact_paths: payload.artifact_paths.clone(),
    }
}

fn amendment_target_label(target: &RoadmapPatchTarget) -> String {
    match target {
        RoadmapPatchTarget::ProjectRoadmap { roadmap_path } => {
            format!("project:{roadmap_path}")
        },
        RoadmapPatchTarget::RunRoadmap { run_id, .. } => format!("run:{run_id}"),
    }
}

#[cfg(test)]
mod inbox_card_tests {
    use super::*;

    #[test]
    fn round_trip_inbox_card() {
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
            },
            NotifyMessage::RoadmapAmendment(_) => panic!("expected inbox card"),
        }
    }

    #[test]
    fn round_trip_and_render_roadmap_amendment() {
        let patch_id = RoadmapPatchId::new("rpatch-notify").unwrap();
        let payload = RoadmapAmendmentNotificationPayload {
            kind: RoadmapAmendmentNotificationKind::ConflictDetected,
            patch_id: patch_id.clone(),
            target: RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: ".ai-factory/ROADMAP.md".into(),
            },
            run_id: None,
            follow_up_run_id: Some(RunId::new()),
            status: Some(RoadmapPatchStatus::Approved),
            summary: "Patch needs a decision".into(),
            detail: Some("Milestone is already running".into()),
            conflict_codes: vec!["running_milestone".into()],
            conflict_choices: vec![OperatorConflictChoice::CreateFollowUpRun],
            artifact_paths: vec!["roadmap-patch.toml".into()],
        };
        let msg = NotifyMessage::RoadmapAmendment(Box::new(payload));
        let s = serde_json::to_string(&msg).unwrap();
        let back: NotifyMessage = serde_json::from_str(&s).unwrap();
        let rendered = back.render();
        assert_eq!(rendered.severity, NotifySeverity::Warn);
        assert!(rendered.body.contains(patch_id.as_str()));
        assert!(rendered.body.contains("running_milestone"));
        assert_eq!(rendered.artifact_paths.len(), 1);
    }
}
