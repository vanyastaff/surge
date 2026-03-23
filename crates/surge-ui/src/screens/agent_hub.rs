use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
use crate::theme;

// ── Data Models ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ModelOption {
    pub name: String,
    pub price: String,     // "$3/$15"
    pub context: String,   // "1M ctx"
    pub note: String,      // "Daily driver"
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel { High, Medium, Low, Adaptive }

impl EffortLevel {
    fn label(self) -> &'static str {
        match self { Self::High => "High", Self::Medium => "Medium", Self::Low => "Low", Self::Adaptive => "Adaptive" }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionSetting {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct AgentCapabilities {
    /// Available models (None = agent doesn't expose model selection)
    pub models: Option<Vec<ModelOption>>,
    /// Effort/thinking levels (None = not supported)
    pub effort: Option<AgentEffortConfig>,
    /// Permissions (None = not managed via ACP)
    pub permissions: Option<Vec<PermissionSetting>>,
    /// Dangerous ops policy
    pub dangerous_ops: Option<String>, // "Ask permission", "Allow", "Block"
}

#[derive(Debug, Clone)]
pub struct AgentEffortConfig {
    pub default: EffortLevel,
    pub planning: EffortLevel,
    pub coding: EffortLevel,
    pub qa_review: EffortLevel,
}

#[derive(Debug, Clone)]
pub struct ConfiguredAgent {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: Option<String>,
    pub binary: String,
    pub version: Option<String>,
    pub active_sessions: u32,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub cost_today: f64,
    pub avg_latency_ms: u32,
    pub sessions_today: u32,
    // Agent-specific capabilities
    pub capabilities: AgentCapabilities,
    // Usage & Limits — varies per agent
    pub usage: AgentUsage,
    // Today stats
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

/// Usage data varies by agent — Level 1 (native API), Level 2 (estimated), Level 3 (429 detection).
#[derive(Debug, Clone)]
pub enum AgentUsage {
    /// Claude Code: native statusline data
    ClaudeCode {
        five_hour_pct: f32,
        five_hour_reset: String,   // "2h 14m"
        weekly_pct: f32,
        weekly_reset: String,      // "Mon"
        extra_usage_enabled: bool,
        extra_usage_cost: f64,
    },
    /// Estimated from ACP response tokens (Aider, Goose, Cline)
    Estimated {
        provider: String,          // "Anthropic API", "OpenAI API", "Local (Ollama)"
        estimated_tokens: u64,
        estimated_cost: f64,
        is_local: bool,
    },
    /// No data yet
    Unknown,
}

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
    state: Entity<AppState>,
    selected: Option<usize>,
    active_tab: HubTab,
    search: String,
    filter: CatalogFilter,
}

impl AgentHubScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            selected: Some(0),
            active_tab: HubTab::Installed,
            search: String::new(),
            filter: CatalogFilter::All,
        }
    }

    /// Build ConfiguredAgent display data from real DetectedAgent + AgentHealth.
    fn build_configured(&self, cx: &Context<Self>) -> Vec<ConfiguredAgent> {
        let state = self.state.read(cx);
        state.installed_agents.iter().map(|detected| {
            let health = state.health.get_health(&detected.entry.id);
            let (requests, latency, failures) = match health {
                Some(h) => (h.total_requests, h.avg_latency_ms, h.total_failures),
                None => (0, 0, 0),
            };

            // Determine capabilities based on agent ID (from registry knowledge).
            let capabilities = build_agent_capabilities(&detected.entry.id);
            let usage = build_agent_usage(&detected.entry.id);

            ConfiguredAgent {
                name: detected.entry.id.clone(),
                display_name: detected.entry.display_name.clone(),
                description: detected.entry.long_description.clone(),
                model: detected.entry.models.first().cloned(),
                binary: detected.entry.command.clone(),
                version: None, // TODO: detect via `command --version`
                active_sessions: 0, // TODO: from AgentPool
                requests_today: requests as u32,
                tokens_today: 0, // TODO: from usage tracking
                cost_today: 0.0,
                avg_latency_ms: latency as u32,
                sessions_today: 0,
                capabilities,
                usage,
                subtasks_completed: 0,
                subtasks_failed: failures as u32,
                avg_subtask_secs: 0,
                qa_first_pass_rate: 0.0,
                uptime: "—".into(),
                last_seen: None,
                recent_sessions: vec![],
            }
        }).collect()
    }

    /// Build AvailableAgent display data from registry entries NOT installed.
    fn build_available(&self, cx: &Context<Self>) -> Vec<AvailableAgent> {
        let state = self.state.read(cx);
        let installed_ids: Vec<&str> = state.installed_agents.iter().map(|a| a.entry.id.as_str()).collect();

        state.registry.list().iter()
            .filter(|e| !installed_ids.contains(&e.id.as_str()))
            .map(|entry| {
                let vendor_color = vendor_color_for(&entry.id);
                let install_method = extract_install_method(&entry.install_instructions);
                AvailableAgent {
                    name: entry.id.clone(),
                    display_name: entry.display_name.clone(),
                    vendor: entry.tags.first().cloned().unwrap_or_default(),
                    vendor_color,
                    description: entry.long_description.clone(),
                    pricing: if entry.tags.contains(&"free".to_string()) { "Free".into() } else { "API key".into() },
                    install_command: entry.install_instructions.clone(),
                    install_method,
                    badges: build_badges(entry),
                }
            })
            .collect()
    }

    // ── Tabs ─────────────────────────────────────────────────

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = HubTab::all().iter().map(|&tab| {
            let is_active = tab == self.active_tab;
            let label = match tab {
                HubTab::Installed => format!("Installed ({})", self.state.read(cx).installed_agents.len()),
                HubTab::Available => format!("Available ({})", self.build_available(cx).len()),
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

    fn render_detail(&self, configured: &[ConfiguredAgent]) -> Div {
        let Some(idx) = self.selected else {
            return div().flex_1().v_flex().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Bot).size_8().text_color(theme::TEXT_MUTED.opacity(0.15)))
                .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.3)).child("Select an agent".to_string()));
        };
        let Some(agent) = configured.get(idx) else {
            return div().flex_1();
        };

        let version_str = agent.version.as_deref().unwrap_or("—");

        div().flex_1().v_flex().gap_3()
            // Header: icon + name + status + version
            .child(
                div().h_flex().justify_between().items_center()
                    .child(
                        div().h_flex().gap_2().items_center()
                            .child(Icon::new(IconName::Bot).size_5().text_color(theme::PRIMARY))
                            .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY).child(agent.display_name.clone()))
                            .child(div().h_flex().gap(px(4.0)).items_center().px(px(8.0)).py(px(3.0)).rounded_full()
                                .bg(theme::SUCCESS.opacity(0.1))
                                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(theme::SUCCESS))
                                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::SUCCESS).child("Ready".to_string()))),
                    )
                    .child(
                        div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(version_str.to_string()),
                    ),
            )
            // Description
            .child(div().text_xs().text_color(theme::TEXT_MUTED).line_height(relative(1.5)).child(agent.description.clone()))
            // Info row (with version)
            .child(
                div().h_flex().gap_6().pt_2().pb_1().border_t_1().border_b_1().border_color(theme::TEXT_MUTED.opacity(0.06))
                    .child(info_item("Model", agent.model.as_deref().unwrap_or("—")))
                    .child(info_item("Binary", &agent.binary))
                    .child(info_item("Version", version_str))
                    .child(info_item("Uptime", &agent.uptime))
                    .child(info_item("Sessions today", &format!("{}", agent.sessions_today))),
            )
            // ── Models ── (label outside, card inside)
            .when(agent.capabilities.models.is_some(), |el: Div| {
                let rows: Vec<Div> = agent.capabilities.models.as_ref().unwrap().iter().map(|m| {
                    let o = if m.enabled { 1.0 } else { 0.4 };
                    div().w_full().h_flex().items_center().px_3().py(px(6.0))
                        .border_b_1().border_color(theme::TEXT_MUTED.opacity(0.04))
                        .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.02)))
                        .child(div().flex_shrink_0().w(px(20.0)).text_xs()
                            .text_color(if m.enabled { theme::SUCCESS } else { theme::TEXT_MUTED.opacity(0.3) })
                            .child(if m.enabled { "☑" } else { "☐" }))
                        .child(div().flex_1().min_w_0().text_xs().font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY.opacity(o)).child(m.name.clone()))
                        .child(div().flex_shrink_0().w(px(65.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(o * 0.7)).child(m.price.clone()))
                        .child(div().flex_shrink_0().w(px(55.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(o * 0.6)).child(m.context.clone()))
                        .child(div().flex_shrink_0().w(px(100.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(o * 0.5)).child(m.note.clone()))
                }).collect();
                el.child(section_label("Models")).child(section_card(div().v_flex().children(rows)))
            })
            // ── Effort ── (label outside, card inside)
            .when(agent.capabilities.effort.is_some(), |el: Div| {
                let eff = agent.capabilities.effort.as_ref().unwrap();
                el.child(section_label("Effort / Thinking"))
                    .child(section_card(div().v_flex().gap(px(4.0)).p_3()
                        .child(effort_row("Default effort", eff.default))
                        .child(effort_row("Planning", eff.planning))
                        .child(effort_row("Coding", eff.coding))
                        .child(effort_row("QA Review", eff.qa_review))))
            })
            // ── Permissions ── (label outside, chips inside card)
            .when(agent.capabilities.permissions.is_some(), |el: Div| {
                let perms = agent.capabilities.permissions.as_ref().unwrap();
                let chips: Vec<Div> = perms.iter().map(|p| {
                    div().h_flex().gap(px(4.0)).items_center().px(px(8.0)).py(px(4.0)).rounded_md()
                        .bg(if p.enabled { theme::SUCCESS.opacity(0.06) } else { theme::TEXT_MUTED.opacity(0.03) })
                        .border_1().border_color(if p.enabled { theme::SUCCESS.opacity(0.12) } else { theme::TEXT_MUTED.opacity(0.05) })
                        .child(div().text_xs().text_color(if p.enabled { theme::SUCCESS } else { theme::TEXT_MUTED.opacity(0.3) })
                            .child(if p.enabled { "✓" } else { "✕" }))
                        .child(div().text_xs().text_color(if p.enabled { theme::TEXT_PRIMARY } else { theme::TEXT_MUTED.opacity(0.4) })
                            .child(p.name.clone()))
                }).collect();
                let mut content = div().v_flex().gap(px(6.0)).p_3()
                    .child(div().h_flex().gap(px(4.0)).flex_wrap().children(chips));
                if let Some(d) = &agent.capabilities.dangerous_ops {
                    content = content.child(div().h_flex().gap_2().items_center().pt_1()
                        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child("Dangerous ops:".to_string()))
                        .child(div().text_xs().px(px(6.0)).py(px(1.0)).rounded(px(3.0))
                            .bg(theme::WARNING.opacity(0.1)).text_color(theme::WARNING).child(d.clone())));
                }
                el.child(section_label("Permissions")).child(section_card(content))
            })
            // ── Stats ── (label outside)
            .child(section_label("Stats"))
            .child(
                div().h_flex().gap_2()
                    .child(stat_card_with_period("Requests", &format!("{}", agent.requests_today), "today", theme::PRIMARY))
                    .child(stat_card_with_period("Tokens", &format_tokens(agent.tokens_today), "today", theme::PRIMARY))
                    .child(stat_card_with_period("Cost", &format!("${:.2}", agent.cost_today), "today", theme::WARNING))
                    .child(stat_card_with_period("Latency", &format!("{}ms", agent.avg_latency_ms), "avg", latency_color(agent.avg_latency_ms))),
            )
            // ── Usage & Limits ── (label outside)
            .child(section_label("Usage & Limits"))
            .child({
                let mut section = div().v_flex().gap(px(8.0)).p_3().rounded_lg()
                    .bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06));

                match &agent.usage {
                    AgentUsage::ClaudeCode { five_hour_pct, five_hour_reset, weekly_pct, weekly_reset, extra_usage_enabled, extra_usage_cost } => {
                        let fh_color = quota_color(*five_hour_pct);
                        let wk_color = quota_color(*weekly_pct);
                        section = section
                            .child(usage_bar("5-Hour Window", &format!("{:.0}% used · resets {five_hour_reset}", five_hour_pct * 100.0), *five_hour_pct, fh_color))
                            .child(usage_bar("Weekly Quota", &format!("{:.0}% used · resets {weekly_reset}", weekly_pct * 100.0), *weekly_pct, wk_color))
                            .child(
                                div().h_flex().gap_2().items_center()
                                    .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child("Extra Usage".to_string()))
                                    .child(
                                        if *extra_usage_enabled {
                                            div().text_xs().text_color(theme::SUCCESS)
                                                .child(format!("Enabled · ${:.2} this period", extra_usage_cost))
                                        } else {
                                            div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.4))
                                                .child("Disabled".to_string())
                                        },
                                    ),
                            );
                    }
                    AgentUsage::Estimated { provider, estimated_tokens, estimated_cost, is_local } => {
                        section = section
                            .child(
                                div().h_flex().justify_between()
                                    .child(div().text_xs().text_color(theme::TEXT_MUTED).child("Provider".to_string()))
                                    .child(div().text_xs().text_color(theme::TEXT_PRIMARY).child(provider.clone())),
                            )
                            .child(
                                div().h_flex().justify_between()
                                    .child(div().text_xs().text_color(theme::TEXT_MUTED).child("Provider Limits".to_string()))
                                    .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(
                                        if *is_local { "No limits (local model)".to_string() }
                                        else { "Inherited from provider API".to_string() }
                                    )),
                            )
                            .child(
                                div().h_flex().justify_between()
                                    .child(div().text_xs().text_color(theme::TEXT_MUTED).child("Estimated Cost".to_string()))
                                    .child(div().text_xs().text_color(theme::TEXT_PRIMARY)
                                        .child(format!("${:.2} today (~{} tokens)", estimated_cost, format_tokens(*estimated_tokens)))),
                            );
                    }
                    AgentUsage::Unknown => {
                        section = section.child(
                            div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child("No usage data available".to_string()),
                        );
                    }
                }
                section
            })
            // ── Today ── (label outside)
            .child(section_label("Today"))
            .child(
                div().h_flex().gap_2()
                    .child(stat_card_with_period("Subtasks", &format!("{}", agent.subtasks_completed), "completed", theme::SUCCESS))
                    .child(stat_card_with_period("Failures", &format!("{}", agent.subtasks_failed), "", if agent.subtasks_failed > 0 { theme::ERROR } else { theme::TEXT_MUTED }))
                    .child(stat_card_with_period("Avg time", &format!("{}s", agent.avg_subtask_secs), "/subtask", theme::TEXT_MUTED))
                    .child(stat_card_with_period("QA rate", &format!("{:.0}%", agent.qa_first_pass_rate * 100.0), "first-pass", if agent.qa_first_pass_rate > 0.8 { theme::SUCCESS } else { theme::WARNING })),
            )
            // Recent Sessions
            .when(!agent.recent_sessions.is_empty(), |el: Div| {
                let sessions: Vec<Div> = agent.recent_sessions.iter().map(|s| {
                    let (icon, color) = match s.status {
                        SessionStatus::Running => ("⚡", theme::WARNING),
                        SessionStatus::Completed => ("✓", theme::SUCCESS),
                        SessionStatus::Failed => ("✕", theme::ERROR),
                    };
                    let status_label = match s.status {
                        SessionStatus::Running => "running",
                        SessionStatus::Completed => "completed",
                        SessionStatus::Failed => "failed",
                    };
                    let detail = [
                        s.tokens.map(|t| format!("{} tok", format_tokens(t))),
                        s.duration.clone(),
                    ].into_iter().flatten().collect::<Vec<_>>().join(" · ");

                    div().w_full().h_flex().items_center().px_3().py(px(7.0))
                        .border_b_1().border_color(theme::TEXT_MUTED.opacity(0.04))
                        .hover(|s: StyleRefinement| s.bg(theme::PRIMARY.opacity(0.02)))
                        // Icon
                        .child(div().flex_shrink_0().w(px(20.0)).text_xs().text_color(color).child(icon.to_string()))
                        // Label (takes available space)
                        .child(div().flex_1().min_w_0().text_xs().text_color(theme::TEXT_PRIMARY).child(s.label.clone()))
                        // Status badge
                        .child(
                            div().flex_shrink_0().px_2()
                                .child(
                                    div().text_xs().px(px(6.0)).py(px(1.0)).rounded(px(3.0))
                                        .bg(color.opacity(0.1)).text_color(color)
                                        .child(status_label.to_string()),
                                ),
                        )
                        // Time ago
                        .child(div().flex_shrink_0().w(px(70.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).child(s.time_ago.clone()))
                        // Tokens + duration
                        .child(div().flex_shrink_0().w(px(130.0)).text_xs().text_color(theme::TEXT_MUTED.opacity(0.4)).child(
                            if detail.is_empty() { "—".to_string() } else { detail }
                        ))
                }).collect();

                el.child(section_label("Recent Sessions"))
                    .child(section_card(div().v_flex().children(sessions)))
            })
    }

    // ── Available Tab ────────────────────────────────────────

    fn filtered_available<'a>(&self, available: &'a [AvailableAgent]) -> Vec<&'a AvailableAgent> {
        available.iter().filter(|a| {
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
        let available = self.build_available(cx);
        let filtered = self.filtered_available(&available);
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
                // Description (brighter, 3 lines)
                .child(
                    div().flex_1().min_w_0()
                        .text_xs().text_color(theme::TEXT_MUTED.opacity(0.8))
                        .line_height(relative(1.5))
                        .max_h(px(48.0)).overflow_hidden()
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

fn section_label(title: &str) -> Div {
    div().text_xs().font_weight(FontWeight::BOLD).text_color(theme::TEXT_MUTED.opacity(0.6))
        .pt(px(4.0)).child(title.to_string())
}

fn section_card(content: Div) -> Div {
    content.rounded_lg().bg(theme::SURFACE).border_1().border_color(theme::TEXT_MUTED.opacity(0.06)).overflow_hidden()
}

fn kv_row(label: &str, value: &str) -> Div {
    div().h_flex().justify_between()
        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string()))
        .child(div().text_xs().text_color(theme::TEXT_PRIMARY).child(value.to_string()))
}

fn effort_color(level: EffortLevel) -> Hsla {
    match level {
        EffortLevel::High => theme::WARNING,
        EffortLevel::Medium => theme::PRIMARY,
        EffortLevel::Low => theme::SUCCESS,
        EffortLevel::Adaptive => theme::TEXT_MUTED,
    }
}

fn effort_row(label: &str, level: EffortLevel) -> Div {
    let color = effort_color(level);
    div().h_flex().justify_between().items_center()
        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string()))
        .child(
            div().text_xs().px(px(6.0)).py(px(1.0)).rounded(px(3.0))
                .bg(color.opacity(0.1)).text_color(color)
                .child(level.label().to_string()),
        )
}

