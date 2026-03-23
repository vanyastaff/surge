use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName, StyledExt};

use crate::theme;

// ── Data Models ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Connected,
    Offline,
    NotInstalled,
}

impl AgentStatus {
    fn color(self) -> Hsla {
        match self {
            Self::Connected => theme::SUCCESS,
            Self::Offline => theme::ERROR,
            Self::NotInstalled => theme::TEXT_MUTED.opacity(0.4),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Offline => "Offline",
            Self::NotInstalled => "Not Installed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub status: AgentStatus,
    pub model: Option<String>,
    pub binary: String,
    pub transport: String,
    pub flags: Vec<String>,
    pub capabilities: Vec<String>,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub cost_today: f64,
    pub avg_latency_ms: u32,
    pub active_sessions: u32,
    pub sessions_today: u32,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_total: Option<u32>,
    pub rate_limit_reset_secs: Option<u64>,
    pub last_error: Option<String>,
    pub last_seen: Option<String>,
    pub uptime: Option<String>,
    // Today stats
    pub subtasks_completed: u32,
    pub subtasks_failed: u32,
    pub avg_subtask_secs: u32,
    pub qa_first_pass_rate: f32,
    pub assigned_patterns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CatalogAgent {
    pub name: String,
    pub display_name: String,
    pub vendor: String,
    pub description: String,
    pub model: Option<String>,
    pub capabilities: Vec<String>,
    pub pricing: String,
    pub install_hint: String,
}

#[derive(Debug, Clone)]
pub struct RoutingRule {
    pub pattern: String,
    pub agent: String,
    pub priority: u32,
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HubTab {
    Configured,
    Available,
    Benchmarks,
}

impl HubTab {
    fn label(self) -> &'static str {
        match self {
            Self::Configured => "Configured",
            Self::Available => "Available",
            Self::Benchmarks => "Benchmarks",
        }
    }
    fn all() -> &'static [HubTab] { &[Self::Configured, Self::Available, Self::Benchmarks] }
}

// ── Screen ───────────────────────────────────────────────────

pub struct AgentHubScreen {
    agents: Vec<AgentInfo>,
    catalog: Vec<CatalogAgent>,
    routing_rules: Vec<RoutingRule>,
    selected_agent: Option<usize>,
    active_tab: HubTab,
}

impl AgentHubScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            agents: demo_agents(),
            catalog: demo_catalog(),
            routing_rules: demo_rules(),
            selected_agent: Some(0),
            active_tab: HubTab::Configured,
        }
    }

