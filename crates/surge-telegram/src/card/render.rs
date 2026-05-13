//! Per-kind card-renderer implementations.
//!
//! Each `render_*` function takes a typed payload, produces a Markdown body
//! plus an inline-keyboard layout, and returns a [`RenderedCard`] whose
//! `content_hash` is a stable SHA-256 over the rendered body+keyboard. The
//! cockpit emitter compares `content_hash` against the stored value before
//! calling `editMessageText`, short-circuiting no-op updates per ADR 0011
//! Decision 8.
//!
//! All `callback_data` strings follow the `cockpit:<verb>:<card_id>` schema
//! locked in ADR 0010.

use sha2::{Digest, Sha256};
use surge_core::id::RunId;
use surge_core::run_event::BootstrapStage;
use surge_notify::telegram::InboxKeyboardButton;
use surge_persistence::runs::RunStatusSnapshot;

/// Discriminator for the cockpit's card kinds. Lives on the
/// `telegram_cards.kind` column and drives observability tagging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// A `HumanGate` pause that is not part of bootstrap (generic prompt
    /// + approve/edit/reject buttons).
    HumanGate,
    /// Bootstrap Description-stage approval.
    BootstrapDescription,
    /// Bootstrap Roadmap-stage approval.
    BootstrapRoadmap,
    /// Bootstrap Flow-stage approval.
    BootstrapFlow,
    /// Run-level status singleton (no actionable buttons).
    Status,
    /// Terminal-success card.
    Completion,
    /// Terminal-failure card.
    Failure,
    /// Edit-loop cap exceeded or other escalation.
    Escalation,
}

impl CardKind {
    /// Stable string form for the `telegram_cards.kind` column.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HumanGate => "human_gate",
            Self::BootstrapDescription => "bootstrap_description",
            Self::BootstrapRoadmap => "bootstrap_roadmap",
            Self::BootstrapFlow => "bootstrap_flow",
            Self::Status => "status",
            Self::Completion => "completion",
            Self::Failure => "failure",
            Self::Escalation => "escalation",
        }
    }
}

impl From<BootstrapStage> for CardKind {
    fn from(stage: BootstrapStage) -> Self {
        match stage {
            BootstrapStage::Description => Self::BootstrapDescription,
            BootstrapStage::Roadmap => Self::BootstrapRoadmap,
            BootstrapStage::Flow => Self::BootstrapFlow,
        }
    }
}

/// One rendered card — what the cockpit emitter sends as a Telegram message
/// body plus its inline keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCard {
    /// The card's discriminator. Stored in `telegram_cards.kind`.
    pub kind: CardKind,
    /// Telegram-rendered Markdown body. The cockpit posts this verbatim via
    /// `sendMessage(parse_mode=Markdown)`.
    pub body_md: String,
    /// Inline-keyboard layout; each `Vec<InboxKeyboardButton>` is a row.
    /// Reuses the existing [`InboxKeyboardButton`] type from `surge-notify`.
    pub keyboard: Vec<Vec<InboxKeyboardButton>>,
    /// Stable SHA-256 over `(body_md, keyboard)` for the `editMessageText`
    /// short-circuit. Hex-encoded; 64 characters.
    pub content_hash: String,
}

/// Render a generic (non-bootstrap) `HumanGate` card.
///
/// The buttons are approve/edit/reject following Decision 5 verb set. The
/// gate's free-form prompt text is included verbatim in the body.
#[must_use]
pub fn render_human_gate(card_id: &str, prompt: &str) -> RenderedCard {
    let body_md = format!("🛑 *Approval required*\n\n{prompt}");
    let keyboard = vec![vec![
        InboxKeyboardButton::callback("✅ Approve", cockpit_data("approve", card_id)),
        InboxKeyboardButton::callback("✏ Edit", cockpit_data("edit", card_id)),
        InboxKeyboardButton::callback("❌ Reject", cockpit_data("reject", card_id)),
    ]];
    finalize(CardKind::HumanGate, body_md, keyboard)
}