fn effort_card(label: &str, level: EffortLevel) -> Div {
    let color = effort_color(level);
    div().flex_1().v_flex().gap(px(3.0)).items_center()
        .px(px(8.0)).py(px(6.0)).rounded_md()
        .bg(color.opacity(0.05)).border_1().border_color(color.opacity(0.1))
        .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(label.to_string()))
        .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(color).child(level.label().to_string()))
}

fn quota_color(pct: f32) -> Hsla {
    if pct < 0.5 { theme::SUCCESS } else if pct < 0.8 { theme::WARNING } else { theme::ERROR }
}

fn usage_bar(label: &str, detail: &str, pct: f32, color: Hsla) -> Div {
    div().v_flex().gap(px(4.0))
        .child(
            div().h_flex().justify_between()
                .child(div().text_xs().text_color(theme::TEXT_MUTED).child(label.to_string()))
                .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.6)).child(detail.to_string())),
        )
        .child(
            div().w_full().h(px(5.0)).rounded_full().bg(theme::TEXT_MUTED.opacity(0.1))
                .child(div().h_full().rounded_full().bg(color).w(relative(pct.clamp(0.0, 1.0)))),
        )
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

/// Build agent-specific capabilities (models, effort, permissions) based on agent ID.
fn build_agent_capabilities(agent_id: &str) -> AgentCapabilities {
    match agent_id {
        "claude-code" => AgentCapabilities {
            models: Some(vec![
                ModelOption { name: "Opus 4.6".into(), price: "$5/$25".into(), context: "1M ctx".into(), note: "Heavy reasoning".into(), enabled: true },
                ModelOption { name: "Sonnet 4.6".into(), price: "$3/$15".into(), context: "1M ctx".into(), note: "Daily driver".into(), enabled: true },
                ModelOption { name: "Haiku 4.5".into(), price: "$0.80/$4".into(), context: "200K".into(), note: "Quick tasks".into(), enabled: true },
            ]),
            effort: Some(AgentEffortConfig {
                default: EffortLevel::Adaptive,
                planning: EffortLevel::High,
                coding: EffortLevel::Adaptive,
                qa_review: EffortLevel::Low,
            }),
            permissions: Some(vec![
                PermissionSetting { name: "File read".into(), enabled: true },
                PermissionSetting { name: "File write".into(), enabled: true },
                PermissionSetting { name: "Bash commands".into(), enabled: true },
                PermissionSetting { name: "Network access".into(), enabled: false },
                PermissionSetting { name: "Git push".into(), enabled: false },
            ]),
            dangerous_ops: Some("Ask permission".into()),
        },
        // Other agents — no ACP-managed capabilities
        _ => AgentCapabilities { models: None, effort: None, permissions: None, dangerous_ops: None },
    }
}