    // ── Tabs ─────────────────────────────────────────────────

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = HubTab::all()
            .iter()
            .map(|&tab| {
                let is_active = tab == self.active_tab;
                let count_label = match tab {
                    HubTab::Configured => format!(" ({})", self.agents.len()),
                    HubTab::Available => format!(" ({})", self.catalog.len()),
                    HubTab::Benchmarks => String::new(),
                };
                div()
                    .id(SharedString::from(format!("tab-{}", tab.label())))
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .text_xs()
                    .font_weight(if is_active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                    .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                    .bg(if is_active { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                    .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.05)))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.active_tab = tab;
                        cx.notify();
                    }))
                    .child(format!("{}{}", tab.label(), count_label))
            })
            .collect();

        div().h_flex().gap_1().children(tabs)
    }

    // ── Configured: Agent Card (left) ────────────────────────

    fn render_agent_card(&self, idx: usize, agent: &AgentInfo, cx: &mut Context<Self>) -> Stateful<Div> {
        let is_selected = self.selected_agent == Some(idx);

        div()
            .id(SharedString::from(format!("agent-{}", agent.name)))
            .w_full()
            .v_flex()
            .gap(px(6.0))
            .p(px(10.0))
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(if is_selected { theme::PRIMARY } else { theme::TEXT_MUTED.opacity(0.06) })
            .bg(if is_selected { theme::PRIMARY.opacity(0.05) } else { theme::SURFACE })
            .hover(|s: StyleRefinement| s.border_color(theme::PRIMARY.opacity(0.3)))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.selected_agent = Some(idx);
                cx.notify();
            }))
            // Row 1: dot + name + session badge
            .child(
                div().h_flex().gap_2().items_center()
                    .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(agent.status.color()))
                    .child(
                        div().flex_1().text_sm().font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()),
                    )
                    .when(agent.active_sessions > 0, |el: Div| {
                        el.child(
                            div().text_xs().px(px(6.0)).py(px(1.0)).rounded_full()
                                .bg(theme::PRIMARY.opacity(0.15)).text_color(theme::PRIMARY)
                                .child(format!("{} active", agent.active_sessions)),
                        )
                    }),
            )
            // Row 2: model + tokens (connected) OR last seen (offline) OR setup guide (not installed)
            .child(match agent.status {
                AgentStatus::Connected => {
                    div().h_flex().gap_1().items_center()
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(agent.model.clone().unwrap_or_default()))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.3)).child("·".to_string()))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(format!("{} tok", format_tokens(agent.tokens_today))))
                }
                AgentStatus::Offline => {
                    div().h_flex().gap_2().items_center()
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6))
                            .child(format!("Last seen: {}", agent.last_seen.as_deref().unwrap_or("—"))))
                }
                AgentStatus::NotInstalled => {
                    div().h_flex().gap_1().items_center()
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child("Not configured".to_string()))
                }
            })
            // Row 3: capabilities
            .child(
                div().h_flex().gap_1().flex_wrap().children(
                    agent.capabilities.iter().map(|cap| badge(cap, cap_color(cap))),
                ),
            )
    }

    // ── Detail Panel (right) ─────────────────────────────────

    fn render_detail_panel(&self) -> Div {
        let Some(idx) = self.selected_agent else {
            return div()
                .flex_1().v_flex().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Bot).size_8().text_color(theme::TEXT_MUTED.opacity(0.2)))
                .child(div().text_sm().text_color(theme::TEXT_MUTED.opacity(0.4)).child("Select an agent".to_string()));
        };

        let agent = &self.agents[idx];

        div()
            .flex_1()
            .v_flex()
            .gap_3()
            // ── Header card ──
            .child(
                div().v_flex().gap_3().p_4().rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                    // Name + status pill
                    .child(
                        div().h_flex().justify_between().items_center()
                            .child(
                                div().h_flex().gap_2().items_center()
                                    .child(Icon::new(IconName::Bot).size_5().text_color(theme::PRIMARY))
                                    .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone())),
                            )
                            .child(status_pill(agent.status)),
                    )
                    // Description
                    .child(div().text_xs().text_color(theme::TEXT_MUTED).line_height(relative(1.5)).child(agent.description.clone()))
                    // Config row
                    .child(
                        div().h_flex().gap_6().pt_2().border_t_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                            .child(config_item("Model", agent.model.as_deref().unwrap_or("—")))
                            .child(config_item("Transport", &agent.transport))
                            .child(config_item("Uptime", agent.uptime.as_deref().unwrap_or("—")))
                            .child(config_item("Sessions", &format!("{} today", agent.sessions_today)))
                            .child(
                                div().v_flex().gap(px(2.0))
                                    .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child("Command".to_string()))
                                    .child(
                                        div().h_flex().gap_1().items_center()
                                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::TEXT_PRIMARY).child(agent.binary.clone()))
                                            .children(agent.flags.iter().map(|f| {
                                                div().text_xs().px(px(4.0)).py(px(1.0)).rounded(px(3.0))
                                                    .bg(theme::WARNING.opacity(0.12)).text_color(theme::WARNING)
                                                    .child(f.clone())
                                            })),
                                    ),
                            ),
                    ),
            )
            // ── Stats grid (2x2) ──
            .child(
                div().v_flex().gap_2()
                    .child(div().h_flex().gap_2()
                        .child(stat_card("Requests", &format!("{}", agent.requests_today), IconName::ArrowUp, theme::PRIMARY))
                        .child(stat_card("Tokens", &format_tokens(agent.tokens_today), IconName::Asterisk, theme::PRIMARY)))
                    .child(div().h_flex().gap_2()
                        .child(stat_card("Cost", &format!("${:.2}", agent.cost_today), IconName::Star, theme::WARNING))
                        .child(stat_card("Latency", &format!("{}ms", agent.avg_latency_ms), IconName::Loader, latency_color(agent.avg_latency_ms)))),
            )
            // ── Rate Limit ──
            .when(agent.rate_limit_total.is_some(), |el: Div| {
                let rem = agent.rate_limit_remaining.unwrap_or(0);
                let total = agent.rate_limit_total.unwrap_or(1);
                let pct = rem as f32 / total as f32;
                let color = if pct > 0.5 { theme::SUCCESS } else if pct > 0.2 { theme::WARNING } else { theme::ERROR };
                let reset = agent.rate_limit_reset_secs.unwrap_or(0);
                el.child(
                    div().v_flex().gap_2().p_3().rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                        .child(
                            div().h_flex().justify_between()
                                .child(div().h_flex().gap_2().items_center()
                                    .child(Icon::new(IconName::Loader).size_3p5().text_color(color))
                                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY).child("Rate Limit".to_string())))
                                .child(div().text_xs().text_color(theme::TEXT_MUTED).child(format!("{rem}/{total} remaining · resets in {}m", reset / 60))))
                        .child(div().w_full().h(px(5.0)).rounded_full().bg(theme::TEXT_MUTED.opacity(0.1))
                            .child(div().h_full().rounded_full().bg(color).w(relative(pct)))),
                )
            })
            // ── Capabilities + Assigned Files + Today ──
            .child(
                div().v_flex().gap_3().p_3().rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                    // Capabilities
                    .child(
                        div().v_flex().gap(px(6.0))
                            .child(div().h_flex().gap_2().items_center()
                                .child(Icon::new(IconName::Star).size_3p5().text_color(theme::TEXT_MUTED.opacity(0.5)))
                                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY).child("Capabilities".to_string())))
                            .child(div().h_flex().gap_1().flex_wrap().children(
                                agent.capabilities.iter().map(|c| badge(c, cap_color(c))))),
                    )
                    // Assigned Files
                    .child(
                        div().v_flex().gap(px(6.0)).pt_2().border_t_1().border_color(theme::TEXT_MUTED.opacity(0.04))
                            .child(div().h_flex().gap_2().items_center()
                                .child(Icon::new(IconName::Folder).size_3p5().text_color(theme::TEXT_MUTED.opacity(0.5)))
                                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY).child("Assigned Files".to_string())))
                            .child(div().text_xs().text_color(theme::TEXT_MUTED)
                                .child(if agent.assigned_patterns.is_empty() {
                                    "No patterns assigned".to_string()
                                } else {
                                    format!("{} ({} patterns)", agent.assigned_patterns.join(", "), agent.assigned_patterns.len())
                                })),
                    )
                    // Today stats
                    .child(
                        div().v_flex().gap(px(6.0)).pt_2().border_t_1().border_color(theme::TEXT_MUTED.opacity(0.04))
                            .child(div().h_flex().gap_2().items_center()
                                .child(Icon::new(IconName::ChartPie).size_3p5().text_color(theme::TEXT_MUTED.opacity(0.5)))
                                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY).child("Today".to_string())))
                            .child(
                                div().h_flex().gap_4()
                                    .child(today_stat("Subtasks", &format!("{} completed", agent.subtasks_completed), theme::SUCCESS))
                                    .child(today_stat("Failures", &format!("{}", agent.subtasks_failed), if agent.subtasks_failed > 0 { theme::ERROR } else { theme::TEXT_MUTED }))
                                    .child(today_stat("Avg time", &format!("{}s/subtask", agent.avg_subtask_secs), theme::TEXT_MUTED))
                                    .child(today_stat("QA rate", &format!("{:.0}%", agent.qa_first_pass_rate * 100.0), if agent.qa_first_pass_rate > 0.8 { theme::SUCCESS } else { theme::WARNING }))),
                    ),
            )
            // ── Error ──
            .when(agent.last_error.is_some(), |el: Div| {
                let err = agent.last_error.as_deref().unwrap_or("");
                el.child(
                    div().h_flex().gap_2().items_center().px_3().py_2().rounded_lg()
                        .bg(theme::ERROR.opacity(0.08)).border_1().border_color(theme::ERROR.opacity(0.2))
                        .child(Icon::new(IconName::TriangleAlert).size_4().text_color(theme::ERROR))
                        .child(div().flex_1().text_xs().text_color(theme::ERROR.opacity(0.9)).child(err.to_string())),
                )
            })
            // ── Actions ──
            .child(
                div().h_flex().gap_2()
                    .child(Button::new("agent-test").primary().compact().label("Test Connection"))
                    .child(Button::new("agent-configure").compact().label("Configure"))
                    .child(Button::new("agent-logs").compact().label("View Logs"))
                    .when(agent.status == AgentStatus::Connected, |el: Div| {
                        el.child(Button::new("agent-disconnect").ghost().compact().label("Disconnect"))
                    })
                    .when(agent.status == AgentStatus::Offline, |el: Div| {
                        el.child(Button::new("agent-reconnect").compact().label("Reconnect"))
                    })
                    .when(agent.status == AgentStatus::NotInstalled, |el: Div| {
                        el.child(Button::new("agent-setup").primary().compact().label("Setup"))
                    }),
            )
    }

    // ── Available Tab ────────────────────────────────────────

    fn render_available_tab(&self) -> Div {
        let cards: Vec<Div> = self.catalog.iter().map(|agent| {
            div()
                .v_flex()
                .gap(px(8.0))
                .p_3()
                .rounded_lg()
                .bg(theme::SURFACE)
                .border_1()
                .border_color(theme::TEXT_MUTED.opacity(0.06))
                .hover(|s: StyleRefinement| s.border_color(theme::TEXT_MUTED.opacity(0.15)))
                // Header: name + Setup button
                .child(
                    div().h_flex().justify_between().items_center()
                        .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()))
                        .child(
                            Button::new(SharedString::from(format!("setup-{}", agent.name)))
                                .compact().label("Setup"),
                        ),
                )
                // Vendor + pricing
                .child(
                    div().h_flex().gap_2().items_center()
                        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(agent.vendor.clone()))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.3)).child("·".to_string()))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(agent.pricing.clone())),
                )
                // Description
                .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.7)).line_height(relative(1.4)).child(agent.description.clone()))
                // Capabilities + model
                .child(
                    div().h_flex().gap_1().flex_wrap()
                        .children(agent.capabilities.iter().map(|c| badge(c, cap_color(c))))
                        .when(agent.model.is_some(), |el: Div| {
                            el.child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(agent.model.clone().unwrap_or_default()))
                        }),
                )
                // Install hint
                .child(
                    div().h_flex().gap_1().items_center().pt_1().border_t_1().border_color(theme::TEXT_MUTED.opacity(0.04))
                        .child(Icon::new(IconName::SquareTerminal).size_3().text_color(theme::TEXT_MUTED.opacity(0.4)))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(agent.install_hint.clone())),
                )
        }).collect();

        div()
            .size_full()
            .v_flex()
            .gap_3()
            .p_4()
            .child(
                div().text_xs().text_color(theme::TEXT_MUTED)
                    .child("Agents available for installation. Click Setup to configure.".to_string()),
            )
            .child(
                div().flex().flex_wrap().gap_3().content_start()
                    .children(cards.into_iter().map(|c| div().w(px(320.0)).child(c))),
            )
    }

    // ── Benchmarks Tab ───────────────────────────────────────

    fn render_benchmarks_tab(&self) -> Div {
        div()
            .size_full().v_flex().items_center().justify_center().gap_2()
            .child(Icon::new(IconName::ChartPie).size_8().text_color(theme::TEXT_MUTED.opacity(0.2)))
            .child(div().text_sm().text_color(theme::TEXT_MUTED.opacity(0.4)).child("Agent Benchmarks".to_string()))
            .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.3)).child("Compare agent performance metrics — coming in Phase 7".to_string()))
    }

    // ── Routing Rules ────────────────────────────────────────

    fn render_routing_rules(&self) -> Div {
        let header = div().h_flex().items_center().px_3().py(px(6.0)).border_b_1().border_color(theme::TEXT_MUTED.opacity(0.06))
            .child(div().w(px(24.0)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED.opacity(0.5)).child("#".to_string()))
            .child(div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED.opacity(0.5)).child("Pattern".to_string()))
            .child(div().w(px(120.0)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED.opacity(0.5)).child("Agent".to_string()))
            .child(div().w(px(100.0)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_MUTED.opacity(0.5)).child("Fallback".to_string()));

        let rows: Vec<Div> = self.routing_rules.iter().map(|rule| {
            div().h_flex().items_center().px_3().py(px(7.0)).border_b_1().border_color(theme::TEXT_MUTED.opacity(0.03))
                .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.02)))
                .child(div().w(px(24.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(0.4)).child(format!("{}", rule.priority)))
                .child(div().flex_1().text_xs().text_color(theme::TEXT_PRIMARY).child(rule.pattern.clone()))
                .child(div().w(px(120.0)).text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::PRIMARY).child(rule.agent.clone()))
                .child(div().w(px(100.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(rule.fallback.clone().unwrap_or("—".into())))
        }).collect();

        div().v_flex().gap_2()
            .child(
                div().h_flex().justify_between().items_center()
                    .child(div().h_flex().gap_2().items_center()
                        .child(Icon::new(IconName::ArrowRight).size_3p5().text_color(theme::TEXT_MUTED))
                        .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child("Routing Rules".to_string())))
                    .child(div().h_flex().gap_2()
                        .child(Button::new("auto-detect").ghost().compact().label("Auto-detect"))
                        .child(Button::new("add-rule").ghost().compact().label("+ Add Rule"))),
            )
            .child(
                div().v_flex().rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06)).overflow_hidden()
                    .child(header)
                    .children(rows),
            )
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn badge(text: &str, color: Hsla) -> Div {
    div().text_xs().px(px(6.0)).py(px(2.0)).rounded(px(4.0))
        .bg(color.opacity(0.12)).text_color(color).child(text.to_string())
}

