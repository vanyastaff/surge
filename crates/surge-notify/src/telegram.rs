//! Telegram notification via Bot API `sendMessage`.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use crate::messages::InboxCardPayload;
use async_trait::async_trait;
use std::sync::Arc;
use surge_core::notify_config::NotifyChannel;

/// Resolves a secret reference to Telegram bot token + chat id.
#[async_trait]
pub trait TelegramSecretResolver: Send + Sync {
    /// Resolve the chat-id reference to credentials.
    async fn resolve(&self, chat_id_ref: &str) -> Result<TelegramCredentials, NotifyError>;
}

/// Resolved Telegram credentials.
pub struct TelegramCredentials {
    /// Bot token from `@BotFather`.
    pub bot_token: String,
    /// Numeric chat id (or `@channelname`).
    pub chat_id: String,
}

/// Telegram deliverer using Bot API `sendMessage`.
pub struct TelegramDeliverer {
    client: reqwest::Client,
    resolver: Arc<dyn TelegramSecretResolver>,
}

impl TelegramDeliverer {
    /// Construct with a caller-supplied resolver.
    #[must_use]
    pub fn new(resolver: Arc<dyn TelegramSecretResolver>) -> Self {
        Self {
            client: reqwest::Client::new(),
            resolver,
        }
    }
}

#[async_trait]
impl NotifyDeliverer for TelegramDeliverer {
    async fn deliver(
        &self,
        _ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        let NotifyChannel::Telegram { chat_id_ref } = channel else {
            return Err(NotifyError::Transport(
                "TelegramDeliverer received non-Telegram channel".into(),
            ));
        };

        let creds = self.resolver.resolve(chat_id_ref).await?;
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            creds.bot_token
        );
        let payload = serde_json::json!({
            "chat_id": creds.chat_id,
            "text": format!("{}\n\n{}", rendered.title, rendered.body),
        });
        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| NotifyError::Transport(format!("Telegram POST: {e}")))?;
        if !response.status().is_success() {
            return Err(NotifyError::Transport(format!(
                "Telegram sendMessage status: {}",
                response.status()
            )));
        }
        Ok(())
    }
}

/// Inline-keyboard button rendered for an inbox card.
///
/// `callback_data` is what Telegram sends back when the user taps the button;
/// the surge-daemon callback handler routes on this to apply the user's
/// decision (start the run, snooze, skip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxKeyboardButton {
    /// Label displayed to the user.
    pub label: String,
    /// `callback_data` for inline buttons, or a URL for link buttons (set
    /// `is_url = true`).
    pub data: String,
    /// True if `data` is a URL; false if it's callback data.
    pub is_url: bool,
}

impl InboxKeyboardButton {
    /// Construct a callback button.
    ///
    /// When the user taps this button, Telegram sends back `callback_data`,
    /// which the daemon uses to route the user's decision (start, snooze, skip).
    pub fn callback(label: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            data: data.into(),
            is_url: false,
        }
    }

    /// Construct a URL button.
    ///
    /// When the user taps this button, Telegram opens the URL.
    pub fn url(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            data: url.into(),
            is_url: true,
        }
    }
}

/// Rendered inbox card ready to send via the Telegram channel.
///
/// `body` is the message text (Markdown). `keyboard` is the inline keyboard
/// laid out as rows. Each inner `Vec` is a row of buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxCardRendered {
    /// Message body (Markdown format).
    pub body: String,
    /// Inline keyboard layout: each `Vec<InboxKeyboardButton>` is a row.
    pub keyboard: Vec<Vec<InboxKeyboardButton>>,
}