/// Render a bootstrap-stage approval card.
///
/// `stage_summary` is a short operator-readable summary of what was
/// produced at this stage (e.g. the first lines of the artifact). It is
/// trimmed to the Telegram 4096-byte body limit upstream.
#[must_use]
pub fn render_bootstrap(
    card_id: &str,
    stage: BootstrapStage,
    stage_summary: &str,
) -> RenderedCard {
    let stage_label = match stage {
        BootstrapStage::Description => "Description",
        BootstrapStage::Roadmap => "Roadmap",
        BootstrapStage::Flow => "Flow",
    };
    let body_md = format!("📝 *Bootstrap — {stage_label}*\n\n{stage_summary}");
    let keyboard = vec![vec![
        InboxKeyboardButton::callback("✅ Approve", cockpit_data("approve", card_id)),
        InboxKeyboardButton::callback("✏ Edit", cockpit_data("edit", card_id)),
        InboxKeyboardButton::callback("❌ Reject", cockpit_data("reject", card_id)),
    ]];
    finalize(CardKind::from(stage), body_md, keyboard)
}

/// Render the run-level status singleton.
///
/// Status cards carry no actionable buttons by design — they are the
/// always-visible "what is the run doing right now" surface. The cockpit's
/// `/abort` command is the only mutation path from a status card and is
/// invoked as a message command, not a button.
#[must_use]
pub fn render_status(run_id: &RunId, snapshot: &RunStatusSnapshot) -> RenderedCard {
    let active = snapshot
        .active_node
        .as_deref()
        .unwrap_or("(not yet started)");
    let outcome = snapshot.last_outcome.as_deref().unwrap_or("—");
    let attempt = snapshot
        .last_attempt
        .map_or_else(|| "—".to_owned(), |a| a.to_string());
    let elapsed_s = snapshot
        .elapsed_ms
        .map_or_else(|| "—".to_owned(), |ms| format!("{}s", ms / 1_000));
    let event_count = snapshot.event_count;
    let terminal = if snapshot.terminal {
        if snapshot.failed { "❌ failed" } else { "✅ done" }
    } else {
        "▶ running"
    };
    let body_md = format!(
        "📊 *Run status* — `{run_id}`\n\n\
         active node: `{active}`\n\
         last outcome: `{outcome}` (attempt {attempt})\n\
         elapsed: {elapsed_s}\n\
         events: {event_count}\n\
         state: {terminal}",
    );
    finalize(CardKind::Status, body_md, Vec::new())
}

/// Render a terminal-success card.
#[must_use]
pub fn render_completion(card_id: &str, run_id: &RunId, terminal_node: &str) -> RenderedCard {
    let body_md = format!(
        "✅ *Run completed*\n\n\
         run: `{run_id}`\n\
         terminal node: `{terminal_node}`"
    );
    let keyboard = vec![vec![InboxKeyboardButton::callback(
        "👁 Acknowledge",
        cockpit_data("ack", card_id),
    )]];
    finalize(CardKind::Completion, body_md, keyboard)
}

/// Render a terminal-failure card.
#[must_use]
pub fn render_failure(card_id: &str, run_id: &RunId, error: &str) -> RenderedCard {
    let body_md = format!(
        "❌ *Run failed*\n\n\
         run: `{run_id}`\n\
         error: {error}"
    );
    let keyboard = vec![vec![InboxKeyboardButton::callback(
        "👁 Acknowledge",
        cockpit_data("ack", card_id),
    )]];
    finalize(CardKind::Failure, body_md, keyboard)
}

/// Render an escalation card — bootstrap edit-loop cap hit, etc.
#[must_use]
pub fn render_escalation(
    card_id: &str,
    stage: Option<BootstrapStage>,
    reason: &str,
) -> RenderedCard {
    let stage_line = match stage {
        Some(s) => format!("stage: `{s:?}`\n"),
        None => String::new(),
    };
    let body_md = format!("⚠ *Escalation required*\n\n{stage_line}reason: {reason}");
    let keyboard = vec![vec![InboxKeyboardButton::callback(
        "👁 Acknowledge",
        cockpit_data("ack", card_id),
    )]];
    finalize(CardKind::Escalation, body_md, keyboard)
}

/// Build a `cockpit:<verb>:<card_id>` callback string per ADR 0010.
fn cockpit_data(verb: &str, card_id: &str) -> String {
    format!("cockpit:{verb}:{card_id}")
}

/// Common tail: log the render shape, compute the content hash, and pack
/// the `RenderedCard`.
fn finalize(
    kind: CardKind,
    body_md: String,
    keyboard: Vec<Vec<InboxKeyboardButton>>,
) -> RenderedCard {
    let content_hash = compute_content_hash(&body_md, &keyboard);
    tracing::debug!(
        target: "telegram::card",
        kind = %kind.as_str(),
        body_len = body_md.len(),
        buttons = total_buttons(&keyboard),
        "rendered card"
    );
    RenderedCard {
        kind,
        body_md,
        keyboard,
        content_hash,
    }
}