fn status_pill(status: AgentStatus) -> Div {
    div().h_flex().gap(px(6.0)).items_center().px(px(10.0)).py(px(4.0)).rounded_full()
        .bg(status.color().opacity(0.1))
        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(status.color()))
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(status.color()).child(status.label().to_string()))
}

fn config_item(label: &str, value: &str) -> Div {
    div().v_flex().gap(px(2.0))
        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(label.to_string()))
        .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::TEXT_PRIMARY).child(value.to_string()))
}

fn stat_card(label: &str, value: &str, icon: IconName, color: Hsla) -> Div {
    div().flex_1().v_flex().gap(px(4.0)).p(px(10.0)).rounded_lg()
        .bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
        .child(div().h_flex().gap(px(4.0)).items_center()
            .child(Icon::new(icon).size_3().text_color(color.opacity(0.5)))
            .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string())))
        .child(div().text_base().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(value.to_string()))
}

fn today_stat(label: &str, value: &str, color: Hsla) -> Div {
    div().v_flex().gap(px(2.0))
        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(label.to_string()))
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(color).child(value.to_string()))
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
    if ms == 0 { theme::TEXT_MUTED } else if ms < 1000 { theme::SUCCESS } else if ms < 3000 { theme::WARNING } else { theme::ERROR }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 { format!("{:.1}M", tokens as f64 / 1_000_000.0) }
    else if tokens >= 1_000 { format!("{:.1}K", tokens as f64 / 1_000.0) }
    else { format!("{tokens}") }
}

