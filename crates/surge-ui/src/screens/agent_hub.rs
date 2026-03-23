use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName, StyledExt};

use crate::theme;

/// Agent info for display.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub connected: bool,
    pub model: Option<String>,
    pub transport: String,
    pub command: String,
    pub capabilities: Vec<String>,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub cost_today: f64,
    pub error_rate: f32,
    pub avg_latency_ms: u32,
    pub active_sessions: u32,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_total: Option<u32>,
    pub rate_limit_reset_secs: Option<u64>,
    pub last_error: Option<String>,
    pub uptime: String,
}

/// Routing rule entry.
#[derive(Debug, Clone)]
pub struct RoutingRule {
    pub pattern: String,
    pub agent: String,
    pub priority: u32,
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
                    description: "Anthropic's autonomous coding agent with full file system access, terminal, and git integration".into(),
                    connected: true,
                    model: Some("claude-sonnet-4-5-20250514".into()),
                    transport: "stdio".into(),
                    command: "claude --dangerously-skip-permissions".into(),
                    capabilities: vec!["Code".into(), "Plan".into(), "Review".into(), "Test".into(), "Refactor".into()],
                    requests_today: 42,
                    tokens_today: 156_800,
                    cost_today: 2.34,
                    error_rate: 0.02,
                    avg_latency_ms: 1200,
                    active_sessions: 2,
                    rate_limit_remaining: Some(158),
                    rate_limit_total: Some(200),
                    rate_limit_reset_secs: Some(1800),
                    last_error: None,
                    uptime: "4h 23m".into(),
                },
                AgentInfo {
                    name: "copilot-cli".into(),
                    display_name: "GitHub Copilot CLI".into(),
                    description: "GitHub's AI pair programmer for code suggestions and completions".into(),
                    connected: false,
                    model: Some("gpt-4o".into()),
                    transport: "stdio".into(),
                    command: "gh copilot".into(),
                    capabilities: vec!["Code".into(), "Chat".into()],
                    requests_today: 0,
                    tokens_today: 0,
                    cost_today: 0.0,
                    error_rate: 0.0,
                    avg_latency_ms: 0,
                    active_sessions: 0,
                    rate_limit_remaining: None,
                    rate_limit_total: None,
                    rate_limit_reset_secs: None,
                    last_error: Some("Connection refused: process not running".into()),
                    uptime: "—".into(),
                },
                AgentInfo {
                    name: "aider".into(),
                    display_name: "Aider".into(),
                    description: "AI pair programming in terminal. Supports multiple LLM backends".into(),
                    connected: false,
                    model: Some("claude-sonnet-4-5".into()),
                    transport: "stdio".into(),
                    command: "aider".into(),
                    capabilities: vec!["Code".into(), "Refactor".into()],
                    requests_today: 0,
                    tokens_today: 0,
                    cost_today: 0.0,
                    error_rate: 0.0,
                    avg_latency_ms: 0,
                    active_sessions: 0,
                    rate_limit_remaining: None,
                    rate_limit_total: None,
                    rate_limit_reset_secs: None,
                    last_error: None,
                    uptime: "—".into(),
                },
            ],
            routing_rules: vec![
                RoutingRule { pattern: "**/*.rs".into(), agent: "claude-code".into(), priority: 1 },
                RoutingRule { pattern: "**/*.ts".into(), agent: "claude-code".into(), priority: 2 },
                RoutingRule { pattern: "**/*.py".into(), agent: "claude-code".into(), priority: 3 },
                RoutingRule { pattern: "**/*.md".into(), agent: "claude-code".into(), priority: 4 },
                RoutingRule { pattern: "**/*".into(), agent: "claude-code".into(), priority: 99 },
            ],
            selected_agent: Some(0),
        }
    }

    // ── Agent Card (left panel) ──────────────────────────────────

    fn render_agent_card(&self, idx: usize, agent: &AgentInfo, cx: &mut Context<Self>) -> Stateful<Div> {
        let is_selected = self.selected_agent == Some(idx);
        let status_color = if agent.connected { theme::SUCCESS } else { theme::TEXT_MUTED };

        div()
            .id(SharedString::from(format!("agent-{}", agent.name)))
            .w_full()
            .v_flex()
            .gap(px(8.0))
            .p_3()
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(if is_selected { theme::PRIMARY } else { theme::TEXT_MUTED.opacity(0.06) })
            .bg(if is_selected { theme::PRIMARY.opacity(0.05) } else { theme::SURFACE })
            .hover(|s: StyleRefinement| s.border_color(theme::PRIMARY.opacity(0.3)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.selected_agent = Some(idx);
                cx.notify();
            }))
            // Row 1: status dot + name + session count
            .child(
                div()
                    .h_flex()
                    .gap_2()
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
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(agent.display_name.clone()),
                    )
                    .when(agent.active_sessions > 0, |el: Div| {
                        el.child(
                            div()
                                .text_xs()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded_full()
                                .bg(theme::PRIMARY.opacity(0.15))
                                .text_color(theme::PRIMARY)
                                .child(format!("{} active", agent.active_sessions)),
                        )
                    }),
            )
            // Row 2: description
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .line_height(relative(1.4))
                    .max_h(px(32.0))
                    .overflow_hidden()
                    .child(agent.description.clone()),
            )
            // Row 3: capabilities
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(
                        agent.capabilities.iter().map(|cap| {
                            let color = cap_color(cap);
                            div()
                                .text_xs()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(color.opacity(0.12))
                                .text_color(color)
                                .child(cap.clone())
                        }),
                    ),
            )
            // Row 4: model + quick stats (only if connected)
            .when(agent.connected, |el: Stateful<Div>| {
                el.child(
                    div()
                        .h_flex()
                        .justify_between()
                        .items_center()
                        .pt(px(4.0))
                        .border_t_1()
                        .border_color(theme::TEXT_MUTED.opacity(0.06))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED.opacity(0.6))
                                .child(agent.model.clone().unwrap_or_default()),
                        )
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(self.mini_stat("req", &format!("{}", agent.requests_today)))
                                .child(self.mini_stat("tok", &format_tokens(agent.tokens_today))),
                        ),
                )
            })
    }

    fn mini_stat(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .gap(px(3.0))
            .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(label.to_string()))
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED).child(value.to_string()))
    }

    // ── Detail Panel (right) ─────────────────────────────────────

    fn render_detail_panel(&self, cx: &mut Context<Self>) -> Div {
        let Some(idx) = self.selected_agent else {
            return div()
                .flex_1()
                .v_flex()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Icon::new(IconName::Bot).size_8().text_color(theme::TEXT_MUTED.opacity(0.2)))
                .child(div().text_sm().text_color(theme::TEXT_MUTED.opacity(0.4)).child("Select an agent to view details".to_string()));
        };

        let agent = &self.agents[idx];
        let status_text = if agent.connected { "Connected" } else { "Disconnected" };
        let status_color = if agent.connected { theme::SUCCESS } else { theme::ERROR };

        div()
            .flex_1()
            .v_flex()
            .gap_4()
            .overflow_hidden()
            // ── Header section ──
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.06))
                    // Name + status
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Icon::new(IconName::Bot).size_5().text_color(theme::PRIMARY))
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::TEXT_PRIMARY)
                                            .child(agent.display_name.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(6.0))
                                    .items_center()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded_full()
                                    .bg(status_color.opacity(0.1))
                                    .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(status_color))
                                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(status_color).child(status_text.to_string())),
                            ),
                    )
                    // Description
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .line_height(relative(1.5))
                            .child(agent.description.clone()),
                    )
                    // Config info (2x2 grid)
                    .child(
                        div()
                            .v_flex()
                            .gap_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(theme::TEXT_MUTED.opacity(0.06))
                            .child(
                                div().h_flex().gap_4()
                                    .child(self.config_item("Model", agent.model.as_deref().unwrap_or("—")))
                                    .child(self.config_item("Transport", &agent.transport))
                                    .child(self.config_item("Uptime", &agent.uptime)),
                            )
                            .child(
                                div().h_flex().gap_4()
                                    .child(self.config_item("Command", &agent.command)),
                            ),
                    ),
            )
            // ── Stats grid (2x2) ──
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(
                        div().h_flex().gap_3()
                            .child(self.stat_card("Requests", &format!("{}", agent.requests_today), IconName::ArrowUp, theme::PRIMARY))
                            .child(self.stat_card("Tokens", &format_tokens(agent.tokens_today), IconName::Asterisk, theme::PRIMARY)),
                    )
                    .child(
                        div().h_flex().gap_3()
                            .child(self.stat_card("Cost", &format!("${:.2}", agent.cost_today), IconName::Star, theme::WARNING))
                            .child(self.stat_card("Latency", &format!("{}ms", agent.avg_latency_ms), IconName::Loader, latency_color(agent.avg_latency_ms))),
                    ),
            )
            // ── Rate limit bar ──
            .when(agent.rate_limit_total.is_some(), |el: Div| {
                let remaining = agent.rate_limit_remaining.unwrap_or(0);
                let total = agent.rate_limit_total.unwrap_or(1);
                let pct = remaining as f32 / total as f32;
                let bar_color = if pct > 0.5 { theme::SUCCESS } else if pct > 0.2 { theme::WARNING } else { theme::ERROR };
                let reset = agent.rate_limit_reset_secs.unwrap_or(0);

                el.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .p_3()
                        .rounded_lg()
                        .bg(theme::SURFACE)
                        .border_1()
                        .border_color(theme::TEXT_MUTED.opacity(0.06))
                        .child(
                            div()
                                .h_flex()
                                .justify_between()
                                .child(
                                    div().h_flex().gap_2().items_center()
                                        .child(Icon::new(IconName::Loader).size_3p5().text_color(bar_color))
                                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY).child("Rate Limit".to_string())),
                                )
                                .child(
                                    div().text_xs().text_color(theme::TEXT_MUTED)
                                        .child(format!("{remaining}/{total} remaining · resets in {}m", reset / 60)),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .h(px(6.0))
                                .rounded_full()
                                .bg(theme::TEXT_MUTED.opacity(0.1))
                                .child(div().h_full().rounded_full().bg(bar_color).w(relative(pct))),
                        ),
                )
            })
            // ── Error alert ──
            .when(agent.last_error.is_some(), |el: Div| {
                let err = agent.last_error.as_deref().unwrap_or("");
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(theme::ERROR.opacity(0.08))
                        .border_1()
                        .border_color(theme::ERROR.opacity(0.2))
                        .child(Icon::new(IconName::TriangleAlert).size_4().text_color(theme::ERROR))
                        .child(
                            div().flex_1().text_xs().text_color(theme::ERROR.opacity(0.9)).child(err.to_string()),
                        ),
                )
            })
            // ── Actions ──
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("agent-test")
                            .primary()
                            .compact()
                            .label("Test Connection"),
                    )
                    .child(
                        Button::new("agent-configure")
                            .compact()
                            .label("Configure"),
                    )
                    .when(!agent.connected, |el: Div| {
                        el.child(
                            Button::new("agent-connect")
                                .compact()
                                .label("Connect"),
                        )
                    })
                    .when(agent.connected, |el: Div| {
                        el.child(
                            Button::new("agent-disconnect")
                                .ghost()
                                .compact()
                                .label("Disconnect"),
                        )
                    }),
            )
    }

    fn config_item(&self, label: &str, value: &str) -> Div {
        div()
            .v_flex()
            .gap(px(2.0))
            .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(label.to_string()))
            .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::TEXT_PRIMARY).child(value.to_string()))
    }

    fn stat_card(&self, label: &str, value: &str, icon: IconName, color: Hsla) -> Div {
        div()
            .flex_1()
            .v_flex()
            .gap(px(6.0))
            .p_3()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.06))
            .child(
                div()
                    .h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(Icon::new(icon).size_3p5().text_color(color.opacity(0.6)))
                    .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string())),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(value.to_string()),
            )
    }

    // ── Routing Rules ────────────────────────────────────────────

    fn render_routing_rules(&self) -> Div {
        let rows: Vec<Div> = self
            .routing_rules
            .iter()
            .map(|rule| {
                div()
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.04))
                    .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.02)))
                    // Priority
                    .child(
                        div()
                            .w(px(24.0))
                            .text_xs()
                            .text_color(theme::TEXT_MUTED.opacity(0.4))
                            .child(format!("#{}", rule.priority)),
                    )
                    // Pattern
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(rule.pattern.clone()),
                    )
                    // Arrow
                    .child(Icon::new(IconName::ArrowRight).size_3().text_color(theme::TEXT_MUTED.opacity(0.3)))
                    // Agent
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::PRIMARY)
                            .font_weight(FontWeight::MEDIUM)
                            .child(rule.agent.clone()),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div().h_flex().gap_2().items_center()
                            .child(Icon::new(IconName::ArrowRight).size_4().text_color(theme::TEXT_MUTED))
                            .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child("Routing Rules".to_string())),
                    )
                    .child(
                        Button::new("add-rule")
                            .ghost()
                            .compact()
                            .label("+ Add Rule"),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .rounded_lg()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.06))
                    .overflow_hidden()
                    .children(rows),
            )
    }
}

