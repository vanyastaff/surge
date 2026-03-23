use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::{Icon, IconName, StyledExt};

use crate::theme;

// ── Data Models ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConfiguredAgent {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: Option<String>,
    pub binary: String,
    pub active_sessions: u32,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub cost_today: f64,
    pub avg_latency_ms: u32,
    pub sessions_today: u32,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_total: Option<u32>,
    pub rate_limit_reset_secs: Option<u64>,
    pub subtasks_completed: u32,
    pub subtasks_failed: u32,
    pub avg_subtask_secs: u32,
    pub qa_first_pass_rate: f32,
    pub uptime: String,
}

#[derive(Debug, Clone)]
pub struct AvailableAgent {
    pub name: String,
    pub display_name: String,
    pub vendor: String,
    pub description: String,
    pub pricing: String,
    pub install_command: String,
    pub badge: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HubTab { Configured, Available }

impl HubTab {
    fn all() -> &'static [Self] { &[Self::Configured, Self::Available] }
}

// ── Screen ───────────────────────────────────────────────────

pub struct AgentHubScreen {
    configured: Vec<ConfiguredAgent>,
    available: Vec<AvailableAgent>,
    selected: Option<usize>,
    active_tab: HubTab,
}

impl AgentHubScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            configured: vec![
                ConfiguredAgent {
                    name: "claude-code".into(), display_name: "Claude Code".into(),
                    description: "Anthropic's autonomous coding agent with full file system access, terminal, and git integration".into(),
                    model: Some("claude-sonnet-4-5".into()), binary: "claude".into(),
                    active_sessions: 2, requests_today: 42, tokens_today: 156_800,
                    cost_today: 2.34, avg_latency_ms: 1200, sessions_today: 12,
                    rate_limit_remaining: Some(158), rate_limit_total: Some(200), rate_limit_reset_secs: Some(1800),
                    subtasks_completed: 8, subtasks_failed: 0, avg_subtask_secs: 45, qa_first_pass_rate: 0.87,
                    uptime: "4h 23m".into(),
                },
                ConfiguredAgent {
                    name: "aider".into(), display_name: "Aider".into(),
                    description: "AI pair programming in terminal. Supports multiple LLM backends".into(),
                    model: Some("claude-sonnet-4-5".into()), binary: "aider".into(),
                    active_sessions: 0, requests_today: 0, tokens_today: 0,
                    cost_today: 0.0, avg_latency_ms: 0, sessions_today: 0,
                    rate_limit_remaining: None, rate_limit_total: None, rate_limit_reset_secs: None,
                    subtasks_completed: 0, subtasks_failed: 0, avg_subtask_secs: 0, qa_first_pass_rate: 0.0,
                    uptime: "—".into(),
                },
            ],
            available: vec![
                AvailableAgent { name: "gemini-cli".into(), display_name: "Gemini CLI".into(), vendor: "Google".into(),
                    description: "Google's AI coding assistant with Gemini models".into(),
                    pricing: "Free tier".into(), install_command: "npm install -g @anthropic/gemini-cli".into(), badge: Some("Popular".into()) },
                AvailableAgent { name: "codex".into(), display_name: "OpenAI Codex".into(), vendor: "OpenAI".into(),
                    description: "OpenAI's autonomous coding agent".into(),
                    pricing: "API key".into(), install_command: "npm install -g @openai/codex".into(), badge: Some("Popular".into()) },
                AvailableAgent { name: "goose".into(), display_name: "Goose".into(), vendor: "Square (OSS)".into(),
                    description: "Open source AI developer agent".into(),
                    pricing: "Free (OSS)".into(), install_command: "brew install goose".into(), badge: Some("Popular".into()) },
                AvailableAgent { name: "cline".into(), display_name: "Cline".into(), vendor: "Open source".into(),
                    description: "Autonomous coding agent with multi-model support".into(),
                    pricing: "Free (OSS)".into(), install_command: "npm install -g cline".into(), badge: None },
                AvailableAgent { name: "devstral".into(), display_name: "Devstral".into(), vendor: "Mistral".into(),
                    description: "Mistral's coding-focused model".into(),
                    pricing: "API key".into(), install_command: "pip install mistral-cli".into(), badge: Some("New".into()) },
                AvailableAgent { name: "kiro".into(), display_name: "Kiro".into(), vendor: "Amazon".into(),
                    description: "Amazon's spec-driven AI IDE".into(),
                    pricing: "Free preview".into(), install_command: "Download from kiro.dev".into(), badge: Some("New".into()) },
            ],
            selected: Some(0),
            active_tab: HubTab::Configured,
        }
    }

    // ── Tabs ─────────────────────────────────────────────────

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = HubTab::all().iter().map(|&tab| {
            let is_active = tab == self.active_tab;
            let label = match tab {
                HubTab::Configured => format!("Configured ({})", self.configured.len()),
                HubTab::Available => format!("Available ({})", self.available.len()),
            };
            div()
                .id(SharedString::from(format!("tab-{:?}", tab)))
                .px_3().py(px(5.0)).cursor_pointer().rounded_md().text_xs()
                .font_weight(if is_active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                .bg(if is_active { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.05)))
                .on_click(cx.listener(move |this, _e, _w, cx| { this.active_tab = tab; cx.notify(); }))
                .child(label)
        }).collect();
        div().h_flex().gap_1().children(tabs)
    }

    // ── Configured: Agent List (left) ────────────────────────

    fn render_agent_item(&self, idx: usize, agent: &ConfiguredAgent, cx: &mut Context<Self>) -> Stateful<Div> {
        let is_selected = self.selected == Some(idx);
        let is_active = agent.active_sessions > 0;

        div()
            .id(SharedString::from(format!("agent-{}", agent.name)))
            .w_full().h_flex().gap_2().items_center()
            .px_3().py(px(8.0)).rounded_lg().cursor_pointer()
            .bg(if is_selected { theme::PRIMARY.opacity(0.08) } else { gpui::transparent_black() })
            .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.05)))
            .on_click(cx.listener(move |this, _e, _w, cx| { this.selected = Some(idx); cx.notify(); }))
            // Status dot
            .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(theme::SUCCESS))
            // Name
            .child(div().flex_1().text_sm().font_weight(FontWeight::MEDIUM).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()))
            // Right info
            .child(
                div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6))
                    .child(if is_active {
                        format!("{} active · {} tok", agent.active_sessions, format_tokens(agent.tokens_today))
                    } else {
                        "idle".to_string()
                    }),
            )
    }

    // ── Detail Panel (right, read-only) ──────────────────────

    fn render_detail(&self) -> Div {
        let Some(idx) = self.selected else {
            return div().flex_1().v_flex().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Bot).size_8().text_color(theme::TEXT_MUTED.opacity(0.15)))
                .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.3)).child("Select an agent".to_string()));
        };
        let agent = &self.configured[idx];

        div().flex_1().v_flex().gap_3()
            // Header
            .child(
                div().h_flex().gap_2().items_center()
                    .child(Icon::new(IconName::Bot).size_5().text_color(theme::PRIMARY))
                    .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()))
                    .child(div().h_flex().gap(px(4.0)).items_center().px(px(8.0)).py(px(3.0)).rounded_full()
                        .bg(theme::SUCCESS.opacity(0.1))
                        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(theme::SUCCESS))
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::SUCCESS).child("Ready".to_string()))),
            )
            // Description
            .child(div().text_xs().text_color(theme::TEXT_MUTED).child(agent.description.clone()))
            // Info row
            .child(
                div().h_flex().gap_6().pt_2().pb_1().border_t_1().border_b_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                    .child(info_item("Model", agent.model.as_deref().unwrap_or("—")))
                    .child(info_item("Binary", &agent.binary))
                    .child(info_item("Uptime", &agent.uptime))
                    .child(info_item("Sessions today", &format!("{}", agent.sessions_today))),
            )
            // Stats (4 cards)
            .child(
                div().h_flex().gap_2()
                    .child(stat_card("Requests", &format!("{}", agent.requests_today), theme::PRIMARY))
                    .child(stat_card("Tokens", &format_tokens(agent.tokens_today), theme::PRIMARY))
                    .child(stat_card("Cost", &format!("${:.2}", agent.cost_today), theme::WARNING))
                    .child(stat_card("Latency", &format!("{}ms", agent.avg_latency_ms), latency_color(agent.avg_latency_ms))),
            )
            // Rate limit
            .when(agent.rate_limit_total.is_some(), |el: Div| {
                let rem = agent.rate_limit_remaining.unwrap_or(0);
                let total = agent.rate_limit_total.unwrap_or(1);
                let pct = rem as f32 / total as f32;
                let color = if pct > 0.5 { theme::SUCCESS } else if pct > 0.2 { theme::WARNING } else { theme::ERROR };
                let reset = agent.rate_limit_reset_secs.unwrap_or(0);
                el.child(
                    div().v_flex().gap(px(6.0)).p_3().rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                        .child(div().h_flex().justify_between()
                            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY).child("Rate Limit".to_string()))
                            .child(div().text_xs().text_color(theme::TEXT_MUTED).child(format!("{rem}/{total} · resets {}m", reset / 60))))
                        .child(div().w_full().h(px(4.0)).rounded_full().bg(theme::TEXT_MUTED.opacity(0.1))
                            .child(div().h_full().rounded_full().bg(color).w(relative(pct)))),
                )
            })
            // Today stats (4 cards)
            .child(
                div().h_flex().gap_2()
                    .child(stat_card("Subtasks", &format!("{}", agent.subtasks_completed), theme::SUCCESS))
                    .child(stat_card("Failures", &format!("{}", agent.subtasks_failed), if agent.subtasks_failed > 0 { theme::ERROR } else { theme::TEXT_MUTED }))
                    .child(stat_card("Avg time", &format!("{}s", agent.avg_subtask_secs), theme::TEXT_MUTED))
                    .child(stat_card("QA rate", &format!("{:.0}%", agent.qa_first_pass_rate * 100.0), if agent.qa_first_pass_rate > 0.8 { theme::SUCCESS } else { theme::WARNING })),
            )
    }

    // ── Available Tab ────────────────────────────────────────

    fn render_available(&self, cx: &mut Context<Self>) -> Div {
        let cards: Vec<Div> = self.available.iter().map(|agent| {
            let cmd = agent.install_command.clone();
            let badge_text = agent.badge.clone();

            div()
                .v_flex().gap(px(6.0)).p_3().rounded_lg()
                .bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                // Name + badge + Install
                .child(
                    div().h_flex().justify_between().items_center()
                        .child(
                            div().h_flex().gap_2().items_center()
                                .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()))
                                .when(badge_text.is_some(), |el: Div| {
                                    let b = badge_text.unwrap_or_default();
                                    let c = if b == "Popular" { theme::WARNING } else { theme::PRIMARY };
                                    el.child(div().text_xs().px(px(5.0)).py(px(1.0)).rounded(px(3.0))
                                        .bg(c.opacity(0.15)).text_color(c).font_weight(FontWeight::BOLD).child(b))
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("install-{}", agent.name)))
                                .cursor_pointer()
                                .on_click(cx.listener(move |_this, _e, _window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(cmd.clone()));
                                }))
                                .child(
                                    div().h_flex().gap_1().items_center()
                                        .px(px(8.0)).py(px(4.0)).rounded_md()
                                        .bg(theme::PRIMARY.opacity(0.1)).text_color(theme::PRIMARY)
                                        .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.2)))
                                        .child(Icon::new(IconName::ArrowDown).size_3().text_color(theme::PRIMARY))
                                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child("Install".to_string())),
                                ),
                        ),
                )
                // Vendor + pricing
                .child(
                    div().text_xs().text_color(theme::TEXT_MUTED)
                        .child(format!("{} · {}", agent.vendor, agent.pricing)),
                )
                // Install command (click to copy)
                .child(
                    div()
                        .id(SharedString::from(format!("copy-{}", agent.name)))
                        .h_flex().gap_1().items_center().cursor_pointer()
                        .px(px(6.0)).py(px(3.0)).rounded(px(4.0))
                        .bg(theme::BACKGROUND.opacity(0.5))
                        .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.06)))
                        .on_click(cx.listener({
                            let cmd = agent.install_command.clone();
                            move |_this, _e, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(cmd.clone()));
                            }
                        }))
                        .child(Icon::new(IconName::Copy).size_3().text_color(theme::TEXT_MUTED.opacity(0.4)))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(agent.install_command.clone())),
                )
        }).collect();

        div().size_full().v_flex().gap_3().p_4()
            .child(div().text_xs().text_color(theme::TEXT_MUTED)
                .child("Install an agent, then restart Surge to auto-detect it.".to_string()))
            .child(
                div().flex().flex_wrap().gap_3().content_start()
                    .children(cards.into_iter().map(|c| div().w(px(300.0)).child(c))),
            )
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn info_item(label: &str, value: &str) -> Div {
    div().v_flex().gap(px(2.0))
        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(label.to_string()))
        .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::TEXT_PRIMARY).child(value.to_string()))
}

