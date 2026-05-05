use std::collections::HashMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
use crate::markdown;
use crate::theme;

// ── Data model ──────────────────────────────────────────────────────

/// A tool call activity with rich ACP metadata.
#[derive(Debug, Clone)]
pub struct ToolCallBlock {
    pub call_id: String,
    pub title: String,
    pub kind: surge_core::ToolKind,
    pub status: surge_core::ToolCallStatus,
    pub locations: Vec<surge_core::ToolLocation>,
    pub diffs: Vec<surge_core::ToolDiff>,
    pub raw_input: Option<String>,
    pub raw_output: Option<String>,
}

/// A thinking block accumulated from AgentThoughtChunk events.
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub text: String,
}

/// A permission request from the agent.
#[derive(Debug, Clone)]
pub struct PermissionBlock {
    pub description: String,
    pub tool_call_id: String,
    pub options: Vec<String>,
    pub resolved: Option<bool>,
}

/// Items that appear in the conversation timeline.
#[derive(Debug, Clone)]
pub enum ChatItem {
    /// User prompt message.
    UserMessage { content: String },
    /// Agent text response (markdown).
    AgentText { content: String },
    /// Agent thinking/reasoning (collapsible).
    Thinking(ThinkingBlock),
    /// A tool call with rich metadata (collapsible).
    ToolCall(ToolCallBlock),
    /// Agent execution plan (collapsible).
    Plan { entries: Vec<surge_core::PlanEntry> },
    /// Permission request from agent.
    Permission(PermissionBlock),
    /// System message (errors, status).
    System { content: String },
}

// ── Screen ──────────────────────────────────────────────────────────

/// Agent Terminal screen — IDE-style chat with streaming responses.
pub struct AgentTerminalScreen {
    state: Entity<AppState>,
    items: Vec<ChatItem>,
    input_state: Option<Entity<InputState>>,
    is_sending: bool,
    agent_name: String,
    session: Option<surge_acp::SessionHandle>,
    /// Collapse state: key → is_collapsed.
    collapsed: HashMap<String, bool>,
    scroll_handle: ScrollHandle,
}

