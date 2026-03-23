use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
use crate::markdown;
use crate::theme;

// ── Message model ───────────────────────────────────────────────────

/// A tool call activity shown inline within an agent response.
#[derive(Debug, Clone)]
pub struct ToolActivity {
    pub title: String,
    pub done: bool,
}

/// A message in the terminal conversation.
#[derive(Debug, Clone)]
pub struct TerminalMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
    /// Tool activities (only for Agent messages).
    pub activities: Vec<ToolActivity>,
}

impl TerminalMessage {
    fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            timestamp: String::new(),
            activities: Vec::new(),
        }
    }

    fn with_timestamp(mut self, ts: impl Into<String>) -> Self {
        self.timestamp = ts.into();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Agent,
    System,
}

// ── Screen ──────────────────────────────────────────────────────────

/// Agent Terminal screen — send prompts, see streaming responses.
pub struct AgentTerminalScreen {
    state: Entity<AppState>,
    messages: Vec<TerminalMessage>,
    input_state: Option<Entity<InputState>>,
    is_sending: bool,
    agent_name: String,
    session_active: bool,
}

impl AgentTerminalScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        let agent_name = {
            let s = state.read(_cx);
            s.config.as_ref()
                .map(|c| c.default_agent.clone())
                .unwrap_or_else(|| "claude-acp".to_string())
        };

        Self {
            state,
            messages: vec![
                TerminalMessage::new(
                    MessageRole::System,
                    format!("Terminal ready. Agent: {agent_name}. Type a message and press Enter to send."),
                ),
            ],
            input_state: None,
            is_sending: false,
            agent_name,
            session_active: false,
        }
    }

    /// Find or create the current Agent message to append to.
    fn ensure_agent_message(&mut self) -> &mut TerminalMessage {
        let needs_new = self.messages.last()
            .map_or(true, |m| m.role != MessageRole::Agent);
        if needs_new {
            self.messages.push(
                TerminalMessage::new(MessageRole::Agent, "")
                    .with_timestamp("just now"),
            );
        }
        self.messages.last_mut().unwrap()
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input_entity) = &self.input_state else { return; };
        let input = input_entity.read(cx).value().to_string();
        let input = input.trim().to_string();
        if input.is_empty() || self.is_sending {
            return;
        }

        self.messages.push(
            TerminalMessage::new(MessageRole::User, input.clone())
                .with_timestamp("now"),
        );
        input_entity.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.is_sending = true;
        cx.notify();

        let pool = {
            let state = self.state.read(cx);
            state.agent_pool.clone()
        };

        let Some(pool) = pool else {
            self.messages.push(TerminalMessage::new(
                MessageRole::System,
                "No agent pool configured. Open a project with surge.toml first.",
            ));
            self.is_sending = false;
            cx.notify();
            return;
        };

        let agent_name = self.agent_name.clone();
        let cwd = {
            let state = self.state.read(cx);
            state.project_path.clone().unwrap_or_else(|| std::path::PathBuf::from("."))
        };

        let mut event_rx = pool.subscribe();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Create session
            let session = match pool
                .create_session(Some(&agent_name), None, &cwd)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    let err = format!("Session error: {e}");
                    cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.messages.push(TerminalMessage::new(MessageRole::System, err));
                            this.is_sending = false;
                            cx.notify();
                        });
                    }).ok();
                    return;
                }
            };

            // Event listener — tool calls go inline into Agent message
            let this_for_events = this.clone();
            let event_task = cx.spawn(async move |cx: &mut AsyncApp| {
                while let Ok(event) = event_rx.recv().await {
                    match &event {
                        surge_core::SurgeEvent::AgentMessageChunk { text, .. } => {
                            let text = text.clone();
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    let msg = this.ensure_agent_message();
                                    msg.content.push_str(&text);
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::ToolCallStarted { title, .. } => {
                            let title = title.clone();
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    let msg = this.ensure_agent_message();
                                    msg.activities.push(ToolActivity {
                                        title,
                                        done: false,
                                    });
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::ToolCallFinished { .. } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    let msg = this.ensure_agent_message();
                                    // Mark the last unfinished activity as done
                                    if let Some(act) = msg.activities.iter_mut().rev().find(|a| !a.done) {
                                        act.done = true;
                                    }
                                    cx.notify();
                                });
                            });
                        }
                        _ => {}
                    }
                }
            });

            // Send prompt
            let content = vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent::new(input),
            )];

            let result = pool.prompt(&session, content).await;

            drop(event_task);

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(_) => {
                            if let Some(last) = this.messages.last() {
                                if last.role == MessageRole::Agent && last.content.is_empty()
                                    && last.activities.is_empty()
                                {
                                    if let Some(msg) = this.messages.last_mut() {
                                        msg.content = "(Agent completed with no text output)".into();
                                    }
                                }
                            }
                            this.session_active = true;
                        }
                        Err(err) => {
                            this.messages.push(TerminalMessage::new(
                                MessageRole::System,
                                format!("Error: {err}"),
                            ));
                        }
                    }
                    this.is_sending = false;
                    cx.notify();
                });
            }).ok();
        })
        .detach();
    }

    // ── Rendering ───────────────────────────────────────────────────

    fn render_header(&self) -> Div {
        div()
            .w_full()
            .h(px(48.0))
            .px(px(16.0))
            .flex()
            .items_center()
            .justify_between()
            .bg(theme::SURFACE)
            .border_b_1()
            .border_color(theme::TEXT_MUTED)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(Icon::new(IconName::SquareTerminal).size_4().text_color(theme::PRIMARY))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Agent Terminal"),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(theme::SIDEBAR_BG)
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(self.agent_name.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("{} messages", self.messages.len())),
            )
    }

    fn render_activity(activity: &ToolActivity) -> Div {
        let status_icon = if activity.done {
            IconName::Check
        } else {
            IconName::LoaderCircle
        };

        let status_color = if activity.done {
            theme::SUCCESS
        } else {
            theme::WARNING
        };

        div()
            .w_full()
            .px(px(10.0))
            .py(px(5.0))
            .my(px(2.0))
            .rounded(px(6.0))
            .bg(hsla(0.0, 0.0, 0.1, 1.0))
            .border_1()
            .border_color(hsla(0.0, 0.0, 0.18, 1.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(Icon::new(status_icon).size_3().text_color(status_color))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(activity.title.clone()),
            )
            .when(activity.done, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme::SUCCESS)
                        .child("✓"),
                )
            })
    }

    fn render_message(&self, msg: &TerminalMessage) -> Div {
        let (icon, color, label) = match msg.role {
            MessageRole::User => (IconName::User, theme::PRIMARY, "You"),
            MessageRole::Agent => (IconName::Bot, theme::SUCCESS, "Agent"),
            MessageRole::System => (IconName::Info, theme::TEXT_MUTED, "System"),
        };

        let bg = match msg.role {
            MessageRole::User => theme::PRIMARY.opacity(0.08),
            MessageRole::Agent => theme::SUCCESS.opacity(0.06),
            MessageRole::System => theme::SIDEBAR_BG.opacity(0.5),
        };

        let mut message_div = div()
            .w_full()
            .overflow_x_hidden()
            .p(px(12.0))
            .bg(bg)
            // Header row: icon + role + timestamp
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(Icon::new(icon).size_4().text_color(color))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(label),
                    )
                    .when(!msg.timestamp.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child(msg.timestamp.clone()),
                        )
                    }),
            );

        // Tool activities — inline, compact, Copilot-style
        if !msg.activities.is_empty() {
            let mut activities_container = div()
                .mt(px(6.0))
                .mb(px(4.0))
                .flex()
                .flex_col()
                .gap(px(2.0));

            for activity in &msg.activities {
                activities_container = activities_container.child(Self::render_activity(activity));
            }

            message_div = message_div.child(activities_container);
        }

        // Message content
        if !msg.content.is_empty() {
            message_div = message_div.child(
                div()
                    .mt(px(4.0))
                    .text_sm()
                    .text_color(theme::TEXT_PRIMARY)
                    .child(if msg.role == MessageRole::Agent {
                        markdown::render_markdown(&msg.content)
                    } else {
                        div().child(msg.content.clone())
                    }),
            );
        }

        message_div
    }
}

