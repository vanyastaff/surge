use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
use crate::theme;

/// A message in the terminal conversation.
#[derive(Debug, Clone)]
pub struct TerminalMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Agent,
    System,
}

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
                TerminalMessage {
                    role: MessageRole::System,
                    content: format!("Terminal ready. Agent: {agent_name}. Type a message and press Enter to send."),
                    timestamp: String::new(),
                },
            ],
            input_state: None,
            is_sending: false,
            agent_name,
            session_active: false,
        }
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input_entity) = &self.input_state else { return; };
        let input = input_entity.read(cx).value().to_string();
        let input = input.trim().to_string();
        if input.is_empty() || self.is_sending {
            return;
        }

        // Add user message.
        self.messages.push(TerminalMessage {
            role: MessageRole::User,
            content: input.clone(),
            timestamp: "now".into(),
        });
        // Clear input.
        input_entity.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.is_sending = true;
        cx.notify();

        // Get pool from state.
        let pool = {
            let state = self.state.read(cx);
            state.agent_pool.clone()
        };

        let Some(pool) = pool else {
            self.messages.push(TerminalMessage {
                role: MessageRole::System,
                content: "No agent pool configured. Open a project with surge.toml first.".into(),
                timestamp: String::new(),
            });
            self.is_sending = false;
            cx.notify();
            return;
        };

        let agent_name = self.agent_name.clone();
        let cwd = {
            let state = self.state.read(cx);
            state.project_path.clone().unwrap_or_else(|| std::path::PathBuf::from("."))
        };

        // Subscribe to events for streaming chunks
        let mut event_rx = pool.subscribe();

        // Spawn async task — pool is Send-safe, no separate thread needed.
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
                            this.messages.push(TerminalMessage {
                                role: MessageRole::System,
                                content: err,
                                timestamp: String::new(),
                            });
                            this.is_sending = false;
                            cx.notify();
                        });
                    }).ok();
                    return;
                }
            };

            // Add empty agent message that we'll append chunks to
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.messages.push(TerminalMessage {
                        role: MessageRole::Agent,
                        content: String::new(),
                        timestamp: "just now".into(),
                    });
                    cx.notify();
                });
            }).ok();

            // Spawn event listener for streaming chunks
            let this_for_events = this.clone();
            let event_task = cx.spawn(async move |cx: &mut AsyncApp| {
                while let Ok(event) = event_rx.recv().await {
                    if let surge_core::SurgeEvent::AgentMessageChunk { text, .. } = event {
                        let _ = cx.update(|cx| {
                            let _ = this_for_events.update(cx, |this, cx| {
                                if let Some(last) = this.messages.last_mut() {
                                    if last.role == MessageRole::Agent {
                                        last.content.push_str(&text);
                                        cx.notify();
                                    }
                                }
                            });
                        });
                    }
                }
            });

            // Send prompt
            let content = vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent::new(input),
            )];

            let result = pool.prompt(&session, content).await;

            // Drop event task
            drop(event_task);

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(_response) => {
                            // If no streaming text was received, show stop reason
                            if let Some(last) = this.messages.last() {
                                if last.role == MessageRole::Agent && last.content.is_empty() {
                                    if let Some(msg) = this.messages.last_mut() {
                                        msg.content = "(Agent completed with no text output)".into();
                                    }
                                }
                            }
                            this.session_active = true;
                        }
                        Err(err) => {
                            this.messages.push(TerminalMessage {
                                role: MessageRole::System,
                                content: format!("Error: {err}"),
                                timestamp: String::new(),
                            });
                        }
                    }
                    this.is_sending = false;
                    cx.notify();
                });
            }).ok();
        })
        .detach();
    }

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

        div()
            .w_full()
            .overflow_x_hidden()
            .p(px(12.0))
            .bg(bg)
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
            )
            .child(
                div()
                    .mt(px(4.0))
                    .text_sm()
                    .text_color(theme::TEXT_PRIMARY)
                    .child(msg.content.clone()),
            )
    }
}

impl Render for AgentTerminalScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Lazily create InputState (needs Window).
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
            // Header
            .child(self.render_header())
            // Messages area
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
            // Input area — pinned to bottom
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
