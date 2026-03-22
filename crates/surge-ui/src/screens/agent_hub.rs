use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

/// Agent info for display.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub connected: bool,
    pub model: Option<String>,
    pub capabilities: Vec<String>,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub error_rate: f32,
    pub avg_latency_ms: u32,
}

/// Routing rule entry.
#[derive(Debug, Clone)]
pub struct RoutingRule {
    pub pattern: String,
    pub agent: String,
}

/// Agent Hub screen.
pub struct AgentHubScreen {
    agents: Vec<AgentInfo>,
    routing_rules: Vec<RoutingRule>,
    selected_agent: Option<usize>,
}

impl AgentHubScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            agents: vec![
                AgentInfo {
                    name: "claude-code".into(),
                    display_name: "Claude Code".into(),
                    description: "Anthropic's autonomous coding agent".into(),
                    connected: true,
                    model: Some("claude-sonnet-4-5-20250514".into()),
                    capabilities: vec!["Code".into(), "Plan".into(), "Review".into(), "Test".into()],
                    requests_today: 42,
                    tokens_today: 156_000,
                    error_rate: 0.02,
                    avg_latency_ms: 1200,
                },
                AgentInfo {
                    name: "copilot-cli".into(),
                    display_name: "GitHub Copilot CLI".into(),
                    description: "GitHub's AI coding assistant".into(),
                    connected: false,
                    model: Some("gpt-4o".into()),
                    capabilities: vec!["Code".into(), "Chat".into()],
                    requests_today: 0,
                    tokens_today: 0,
                    error_rate: 0.0,
                    avg_latency_ms: 0,
                },
                AgentInfo {
                    name: "zed-agent".into(),
                    display_name: "Zed Agent".into(),
                    description: "Zed editor's built-in AI agent".into(),
                    connected: false,
                    model: None,
                    capabilities: vec!["Code".into()],
                    requests_today: 0,
                    tokens_today: 0,
                    error_rate: 0.0,
                    avg_latency_ms: 0,
                },
            ],
            routing_rules: vec![
                RoutingRule { pattern: "*.rs".into(), agent: "claude-code".into() },
                RoutingRule { pattern: "*.ts".into(), agent: "claude-code".into() },
                RoutingRule { pattern: "*.py".into(), agent: "claude-code".into() },
            ],
            selected_agent: Some(0),
        }
    }

    fn render_agent_card(&self, idx: usize, agent: &AgentInfo, cx: &mut Context<Self>) -> Stateful<Div> {
        let is_selected = self.selected_agent == Some(idx);
        let status_color = if agent.connected { theme::SUCCESS } else { theme::TEXT_MUTED };

        div()
            .id(SharedString::from(format!("agent-{}", agent.name)))
            .h_flex()
            .gap_3()
            .p_3()
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(if is_selected { theme::PRIMARY } else { theme::TEXT_MUTED.opacity(0.1) })
            .bg(if is_selected { theme::PRIMARY.opacity(0.05) } else { theme::SURFACE })
            .hover(|s: StyleRefinement| s.border_color(theme::PRIMARY.opacity(0.3)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.selected_agent = Some(idx);
                cx.notify();
            }))
            // Status dot
            .child(
                div()
                    .w(px(10.0))
                    .h(px(10.0))
                    .rounded_full()
                    .bg(status_color)
                    .mt_1(),
            )
            // Info
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(agent.display_name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(agent.description.clone()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .children(
                                agent.capabilities.iter().map(|cap| {
                                    div()
                                        .text_xs()
                                        .px(px(6.0))
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(theme::PRIMARY.opacity(0.1))
                                        .text_color(theme::PRIMARY)
                                        .child(cap.clone())
                                }),
                            ),
                    ),
            )
    }

    fn render_detail_panel(&self) -> Div {
        let Some(idx) = self.selected_agent else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::TEXT_MUTED)
                .child("Select an agent".to_string());
        };

        let agent = &self.agents[idx];
        let status_text = if agent.connected { "Connected" } else { "Disconnected" };
        let status_color = if agent.connected { theme::SUCCESS } else { theme::TEXT_MUTED };

        div()
            .flex_1()
            .v_flex()
            .gap_4()
            .p_4()
            .bg(theme::SURFACE)
            .rounded_lg()
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            // Header
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(agent.display_name.clone()),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(8.0))
                                            .h(px(8.0))
                                            .rounded_full()
                                            .bg(status_color),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(status_color)
                                            .child(status_text.to_string()),
                                    ),
                            ),
                    )
                    .when(agent.model.is_some(), |el: Div| {
                        let model = agent.model.as_deref().unwrap_or("");
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child(format!("Model: {model}")),
                        )
                    }),
            )
            // Stats
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(self.stat_row("Requests today", &format!("{}", agent.requests_today)))
                    .child(self.stat_row("Tokens today", &format_tokens(agent.tokens_today)))
                    .child(self.stat_row("Error rate", &format!("{:.1}%", agent.error_rate * 100.0)))
                    .child(self.stat_row("Avg latency", &format!("{}ms", agent.avg_latency_ms))),
            )
            // Actions
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("agent-test")
                            .primary()
                            .label("Test Connection"),
                    )
                    .child(
                        Button::new("agent-configure")
                            .ghost()
                            .label("Configure"),
                    ),
            )
    }

    fn render_routing_rules(&self) -> Div {
        let rows: Vec<Div> = self
            .routing_rules
            .iter()
            .map(|rule| {
                div()
                    .h_flex()
                    .justify_between()
                    .px_3()
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.05))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .font_family("monospace")
                            .child(rule.pattern.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::PRIMARY)
                            .child(rule.agent.clone()),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Routing Rules".to_string()),
            )
            .child(
                div()
                    .v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .overflow_hidden()
                    .children(rows),
            )
    }

    fn stat_row(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .justify_between()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(value.to_string()),
            )
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}

impl Render for AgentHubScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cards: Vec<Stateful<Div>> = self
            .agents
            .iter()
            .enumerate()
            .map(|(idx, agent)| self.render_agent_card(idx, agent, cx))
            .collect();

        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            // Header
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Agent Hub".to_string()),
                    )
                    .child(
                        Button::new("add-agent")
                            .primary()
                            .label("+ Add Agent"),
                    ),
            )
            // Main content: agent list + detail
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap_4()
                    .overflow_hidden()
                    // Left: agent cards
                    .child(
                        div()
                            .w(px(350.0))
                            .v_flex()
                            .gap_2()
                            .overflow_hidden()
                            .children(cards),
                    )
                    // Right: detail panel
                    .child(self.render_detail_panel()),
            )
            // Bottom: routing rules
            .child(self.render_routing_rules())
    }
}
