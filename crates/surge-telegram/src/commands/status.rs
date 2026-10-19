//! `/status [run_id]` — render a [`RunStatusSnapshot`] for the cockpit chat.
//!
//! Requires the chat to be on the pairings allowlist (Decision 6); the
//! admission check lives in the bot loop's command router and is asserted
//! here only via test fakes.

use async_trait::async_trait;
use surge_core::id::RunId;
use surge_persistence::runs::RunStatusSnapshot;

use crate::commands::CommandReply;
use crate::error::Result;

/// Healthy requests and isolated journal failures from one recovery scan.
#[derive(Default)]
pub struct PendingRequestBatch {
    /// Actionable requests backed by readable durable evidence.
    pub requests: Vec<surge_orchestrator::engine::RunEventTap>,
    /// Unreadable runs remain unknown, never absent or resolved.
    pub failures: Vec<PendingRequestFailure>,
}

/// A run whose durable evidence could not be inspected.
pub struct PendingRequestFailure {
    /// Run requiring a later retry.
    pub run_id: RunId,
    /// Inspection or replay failure, preserved for diagnostics.
    pub error: crate::error::TelegramCockpitError,
}

/// Reads a per-run status snapshot. Production wraps
/// `surge_persistence::runs::query::current_status`.
#[async_trait]
pub trait RunSnapshotProvider: Send + Sync {
    /// Durable pending requests, used to recover cards missed by the live tap.
    /// Per-run failures are isolated in the batch; an outer error means discovery
    /// itself failed. Neither kind of failure authorizes closing a card.
    async fn pending_requests(&self) -> Result<PendingRequestBatch> {
        Ok(PendingRequestBatch::default())
    }

    /// Whether the exact durable source request has a matching resolution/timeout.
    /// Unreadable evidence returns an error and must not close a card.
    async fn request_settled(
        &self,
        _card: &surge_persistence::telegram::cards::Card,
    ) -> Result<bool> {
        Ok(false)
    }

    /// Look up the snapshot for `run_id`. Returns `Ok(None)` when the
    /// run does not exist.
    async fn snapshot(&self, run_id: RunId) -> Result<Option<RunStatusSnapshot>>;
}