fn total_buttons(keyboard: &[Vec<InboxKeyboardButton>]) -> usize {
    keyboard.iter().map(Vec::len).sum()
}

/// Stable SHA-256 hash over the rendered body and keyboard.
///
/// The hash domain-separates body bytes from keyboard bytes with a fixed
/// marker so a body that happens to spell a button label cannot produce a
/// collision with a keyboard arrangement. Each button contributes
/// `label|data|c-or-u`, rows are separated by a blank line.
fn compute_content_hash(body_md: &str, keyboard: &[Vec<InboxKeyboardButton>]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body_md.as_bytes());
    hasher.update(b"\n--keyboard--\n");
    for row in keyboard {
        for btn in row {
            hasher.update(btn.label.as_bytes());
            hasher.update(b"|");
            hasher.update(btn.data.as_bytes());
            hasher.update(b"|");
            hasher.update(if btn.is_url { b"u" } else { b"c" });
            hasher.update(b"\n");
        }
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CARD_ID: &str = "01HKGZTOKABCDEFGHJKMNPQRST";

    #[test]
    fn card_kind_string_form_round_trips() {
        for kind in [
            CardKind::HumanGate,
            CardKind::BootstrapDescription,
            CardKind::BootstrapRoadmap,
            CardKind::BootstrapFlow,
            CardKind::Status,
            CardKind::Completion,
            CardKind::Failure,
            CardKind::Escalation,
        ] {
            // Each kind has a non-empty, snake_case string.
            assert!(!kind.as_str().is_empty());
            assert!(
                kind.as_str()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            );
        }
    }

    #[test]
    fn bootstrap_stage_maps_to_card_kind() {
        assert_eq!(
            CardKind::from(BootstrapStage::Description),
            CardKind::BootstrapDescription
        );
        assert_eq!(
            CardKind::from(BootstrapStage::Roadmap),
            CardKind::BootstrapRoadmap
        );
        assert_eq!(
            CardKind::from(BootstrapStage::Flow),
            CardKind::BootstrapFlow
        );
    }

    #[test]
    fn human_gate_card_carries_three_action_buttons() {
        let card = render_human_gate(SAMPLE_CARD_ID, "Approve the plan?");
        assert_eq!(card.kind, CardKind::HumanGate);
        assert!(card.body_md.contains("Approval required"));
        assert!(card.body_md.contains("Approve the plan?"));

        assert_eq!(card.keyboard.len(), 1);
        let row = &card.keyboard[0];
        assert_eq!(row.len(), 3);

        assert_eq!(row[0].label, "✅ Approve");
        assert_eq!(row[0].data, format!("cockpit:approve:{SAMPLE_CARD_ID}"));
        assert_eq!(row[1].data, format!("cockpit:edit:{SAMPLE_CARD_ID}"));
        assert_eq!(row[2].data, format!("cockpit:reject:{SAMPLE_CARD_ID}"));
        for btn in row {
            assert!(!btn.is_url, "no URL buttons on a HumanGate card");
        }
    }

    #[test]
    fn bootstrap_cards_have_stage_label_and_three_buttons() {
        for (stage, label) in [
            (BootstrapStage::Description, "Description"),
            (BootstrapStage::Roadmap, "Roadmap"),
            (BootstrapStage::Flow, "Flow"),
        ] {
            let card = render_bootstrap(SAMPLE_CARD_ID, stage, "preview text");
            assert_eq!(card.kind, CardKind::from(stage));
            assert!(
                card.body_md.contains(label),
                "body must mention the stage label, got: {}",
                card.body_md
            );
            assert!(card.body_md.contains("preview text"));
            assert_eq!(card.keyboard.len(), 1);
            assert_eq!(card.keyboard[0].len(), 3);
        }
    }

    #[test]
    fn status_card_has_no_keyboard() {
        let run_id = RunId::new();
        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.active_node = Some("approve_plan".into());
        snap.last_outcome = Some("approve".into());
        snap.last_attempt = Some(1);
        snap.event_count = 7;
        snap.started_at_ms = Some(0);
        snap.last_event_at_ms = Some(45_000);
        snap.elapsed_ms = Some(45_000);

        let card = render_status(&run_id, &snap);
        assert_eq!(card.kind, CardKind::Status);
        assert!(card.keyboard.is_empty(), "status card carries no buttons");
        assert!(card.body_md.contains("approve_plan"));
        assert!(card.body_md.contains("45s"));
        assert!(card.body_md.contains("running"));
    }

    #[test]
    fn status_card_marks_failed_state() {
        let run_id = RunId::new();
        let mut snap = RunStatusSnapshot::empty(run_id);
        snap.terminal = true;
        snap.failed = true;
        let card = render_status(&run_id, &snap);
        assert!(card.body_md.contains("failed"));
    }

    #[test]
    fn completion_card_has_single_ack_button() {
        let run_id = RunId::new();
        let card = render_completion(SAMPLE_CARD_ID, &run_id, "end");
        assert_eq!(card.kind, CardKind::Completion);
        assert_eq!(card.keyboard.len(), 1);
        assert_eq!(card.keyboard[0].len(), 1);
        assert_eq!(card.keyboard[0][0].label, "👁 Acknowledge");
        assert_eq!(
            card.keyboard[0][0].data,
            format!("cockpit:ack:{SAMPLE_CARD_ID}")
        );
    }

    #[test]
    fn failure_card_carries_error_message() {
        let run_id = RunId::new();
        let card = render_failure(SAMPLE_CARD_ID, &run_id, "agent crashed");
        assert_eq!(card.kind, CardKind::Failure);
        assert!(card.body_md.contains("agent crashed"));
        assert!(card.body_md.contains("Run failed"));
    }

    #[test]
    fn escalation_card_includes_stage_line_when_present() {
        let card = render_escalation(SAMPLE_CARD_ID, Some(BootstrapStage::Flow), "cap exceeded");
        assert_eq!(card.kind, CardKind::Escalation);
        assert!(card.body_md.contains("stage:"));
        assert!(card.body_md.contains("cap exceeded"));
    }

    #[test]
    fn escalation_card_omits_stage_line_when_none() {
        let card = render_escalation(SAMPLE_CARD_ID, None, "manual halt");
        assert!(!card.body_md.contains("stage:"));
        assert!(card.body_md.contains("manual halt"));
    }

    #[test]
    fn content_hash_is_stable_for_identical_inputs() {
        let c1 = render_human_gate(SAMPLE_CARD_ID, "Same prompt");
        let c2 = render_human_gate(SAMPLE_CARD_ID, "Same prompt");
        assert_eq!(c1.content_hash, c2.content_hash);
        assert_eq!(c1.content_hash.len(), 64, "SHA-256 is 64 hex chars");
    }

    #[test]
    fn content_hash_changes_when_body_changes() {
        let c1 = render_human_gate(SAMPLE_CARD_ID, "Original");
        let c2 = render_human_gate(SAMPLE_CARD_ID, "Changed");
        assert_ne!(c1.content_hash, c2.content_hash);
    }

    #[test]
    fn content_hash_changes_when_card_id_changes() {
        // card_id appears in keyboard.data, so a different id must produce
        // a different hash.
        let c1 = render_human_gate("01HKABCDEFGHJKMNPQRSTVWXYZ", "Same prompt");
        let c2 = render_human_gate("01HKMNPQRSTVWXYZABCDEFGHJK", "Same prompt");
        assert_ne!(c1.content_hash, c2.content_hash);
    }

    #[test]
    fn content_hash_differs_across_kinds_for_same_payload() {
        let run_id = RunId::new();
        let snap = RunStatusSnapshot::empty(run_id);
        let status = render_status(&run_id, &snap);
        let completion = render_completion(SAMPLE_CARD_ID, &run_id, "end");
        assert_ne!(status.content_hash, completion.content_hash);
    }

    #[test]
    fn callback_data_always_uses_cockpit_prefix() {
        let cards = [
            render_human_gate(SAMPLE_CARD_ID, "p"),
            render_bootstrap(SAMPLE_CARD_ID, BootstrapStage::Description, "p"),
            render_completion(SAMPLE_CARD_ID, &RunId::new(), "end"),
            render_failure(SAMPLE_CARD_ID, &RunId::new(), "boom"),
            render_escalation(SAMPLE_CARD_ID, None, "r"),
        ];
        for card in &cards {
            for row in &card.keyboard {
                for btn in row {
                    assert!(
                        btn.data.starts_with("cockpit:"),
                        "expected cockpit:* prefix on '{}'",
                        btn.data
                    );
                    assert!(
                        btn.data.len() <= 64,
                        "callback_data {:?} exceeds Telegram 64-byte limit",
                        btn.data
                    );
                }
            }
        }
    }
}