fn cap_color(cap: &str) -> Hsla {
    match cap {
        "Code" => theme::PRIMARY,
        "Plan" => theme::WARNING,
        "Review" => hsla(190.0 / 360.0, 0.8, 0.5, 1.0),
        "Test" => theme::SUCCESS,
        "Refactor" => hsla(280.0 / 360.0, 0.6, 0.6, 1.0),
        "Chat" => theme::TEXT_MUTED,
        _ => theme::TEXT_MUTED,
    }
}

fn latency_color(ms: u32) -> Hsla {
    if ms == 0 { theme::TEXT_MUTED }
    else if ms < 1000 { theme::SUCCESS }
    else if ms < 3000 { theme::WARNING }
    else { theme::ERROR }
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
            .h_flex()
            .gap_0()
            .overflow_hidden()
            // Left panel: agent list (fixed width)
            .child(
                div()
                    .w(px(300.0))
                    .flex_shrink_0()
                    .h_full()
                    .v_flex()
                    .gap_2()
                    .p_3()
                    .border_r_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.06))
                    // Header
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .items_center()
                            .pb_1()
                            .child(
                                div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY)
                                    .child(format!("Agents ({})", self.agents.len())),
                            )
                            .child(
                                Button::new("add-agent")
                                    .primary()
                                    .compact()
                                    .label("+ Add"),
                            ),
                    )
                    // Cards
                    .children(cards),
            )
            // Right panel: detail + routing rules (scrollable)
            .child(
                div()
                    .id("agent-detail-scroll")
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .v_flex()
                    .gap_4()
                    .p_4()
                    .overflow_y_scroll()
                    .child(self.render_detail_panel(cx))
                    .child(self.render_routing_rules()),
            )
    }
}