impl AgentTerminalScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        let agent_name = {
            let s = state.read(_cx);
            s.config
                .as_ref()
                .map(|c| c.default_agent.clone())
                .unwrap_or_else(|| "claude-acp".to_string())
        };

        Self {
            state,
            items: vec![ChatItem::System {
                content: format!(
                    "Terminal ready. Agent: {agent_name}. Type a message and press Enter."
                ),
            }],
            input_state: None,
            is_sending: false,
            agent_name,
            session: None,
            collapsed: HashMap::new(),
            scroll_handle: ScrollHandle::new(),
        }
    }

    fn is_collapsed(&self, key: &str, default: bool) -> bool {
        self.collapsed.get(key).copied().unwrap_or(default)
    }

    fn toggle_collapsed(&mut self, key: &str, default: bool) {
        let current = self.is_collapsed(key, default);
        self.collapsed.insert(key.to_string(), !current);
    }

    /// Find or create the current AgentText item at the tail.
    fn ensure_agent_text(&mut self) -> &mut String {
        let needs_new = !matches!(self.items.last(), Some(ChatItem::AgentText { .. }));
        if needs_new {
            self.items.push(ChatItem::AgentText {
                content: String::new(),
            });
        }
        let Some(ChatItem::AgentText { content }) = self.items.last_mut() else {
            unreachable!("just pushed AgentText");
        };
        content
    }

    /// Find or create a ThinkingBlock at the tail.
    fn ensure_thinking(&mut self) -> &mut ThinkingBlock {
        let needs_new = !matches!(self.items.last(), Some(ChatItem::Thinking(_)));
        if needs_new {
            self.items.push(ChatItem::Thinking(ThinkingBlock {
                text: String::new(),
            }));
        }
        let Some(ChatItem::Thinking(block)) = self.items.last_mut() else {
            unreachable!("just pushed Thinking");
        };
        block
    }

    /// Find a ToolCallBlock by call_id.
    fn find_tool_call_mut(&mut self, call_id: &str) -> Option<&mut ToolCallBlock> {
        self.items.iter_mut().find_map(|item| match item {
            ChatItem::ToolCall(tc) if tc.call_id == call_id => Some(tc),
            _ => None,
        })
    }

    /// Find the last unresolved PermissionBlock.
    fn find_pending_permission_mut(&mut self) -> Option<&mut PermissionBlock> {
        self.items.iter_mut().rev().find_map(|item| match item {
            ChatItem::Permission(p) if p.resolved.is_none() => Some(p),
            _ => None,
        })
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input_entity) = &self.input_state else {
            return;
        };
        let input = input_entity.read(cx).value().to_string();
        let input = input.trim().to_string();
        if input.is_empty() || self.is_sending {
            return;
        }

        self.items.push(ChatItem::UserMessage {
            content: input.clone(),
        });
        input_entity.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.is_sending = true;
        self.scroll_handle.scroll_to_bottom();
        cx.notify();

        let pool = {
            let state = self.state.read(cx);
            state.agent_pool.clone()
        };

        let Some(pool) = pool else {
            self.items.push(ChatItem::System {
                content: "No agent pool configured. Open a project with surge.toml first."
                    .to_string(),
            });
            self.is_sending = false;
            cx.notify();
            return;
        };

        let agent_name = self.agent_name.clone();
        let cwd = {
            let state = self.state.read(cx);
            state
                .project_path
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        };

        let mut event_rx = pool.subscribe();
        let existing_session = self.session.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let session = if let Some(s) = existing_session {
                s
            } else {
                match pool
                    .create_session(Some(&agent_name), None, &cwd)
                    .await
                {
                    Ok(s) => {
                        let s_clone = s.clone();
                        cx.update(|cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.session = Some(s_clone);
                                cx.notify();
                            });
                        })
                        .ok();
                        s
                    }
                    Err(e) => {
                        let err = format!("Session error: {e}");
                        cx.update(|cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.items.push(ChatItem::System { content: err });
                                this.is_sending = false;
                                cx.notify();
                            });
                        })
                        .ok();
                        return;
                    }
                }
            };

            let this_for_events = this.clone();
            let event_task = cx.spawn(async move |cx: &mut AsyncApp| {
                while let Ok(event) = event_rx.recv().await {
                    match event {
                        surge_core::SurgeEvent::AgentMessageChunk { text, .. } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    this.ensure_agent_text().push_str(&text);
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::AgentThoughtChunk { text, .. } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    this.ensure_thinking().text.push_str(&text);
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::ToolCallStarted {
                            call_id,
                            title,
                            kind,
                            locations,
                            raw_input,
                            ..
                        } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    this.collapsed.insert(call_id.clone(), true);
                                    this.items.push(ChatItem::ToolCall(ToolCallBlock {
                                        call_id,
                                        title,
                                        kind,
                                        status: surge_core::ToolCallStatus::InProgress,
                                        locations,
                                        raw_input,
                                        diffs: Vec::new(),
                                        raw_output: None,
                                    }));
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::ToolCallUpdated {
                            call_id,
                            status,
                            title,
                            diffs,
                            locations,
                            raw_output,
                            ..
                        } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    if let Some(tc) = this.find_tool_call_mut(&call_id) {
                                        if let Some(s) = status {
                                            tc.status = s;
                                        }
                                        if let Some(t) = title {
                                            tc.title = t;
                                        }
                                        if !diffs.is_empty() {
                                            tc.diffs.extend(diffs);
                                        }
                                        if !locations.is_empty() {
                                            tc.locations = locations;
                                        }
                                        if raw_output.is_some() {
                                            tc.raw_output = raw_output;
                                        }
                                    }
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::PlanUpdated { entries, .. } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    let found = this.items.iter_mut().rev().any(|item| {
                                        if let ChatItem::Plan {
                                            entries: existing, ..
                                        } = item
                                        {
                                            *existing = entries.clone();
                                            true
                                        } else {
                                            false
                                        }
                                    });
                                    if !found {
                                        this.items.push(ChatItem::Plan { entries });
                                    }
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::PermissionRequested {
                            description,
                            tool_call_id,
                            options,
                            ..
                        } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    this.items.push(ChatItem::Permission(PermissionBlock {
                                        description,
                                        tool_call_id,
                                        options,
                                        resolved: None,
                                    }));
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                });
                            });
                        }
                        surge_core::SurgeEvent::PermissionResolved { granted, .. } => {
                            let _ = cx.update(|cx| {
                                let _ = this_for_events.update(cx, |this, cx| {
                                    if let Some(p) = this.find_pending_permission_mut() {
                                        p.resolved = Some(granted);
                                    }
                                    cx.notify();
                                });
                            });
                        }
                        _ => {}
                    }
                }
            });

            let content = vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent::new(input),
            )];

            let result = pool.prompt(&session, content).await;
            drop(event_task);

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(_) => {
                            let has_content = this.items.iter().rev().take(20).any(|item| {
                                matches!(item, ChatItem::AgentText { content } if !content.is_empty())
                                    || matches!(item, ChatItem::ToolCall(_))
                            });
                            if !has_content {
                                this.items.push(ChatItem::System {
                                    content: "(Agent completed with no output)".to_string(),
                                });
                            }
                        }
                        Err(err) => {
                            this.items.push(ChatItem::System {
                                content: format!("Error: {err}"),
                            });
                        }
                    }
                    this.is_sending = false;
                    this.scroll_handle.scroll_to_bottom();
                    cx.notify();
                });
            })
            .ok();
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
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::text_muted().opacity(0.3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .size_4()
                            .text_color(theme::primary()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child("Agent Terminal"),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(theme::sidebar_bg())
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(self.agent_name.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(format!("{} items", self.items.len())),
            )
    }

    /// Render all items sequentially.
    fn render_all_items(&self, cx: &mut Context<Self>) -> Vec<Div> {
        (0..self.items.len())
            .map(|i| match &self.items[i] {
                ChatItem::UserMessage { content } => render_user_message(content),
                ChatItem::AgentText { content } => render_agent_text(content),
                ChatItem::Thinking(block) => self.render_thinking(i, block, cx),
                ChatItem::ToolCall(tc) => self.render_tool_call(tc, cx),
                ChatItem::Plan { entries } => self.render_plan(i, entries, cx),
                ChatItem::Permission(perm) => render_permission(perm),
                ChatItem::System { content } => render_system(content),
            })
            .collect()
    }

    /// Render a tool call — collapsed: just status + title; expanded: request + response.
    fn render_tool_call(&self, tc: &ToolCallBlock, cx: &mut Context<Self>) -> Div {
        let is_collapsed = self.is_collapsed(&tc.call_id, true);

        let (status_icon, status_color) = match tc.status {
            surge_core::ToolCallStatus::Completed => (IconName::Check, theme::success()),
            surge_core::ToolCallStatus::Failed => (IconName::CircleX, theme::error()),
            surge_core::ToolCallStatus::InProgress => (IconName::LoaderCircle, theme::warning()),
            surge_core::ToolCallStatus::Pending => (IconName::Loader, theme::text_muted()),
        };
        let kind_icon = tool_kind_icon(&tc.kind);
        let chevron = if is_collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };

        let mut container = div().w_full().px(px(12.0)).py(px(1.0));

        // Header row — always visible, clickable
        let call_id = tc.call_id.clone();
        container = container.child(
            div()
                .id(SharedString::from(format!("tc-{}", tc.call_id)))
                .w_full()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .bg(hsla(0.0, 0.0, 0.1, 1.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_collapsed(&call_id, true);
                    cx.notify();
                }))
                .child(Icon::new(status_icon).size_3().text_color(status_color))
                .child(
                    Icon::new(kind_icon)
                        .size_3()
                        .text_color(theme::text_muted()),
                )
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(tc.title.clone()),
                )
                .children(tc.locations.iter().map(|loc| {
                    let filename = loc
                        .path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| loc.path.display().to_string());
                    let label = match loc.line {
                        Some(line) => format!("{filename}:{line}"),
                        None => filename,
                    };
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .bg(theme::sidebar_bg())
                        .text_xs()
                        .text_color(theme::warning())
                        .font_family("Consolas")
                        .child(label)
                }))
                .child(Icon::new(chevron).size_3().text_color(theme::text_muted())),
        );

        // Expanded details — request, response, diffs
        if !is_collapsed {
            let mut details = div()
                .w_full()
                .ml(px(20.0))
                .mr(px(8.0))
                .mt(px(2.0))
                .mb(px(2.0))
                .flex()
                .flex_col()
                .gap(px(2.0));

            // Request (raw_input)
            if let Some(input) = &tc.raw_input {
                details = details.child(render_json_block("Request", input));
            }

            // Response (raw_output)
            if let Some(output) = &tc.raw_output {
                details = details.child(render_json_block("Response", output));
            }

            // Diffs
            for diff in &tc.diffs {
                details = details.child(render_diff(diff));
            }

            container = container.child(details);
        }

        container
    }

    fn render_thinking(&self, idx: usize, block: &ThinkingBlock, cx: &mut Context<Self>) -> Div {
        let key = format!("thinking-{idx}");
        let is_collapsed = self.is_collapsed(&key, false);
        let chevron = if is_collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };

        let mut container = div().w_full().px(px(12.0)).py(px(1.0));

        container = container.child(
            div()
                .id(SharedString::from(key.clone()))
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .on_click({
                    let key = key.clone();
                    cx.listener(move |this, _event, _window, cx| {
                        this.toggle_collapsed(&key, false);
                        cx.notify();
                    })
                })
                .child(Icon::new(chevron).size_3().text_color(theme::text_muted()))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("Thinking..."),
                ),
        );

        if !is_collapsed {
            container = container.child(
                div()
                    .pl(px(18.0))
                    .text_xs()
                    .text_color(theme::text_muted().opacity(0.7))
                    .font_family("Consolas")
                    .child(block.text.clone()),
            );
        }

        container
    }

    fn render_plan(
        &self,
        idx: usize,
        entries: &[surge_core::PlanEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let key = format!("plan-{idx}");
        let is_collapsed = self.is_collapsed(&key, false);
        let chevron = if is_collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };

        let completed = entries
            .iter()
            .filter(|e| e.status == surge_core::PlanStatus::Completed)
            .count();
        let total = entries.len();

        let mut container = div().w_full().px(px(12.0)).py(px(1.0));

        container = container.child(
            div()
                .id(SharedString::from(key.clone()))
                .flex()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_click({
                    let key = key.clone();
                    cx.listener(move |this, _event, _window, cx| {
                        this.toggle_collapsed(&key, false);
                        cx.notify();
                    })
                })
                .child(Icon::new(chevron).size_3().text_color(theme::primary()))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::primary())
                        .child(format!("Plan ({completed}/{total})")),
                ),
        );

        if !is_collapsed {
            let mut list = div()
                .pl(px(20.0))
                .mt(px(2.0))
                .flex()
                .flex_col()
                .gap(px(1.0));

            for entry in entries {
                let (icon, color) = match entry.status {
                    surge_core::PlanStatus::Completed => (IconName::CircleCheck, theme::success()),
                    surge_core::PlanStatus::InProgress => {
                        (IconName::LoaderCircle, theme::warning())
                    },
                    surge_core::PlanStatus::Pending => (IconName::Dash, theme::text_muted()),
                };

                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(Icon::new(icon).size_3().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme::text_primary())
                            .child(entry.content.clone()),
                    );

                if entry.priority == surge_core::PlanPriority::High {
                    row = row.child(
                        div()
                            .px(px(4.0))
                            .rounded(px(2.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::error())
                            .child("!"),
                    );
                }

                list = list.child(row);
            }

            container = container.child(list);
        }

        container
    }
}