// ── Render ───────────────────────────────────────────────────

impl Render for AgentHubScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.active_tab {
            HubTab::Configured => self.render_configured_tab(cx),
            HubTab::Available => {
                div().size_full().v_flex()
                    .child(self.render_header(cx))
                    .child(self.render_available_tab())
            }
            HubTab::Benchmarks => {
                div().size_full().v_flex()
                    .child(self.render_header(cx))
                    .child(self.render_benchmarks_tab())
            }
        }
    }
}

impl AgentHubScreen {
    fn render_header(&self, cx: &mut Context<Self>) -> Div {
        div().h_flex().justify_between().items_center().px_4().pt_3().pb_1()
            .child(
                div().h_flex().gap_3().items_center()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY)
                        .child(format!("Agents ({})", self.agents.len())))
                    .child(self.render_tabs(cx)),
            )
            .child(Button::new("add-agent").primary().compact().label("+ Add"))
    }

    fn render_configured_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let cards: Vec<Stateful<Div>> = self.agents.iter().enumerate()
            .map(|(idx, agent)| self.render_agent_card(idx, agent, cx))
            .collect();

        div().size_full().v_flex()
            // Header with tabs
            .child(self.render_header(cx))
            // Content: list + detail
            .child(
                div().flex_1().h_flex().gap_0().overflow_hidden()
                    // Left list
                    .child(
                        div().w(px(280.0)).flex_shrink_0().h_full().v_flex().gap_2().p_3()
                            .border_r_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                            .children(cards),
                    )
                    // Right detail (scrollable)
                    .child(
                        div().id("detail-scroll").flex_1().h_full().min_w_0().v_flex().gap_3().p_4()
                            .overflow_y_scroll()
                            .child(self.render_detail_panel()),
                    ),
            )
    }
}