fn stat_card(label: &str, value: &str, color: Hsla) -> Div {
    div().flex_1().v_flex().gap(px(3.0)).p(px(10.0)).rounded_lg()
        .bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string()))
        .child(div().text_base().font_weight(FontWeight::BOLD).text_color(color).child(value.to_string()))
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
        div().size_full().v_flex()
            // Header: title + tabs
            .child(
                div().h_flex().gap_3().items_center().px_4().pt_3().pb_1()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY)
                        .child(format!("Agents ({})", self.configured.len())))
                    .child(self.render_tabs(cx)),
            )
            // Content
            .child(match self.active_tab {
                HubTab::Configured => {
                    let items: Vec<Stateful<Div>> = self.configured.iter().enumerate()
                        .map(|(i, a)| self.render_agent_item(i, a, cx)).collect();

                    div().flex_1().h_flex().overflow_hidden()
                        // Left: simple list
                        .child(
                            div().w(px(300.0)).flex_shrink_0().h_full().v_flex().gap_0().p_2()
                                .border_r_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                                .children(items),
                        )
                        // Right: read-only detail
                        .child(
                            div().id("detail-scroll").flex_1().h_full().min_w_0()
                                .v_flex().gap_3().p_4().overflow_y_scroll()
                                .child(self.render_detail()),
                        )
                }
                HubTab::Available => self.render_available(cx),
            })
    }
}