impl Render for AgentTerminalScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.input_state.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(format!("Send a message to {}...", self.agent_name))
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.send_prompt(window, cx);
                    }
                },
            )
            .detach();
            self.input_state = Some(input);
        }

        let items = self.render_all_items(cx);

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
                    .track_scroll(&self.scroll_handle)
                    .bg(theme::background())
                    .children(items),
            )
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .p(px(12.0))
                    .bg(theme::surface())
                    .border_t_1()
                    .border_color(theme::text_muted().opacity(0.3))
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
                                theme::sidebar_bg()
                            } else {
                                theme::primary()
                            })
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .when(self.is_sending, |d| {
                                d.child(
                                    div()
                                        .text_sm()
                                        .text_color(theme::text_muted())
                                        .child("Sending..."),
                                )
                            })
                            .when(!self.is_sending, |d| {
                                d.child(
                                    Icon::new(IconName::ArrowUp)
                                        .size_4()
                                        .text_color(gpui::white()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(gpui::white())
                                        .child("Send"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.send_prompt(window, cx);
                                    }),
                                )
                            }),
                    ),
            )
    }
}

// ── Stateless render functions ──────────────────────────────────────

fn render_user_message(content: &str) -> Div {
    div()
        .w_full()
        .px(px(12.0))
        .py(px(6.0))
        .bg(theme::primary().opacity(0.05))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .mb(px(2.0))
                .child(
                    Icon::new(IconName::User)
                        .size_3()
                        .text_color(theme::primary()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::primary())
                        .child("You"),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::text_primary())
                .child(content.to_string()),
        )
}