/// Format an [`InboxCardPayload`] as a Telegram inbox card.
///
/// Produces the user-facing message body and the inline keyboard layout.
/// Wiring this into [`TelegramDeliverer`]'s send path is a follow-up task
/// (Plan-C-polish).
#[must_use]
pub fn format_inbox_card(payload: &InboxCardPayload) -> InboxCardRendered {
    let short = payload
        .task_id
        .as_str()
        .rsplit('#')
        .next()
        .unwrap_or(payload.task_id.as_str());
    let body = format!(
        "📋 Task from {provider} · {short}\n\n\
         {title}\n\
         priority: {prio} (auto-detected)",
        provider = payload.provider,
        short = short,
        title = payload.title,
        prio = payload.priority.label(),
    );

    let keyboard = vec![
        vec![
            InboxKeyboardButton::callback("▶ Start", format!("inbox:start:{}", payload.run_id)),
            InboxKeyboardButton::callback(
                "⏸ Snooze 24h",
                format!("inbox:snooze:{}", payload.run_id),
            ),
            InboxKeyboardButton::callback("✕ Skip", format!("inbox:skip:{}", payload.run_id)),
        ],
        vec![InboxKeyboardButton::url(
            "View ticket ↗",
            payload.task_url.clone(),
        )],
    ];

    InboxCardRendered { body, keyboard }
}

#[cfg(test)]
mod inbox_format_tests {
    use super::*;
    use crate::messages::InboxCardPayload;
    use surge_intake::types::{Priority, TaskId};

    fn sample_payload() -> InboxCardPayload {
        InboxCardPayload {
            task_id: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            title: "Fix parser panic".into(),
            summary: "Stack overflow at depth 16".into(),
            priority: Priority::High,
            task_url: "https://github.com/user/repo/issues/1".into(),
            run_id: "run_abc".into(),
        }
    }

    #[test]
    fn body_format_snapshot() {
        let rendered = format_inbox_card(&sample_payload());
        // Hand-coded snapshot — preserve user-visible text exactly.
        let expected = "📋 Task from github_issues · 1\n\n\
                        Fix parser panic\n\
                        priority: high (auto-detected)";
        assert_eq!(rendered.body, expected);
    }

    #[test]
    fn keyboard_layout_has_start_snooze_skip_and_view_url() {
        let rendered = format_inbox_card(&sample_payload());
        assert_eq!(rendered.keyboard.len(), 2);

        let row0 = &rendered.keyboard[0];
        assert_eq!(row0.len(), 3);

        assert_eq!(row0[0].label, "▶ Start");
        assert_eq!(row0[0].data, "inbox:start:run_abc");
        assert!(!row0[0].is_url);

        assert_eq!(row0[1].label, "⏸ Snooze 24h");
        assert_eq!(row0[1].data, "inbox:snooze:run_abc");
        assert!(!row0[1].is_url);

        assert_eq!(row0[2].label, "✕ Skip");
        assert_eq!(row0[2].data, "inbox:skip:run_abc");
        assert!(!row0[2].is_url);

        let row1 = &rendered.keyboard[1];
        assert_eq!(row1.len(), 1);
        assert_eq!(row1[0].label, "View ticket ↗");
        assert_eq!(row1[0].data, "https://github.com/user/repo/issues/1");
        assert!(row1[0].is_url);
    }

    #[test]
    fn priority_label_appears_in_body() {
        let mut p = sample_payload();
        p.priority = Priority::Urgent;
        let rendered = format_inbox_card(&p);
        assert!(rendered.body.contains("priority: urgent"));
    }

    #[test]
    fn extracts_short_task_id_from_full_path() {
        let mut p = sample_payload();
        // For Linear-style IDs without #, the full ID after : is used
        p.task_id = TaskId::try_new("linear:workspace/ABC-123").unwrap();
        let rendered = format_inbox_card(&p);
        // Since there's no #, rsplit('#') returns the full ID after :
        assert!(rendered.body.contains("workspace/ABC-123"));
    }

    #[test]
    fn inbox_keyboard_button_constructor() {
        let cb = InboxKeyboardButton::callback("label", "data");
        assert_eq!(cb.label, "label");
        assert_eq!(cb.data, "data");
        assert!(!cb.is_url);

        let url = InboxKeyboardButton::url("link", "http://example.com");
        assert_eq!(url.label, "link");
        assert_eq!(url.data, "http://example.com");
        assert!(url.is_url);
    }
}