// ── Demo Data ────────────────────────────────────────────────

fn demo_agents() -> Vec<AgentInfo> {
    vec![
        AgentInfo {
            name: "claude-code".into(), display_name: "Claude Code".into(),
            description: "Anthropic's autonomous coding agent with full file system access, terminal, and git integration".into(),
            status: AgentStatus::Connected,
            model: Some("claude-sonnet-4-5".into()), binary: "claude".into(), transport: "stdio".into(),
            flags: vec!["skip-permissions".into()],
            capabilities: vec!["Code".into(), "Plan".into(), "Review".into(), "Test".into(), "Refactor".into()],
            requests_today: 42, tokens_today: 156_800, cost_today: 2.34, avg_latency_ms: 1200,
            active_sessions: 2, sessions_today: 12,
            rate_limit_remaining: Some(158), rate_limit_total: Some(200), rate_limit_reset_secs: Some(1800),
            last_error: None, last_seen: None, uptime: Some("4h 23m".into()),
            subtasks_completed: 8, subtasks_failed: 0, avg_subtask_secs: 45, qa_first_pass_rate: 0.87,
            assigned_patterns: vec!["**/*.rs".into(), "**/*.toml".into(), "**/*.md".into()],
        },
        AgentInfo {
            name: "copilot-cli".into(), display_name: "GitHub Copilot CLI".into(),
            description: "GitHub's AI pair programmer for code suggestions and completions".into(),
            status: AgentStatus::Offline,
            model: Some("gpt-4o".into()), binary: "gh".into(), transport: "stdio".into(),
            flags: vec!["copilot".into()],
            capabilities: vec!["Code".into(), "Chat".into()],
            requests_today: 0, tokens_today: 0, cost_today: 0.0, avg_latency_ms: 0,
            active_sessions: 0, sessions_today: 0,
            rate_limit_remaining: None, rate_limit_total: None, rate_limit_reset_secs: None,
            last_error: Some("Connection refused: process not running".into()),
            last_seen: Some("2h ago".into()), uptime: None,
            subtasks_completed: 0, subtasks_failed: 0, avg_subtask_secs: 0, qa_first_pass_rate: 0.0,
            assigned_patterns: vec![],
        },
        AgentInfo {
            name: "aider".into(), display_name: "Aider".into(),
            description: "AI pair programming in terminal. Supports multiple LLM backends".into(),
            status: AgentStatus::NotInstalled,
            model: None, binary: "aider".into(), transport: "stdio".into(), flags: vec![],
            capabilities: vec!["Code".into(), "Refactor".into()],
            requests_today: 0, tokens_today: 0, cost_today: 0.0, avg_latency_ms: 0,
            active_sessions: 0, sessions_today: 0,
            rate_limit_remaining: None, rate_limit_total: None, rate_limit_reset_secs: None,
            last_error: None, last_seen: None, uptime: None,
            subtasks_completed: 0, subtasks_failed: 0, avg_subtask_secs: 0, qa_first_pass_rate: 0.0,
            assigned_patterns: vec![],
        },
    ]
}