fn render_agent_text(content: &str) -> Div {
    if content.is_empty() {
        return div();
    }
    div()
        .w_full()
        .overflow_x_hidden()
        .px(px(12.0))
        .py(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .mb(px(2.0))
                .child(
                    Icon::new(IconName::Bot)
                        .size_3()
                        .text_color(theme::success()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::success())
                        .child("Agent"),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::text_primary())
                .child(markdown::render_markdown(content)),
        )
}

fn render_permission(perm: &PermissionBlock) -> Div {
    let (icon, status_text, status_color) = match perm.resolved {
        None => (IconName::TriangleAlert, "Awaiting", theme::warning()),
        Some(true) => (IconName::Check, "Granted", theme::success()),
        Some(false) => (IconName::CircleX, "Denied", theme::error()),
    };

    div().w_full().px(px(12.0)).py(px(1.0)).child(
        div()
            .w_full()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(status_color.opacity(0.06))
            .flex()
            .items_center()
            .gap(px(5.0))
            .child(Icon::new(icon).size_3().text_color(status_color))
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(perm.description.clone()),
            )
            .child(
                div()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .bg(status_color.opacity(0.15))
                    .text_xs()
                    .text_color(status_color)
                    .child(status_text),
            ),
    )
}

fn render_system(content: &str) -> Div {
    div()
        .w_full()
        .px(px(12.0))
        .py(px(2.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            Icon::new(IconName::Info)
                .size_3()
                .text_color(theme::text_muted()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child(content.to_string()),
        )
}

/// Map ToolKind to an appropriate icon.
fn tool_kind_icon(kind: &surge_core::ToolKind) -> IconName {
    match kind {
        surge_core::ToolKind::Read => IconName::File,
        surge_core::ToolKind::Edit => IconName::Replace,
        surge_core::ToolKind::Delete => IconName::Delete,
        surge_core::ToolKind::Move => IconName::FolderOpen,
        surge_core::ToolKind::Search => IconName::Search,
        surge_core::ToolKind::Execute => IconName::SquareTerminal,
        surge_core::ToolKind::Think => IconName::Loader,
        surge_core::ToolKind::Fetch => IconName::Globe,
        surge_core::ToolKind::SwitchMode => IconName::Settings,
        surge_core::ToolKind::Other => IconName::Asterisk,
    }
}

/// Render a file diff with before/after display.
/// Render a JSON block with a label (for request/response).
fn render_json_block(label: &str, json: &str) -> Div {
    let label = label.to_string();
    let display = if json.len() > 2000 {
        format!("{}...", &json[..2000])
    } else {
        json.to_string()
    };

    div()
        .w_full()
        .rounded(px(4.0))
        .bg(hsla(0.0, 0.0, 0.08, 1.0))
        .overflow_x_hidden()
        .child(
            div()
                .px(px(8.0))
                .py(px(2.0))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .font_family("Consolas")
                .text_xs()
                .text_color(hsla(0.0, 0.0, 0.75, 1.0))
                .child(display),
        )
}

fn render_diff(diff: &surge_core::ToolDiff) -> Div {
    let filename = diff
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| diff.path.display().to_string());

    let mut container = div()
        .w_full()
        .rounded(px(6.0))
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.2, 1.0))
        .overflow_x_hidden()
        .mb(px(4.0));

    // File header
    container = container.child(
        div()
            .px(px(10.0))
            .py(px(4.0))
            .bg(hsla(0.0, 0.0, 0.12, 1.0))
            .border_b_1()
            .border_color(hsla(0.0, 0.0, 0.2, 1.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                Icon::new(IconName::File)
                    .size_3()
                    .text_color(theme::text_muted()),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_primary())
                    .font_family("Consolas")
                    .child(filename),
            ),
    );

    // Removed lines (red)
    if let Some(old) = &diff.old_text {
        if !old.is_empty() {
            let mut old_block = div().w_full();
            for line in old.lines() {
                old_block = old_block.child(
                    div()
                        .px(px(10.0))
                        .py(px(1.0))
                        .bg(hsla(0.0, 0.4, 0.15, 1.0))
                        .font_family("Consolas")
                        .text_xs()
                        .text_color(hsla(0.0, 0.7, 0.7, 1.0))
                        .child(format!("- {line}")),
                );
            }
            container = container.child(old_block);
        }
    }

    // Added lines (green)
    if !diff.new_text.is_empty() {
        let mut new_block = div().w_full();
        for line in diff.new_text.lines() {
            new_block = new_block.child(
                div()
                    .px(px(10.0))
                    .py(px(1.0))
                    .bg(hsla(0.33, 0.4, 0.15, 1.0))
                    .font_family("Consolas")
                    .text_xs()
                    .text_color(hsla(0.33, 0.7, 0.7, 1.0))
                    .child(format!("+ {line}")),
            );
        }
        container = container.child(new_block);
    }

    container
}