/// Handle `/status [run_id]`.
///
/// `args` is the rest of the message after `/status`. Empty args mean
/// "give me the latest active run" — the bot loop's command router is
/// expected to resolve "latest" by calling
/// [`crate::commands::runs::RunListProvider::list_recent`] and passing
/// the first row's id here. The actual lookup of "latest" is **not**
/// owned by this handler — it stays a single-snapshot renderer.
///
/// # Errors
///
/// Returns whatever the underlying [`RunSnapshotProvider`] returns.
pub async fn handle_status<P>(chat_id: i64, args: &str, provider: &P) -> Result<CommandReply>
where
    P: RunSnapshotProvider,
{
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/status <run_id>` — pass a run id (use `/runs` to list).",
        ));
    }

    let run_id = match trimmed.parse::<RunId>() {
        Ok(id) => id,
        Err(err) => {
            tracing::info!(
                target: "telegram::cmd::status",
                chat_id = %chat_id,
                arg = %trimmed,
                error = %err,
                "status rejected — invalid run id",
            );
            return Ok(CommandReply::new(format!(
                "❌ `{trimmed}` is not a valid run id."
            )));
        },
    };

    let Some(snap) = provider.snapshot(run_id).await? else {
        tracing::info!(
            target: "telegram::cmd::status",
            chat_id = %chat_id,
            %run_id,
            "status — no such run",
        );
        return Ok(CommandReply::new(format!(
            "❌ No run found for `{run_id}`."
        )));
    };

    tracing::info!(
        target: "telegram::cmd::status",
        chat_id = %chat_id,
        %run_id,
        "status rendered",
    );

    Ok(CommandReply::new(render_snapshot(&snap)))
}

/// Render a [`RunStatusSnapshot`] as a Markdown-formatted text block. The
/// same body schema is used by the cockpit's status card (see
/// `card::render::render_status`), kept distinct so callers can choose
/// between a card and a plain reply without coupling.
pub(crate) fn render_snapshot(snap: &RunStatusSnapshot) -> String {
    render_snapshot_for(&snap.run_id, snap)
}

/// One escaped body for the command reply and status card.
pub(crate) fn render_snapshot_for(run_id: &RunId, snap: &RunStatusSnapshot) -> String {
    let active = snap.active_node.as_deref().unwrap_or("(not yet started)");
    let outcome = snap.last_outcome.as_deref().unwrap_or("—");
    let attempt = snap
        .last_attempt
        .map_or_else(|| "—".to_owned(), |a| a.to_string());
    let elapsed_s = snap
        .elapsed_ms
        .map_or_else(|| "—".to_owned(), |ms| format!("{}s", ms / 1_000));
    let state = snap.display.label();
    let mut action = snap
        .display
        .next_action()
        .map_or_else(String::new, |text| format!("\nnext action: {text}"));
    if let Some((until, basis)) = snap.display.wake() {
        action.push_str(&format!(
            "\nwake: {} ({})",
            until.to_rfc3339(),
            match basis {
                surge_core::capacity::WakeBasis::ObservedReset => "observed provider reset",
                surge_core::capacity::WakeBasis::PolicyBackoff => "policy backoff",
            }
        ));
    }
    let active = escape_code(active);
    let outcome = escape_code(outcome);
    format!(
        "📊 *Run status* — `{run_id}`\n\n\
         active node: `{active}`\n\
         last outcome: `{outcome}` (attempt {attempt})\n\
         elapsed: {elapsed_s}\n\
         events: {events}\n\
         state: {state}{action}",
        events = snap.event_count,
    )
}

fn escape_code(text: &str) -> String {
    text.replace('\\', "\\\\").replace('`', "\\`")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn status_command_and_card_explain_waits_with_identical_safe_text() {
        use surge_core::run_display::{RunDisplayState, WaitingReason};
        let id = RunId::new();
        let until = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        for (display, label, action) in [
            (
                RunDisplayState::Waiting(WaitingReason::HumanInput),
                "Waiting for human input",
                "Answer the pending request in the Inbox.",
            ),
            (
                RunDisplayState::Waiting(WaitingReason::Capacity {
                    until,
                    basis: surge_core::capacity::WakeBasis::ObservedReset,
                    runtime: Some("runtime-a".into()),
                }),
                "Waiting for capacity",
                "Wait for the recorded wake time; successful dispatch depends on available capacity.",
            ),
            (
                RunDisplayState::Waiting(WaitingReason::RecoveryRequired),
                "Recovery requires an operator",
                "Inspect the run and choose whether to resume or abort.",
            ),
            (
                RunDisplayState::Unknown,
                "Run state is unconfirmed",
                "Inspect the run journal before taking action.",
            ),
        ] {
            let mut snap = RunStatusSnapshot::empty(id);
            snap.display = display;
            snap.active_node = Some("node`injected\\name".into());
            let text = render_snapshot(&snap);
            assert!(text.contains(label));
            assert!(text.contains(action));
            assert!(text.contains("node\\`injected\\\\name"));
            assert_eq!(crate::card::render::render_status(&id, &snap).body_md, text);
            assert!(
                !snap.terminal,
                "a waiting display is no completion evidence"
            );
        }
        let mut snap = RunStatusSnapshot::empty(id);
        snap.display = RunDisplayState::Waiting(WaitingReason::Capacity {
            until,
            basis: surge_core::capacity::WakeBasis::ObservedReset,
            runtime: None,
        });
        assert!(
            render_snapshot(&snap).contains("2027-01-15T08:00:00+00:00 (observed provider reset)")
        );
    }

    #[derive(Default)]
    struct FakeProvider {
        result: Mutex<Option<RunStatusSnapshot>>,
        calls: Mutex<Vec<RunId>>,
    }

    impl FakeProvider {
        fn returning(snapshot: RunStatusSnapshot) -> Self {
            Self {
                result: Mutex::new(Some(snapshot)),
                calls: Mutex::default(),
            }
        }
        fn returning_none() -> Self {
            Self::default()
        }
    }

    #[async_trait]
    impl RunSnapshotProvider for FakeProvider {
        async fn snapshot(&self, run_id: RunId) -> Result<Option<RunStatusSnapshot>> {
            self.calls.lock().unwrap().push(run_id);
            Ok(self.result.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn empty_args_returns_usage_message() {
        let provider = FakeProvider::default();
        let reply = handle_status(42, "", &provider).await.unwrap();
        assert!(reply.text.contains("/status"));
        assert!(provider.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_run_id_returns_friendly_error() {
        let provider = FakeProvider::default();
        let reply = handle_status(42, "not-a-run-id", &provider).await.unwrap();
        assert!(reply.text.contains("not a valid run id"));
        assert!(provider.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_run_id_returns_no_such_run() {
        let provider = FakeProvider::returning_none();
        let run_id = RunId::new();
        let reply = handle_status(42, &run_id.to_string(), &provider)
            .await
            .unwrap();
        assert!(reply.text.contains("No run found"));
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn known_run_renders_snapshot_block() {
        let run_id = RunId::new();
        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.display = surge_core::run_display::RunDisplayState::Working;
        snap.active_node = Some("approve_plan".into());
        snap.last_outcome = Some("approve".into());
        snap.last_attempt = Some(1);
        snap.event_count = 7;
        snap.started_at_ms = Some(0);
        snap.last_event_at_ms = Some(45_000);
        snap.elapsed_ms = Some(45_000);

        let provider = FakeProvider::returning(snap);
        let reply = handle_status(42, &run_id.to_string(), &provider)
            .await
            .unwrap();
        assert!(reply.text.contains("approve_plan"));
        assert!(reply.text.contains("45s"));
        assert!(reply.text.contains("Running"));
        assert!(reply.text.contains("events: 7"));
    }

    #[tokio::test]
    async fn terminal_failed_run_shows_failed_state() {
        let run_id = RunId::new();
        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.display =
            surge_core::run_display::RunDisplayState::Done(surge_core::TerminalReason::Failed);
        snap.terminal = true;
        snap.failed = true;
        let provider = FakeProvider::returning(snap);
        let reply = handle_status(42, &run_id.to_string(), &provider)
            .await
            .unwrap();
        assert!(reply.text.contains("Failed"));
    }
}
