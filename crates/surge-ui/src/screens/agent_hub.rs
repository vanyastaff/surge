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
    pub last_seen: Option<String>,
    pub recent_sessions: Vec<SessionEntry>,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub label: String,
    pub status: SessionStatus,
    pub time_ago: String,
    pub tokens: Option<u64>,
    pub duration: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus { Running, Completed, Failed }

#[derive(Debug, Clone)]
pub struct AvailableAgent {
    pub name: String,
    pub display_name: String,
    pub vendor: String,
    pub vendor_color: Hsla,
    pub description: String,
    pub pricing: String,
    pub install_command: String,
    pub install_method: String, // "npm", "brew", "pip", "download", "ollama"
    pub badges: Vec<(String, Hsla)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HubTab { Installed, Available, Benchmarks }

impl HubTab {
    fn all() -> &'static [Self] { &[Self::Installed, Self::Available, Self::Benchmarks] }
}

// ── Screen ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogFilter { All, Free, Paid }

impl CatalogFilter {
    fn label(self) -> &'static str {
        match self { Self::All => "All", Self::Free => "Free", Self::Paid => "Paid" }
    }
    fn all() -> &'static [Self] { &[Self::All, Self::Free, Self::Paid] }
}

pub struct AgentHubScreen {
    configured: Vec<ConfiguredAgent>,
    available: Vec<AvailableAgent>,
    selected: Option<usize>,
    active_tab: HubTab,
    search: String,
    filter: CatalogFilter,
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
                    uptime: "4h 23m".into(), last_seen: None,
                    recent_sessions: vec![
                        SessionEntry { label: "auth-refactor / subtask 3".into(), status: SessionStatus::Running, time_ago: "12s ago".into(), tokens: None, duration: None },
                        SessionEntry { label: "auth-refactor / subtask 2".into(), status: SessionStatus::Completed, time_ago: "2m ago".into(), tokens: Some(23_000), duration: Some("1m 12s".into()) },
                        SessionEntry { label: "auth-refactor / subtask 1".into(), status: SessionStatus::Completed, time_ago: "4m ago".into(), tokens: Some(12_000), duration: Some("34s".into()) },
                        SessionEntry { label: "fix-login / subtask 3".into(), status: SessionStatus::Failed, time_ago: "1h ago".into(), tokens: Some(31_000), duration: Some("2m 01s".into()) },
                    ],
                },
                ConfiguredAgent {
                    name: "aider".into(), display_name: "Aider".into(),
                    description: "AI pair programming in terminal. Supports multiple LLM backends".into(),
                    model: Some("claude-sonnet-4-5".into()), binary: "aider".into(),
                    active_sessions: 0, requests_today: 0, tokens_today: 0,
                    cost_today: 0.0, avg_latency_ms: 0, sessions_today: 0,
                    rate_limit_remaining: None, rate_limit_total: None, rate_limit_reset_secs: None,
                    subtasks_completed: 0, subtasks_failed: 0, avg_subtask_secs: 0, qa_first_pass_rate: 0.0,
                    uptime: "—".into(), last_seen: None, recent_sessions: vec![],
                },
            ],
            available: {
                let google = hsla(217.0/360.0, 0.9, 0.6, 1.0);
                let openai = hsla(150.0/360.0, 0.6, 0.45, 1.0);
                let square = hsla(25.0/360.0, 0.8, 0.55, 1.0);
                let mistral = hsla(25.0/360.0, 0.8, 0.55, 1.0);
                let amazon = hsla(30.0/360.0, 0.9, 0.5, 1.0);
                let oss = theme::TEXT_MUTED;
                let alibaba = hsla(210.0/360.0, 0.6, 0.5, 1.0);
                let badge_popular = theme::WARNING;
                let badge_new = theme::SUCCESS;
                let badge_local = hsla(210.0/360.0, 0.7, 0.55, 1.0);
                let badge_oss = theme::TEXT_MUTED;
                vec![
                    AvailableAgent { name: "gemini-cli".into(), display_name: "Gemini CLI".into(), vendor: "Google".into(), vendor_color: google,
                        description: "Google's AI coding assistant with Gemini models and 1M token context".into(),
                        pricing: "Free tier".into(), install_command: "npm install -g @google/gemini-cli".into(), install_method: "npm".into(),
                        badges: vec![("Popular".into(), badge_popular)] },
                    AvailableAgent { name: "codex".into(), display_name: "OpenAI Codex".into(), vendor: "OpenAI".into(), vendor_color: openai,
                        description: "OpenAI's autonomous coding agent with sandboxed execution".into(),
                        pricing: "API key".into(), install_command: "npm install -g @openai/codex".into(), install_method: "npm".into(),
                        badges: vec![("Popular".into(), badge_popular)] },
                    AvailableAgent { name: "goose".into(), display_name: "Goose".into(), vendor: "Square".into(), vendor_color: square,
                        description: "Open source AI developer agent with extensible toolkit".into(),
                        pricing: "Free".into(), install_command: "brew install goose".into(), install_method: "brew".into(),
                        badges: vec![("Popular".into(), badge_popular), ("OSS".into(), badge_oss)] },
                    AvailableAgent { name: "cline".into(), display_name: "Cline".into(), vendor: "Open source".into(), vendor_color: oss,
                        description: "Autonomous coding agent with multi-model support".into(),
                        pricing: "Free".into(), install_command: "npm install -g cline".into(), install_method: "npm".into(),
                        badges: vec![("OSS".into(), badge_oss)] },
                    AvailableAgent { name: "devstral".into(), display_name: "Devstral".into(), vendor: "Mistral".into(), vendor_color: mistral,
                        description: "Mistral's coding-focused model optimized for dev tasks".into(),
                        pricing: "API key".into(), install_command: "pip install mistral-cli".into(), install_method: "pip".into(),
                        badges: vec![("New".into(), badge_new)] },
                    AvailableAgent { name: "kiro".into(), display_name: "Kiro".into(), vendor: "Amazon".into(), vendor_color: amazon,
                        description: "Amazon's spec-driven AI IDE for structured development".into(),
                        pricing: "Free preview".into(), install_command: "Download from kiro.dev".into(), install_method: "download".into(),
                        badges: vec![("New".into(), badge_new)] },
                    AvailableAgent { name: "qwen-coder".into(), display_name: "Qwen3-Coder".into(), vendor: "Alibaba".into(), vendor_color: alibaba,
                        description: "Code generation model, runs fully locally via Ollama".into(),
                        pricing: "Free".into(), install_command: "ollama pull qwen3-coder".into(), install_method: "ollama".into(),
                        badges: vec![("Local".into(), badge_local), ("OSS".into(), badge_oss)] },
                ]
            },
            selected: Some(0),
            active_tab: HubTab::Installed,
            search: String::new(),
            filter: CatalogFilter::All,
        }
    }

    // ── Tabs ─────────────────────────────────────────────────

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = HubTab::all().iter().map(|&tab| {
            let is_active = tab == self.active_tab;
            let label = match tab {
                HubTab::Installed => format!("Installed ({})", self.configured.len()),
                HubTab::Available => format!("Available ({})", self.available.len()),
                HubTab::Benchmarks => "Benchmarks".to_string(),
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
                    } else if agent.sessions_today > 0 {
                        format!("idle · last {}", agent.last_seen.as_deref().unwrap_or("recently"))
                    } else {
                        "never used".to_string()
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
            // Stats (4 cards with period labels)
            .child(
                div().h_flex().gap_2()
                    .child(stat_card_with_period("Requests", &format!("{}", agent.requests_today), "today", theme::PRIMARY))
                    .child(stat_card_with_period("Tokens", &format_tokens(agent.tokens_today), "today", theme::PRIMARY))
                    .child(stat_card_with_period("Cost", &format!("${:.2}", agent.cost_today), "today", theme::WARNING))
                    .child(stat_card_with_period("Latency", &format!("{}ms", agent.avg_latency_ms), "avg", latency_color(agent.avg_latency_ms))),
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
            // Today header + stats
            .child(
                div().v_flex().gap_2()
                    .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child("Today".to_string()))
                    .child(
                        div().h_flex().gap_2()
                            .child(stat_card_with_period("Subtasks", &format!("{}", agent.subtasks_completed), "completed", theme::SUCCESS))
                            .child(stat_card_with_period("Failures", &format!("{}", agent.subtasks_failed), "", if agent.subtasks_failed > 0 { theme::ERROR } else { theme::TEXT_MUTED }))
                            .child(stat_card_with_period("Avg time", &format!("{}s", agent.avg_subtask_secs), "/subtask", theme::TEXT_MUTED))
                            .child(stat_card_with_period("QA rate", &format!("{:.0}%", agent.qa_first_pass_rate * 100.0), "first-pass", if agent.qa_first_pass_rate > 0.8 { theme::SUCCESS } else { theme::WARNING })),
                    ),
            )
            // Recent Sessions
            .when(!agent.recent_sessions.is_empty(), |el: Div| {
                let sessions: Vec<Div> = agent.recent_sessions.iter().map(|s| {
                    let (icon, color) = match s.status {
                        SessionStatus::Running => ("⚡", theme::WARNING),
                        SessionStatus::Completed => ("✓", theme::SUCCESS),
                        SessionStatus::Failed => ("✕", theme::ERROR),
                    };
                    div().h_flex().gap_3().items_center().px_3().py(px(6.0))
                        .border_b_1().border_color(theme::TEXT_MUTED.opacity(0.04))
                        .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.02)))
                        // Icon
                        .child(div().text_xs().text_color(color).w(px(14.0)).child(icon.to_string()))
                        // Label
                        .child(div().flex_1().text_xs().text_color(theme::TEXT_PRIMARY).child(s.label.clone()))
                        // Status
                        .child(div().text_xs().text_color(color).w(px(70.0)).child(match s.status {
                            SessionStatus::Running => "running".to_string(),
                            SessionStatus::Completed => "completed".to_string(),
                            SessionStatus::Failed => "failed".to_string(),
                        }))
                        // Time ago
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).w(px(60.0)).child(s.time_ago.clone()))
                        // Tokens + duration
                        .when(s.tokens.is_some() || s.duration.is_some(), |el: Div| {
                            let info = [
                                s.tokens.map(|t| format!("{} tok", format_tokens(t))),
                                s.duration.clone(),
                            ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                            el.child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.4)).child(info))
                        })
                }).collect();

                el.child(
                    div().v_flex().gap_2()
                        .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child("Recent Sessions".to_string()))
                        .child(
                            div().v_flex().rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06)).overflow_hidden()
                                .children(sessions)),
                )
            })
    }

    // ── Available Tab ────────────────────────────────────────

    fn filtered_available(&self) -> Vec<&AvailableAgent> {
        self.available.iter().filter(|a| {
            if !self.search.is_empty() {
                let q = self.search.to_lowercase();
                if !a.display_name.to_lowercase().contains(&q)
                    && !a.vendor.to_lowercase().contains(&q)
                    && !a.description.to_lowercase().contains(&q) {
                    return false;
                }
            }
            match self.filter {
                CatalogFilter::All => true,
                CatalogFilter::Free => a.pricing.to_lowercase().contains("free"),
                CatalogFilter::Paid => !a.pricing.to_lowercase().contains("free"),
            }
        }).collect()
    }

    fn render_available(&self, cx: &mut Context<Self>) -> Div {
        let filtered = self.filtered_available();
        let cards: Vec<Div> = filtered.iter().enumerate().map(|(i, agent)| {
            let cmd = agent.install_command.clone();
            let is_even = i % 2 == 0;
            let initial = agent.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();

            div()
                .w_full()
                .h_flex().gap_3().items_center()
                .px_3().py(px(10.0))
                .rounded_lg()
                .bg(if is_even { theme::SURFACE.opacity(0.5) } else { gpui::transparent_black() })
                .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.04)))
                // Vendor avatar (colored initial)
                .child(
                    div().w(px(32.0)).h(px(32.0)).rounded_lg().flex_shrink_0()
                        .flex().items_center().justify_center()
                        .bg(agent.vendor_color.opacity(0.15))
                        .text_color(agent.vendor_color)
                        .text_xs().font_weight(FontWeight::BOLD)
                        .child(initial),
                )
                // Name + vendor + badges
                .child(
                    div().w(px(200.0)).flex_shrink_0().v_flex().gap(px(3.0))
                        .child(
                            div().h_flex().gap(px(6.0)).items_center()
                                .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()))
                                .children(agent.badges.iter().map(|(label, color)| {
                                    div().text_xs().px(px(5.0)).py(px(1.0)).rounded(px(3.0))
                                        .bg(color.opacity(0.15)).text_color(*color)
                                        .font_weight(FontWeight::BOLD).child(label.clone())
                                })),
                        )
                        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(format!("{} · {}", agent.vendor, agent.pricing))),
                )
                // Description (brighter)
                .child(
                    div().flex_1().min_w_0()
                        .text_xs().text_color(theme::TEXT_MUTED.opacity(0.8))
                        .line_height(relative(1.4))
                        .max_h(px(30.0)).overflow_hidden()
                        .child(agent.description.clone()),
                )
                // Install method label
                .child(
                    div().flex_shrink_0().w(px(60.0))
                        .text_xs().text_color(theme::TEXT_MUTED.opacity(0.5))
                        .child(format!("via {}", agent.install_method)),
                )
                // Install button (filled primary)
                .child(
                    div()
                        .id(SharedString::from(format!("install-{}", agent.name)))
                        .flex_shrink_0().cursor_pointer()
                        .on_click(cx.listener(move |_this, _e, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(cmd.clone()));
                        }))
                        .child(
                            div().h_flex().gap_1().items_center()
                                .px(px(12.0)).py(px(5.0)).rounded_md()
                                .bg(theme::PRIMARY)
                                .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                                .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.85)))
                                .child(Icon::new(IconName::ArrowDown).size_3().text_color(hsla(0.0, 0.0, 1.0, 1.0)))
                                .child(div().text_xs().font_weight(FontWeight::BOLD).child("Install".to_string())),
                        ),
                )
        }).collect();

        div().size_full().v_flex().gap_3().p_4()
            // Search bar + filter chips
            .child(
                div().h_flex().gap_3().items_center()
                    .child(
                        div().flex_1().h_flex().gap_2().items_center()
                            .px_3().py(px(6.0)).rounded_lg()
                            .bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.08))
                            .child(Icon::new(IconName::Search).size_3p5().text_color(theme::TEXT_MUTED.opacity(0.4)))
                            .child(
                                div().text_xs()
                                    .text_color(if self.search.is_empty() { theme::TEXT_MUTED.opacity(0.4) } else { theme::TEXT_PRIMARY })
                                    .child(if self.search.is_empty() { "Search agents...".to_string() } else { self.search.clone() }),
                            ),
                    )
                    .child(
                        div().h_flex().gap_1().children(
                            CatalogFilter::all().iter().map(|&f| {
                                let is_active = f == self.filter;
                                div()
                                    .id(SharedString::from(format!("cf-{}", f.label())))
                                    .px(px(8.0)).py(px(4.0)).rounded_full().cursor_pointer().text_xs()
                                    .bg(if is_active { theme::PRIMARY.opacity(0.12) } else { gpui::transparent_black() })
                                    .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                                    .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.06)))
                                    .on_click(cx.listener(move |this, _e, _w, cx| { this.filter = f; cx.notify(); }))
                                    .child(f.label().to_string())
                            }),
                        ),
                    ),
            )
            // Subtitle
            .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5))
                .child("Click Install to copy the command. Surge auto-detects agents after installation.".to_string()))
            // Rows
            .child(
                div().v_flex().gap_0().rounded_lg().overflow_hidden().children(cards),
            )
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn info_item(label: &str, value: &str) -> Div {
    div().v_flex().gap(px(2.0))
        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(label.to_string()))
        .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::TEXT_PRIMARY).child(value.to_string()))
}