fn demo_catalog() -> Vec<CatalogAgent> {
    vec![
        CatalogAgent { name: "gemini-cli".into(), display_name: "Gemini CLI".into(), vendor: "Google".into(),
            description: "Google's AI coding assistant with Gemini models".into(),
            model: Some("gemini-2.5-pro".into()), capabilities: vec!["Code".into(), "Chat".into()],
            pricing: "Free tier".into(), install_hint: "npm install -g @anthropic/gemini-cli".into() },
        CatalogAgent { name: "codex".into(), display_name: "OpenAI Codex".into(), vendor: "OpenAI".into(),
            description: "OpenAI's code generation and editing agent".into(),
            model: Some("codex".into()), capabilities: vec!["Code".into(), "Refactor".into()],
            pricing: "API key".into(), install_hint: "npm install -g @openai/codex".into() },
        CatalogAgent { name: "goose".into(), display_name: "Goose".into(), vendor: "Square (open source)".into(),
            description: "Open source AI developer agent by Square".into(),
            model: None, capabilities: vec!["Code".into(), "Plan".into()],
            pricing: "Free (OSS)".into(), install_hint: "brew install goose".into() },
        CatalogAgent { name: "cline".into(), display_name: "Cline".into(), vendor: "Open source".into(),
            description: "Autonomous coding agent for VS Code and CLI".into(),
            model: None, capabilities: vec!["Code".into(), "Chat".into()],
            pricing: "Free (OSS)".into(), install_hint: "npm install -g cline".into() },
        CatalogAgent { name: "devstral".into(), display_name: "Devstral".into(), vendor: "Mistral".into(),
            description: "Mistral's coding-focused model for development tasks".into(),
            model: Some("devstral".into()), capabilities: vec!["Code".into()],
            pricing: "API key".into(), install_hint: "pip install mistral-cli".into() },
        CatalogAgent { name: "kiro".into(), display_name: "Kiro".into(), vendor: "Amazon".into(),
            description: "Amazon's spec-driven AI IDE for software development".into(),
            model: None, capabilities: vec!["Code".into(), "Plan".into()],
            pricing: "Free preview".into(), install_hint: "Download from kiro.dev".into() },
        CatalogAgent { name: "qwen-coder".into(), display_name: "Qwen3-Coder".into(), vendor: "Alibaba".into(),
            description: "Alibaba's code generation model, runs locally".into(),
            model: Some("qwen3-coder".into()), capabilities: vec!["Code".into()],
            pricing: "Free (local)".into(), install_hint: "ollama pull qwen3-coder".into() },
        CatalogAgent { name: "amp".into(), display_name: "Amp".into(), vendor: "Sourcegraph".into(),
            description: "Sourcegraph's AI coding agent with codebase context".into(),
            model: None, capabilities: vec!["Code".into(), "Review".into()],
            pricing: "Free tier".into(), install_hint: "npm install -g @sourcegraph/amp".into() },
    ]
}

fn demo_rules() -> Vec<RoutingRule> {
    vec![
        RoutingRule { pattern: "**/*.rs".into(), agent: "claude-code".into(), priority: 1, fallback: Some("copilot-cli".into()) },
        RoutingRule { pattern: "**/*.ts".into(), agent: "claude-code".into(), priority: 1, fallback: None },
        RoutingRule { pattern: "**/*.py".into(), agent: "claude-code".into(), priority: 1, fallback: None },
        RoutingRule { pattern: "**/*.md".into(), agent: "claude-code".into(), priority: 2, fallback: None },
        RoutingRule { pattern: "**/*".into(), agent: "claude-code".into(), priority: 99, fallback: None },
    ]
}
