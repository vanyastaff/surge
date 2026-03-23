use std::sync::Arc;

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

/// Agent Terminal screen — send prompts, see responses.
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
            input_state: None, // created lazily in render (needs Window)
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

        // Run ACP on a background thread with its own tokio runtime + LocalSet.
        cx.spawn(async |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Run blocking ACP call on a background thread (needs its own tokio LocalSet).
            let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
            std::thread::spawn(move || {
                let result = send_prompt_blocking(pool, agent_name, cwd, input);
                let _ = tx.send(result);
            });
            let result = match rx.await {
                Ok(r) => r,
                Err(_) => Err("Thread died".into()),
            };

            cx.update(|cx| {
                this.update(cx, |this: &mut Self, cx| {
                    match result {
                        Ok(response_text) => {
                            this.messages.push(TerminalMessage {
                                role: MessageRole::Agent,
                                content: response_text,
                                timestamp: "just now".into(),
                            });
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
                })
            }).ok();
        })
        .detach();
    }

    fn render_message(&self, msg: &TerminalMessage) -> Div {
        let (icon, color, label) = match msg.role {
            MessageRole::User => (IconName::User, theme::PRIMARY, "You"),
            MessageRole::Agent => (IconName::Bot, theme::SUCCESS, "Agent"),
            MessageRole::System => (IconName::Info, theme::TEXT_MUTED, "System"),
        };

        div()
            .w_full()
            .v_flex()
            .gap(px(4.0))
            .p_3()
            .rounded_lg()
            .bg(match msg.role {
                MessageRole::User => theme::PRIMARY.opacity(0.05),
                MessageRole::Agent => theme::SURFACE,
                MessageRole::System => theme::TEXT_MUTED.opacity(0.05),
            })
            // Header: role + timestamp
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(icon).size_3p5().text_color(color))
                    .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(color).child(label.to_string()))
                    .when(!msg.timestamp.is_empty(), |el: Div| {
                        el.child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.4)).child(msg.timestamp.clone()))
                    }),
            )
            // Content
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_PRIMARY)
                    .line_height(relative(1.5))
                    .child(msg.content.clone()),
            )
    }

    fn render_input(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .w_full()
            .h_flex()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(theme::TEXT_MUTED.opacity(0.08));

        // Real Input component
        if let Some(input_state) = &self.input_state {
            row = row.child(
                div().flex_1().child(Input::new(input_state)),
            );
        }

        // Send button — always active (shows error if no pool)
        row = row.child(
            div()
                .id("send-btn")
                .cursor_pointer()
                .h_flex()
                .gap_1()
                .items_center()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(if self.is_sending {
                    theme::TEXT_MUTED.opacity(0.1)
                } else {
                    theme::PRIMARY
                })
                .text_color(if self.is_sending {
                    theme::TEXT_MUTED
                } else {
                    hsla(0.0, 0.0, 1.0, 1.0)
                })
                .when(!self.is_sending, |el: Stateful<Div>| {
                    el.hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.85)))
                        .on_click(cx.listener(|this, _e, window, cx| {
                            this.send_prompt(window, cx);
                        }))
                })
                .child(if self.is_sending {
                    Icon::new(IconName::Loader).size_4().text_color(theme::TEXT_MUTED)
                } else {
                    Icon::new(IconName::ArrowUp).size_4().text_color(hsla(0.0, 0.0, 1.0, 1.0))
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(if self.is_sending { "Sending..." } else { "Send" }),
                ),
        );

        row
    }

    fn render_header(&self) -> Div {
        div()
            .h_flex()
            .justify_between()
            .items_center()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme::TEXT_MUTED.opacity(0.08))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::SquareTerminal).size_4().text_color(theme::PRIMARY))
                    .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child("Agent Terminal".to_string()))
                    .child(
                        div()
                            .text_xs()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(if self.session_active { theme::SUCCESS.opacity(0.12) } else { theme::TEXT_MUTED.opacity(0.1) })
                            .text_color(if self.session_active { theme::SUCCESS } else { theme::TEXT_MUTED })
                            .child(self.agent_name.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED.opacity(0.5))
                    .child(format!("{} messages", self.messages.len())),
            )
    }
}

/// Send prompt to agent on a dedicated tokio runtime with LocalSet.
/// This is needed because ACP library uses spawn_local internally.
fn send_prompt_blocking(
    pool: Arc<surge_acp::AgentPool>,
    agent_name: String,
    cwd: std::path::PathBuf,
    prompt: String,
) -> Result<String, String> {
    // Create a dedicated runtime with LocalSet for ACP's spawn_local requirement.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime error: {e}"))?;

    let local = tokio::task::LocalSet::new();

    local.block_on(&rt, async move {
        // Create session.
        let session = pool
            .create_session(Some(&agent_name), None, &cwd)
            .await
            .map_err(|e| format!("Session error: {e}"))?;

        // Send prompt.
        let content = vec![agent_client_protocol::ContentBlock::Text(
            agent_client_protocol::TextContent::new(prompt),
        )];

        let response = pool
            .prompt(&session, content)
            .await
            .map_err(|e| format!("Prompt error: {e}"))?;

        Ok(format!("Agent completed (stop_reason: {:?})", response.stop_reason))
    })
}

impl Render for AgentTerminalScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Lazily create InputState (needs Window).
        if self.input_state.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(format!("Send a message to {}...", self.agent_name))
            });
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
                    .v_flex()
                    .gap_2()
                    .p_3()
                    .overflow_y_scroll()
                    .children(messages)
                    // Sending indicator
                    .when(self.is_sending, |el: Stateful<Div>| {
                        el.child(
                            div()
                                .h_flex()
                                .gap_2()
                                .items_center()
                                .p_3()
                                .child(Icon::new(IconName::Loader).size_4().text_color(theme::WARNING))
                                .child(div().text_sm().text_color(theme::TEXT_MUTED).child("Agent is thinking...".to_string())),
                        )
                    }),
            )
            // Input area
            .child(self.render_input(cx))
    }
}