/// Build agent-specific usage display based on agent ID.
fn build_agent_usage(agent_id: &str) -> AgentUsage {
    match agent_id {
        "claude-code" => AgentUsage::ClaudeCode {
            five_hour_pct: 0.0, five_hour_reset: "—".into(),
            weekly_pct: 0.0, weekly_reset: "—".into(),
            extra_usage_enabled: false, extra_usage_cost: 0.0,
        },
        _ => AgentUsage::Estimated {
            provider: "Unknown".into(),
            estimated_tokens: 0, estimated_cost: 0.0, is_local: false,
        },
    }
}

fn vendor_color_for(agent_id: &str) -> Hsla {
    match agent_id {
        "claude-code" => hsla(263.0/360.0, 0.85, 0.58, 1.0),
        "copilot-cli" => hsla(210.0/360.0, 0.7, 0.5, 1.0),
        "gemini-cli" => hsla(217.0/360.0, 0.9, 0.6, 1.0),
        "codex-cli" => hsla(150.0/360.0, 0.6, 0.45, 1.0),
        "goose" => hsla(25.0/360.0, 0.8, 0.55, 1.0),
        "aider" => hsla(120.0/360.0, 0.5, 0.5, 1.0),
        "cline" => hsla(340.0/360.0, 0.7, 0.55, 1.0),
        "amp" => hsla(280.0/360.0, 0.6, 0.55, 1.0),
        "devstral" => hsla(35.0/360.0, 0.9, 0.55, 1.0),
        "qwen3-coder" => hsla(200.0/360.0, 0.7, 0.5, 1.0),
        _ => theme::TEXT_MUTED,
    }
}

fn extract_install_method(instructions: &str) -> String {
    let lower = instructions.to_lowercase();
    if lower.contains("npm") { "npm".into() }
    else if lower.contains("brew") { "brew".into() }
    else if lower.contains("pip") { "pip".into() }
    else if lower.contains("cargo") { "cargo".into() }
    else if lower.contains("ollama") { "ollama".into() }
    else if lower.contains("download") { "download".into() }
    else { "manual".into() }
}

fn build_badges(entry: &surge_acp::RegistryEntry) -> Vec<(String, Hsla)> {
    let mut badges = Vec::new();
    // Popular agents
    if entry.tags.contains(&"popular".to_string()) || ["claude-code", "copilot-cli", "codex-cli"].contains(&entry.id.as_str()) {
        badges.push(("Popular".into(), theme::WARNING));
    }
    // OSS tag
    if entry.tags.contains(&"open-source".to_string()) || entry.tags.contains(&"oss".to_string()) {
        badges.push(("OSS".into(), theme::TEXT_MUTED));
    }
    badges
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
                    let configured = self.build_configured(cx);
                    let items: Vec<Stateful<Div>> = configured.iter().enumerate()
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
                                .child(self.render_detail(&configured)),
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