fn stat_card_with_period(label: &str, value: &str, period: &str, color: Hsla) -> Div {
    div().flex_1().v_flex().gap(px(3.0)).p(px(10.0)).rounded_lg()
        .bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06))
        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string()))
        .child(
            div().h_flex().gap(px(4.0)).items_end()
                .child(div().text_base().font_weight(FontWeight::BOLD).text_color(color).child(value.to_string()))
                .when(!period.is_empty(), |el: Div| {
                    el.child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.4)).pb(px(1.0)).child(period.to_string()))
                }),
        )
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
            // Tabs only (no duplicate title — top bar already shows "Agents")
            .child(
                div().px_4().pt_3().pb_2()
                    .child(self.render_tabs(cx)),
            )
            // Content
            .child(match self.active_tab {
                HubTab::Installed => {
                    let items: Vec<Stateful<Div>> = self.configured.iter().enumerate()
                        .map(|(i, a)| self.render_agent_item(i, a, cx)).collect();

                    div().flex_1().h_flex().overflow_hidden()
                        .child(
                            div().w(px(300.0)).flex_shrink_0().h_full().v_flex().gap_0().p_2()
                                .border_r_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                                .children(items),
                        )
                        .child(
                            div().id("detail-scroll").flex_1().h_full().min_w_0()
                                .v_flex().gap_3().p_4().overflow_y_scroll()
                                .child(self.render_detail()),
                        )
                }
                HubTab::Available => self.render_available(cx),
                HubTab::Benchmarks => {
                    div().flex_1().v_flex().items_center().justify_center().gap_2()
                        .child(Icon::new(IconName::ChartPie).size_8().text_color(theme::TEXT_MUTED.opacity(0.15)))
                        .child(div().text_sm().text_color(theme::TEXT_MUTED.opacity(0.4)).child("Agent Benchmarks".to_string()))
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.3)).child("Compare agent performance — coming in Phase 7".to_string()))
                }
            })
    }
}