impl Render for AgentTerminalScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.input_state.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(format!("Send a message to {}...", self.agent_name))
            });
            cx.subscribe_in(&input, window, |this: &mut Self, _input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.send_prompt(window, cx);
                }
            })
            .detach();
            self.input_state = Some(input);
        }

        let messages: Vec<Div> = self.messages.iter().map(|m| self.render_message(m)).collect();

        div()
            .size_full()
            .v_flex()
            .child(self.render_header())
            .child(
                div()
                    .id("terminal-messages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .bg(theme::BACKGROUND)
                    .children(messages),
            )
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .p(px(12.0))
                    .bg(theme::SURFACE)
                    .border_t_1()
                    .border_color(theme::TEXT_MUTED)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div().flex_1().child(
                            Input::new(self.input_state.as_ref().unwrap())
                                .appearance(false)
                                .cleanable(true),
                        ),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(if self.is_sending {
                                theme::SIDEBAR_BG
                            } else {
                                theme::PRIMARY
                            })
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .when(self.is_sending, |d| {
                                d.child(
                                    div()
                                        .text_sm()
                                        .text_color(theme::TEXT_MUTED)
                                        .child("Sending..."),
                                )
                            })
                            .when(!self.is_sending, |d| {
                                d.child(Icon::new(IconName::ArrowUp).size_4().text_color(gpui::white()))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(gpui::white())
                                            .child("Send"),
                                    )
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                                        this.send_prompt(window, cx);
                                    }))
                            }),
                    ),
            )
    }
}
