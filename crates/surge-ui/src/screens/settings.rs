use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, IconName, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::app_state::AppState;
use crate::theme;

// ── Settings page identifiers ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    // App
    Appearance,
    Agents,
    Keybindings,
    EditorPaths,
    General,
    // Project
    Pipeline,
    Routing,
    Budgets,
    GitWorktrees,
    Resilience,
    McpServers,
    ContextMemory,
    Integrations,
}

impl SettingsPage {
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Agents => "Agents",
            Self::Keybindings => "Keybindings",
            Self::EditorPaths => "Editor & Paths",
            Self::General => "General",
            Self::Pipeline => "Pipeline",
            Self::Routing => "Routing",
            Self::Budgets => "Budgets",
            Self::GitWorktrees => "Git & Worktrees",
            Self::Resilience => "Resilience",
            Self::McpServers => "MCP Servers",
            Self::ContextMemory => "Context & Memory",
            Self::Integrations => "Integrations",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Appearance => "Theme, mode, colors",
            Self::Agents => "Default agent",
            Self::Keybindings => "Keyboard shortcuts",
            Self::EditorPaths => "IDE integration",
            Self::General => "Logs, version",
            Self::Pipeline => "Gates, parallelism, QA",
            Self::Routing => "How work is assigned",
            Self::Budgets => "Cost & token limits",
            Self::GitWorktrees => "Worktrees, clean-up",
            Self::Resilience => "Timeouts, retries",
            Self::McpServers => "Context protocol",
            Self::ContextMemory => "Knowledge base",
            Self::Integrations => "Linear, GitHub, Telegram",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Appearance => IconName::Palette,
            Self::Agents => IconName::Bot,
            Self::Keybindings => IconName::Asterisk,
            Self::EditorPaths => IconName::Folder,
            Self::General => IconName::Settings2,
            Self::Pipeline => IconName::Loader,
            Self::Routing => IconName::Replace,
            Self::Budgets => IconName::ChartPie,
            Self::GitWorktrees => IconName::FolderOpen,
            Self::Resilience => IconName::Heart,
            Self::McpServers => IconName::Settings,
            Self::ContextMemory => IconName::BookOpen,
            Self::Integrations => IconName::ExternalLink,
        }
    }

    fn app_pages() -> &'static [SettingsPage] {
        &[
            Self::Appearance,
            Self::Agents,
            Self::Keybindings,
            Self::EditorPaths,
            Self::General,
        ]
    }

    fn project_pages() -> &'static [SettingsPage] {
        &[
            Self::Pipeline,
            Self::Routing,
            Self::Budgets,
            Self::GitWorktrees,
            Self::Resilience,
            Self::McpServers,
            Self::ContextMemory,
            Self::Integrations,
        ]
    }

    fn content_subtitle(self) -> &'static str {
        match self {
            Self::Appearance => "Customize how Surge looks",
            Self::Agents => "Which agent new missions use",
            Self::Keybindings => "Shortcuts available everywhere in the app",
            Self::EditorPaths => "Editor and the files every mission starts from",
            Self::General => "Logging and version",
            Self::Pipeline => "Gates, parallelism, and QA configuration",
            Self::Routing => "How work is assigned to agents",
            Self::Budgets => "Cost and token limits per mission",
            Self::GitWorktrees => "Where missions work, and what is cleaned up",
            Self::Resilience => "How long to wait, and what to do when agents fail",
            Self::McpServers => "Model Context Protocol server configuration",
            Self::ContextMemory => "Knowledge base and memory configuration",
            Self::Integrations => "Trackers and chat configured in surge.toml",
        }
    }

    fn config_ref(self) -> Option<&'static str> {
        match self {
            Self::Pipeline => Some("surge.toml [pipeline]"),
            Self::Routing => Some("surge.toml [routing]"),
            Self::Budgets => Some("surge.toml [analytics]"),
            Self::GitWorktrees => Some("surge.toml [cleanup]"),
            Self::Resilience => Some("surge.toml [resilience]"),
            Self::General => Some("surge.toml [log]"),
            Self::EditorPaths => Some("surge.toml [ide]"),
            _ => None,
        }
    }
}

// ── Appearance mode ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceMode {
    Light,
    Dark,
}

impl AppearanceMode {
    fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Light => IconName::Sun,
            Self::Dark => IconName::Moon,
        }
    }
}

// Use theme system from crate::theme for color themes

// ── Keybinding helper trait ─────────────────────────────────────────

trait AsKeybinding {
    fn as_kb(&self) -> (&str, &str, &str);
}

struct Kb {
    action: &'static str,
    keys: &'static str,
    description: &'static str,
}

impl AsKeybinding for Kb {
    fn as_kb(&self) -> (&str, &str, &str) {
        (self.action, self.keys, self.description)
    }
}

// ── Settings screen ────────────────────────────────────────────────

pub struct SettingsScreen {
    state: Entity<AppState>,
    active_page: SettingsPage,
    // Appearance
    appearance_mode: AppearanceMode,
    selected_theme: theme::ThemeName,
    theme_mode: theme::ThemeMode,
    // Pipeline (synced with config)
    gate_after_spec: bool,
    gate_after_plan: bool,
    gate_after_each_subtask: bool,
    gate_after_qa: bool,
    max_parallel: usize,
    max_qa_iterations: u32,
    // Git
    remove_worktrees_on_complete: bool,
    keep_branches_days: u32,
    // IDE
    editor: String,
    auto_open_worktree: bool,
    // Routing
    routing_strategy: surge_core::config::RoutingStrategy,
    default_agent: String,
    // Logging
    log_level: String,
    log_max_size_mb: u64,
    // Budgets
    budget_usd: Option<f64>,
    budget_tokens: Option<u64>,
    // Resilience
    connect_timeout_secs: u64,
    prompt_timeout_secs: u64,
    prompt_retries: u32,
    circuit_breaker_threshold: u32,
    budget_warn_threshold: u8,
    sandbox_default: surge_core::sandbox::SandboxMode,
    /// Values as last read from / written to surge.toml; saving writes only
    /// what differs from these.
    baseline: Option<Baseline>,
    /// Why the last save failed — shown in the save bar, not just logged.
    save_error: Option<String>,
    /// "Saved" confirmation after a successful save.
    saved: bool,
}

/// The editable values at load / last save.
#[derive(Clone, PartialEq)]
struct Baseline {
    gate_after_spec: bool,
    gate_after_plan: bool,
    gate_after_each_subtask: bool,
    gate_after_qa: bool,
    max_parallel: usize,
    max_qa_iterations: u32,
    log_level: String,
    log_max_size_mb: u64,
    routing_strategy: surge_core::config::RoutingStrategy,
    default_agent: String,
    budget_usd: Option<f64>,
    budget_tokens: Option<u64>,
    budget_warn_threshold: u8,
    connect_timeout_secs: u64,
    prompt_timeout_secs: u64,
    prompt_retries: u32,
    circuit_breaker_threshold: u32,
    remove_worktrees_on_complete: bool,
    keep_branches_days: u32,
    sandbox_default: surge_core::sandbox::SandboxMode,
}

/// Serialized form of an enum as surge.toml spells it.
fn toml_str<T: serde::Serialize>(value: &T) -> String {
    toml::Value::try_from(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

impl SettingsScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let st = state.read(cx);
        let cfg = st.config.as_ref();

        let pipeline = cfg.map(|c| &c.pipeline);
        let gates = pipeline.map(|p| &p.gates);
        let cleanup = cfg.map(|c| &c.cleanup);
        let ide = cfg.map(|c| &c.ide);
        let resilience = cfg.map(|c| &c.resilience);
        let analytics = cfg.map(|c| &c.analytics);
        let log = cfg.map(|c| &c.log);

        Self {
            state,
            active_page: SettingsPage::Pipeline,
            appearance_mode: match theme::current().1 {
                theme::ThemeMode::Dark => AppearanceMode::Dark,
                theme::ThemeMode::Light => AppearanceMode::Light,
            },
            selected_theme: theme::current().0,
            theme_mode: theme::current().1,
            // Pipeline — from config
            gate_after_spec: gates.is_none_or(|g| g.after_spec),
            gate_after_plan: gates.is_none_or(|g| g.after_plan),
            gate_after_each_subtask: gates.is_some_and(|g| g.after_each_subtask),
            gate_after_qa: gates.is_none_or(|g| g.after_qa),
            max_parallel: pipeline.map_or(3, |p| p.max_parallel),
            max_qa_iterations: pipeline.map_or(10, |p| p.max_qa_iterations),
            // Git
            remove_worktrees_on_complete: cleanup.is_none_or(|c| c.remove_worktrees_on_complete),
            keep_branches_days: cleanup.map_or(7, |c| c.keep_branches_days),
            // IDE
            editor: ide
                .and_then(|i| i.editor.clone())
                .unwrap_or_else(|| "Not set".into()),
            auto_open_worktree: ide.is_some_and(|i| i.auto_open_worktree),
            // Routing
            routing_strategy: cfg.map(|c| c.routing.strategy.clone()).unwrap_or_default(),
            default_agent: cfg.map(|c| c.default_agent.clone()).unwrap_or_default(),
            // Logging
            log_level: log.map_or_else(|| "info".into(), |l| l.level.clone()),
            log_max_size_mb: log.map_or(50, |l| l.max_size_mb),
            // Budgets
            budget_usd: analytics.and_then(|a| a.budget_usd),
            budget_tokens: analytics.and_then(|a| a.budget_tokens),
            // Resilience
            connect_timeout_secs: resilience.map_or(120, |r| r.connect_timeout_secs),
            prompt_timeout_secs: resilience.map_or(600, |r| r.prompt_timeout_secs),
            prompt_retries: resilience.map_or(3, |r| r.prompt_retries),
            circuit_breaker_threshold: resilience.map_or(5, |r| r.circuit_breaker_threshold),
            budget_warn_threshold: analytics.map_or(80, |a| a.budget_warn_threshold),
            sandbox_default: cfg.map_or(surge_core::sandbox::SandboxMode::WorkspaceWrite, |c| {
                c.init.sandbox_default
            }),
            baseline: None,
            save_error: None,
            saved: false,
        }
        .with_baseline()
    }

    fn snapshot(&self) -> Baseline {
        Baseline {
            gate_after_spec: self.gate_after_spec,
            gate_after_plan: self.gate_after_plan,
            gate_after_each_subtask: self.gate_after_each_subtask,
            gate_after_qa: self.gate_after_qa,
            max_parallel: self.max_parallel,
            max_qa_iterations: self.max_qa_iterations,
            log_level: self.log_level.clone(),
            log_max_size_mb: self.log_max_size_mb,
            routing_strategy: self.routing_strategy.clone(),
            default_agent: self.default_agent.clone(),
            budget_usd: self.budget_usd,
            budget_tokens: self.budget_tokens,
            budget_warn_threshold: self.budget_warn_threshold,
            connect_timeout_secs: self.connect_timeout_secs,
            prompt_timeout_secs: self.prompt_timeout_secs,
            prompt_retries: self.prompt_retries,
            circuit_breaker_threshold: self.circuit_breaker_threshold,
            remove_worktrees_on_complete: self.remove_worktrees_on_complete,
            keep_branches_days: self.keep_branches_days,
            sandbox_default: self.sandbox_default,
        }
    }

    fn with_baseline(mut self) -> Self {
        self.baseline = Some(self.snapshot());
        self
    }

    /// Keys whose value differs from what surge.toml held.
    fn changes(&self) -> crate::config_edit::Changes {
        use crate::config_edit::Change;
        let mut out = crate::config_edit::Changes::new();
        let Some(base) = &self.baseline else { return out };
        let now = self.snapshot();
        let mut put = |key: &str, changed: bool, change: Change| {
            if changed {
                out.insert(key.to_string(), change);
            }
        };
        put("pipeline.gates.after_spec", now.gate_after_spec != base.gate_after_spec, Change::Bool(now.gate_after_spec));
        put("pipeline.gates.after_plan", now.gate_after_plan != base.gate_after_plan, Change::Bool(now.gate_after_plan));
        put(
            "pipeline.gates.after_each_subtask",
            now.gate_after_each_subtask != base.gate_after_each_subtask,
            Change::Bool(now.gate_after_each_subtask),
        );
        put("pipeline.gates.after_qa", now.gate_after_qa != base.gate_after_qa, Change::Bool(now.gate_after_qa));
        put("pipeline.max_parallel", now.max_parallel != base.max_parallel, Change::Int(now.max_parallel as i64));
        put(
            "pipeline.max_qa_iterations",
            now.max_qa_iterations != base.max_qa_iterations,
            Change::Int(i64::from(now.max_qa_iterations)),
        );
        put("log.level", now.log_level != base.log_level, Change::Str(now.log_level.clone()));
        put("log.max_size_mb", now.log_max_size_mb != base.log_max_size_mb, Change::Int(now.log_max_size_mb as i64));
        put(
            "routing.strategy",
            now.routing_strategy != base.routing_strategy,
            Change::Str(toml_str(&now.routing_strategy)),
        );
        put(
            "default_agent",
            now.default_agent != base.default_agent && !now.default_agent.is_empty(),
            Change::Str(now.default_agent.clone()),
        );
        put(
            "analytics.budget_usd",
            now.budget_usd != base.budget_usd,
            now.budget_usd.map_or(Change::Unset, Change::Float),
        );
        put(
            "analytics.budget_tokens",
            now.budget_tokens != base.budget_tokens,
            now.budget_tokens.map_or(Change::Unset, |t| Change::Int(t as i64)),
        );
        put(
            "analytics.budget_warn_threshold",
            now.budget_warn_threshold != base.budget_warn_threshold,
            Change::Int(i64::from(now.budget_warn_threshold)),
        );
        put(
            "resilience.connect_timeout_secs",
            now.connect_timeout_secs != base.connect_timeout_secs,
            Change::Int(now.connect_timeout_secs as i64),
        );
        put(
            "resilience.prompt_timeout_secs",
            now.prompt_timeout_secs != base.prompt_timeout_secs,
            Change::Int(now.prompt_timeout_secs as i64),
        );
        put(
            "resilience.prompt_retries",
            now.prompt_retries != base.prompt_retries,
            Change::Int(i64::from(now.prompt_retries)),
        );
        put(
            "resilience.circuit_breaker_threshold",
            now.circuit_breaker_threshold != base.circuit_breaker_threshold,
            Change::Int(i64::from(now.circuit_breaker_threshold)),
        );
        put(
            "cleanup.remove_worktrees_on_complete",
            now.remove_worktrees_on_complete != base.remove_worktrees_on_complete,
            Change::Bool(now.remove_worktrees_on_complete),
        );
        put(
            "cleanup.keep_branches_days",
            now.keep_branches_days != base.keep_branches_days,
            Change::Int(i64::from(now.keep_branches_days)),
        );
        put(
            "init.sandbox_default",
            now.sandbox_default != base.sandbox_default,
            Change::Str(toml_str(&now.sandbox_default)),
        );
        out
    }

    fn is_dirty(&self) -> bool {
        !self.changes().is_empty()
    }

    /// Write only the changed keys into surge.toml (comments and every
    /// other key untouched), validated before writing. A failure is shown
    /// in the save bar and the edits stay pending so the person can retry.
    fn save_config(&mut self, cx: &mut Context<Self>) {
        let changes = self.changes();
        if changes.is_empty() {
            return;
        }
        let result: Result<(), String> = self.state.update(cx, |state, _cx| {
            let project_path = state
                .project_path
                .clone()
                .ok_or_else(|| "Open a project before saving settings.".to_string())?;
            let config = crate::config_edit::apply(&project_path.join("surge.toml"), &changes)?;
            state.config = Some(config);
            Ok(())
        });
        match result {
            Ok(()) => {
                tracing::info!(keys = changes.len(), "settings saved to surge.toml");
                self.baseline = Some(self.snapshot());
                self.save_error = None;
                self.saved = true;
            },
            Err(msg) => {
                tracing::error!("{msg}");
                self.save_error = Some(msg);
                self.saved = false;
            },
        }
        cx.notify();
    }

    /// Discard pending edits: back to what surge.toml holds.
    fn revert(&mut self, cx: &mut Context<Self>) {
        if let Some(base) = self.baseline.clone() {
            self.gate_after_spec = base.gate_after_spec;
            self.gate_after_plan = base.gate_after_plan;
            self.gate_after_each_subtask = base.gate_after_each_subtask;
            self.gate_after_qa = base.gate_after_qa;
            self.max_parallel = base.max_parallel;
            self.max_qa_iterations = base.max_qa_iterations;
            self.log_level = base.log_level;
            self.log_max_size_mb = base.log_max_size_mb;
            self.routing_strategy = base.routing_strategy;
            self.default_agent = base.default_agent;
            self.budget_usd = base.budget_usd;
            self.budget_tokens = base.budget_tokens;
            self.budget_warn_threshold = base.budget_warn_threshold;
            self.connect_timeout_secs = base.connect_timeout_secs;
            self.prompt_timeout_secs = base.prompt_timeout_secs;
            self.prompt_retries = base.prompt_retries;
            self.circuit_breaker_threshold = base.circuit_breaker_threshold;
            self.remove_worktrees_on_complete = base.remove_worktrees_on_complete;
            self.keep_branches_days = base.keep_branches_days;
            self.sandbox_default = base.sandbox_default;
        }
        self.save_error = None;
        cx.notify();
    }

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.saved = false;
        self.save_error = None;
        cx.notify();
    }

    // ── Internal sidebar ───────────────────────────────────────────

    fn render_settings_sidebar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("settings-sidebar")
            .v_flex()
            .w(px(240.0))
            .h_full()
            .flex_shrink_0()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .py_4()
            .overflow_y_scroll()
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Settings)
                                    .size_5()
                                    .text_color(theme::text_primary()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("Settings"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child("App & Project configuration"),
                    ),
            )
            .child(self.render_sidebar_section("APP", SettingsPage::app_pages(), cx))
            .child(self.render_project_header(cx))
            .child(self.render_sidebar_section("PROJECT", SettingsPage::project_pages(), cx))
    }

    fn render_sidebar_section(
        &self,
        title: &str,
        pages: &[SettingsPage],
        cx: &mut Context<Self>,
    ) -> Div {
        let items: Vec<Stateful<Div>> = pages
            .iter()
            .map(|&page| self.render_sidebar_item(page, cx))
            .collect();

        div()
            .v_flex()
            .gap_0p5()
            .px_2()
            .pb_1()
            .child(
                div()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_muted().opacity(0.5))
                    .child(title.to_string()),
            )
            .children(items)
    }

    fn render_project_header(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let project_name = &state.project_name;
        let project_path = state
            .project_path
            .as_ref()
            .map(|p| crate::ui::abbreviate_home(p))
            .unwrap_or_else(|| "(no project)".into());

        div()
            .mx_2()
            .mt_3()
            .p_2()
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(theme::hairline())
            .h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(26.0))
                    .h(px(26.0))
                    .rounded_md()
                    .bg(theme::primary().opacity(0.2))
                    .flex_shrink_0()
                    .child(
                        div()
                            .size_full()
                            .h_flex()
                            .justify_center()
                            .items_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::primary())
                            .child(
                                project_name
                                    .chars()
                                    .next()
                                    .unwrap_or('S')
                                    .to_uppercase()
                                    .to_string(),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(project_name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(project_path),
                    ),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .size_4()
                    .text_color(theme::text_muted()),
            )
    }

    fn render_sidebar_item(&self, page: SettingsPage, cx: &mut Context<Self>) -> Stateful<Div> {
        let is_active = page == self.active_page;
        let icon_color = if is_active {
            theme::accent()
        } else {
            theme::text_muted()
        };
        let is_pipeline = matches!(page, SettingsPage::Pipeline);

        let base = div()
            .id(SharedString::from(format!("sp-{}", page.label())))
            .role(Role::Button)
            .aria_label(format!("Settings: {}", page.label()))
            .h_flex()
            .gap_2p5()
            .px_2()
            .py(px(5.0))
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.active_page = page;
                cx.notify();
            }));

        let base = if is_active {
            base.bg(theme::primary().opacity(0.15))
        } else {
            base.hover(|s: StyleRefinement| s.bg(theme::panel_raised()))
        };

        let mut row = base
            .child(
                div()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded_md()
                    .bg(if is_active {
                        theme::accent().opacity(0.14)
                    } else {
                        theme::panel_raised()
                    })
                    .border_1()
                    .border_color(if is_active {
                        theme::accent().opacity(0.35)
                    } else {
                        theme::hairline()
                    })
                    .flex_shrink_0()
                    .child(
                        div()
                            .size_full()
                            .h_flex()
                            .justify_center()
                            .items_center()
                            .child(Icon::new(page.icon()).size_3p5().text_color(icon_color)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_primary())
                            .child(page.label().to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted().opacity(0.6))
                            .child(page.subtitle().to_string()),
                    ),
            );

        if is_pipeline {
            row = row.child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(theme::accent().opacity(0.14))
                    .text_color(theme::accent())
                    .child("CORE"),
            );
        }

        row
    }

    // ── Content area ───────────────────────────────────────────────

    fn render_content(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let page = self.active_page;

        div()
            .id("settings-content")
            .flex_1()
            .min_h(px(0.0))
            .v_flex()
            .overflow_y_scroll()
            .p_6()
            .child(self.render_page_header(page))
            .child(match page {
                SettingsPage::Appearance => self.render_appearance(cx),
                SettingsPage::Agents => self.render_agents(cx),
                SettingsPage::Pipeline => self.render_pipeline(cx),
                SettingsPage::GitWorktrees => self.render_git(cx),
                SettingsPage::EditorPaths => self.render_editor_paths(cx),
                SettingsPage::Budgets => self.render_budgets(cx),
                SettingsPage::General => self.render_general(cx),
                SettingsPage::Resilience => self.render_resilience(cx),
                SettingsPage::Routing => self.render_routing(cx),
                SettingsPage::Keybindings => self.render_keybindings(),
                SettingsPage::McpServers => self.render_mcp_servers(cx),
                SettingsPage::ContextMemory => self.render_context_memory(cx),
                SettingsPage::Integrations => self.render_integrations(cx),
            })

    }

    fn render_page_header(&self, page: SettingsPage) -> Div {
        let mut header = div()
            .v_flex()
            .gap_1()
            .pb_4()
            .mb_6()
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child(page.label().to_string()),
            );

        let subtitle = page.content_subtitle();
        if let Some(config_ref) = page.config_ref() {
            header = header.child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(format!("{subtitle} — maps to")),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::panel_raised())
                            .border_1()
                            .border_color(theme::hairline_strong())
                            .text_color(theme::text_primary())
                            .child(config_ref.to_string()),
                    ),
            );
        } else {
            header = header.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(subtitle.to_string()),
            );
        }

        header
    }

    /// Pinned footer: pending edits, the save error if the last save
    /// failed, or a quiet confirmation after a save.
    fn render_save_bar(&self, cx: &mut Context<Self>) -> Div {
        let pending = self.changes().len();
        let (dot, text): (Hsla, String) = match (&self.save_error, pending) {
            (Some(err), _) => (theme::error(), err.clone()),
            (None, 0) => (theme::success(), "Saved to surge.toml".into()),
            (None, n) => (
                theme::warning(),
                format!("{n} unsaved change{} — only these keys are written; comments stay", if n == 1 { "" } else { "s" }),
            ),
        };
        div()
            .flex_none()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(24.0))
            .py(px(12.0))
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(crate::ui::status_dot(dot))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(if self.save_error.is_some() { theme::error() } else { theme::text_muted() })
                    .child(text),
            )
            .when(pending > 0, |el| {
                el.child(
                    Button::new("settings-revert")
                        .ghost()
                        .label("Discard")
                        .on_click(cx.listener(|this, _event, _window, cx| this.revert(cx))),
                )
                .child(
                    Button::new("settings-save")
                        .primary()
                        .label(if self.save_error.is_some() { "Try again" } else { "Save" })
                        .on_click(cx.listener(|this, _event, _window, cx| this.save_config(cx))),
                )
            })
    }

    // ── Appearance page ────────────────────────────────────────────

    fn render_appearance(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap_8()
            .child(self.render_appearance_mode(cx))
            .child(self.render_color_themes(cx))
            .child(self.render_accent_color())
    }

    fn render_appearance_mode(&self, cx: &mut Context<Self>) -> Div {
        let modes = [AppearanceMode::Dark, AppearanceMode::Light];
        let cards: Vec<Stateful<Div>> = modes
            .iter()
            .map(|&mode| {
                let is_selected = mode == self.appearance_mode;
                div()
                    .id(SharedString::from(format!("mode-{}", mode.label())))
                    .role(Role::Button)
                    .aria_label(format!("Appearance: {}", mode.label()))
                    .flex_1()
                    .v_flex()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .py_6()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(if is_selected {
                        theme::primary().opacity(0.12)
                    } else {
                        theme::panel_raised()
                    })
                    .border_1()
                    .border_color(if is_selected {
                        theme::primary().opacity(0.4)
                    } else {
                        theme::text_muted().opacity(0.1)
                    })
                    .hover(|s: StyleRefinement| {
                        s.bg(theme::primary().opacity(0.08))
                            .border_color(theme::primary().opacity(0.25))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.appearance_mode = mode;
                        this.theme_mode = match mode {
                            AppearanceMode::Dark => theme::ThemeMode::Dark,
                            AppearanceMode::Light => theme::ThemeMode::Light,
                        };
                        theme::apply_theme(this.selected_theme, this.theme_mode, cx);
                        cx.notify();
                    }))
                    .child(Icon::new(mode.icon()).size_6().text_color(if is_selected {
                        theme::text_primary()
                    } else {
                        theme::text_muted()
                    }))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_selected {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .child(mode.label().to_string()),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_3()
            .child(self.section_title("Appearance Mode"))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child("Dark is the primary surface; light keeps the same vocabulary"),
            )
            .child(div().h_flex().gap_3().children(cards))
    }

    fn render_color_themes(&self, cx: &mut Context<Self>) -> Div {
        let cards: Vec<Stateful<Div>> = theme::ThemeName::all()
            .iter()
            .map(|&tn| {
                let is_selected = tn == self.selected_theme;
                div()
                    .id(SharedString::from(format!("theme-{}", tn.label())))
                    .role(Role::Button)
                    .aria_label(format!("Theme: {}", tn.label()))
                    .flex_1()
                    .p_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(if is_selected {
                        theme::primary().opacity(0.5)
                    } else {
                        theme::text_muted().opacity(0.1)
                    })
                    .hover(|s: StyleRefinement| s.border_color(theme::primary().opacity(0.3)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.selected_theme = tn;
                        theme::apply_theme(tn, this.theme_mode, cx);
                        cx.notify();
                    }))
                    .h_flex()
                    .gap_3()
                    .child(
                        div()
                            .w(px(18.0))
                            .h(px(18.0))
                            .rounded_full()
                            .bg(tn.accent())
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .v_flex()
                            .child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text_primary())
                                            .child(tn.label().to_string()),
                                    )
                                    .when(is_selected, |el: Div| {
                                        el.child(
                                            Icon::new(IconName::CircleCheck)
                                                .size_4()
                                                .text_color(theme::success()),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted())
                                    .child(tn.description().to_string()),
                            ),
                    )
            })
            .collect();

        let mut grid = div().v_flex().gap_3();
        let mut current_row = div().h_flex().gap_3();
        for (i, card) in cards.into_iter().enumerate() {
            current_row = current_row.child(card);
            if (i + 1) % 4 == 0 {
                grid = grid.child(current_row);
                current_row = div().h_flex().gap_3();
            }
        }
        // The last, partial row (6 themes → 4 + 2) used to be dropped.
        if !theme::ThemeName::all().len().is_multiple_of(4) {
            grid = grid.child(current_row);
        }

        div()
            .v_flex()
            .gap_3()
            .child(self.section_title("Color Theme"))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child("Select a color palette for the interface"),
            )
            .child(grid)
    }

    /// The fixed color vocabulary every screen uses; only the accent
    /// (agent) color follows the theme.
    fn render_accent_color(&self) -> Div {
        use crate::theme::Semantic;
        let roles = [
            (Semantic::Agent, "an agent is working (follows your theme)"),
            (Semantic::Plan, "written down: plans, specs, roadmaps"),
            (Semantic::Verified, "proven by a check"),
            (Semantic::You, "waiting on you"),
            (Semantic::Failure, "failed or blocked"),
            (Semantic::Loop, "repeats: loops and retries"),
            (Semantic::External, "idle or outside Surge"),
        ];
        div()
            .v_flex()
            .gap_3()
            .child(self.section_title("What the colors mean"))
            .child(
                div()
                    .v_flex()
                    .gap(px(6.0))
                    .children(roles.into_iter().map(|(role, meaning)| {
                        div()
                            .h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .child(div().w(px(110.0)).child(crate::ui::legend_chip(role, None)))
                            .child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(meaning))
                    })),
            )
    }

    // ── Agents page ────────────────────────────────────────────────

    fn render_agents(&self, cx: &mut Context<Self>) -> Div {
        let state = self.state.read(cx);
        let default_agent = if self.default_agent.is_empty() {
            state
                .config
                .as_ref()
                .map(|c| c.default_agent.clone())
                .unwrap_or_default()
        } else {
            self.default_agent.clone()
        };

        let rows: Vec<(String, bool, String, String, gpui_kit::Hsla)> = state
            .installed_agents
            .iter()
            .map(|a| {
                let is_default = a.entry.id == default_agent;
                let model = a
                    .entry
                    .models
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "-".to_string());
                let path = a.command_path.clone().unwrap_or_else(|| "(unknown)".into());
                let dot = match state.agent_health(&a.entry.id).map(|h| h.status()) {
                    Some(surge_acp::HealthStatus::Healthy) => theme::success(),
                    Some(surge_acp::HealthStatus::Degraded) => theme::warning(),
                    Some(surge_acp::HealthStatus::Offline) => theme::error(),
                    None => theme::text_muted(),
                };
                (a.entry.id.clone(), is_default, model, path, dot)
            })
            .collect();

        let agents: Vec<Stateful<Div>> = rows
            .into_iter()
            .map(|(id, is_default, model, path, dot)| {
                let id_for_click = id.clone();
                div()
                    .id(SharedString::from(format!("set-default-{id}")))
            .role(Role::Button)
            .aria_label(format!("Set default agent {id}"))
                    .h_flex()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(if is_default {
                        theme::accent().opacity(0.4)
                    } else {
                        theme::hairline()
                    })
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.3)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.default_agent = id_for_click.clone();
                        this.mark_dirty(cx);
                    }))
                    // Health status dot
                    .child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(dot)
                            .flex_shrink_0(),
                    )
                    // Info
                    .child(
                        div()
                            .flex_1()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text_primary())
                                            .child(id.clone()),
                                    )
                                    .when(is_default, |el: Div| {
                                        el.child(
                                            div()
                                                .text_size(px(10.0))
                                                .px(px(6.0))
                                                .py(px(1.0))
                                                .rounded(px(4.0))
                                                .bg(theme::accent().opacity(0.14))
                                                .text_color(theme::accent())
                                                .child("DEFAULT"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted())
                                    .child(format!("Model: {model}  ·  {path}")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted().opacity(0.7))
                            .child(if is_default { "" } else { "click to set default" }),
                    )
            })
            .collect();

        // Available (not installed)
        let available = state.available_agents();
        let available_rows: Vec<Div> = available
            .iter()
            .map(|entry| {
                div()
                    .h_flex()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::text_muted().opacity(0.06))
                    .child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(theme::text_muted().opacity(0.3))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::text_muted())
                                    .child(entry.id.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted().opacity(0.6))
                                    .child(format!("Install: {}", entry.install_instructions)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::text_muted().opacity(0.1))
                            .text_color(theme::text_muted())
                            .child("Not installed"),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_6()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Installed Agents"))
                    .children(agents),
            )
            .when(!available_rows.is_empty(), |el: Div| {
                el.child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(self.section_title("Available Agents"))
                        .children(available_rows),
                )
            })
    }

    // ── Pipeline page ──────────────────────────────────────────────

    fn render_pipeline(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap_8()
            .child(self.render_pipeline_gates(cx))
            .child(self.render_pipeline_execution(cx))
            .child(self.render_sandbox_default(cx))
    }

    /// The permission mode new missions give their agents (init.sandbox_default).
    fn render_sandbox_default(&self, cx: &mut Context<Self>) -> Div {
        use surge_core::sandbox::SandboxMode;
        let modes = [
            (SandboxMode::ReadOnly, "Read only", "Reads files; changes nothing."),
            (SandboxMode::WorkspaceWrite, "Edit the project", "Writes project files; no shell, no network."),
            (SandboxMode::WorkspaceNetwork, "Edit + web", "Also fetches from the web."),
            (SandboxMode::FullAccess, "Full access", "No restrictions — use with care."),
        ];
        let cards: Vec<Stateful<Div>> = modes
            .into_iter()
            .map(|(mode, name, note)| {
                let active = self.sandbox_default == mode;
                let color = if mode == SandboxMode::FullAccess { theme::error() } else { theme::accent() };
                div()
                    .id(SharedString::from(format!("sandbox-{name}")))
                    .role(Role::Button)
                    .aria_label(name)
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(4.0))
                    .p(px(12.0))
                    .rounded(px(crate::ui::R_CONTROL + 2.0))
                    .border_1()
                    .border_color(if active { theme::stroke(color) } else { theme::hairline() })
                    .bg(if active { theme::tint(color) } else { theme::panel_raised() })
                    .cursor_pointer()
                    .hover(|st: StyleRefinement| st.border_color(theme::hairline_strong()))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.sandbox_default = mode;
                        this.mark_dirty(cx);
                    }))
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_primary()).child(name))
                    .child(div().text_size(px(11.0)).text_color(theme::text_muted()).child(note))
            })
            .collect();
        div()
            .v_flex()
            .gap(px(10.0))
            .child(self.section_title("What agents may touch"))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child("The default for new missions. Each agent's exact flags are on the Agents screen."),
            )
            .child(div().flex().gap(px(10.0)).children(cards))
    }

    fn render_pipeline_gates(&self, cx: &mut Context<Self>) -> Div {
        let gates = [
            (
                0_usize,
                "After Spec",
                "Review requirements",
                self.gate_after_spec,
            ),
            (1, "After Plan", "Review architecture", self.gate_after_plan),
            (
                2,
                "After Each Subtask",
                "Review every step",
                self.gate_after_each_subtask,
            ),
            (3, "After QA", "Review before merge", self.gate_after_qa),
        ];

        let mut row1 = div().h_flex().gap_3();
        let mut row2 = div().h_flex().gap_3();

        for (idx, name, desc, enabled) in gates {
            let card = self.render_gate_card(idx, name, desc, enabled, cx);
            if idx < 2 {
                row1 = row1.child(card);
            } else {
                row2 = row2.child(card);
            }
        }

        div()
            .v_flex()
            .gap_3()
            .child(self.section_title("Gates"))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child("Pause pipeline at these checkpoints for human approval"),
            )
            .child(div().v_flex().gap_3().child(row1).child(row2))
    }

    fn render_gate_card(
        &self,
        idx: usize,
        name: &str,
        description: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let indicator_color = if enabled {
            theme::accent()
        } else {
            theme::text_muted()
        };

        div()
            .id(SharedString::from(format!("gate-card-{idx}")))
            .role(Role::Button)
            .aria_label(format!(
                "{name}: {}",
                if enabled { "enabled" } else { "disabled" }
            ))
            .flex_1()
            .h_flex()
            .justify_between()
            .items_center()
            .p_4()
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(theme::hairline())
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::hairline_strong()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                match idx {
                    0 => this.gate_after_spec = !this.gate_after_spec,
                    1 => this.gate_after_plan = !this.gate_after_plan,
                    2 => this.gate_after_each_subtask = !this.gate_after_each_subtask,
                    3 => this.gate_after_qa = !this.gate_after_qa,
                    _ => {},
                }
                this.mark_dirty(cx);
            }))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(description.to_string()),
                    ),
            )
            .child(self.toggle_switch(enabled, indicator_color))
    }

    fn toggle_switch(&self, enabled: bool, color: Hsla) -> Div {
        div()
            .w(px(44.0))
            .h(px(24.0))
            .rounded_full()
            .flex_shrink_0()
            .bg(if enabled {
                color.opacity(0.3)
            } else {
                theme::text_muted().opacity(0.15)
            })
            .child(
                div()
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded_full()
                    .bg(color)
                    .mt(px(3.0))
                    .ml(if enabled { px(23.0) } else { px(3.0) }),
            )
    }

    fn render_pipeline_execution(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap_4()
            .child(self.section_title("Execution"))
            .child(self.render_slider(
                "Max parallel subtasks",
                "Concurrent agents",
                self.max_parallel,
                1,
                16,
                cx,
                |this, val, cx| {
                    this.max_parallel = val;
                    this.mark_dirty(cx);
                },
            ))
            .child(self.render_slider(
                "Max QA iterations",
                "Retry QA loop",
                self.max_qa_iterations as usize,
                1,
                30,
                cx,
                |this, val, cx| {
                    this.max_qa_iterations = val as u32;
                    this.mark_dirty(cx);
                },
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_slider(
        &self,
        label: &str,
        description: &str,
        value: usize,
        min: usize,
        max: usize,
        cx: &mut Context<Self>,
        on_inc: fn(&mut Self, usize, &mut Context<Self>),
    ) -> Div {
        let track_w = 200.0_f32;
        // Clamp value into [min, max] before subtracting so an out-of-range
        // value (e.g. an old config field with no validator capping it)
        // can't underflow `value - min` and produce a giant fraction.
        let clamped = value.clamp(min, max);
        let denom = max.saturating_sub(min).max(1);
        let fraction = (clamped - min) as f32 / denom as f32;
        let fraction = fraction.clamp(0.0, 1.0);
        let fill_w = (fraction * track_w).max(4.0);
        let thumb_left = (fraction * (track_w - 12.0)).max(0.0);

        div()
            .h_flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(description.to_string()),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    // Slider track with thumb
                    .child(
                        div()
                            .w(px(track_w))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(theme::text_muted().opacity(0.15))
                            .relative()
                            // Filled
                            .child(
                                div()
                                    .w(px(fill_w))
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(theme::success()),
                            )
                            // Thumb
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-3.0))
                                    .left(px(thumb_left))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(theme::success())
                                    .border_2()
                                    .border_color(theme::panel_deep()),
                            ),
                    )
                    // +/- buttons
                    .child(
                        div()
                            .id(SharedString::from(format!("slider-dec-{label}")))
            .role(Role::Button)
            .aria_label(format!("Decrease {label}"))
                            .cursor_pointer()
                            .px(px(4.0))
                            .rounded_md()
                            .hover(|s: StyleRefinement| s.bg(theme::panel_raised()))
                            .child(Icon::new(IconName::Minus).size_3p5().text_color(theme::text_muted()))
                            .when(value > min, |el: Stateful<Div>| {
                                el.on_click(cx.listener(move |this, _event, _window, cx| {
                                    let new_val = value.saturating_sub(1).max(min);
                                    on_inc(this, new_val, cx);
                                }))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .min_w(px(28.0))
                            .text_color(theme::success())
                            .child(format!("{value}")),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("slider-inc-{label}")))
            .role(Role::Button)
            .aria_label(format!("Increase {label}"))
                            .cursor_pointer()
                            .px(px(4.0))
                            .rounded_md()
                            .hover(|s: StyleRefinement| s.bg(theme::panel_raised()))
                            .child(Icon::new(IconName::Plus).size_3p5().text_color(theme::text_muted()))
                            .when(value < max, |el: Stateful<Div>| {
                                el.on_click(cx.listener(move |this, _event, _window, cx| {
                                    let new_val = (value + 1).min(max);
                                    on_inc(this, new_val, cx);
                                }))
                            }),
                    ),
            )
    }

    // ── Git & Worktrees page ───────────────────────────────────────

    fn render_git(&self, cx: &mut Context<Self>) -> Div {
        let (current_branch, placement, worktree_root) = {
            let state = self.state.read(cx);
            let (placement, root) = state
                .config
                .as_ref()
                .map(|c| {
                    let placement = match c.init.worktree_location {
                        surge_core::config::WorktreeLocationConfig::Sibling => "Next to the project (.surge-worktrees)",
                        surge_core::config::WorktreeLocationConfig::Central => "In ~/.surge/runs",
                        surge_core::config::WorktreeLocationConfig::Custom => "Custom folder",
                    };
                    (placement.to_string(), crate::ui::abbreviate_home(&c.init.worktree_root))
                })
                .unwrap_or_else(|| ("—".into(), "—".into()));
            (state.current_branch.clone(), placement, root)
        };
        let remove = self.remove_worktrees_on_complete;
        div()
            .v_flex()
            .gap(px(24.0))
            .child(
                div()
                    .v_flex()
                    .gap(px(3.0))
                    .child(self.section_title("Where missions work"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child("Every mission builds in its own git worktree, so your checkout is never touched until you keep the changes."),
                    )
                    .child(self.setting_row("Current branch", &current_branch))
                    .child(self.setting_row("Worktrees", &placement))
                    .child(self.setting_row("Folder", &worktree_root)),
            )
            .child(
                div()
                    .v_flex()
                    .child(self.section_title("Clean-up"))
                    .child(
                        div()
                            .id("cleanup-remove")
                            .role(Role::CheckBox)
                            .aria_label("Remove worktrees when a mission completes")
                            .h_flex()
                            .justify_between()
                            .items_center()
                            .gap(px(16.0))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(theme::hairline().opacity(0.6))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _e, _w, cx| {
                                this.remove_worktrees_on_complete = !this.remove_worktrees_on_complete;
                                this.mark_dirty(cx);
                            }))
                            .child(
                                div()
                                    .v_flex()
                                    .gap(px(2.0))
                                    .child(div().text_size(px(12.5)).text_color(theme::text_primary()).child("Remove worktrees when a mission completes"))
                                    .child(div().text_size(px(11.0)).text_color(theme::text_muted()).child("The branch stays; only the working folder is deleted.")),
                            )
                            .child(self.toggle_switch(remove, theme::accent())),
                    )
                    .child(self.render_stepper(
                        "keep-branches",
                        "Keep mission branches for",
                        "Branches older than this are deleted by clean-up.",
                        if self.keep_branches_days == 0 {
                            "forever".into()
                        } else {
                            format!("{} days", self.keep_branches_days)
                        },
                        self.keep_branches_days > 0,
                        self.keep_branches_days < 90,
                        |this, d| {
                            this.keep_branches_days = (i64::from(this.keep_branches_days) + 7 * d).clamp(0, 90) as u32;
                        },
                        cx,
                    )),
            )
    }

    // ── Editor & Paths page ────────────────────────────────────────

    fn render_editor_paths(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let project_path = state
            .project_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no project)".into());
        let project_context_path = state
            .config
            .as_ref()
            .map(|c| c.init.project_context_path.display().to_string())
            .unwrap_or_else(|| "project.md".into());
        let seed_context = state
            .config
            .as_ref()
            .is_none_or(|c| c.init.project_context_auto_seed);

        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Editor & IDE"))
                    .child(self.setting_row("Default IDE", &self.editor))
                    .child(self.setting_row(
                        "Auto-open worktree",
                        if self.auto_open_worktree {
                            "Enabled"
                        } else {
                            "Disabled"
                        },
                    )),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Paths"))
                    .child(self.setting_row("Project Path", &project_path))
                    .child(self.setting_row("Project context", &project_context_path))
                    .child(self.setting_row(
                        "Seed context into runs",
                        if seed_context { "Yes" } else { "No" },
                    )),
            )
    }

    // ── Routing page ───────────────────────────────────────────────

    fn render_routing(&self, cx: &mut Context<Self>) -> Div {
        use surge_core::config::RoutingStrategy;
        let strategies: [(RoutingStrategy, &'static str, &'static str); 3] = [
            (
                RoutingStrategy::Default,
                "Default",
                "Every task goes to the default agent",
            ),
            (
                RoutingStrategy::Complexity,
                "Complexity",
                "Route by task complexity",
            ),
            (
                RoutingStrategy::RoundRobin,
                "RoundRobin",
                "Rotate across available agents",
            ),
        ];
        let cards: Vec<Stateful<Div>> = strategies
            .iter()
            .map(|(strategy, name, desc)| {
                let (strategy, name, desc) = (strategy.clone(), *name, *desc);
                let is_selected = self.routing_strategy == strategy;
                div()
                    .id(SharedString::from(format!("routing-{name}")))
                    .role(Role::Button)
                    .aria_label(format!("Routing: {name}"))
                    .flex_1()
                    .v_flex()
                    .gap_1()
                    .p_4()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(if is_selected {
                        theme::accent().opacity(0.1)
                    } else {
                        theme::panel_raised()
                    })
                    .border_1()
                    .border_color(if is_selected {
                        theme::accent().opacity(0.45)
                    } else {
                        theme::hairline()
                    })
                    .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.3)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.routing_strategy = strategy.clone();
                        this.mark_dirty(cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_selected {
                                theme::accent()
                            } else {
                                theme::text_primary()
                            })
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(desc),
                    )
            })
            .collect();

        let default_agent = if self.default_agent.is_empty() {
            "(none — pick one on the Agents page)".to_string()
        } else {
            self.default_agent.clone()
        };

        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Strategy"))
                    .child(div().h_flex().gap_2().children(cards)),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Default Agent"))
                    .child(self.setting_row("Agent", &default_agent)),
            )
    }

    // ── Keybindings page ─────────────────────────────────────────────

    fn render_keybindings(&self) -> Div {
        let navigation = [
            Kb {
                action: "Fleet",
                keys: "Ctrl+1",
                description: "The run constellation (home)",
            },
            Kb {
                action: "Roadmap",
                keys: "Ctrl+2",
                description: "Milestones and delivery line",
            },
            Kb {
                action: "Missions",
                keys: "Ctrl+3",
                description: "Per-run cockpit",
            },
            Kb {
                action: "Flow",
                keys: "Ctrl+4",
                description: "DAG editor",
            },
            Kb {
                action: "Inbox",
                keys: "Ctrl+5",
                description: "Decisions blocked on you",
            },
            Kb {
                action: "Backlog",
                keys: "Ctrl+6",
                description: "Dispatch queue",
            },
            Kb {
                action: "Agents",
                keys: "Ctrl+7",
                description: "The crew",
            },
            Kb {
                action: "Memory",
                keys: "Ctrl+8",
                description: "Project knowledge",
            },
            Kb {
                action: "Settings",
                keys: "Ctrl+9",
                description: "This screen",
            },
        ];

        let ui_toggles = [
            Kb {
                action: "Toggle Sidebar",
                keys: "Ctrl+B",
                description: "Show or hide the sidebar",
            },
            Kb {
                action: "Command Palette",
                keys: "Ctrl+K",
                description: "Open command palette",
            },
        ];

        let actions = [
            Kb {
                action: "Switch Project",
                keys: "Ctrl+Shift+P",
                description: "Open project switcher",
            },
            Kb {
                action: "Plan a task",
                keys: "Ctrl+N",
                description: "Describe new work (new app on the start page)",
            },
            Kb {
                action: "Open project",
                keys: "Ctrl+O",
                description: "Pick a project folder",
            },
        ];

        div()
            .v_flex()
            .gap_8()
            // Navigation
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Navigation"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child("Switch between screens"),
                    )
                    .child(self.render_keybinding_group(&navigation)),
            )
            // UI Toggles
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("UI Toggles"))
                    .child(self.render_keybinding_group(&ui_toggles)),
            )
            // Actions
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Actions"))
                    .child(self.render_keybinding_group(&actions)),
            )
    }

    fn render_keybinding_group(&self, bindings: &[impl AsKeybinding]) -> Div {
        let rows: Vec<Div> = bindings
            .iter()
            .map(|kb| {
                let (action, keys, desc) = kb.as_kb();
                let keys = crate::ui::shortcut_label(keys);
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::text_muted().opacity(0.08))
                    .hover(|s: StyleRefinement| {
                        s.border_color(theme::hairline_strong())
                    })
                    // Left: action + description
                    .child(
                        div()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_primary())
                                    .child(action.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::text_muted().opacity(0.7))
                                    .child(desc.to_string()),
                            ),
                    )
                    // Right: key badges
                    .child(self.render_key_combo(&keys))
            })
            .collect();

        div().v_flex().gap_2().children(rows)
    }

    fn render_key_combo(&self, combo: &str) -> Div {
        let parts: Vec<&str> = combo.split('+').collect();
        let badges: Vec<Div> = parts
            .iter()
            .enumerate()
            .flat_map(|(i, part)| {
                let mut items = Vec::new();
                if i > 0 {
                    // Separator
                    items.push(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted().opacity(0.4))
                            .child("+"),
                    );
                }
                items.push(
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(theme::panel_deep())
                        .border_1()
                        .border_color(theme::hairline_strong())
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_primary())
                        .child(part.to_string()),
                );
                items
            })
            .collect();

        div().h_flex().gap_1().items_center().children(badges)
    }


    /// A labelled −/+ control with a human-readable value; `apply` moves
    /// the value by `delta` steps (negative = down).
    #[allow(clippy::too_many_arguments)]
    fn render_stepper(
        &self,
        id: &'static str,
        label: &str,
        description: &str,
        value: String,
        can_dec: bool,
        can_inc: bool,
        apply: fn(&mut Self, i64),
        cx: &mut Context<Self>,
    ) -> Div {
        let button = |suffix: &'static str, icon: IconName, enabled: bool, delta: i64| {
            div()
                .id(SharedString::from(format!("{id}-{suffix}")))
                .role(Role::Button)
                .aria_label(format!("{label} {suffix}"))
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(crate::ui::R_CONTROL))
                .border_1()
                .border_color(theme::hairline_strong())
                .when(!enabled, |el| el.opacity(0.35))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(|s: StyleRefinement| s.bg(theme::surface()))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            apply(this, delta);
                            this.mark_dirty(cx);
                        }))
                })
                .child(Icon::new(icon).size_3p5().text_color(theme::text_muted()))
        };
        div()
            .h_flex()
            .justify_between()
            .items_center()
            .gap(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(theme::hairline().opacity(0.6))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(2.0))
                    .child(div().text_size(px(12.5)).text_color(theme::text_primary()).child(label.to_string()))
                    .child(div().text_size(px(11.0)).text_color(theme::text_muted()).child(description.to_string())),
            )
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .flex_none()
                    .child(button("down", IconName::Minus, can_dec, -1))
                    .child(
                        div()
                            .min_w(px(96.0))
                            .text_center()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(value),
                    )
                    .child(button("up", IconName::Plus, can_inc, 1)),
            )
    }

    // ── Budgets page ───────────────────────────────────────────────

    fn render_budgets(&self, cx: &mut Context<Self>) -> Div {
        let usd = self.budget_usd;
        let tokens = self.budget_tokens;
        div()
            .v_flex()
            .gap(px(24.0))
            .child(
                div()
                    .v_flex()
                    .child(self.section_title("Spending limits"))
                    .child(self.render_stepper(
                        "budget-usd",
                        "Cost per mission",
                        "A mission stops once it has spent this much. Planning and building share one budget.",
                        usd.map_or_else(|| "Unlimited".into(), |v| format!("${v:.0}")),
                        usd.is_some(),
                        true,
                        |this, d| {
                            let next = this.budget_usd.unwrap_or(0.0) + 5.0 * d as f64;
                            this.budget_usd = (next >= 5.0).then_some(next);
                        },
                        cx,
                    ))
                    .child(self.render_stepper(
                        "budget-tokens",
                        "Tokens per mission",
                        "Stop after this many tokens, for agents that report token usage.",
                        tokens.map_or_else(|| "Unlimited".into(), |v| format!("{}k", v / 1000)),
                        tokens.is_some(),
                        true,
                        |this, d| {
                            let next = this.budget_tokens.unwrap_or(0) as i64 + 100_000 * d;
                            this.budget_tokens = (next >= 100_000).then_some(next as u64);
                        },
                        cx,
                    ))
                    .child(self.render_stepper(
                        "budget-warn",
                        "Warn at",
                        "Tell me when a mission has used this share of its budget.",
                        format!("{}%", self.budget_warn_threshold),
                        self.budget_warn_threshold > 50,
                        self.budget_warn_threshold < 95,
                        |this, d| {
                            let next = i64::from(this.budget_warn_threshold) + 5 * d;
                            this.budget_warn_threshold = next.clamp(50, 95) as u8;
                        },
                        cx,
                    )),
            )
    }

    // ── Resilience page ────────────────────────────────────────────

    fn render_resilience(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap(px(24.0))
            .child(
                div()
                    .v_flex()
                    .child(self.section_title("Waiting on agents"))
                    .child(self.render_stepper(
                        "connect-timeout",
                        "Start-up time",
                        "How long an agent may take to start before the step fails.",
                        format!("{} s", self.connect_timeout_secs),
                        self.connect_timeout_secs > 30,
                        self.connect_timeout_secs < 600,
                        |this, d| {
                            this.connect_timeout_secs = (this.connect_timeout_secs as i64 + 30 * d).clamp(30, 600) as u64;
                        },
                        cx,
                    ))
                    .child(self.render_stepper(
                        "prompt-timeout",
                        "Time per answer",
                        "How long one agent turn may run before it is stopped.",
                        format!("{} min", self.prompt_timeout_secs / 60),
                        self.prompt_timeout_secs > 60,
                        self.prompt_timeout_secs < 3600,
                        |this, d| {
                            this.prompt_timeout_secs = (this.prompt_timeout_secs as i64 + 60 * d).clamp(60, 3600) as u64;
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .v_flex()
                    .child(self.section_title("When something fails"))
                    .child(self.render_stepper(
                        "prompt-retries",
                        "Retries",
                        "How many times a failed agent turn is retried.",
                        self.prompt_retries.to_string(),
                        self.prompt_retries > 0,
                        self.prompt_retries < 10,
                        |this, d| {
                            this.prompt_retries = (i64::from(this.prompt_retries) + d).clamp(0, 10) as u32;
                        },
                        cx,
                    ))
                    .child(self.render_stepper(
                        "circuit-breaker",
                        "Pause an agent after",
                        "Consecutive failures before Surge stops sending work to that agent.",
                        format!("{} failures", self.circuit_breaker_threshold),
                        self.circuit_breaker_threshold > 1,
                        self.circuit_breaker_threshold < 20,
                        |this, d| {
                            this.circuit_breaker_threshold =
                                (i64::from(this.circuit_breaker_threshold) + d).clamp(1, 20) as u32;
                        },
                        cx,
                    )),
            )
    }

    // ── General page ───────────────────────────────────────────────

    fn render_general(&self, cx: &mut Context<Self>) -> Div {
        let log_levels = ["error", "warn", "info", "debug", "trace"];
        let level_cards: Vec<Stateful<Div>> = log_levels
            .iter()
            .map(|&level| {
                let is_selected = self.log_level == level;
                let color = match level {
                    "error" => theme::error(),
                    "warn" => theme::warning(),
                    "info" => theme::success(),
                    "debug" => theme::primary(),
                    "trace" => theme::text_muted(),
                    _ => theme::text_muted(),
                };
                div()
                    .id(SharedString::from(format!("log-{level}")))
                    .role(Role::Button)
                    .aria_label(format!("Log level: {level}"))
                    .flex_1()
                    .v_flex()
                    .items_center()
                    .gap_1()
                    .py_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(if is_selected {
                        color.opacity(0.12)
                    } else {
                        theme::panel_raised()
                    })
                    .border_1()
                    .border_color(if is_selected {
                        color.opacity(0.4)
                    } else {
                        theme::text_muted().opacity(0.1)
                    })
                    .hover(|s: StyleRefinement| s.border_color(color.opacity(0.3)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.log_level = level.to_string();
                        this.mark_dirty(cx);
                    }))
                    .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(color))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(if is_selected {
                                FontWeight::BOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(if is_selected {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .child(level.to_uppercase()),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_8()
            // Logging
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Logging"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child("Set the verbosity level for Surge logs"),
                    )
                    .child(div().h_flex().gap_2().children(level_cards))
                    .child(self.render_slider(
                        "Max log file size",
                        "Rotate after this size",
                        self.log_max_size_mb as usize,
                        10,
                        200,
                        cx,
                        |this, val, cx| {
                            this.log_max_size_mb = val as u64;
                            this.mark_dirty(cx);
                        },
                    )),
            )
            // About
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("About"))
                    .child(self.setting_row("Version", env!("CARGO_PKG_VERSION"))),
            )
    }

    // ── Placeholder ────────────────────────────────────────────────

    // ── MCP Servers page (read-only view of surge.toml [[mcp_servers]]) ──

    fn render_mcp_servers(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let servers: Vec<(String, String, String)> = state
            .config
            .as_ref()
            .map(|c| {
                c.mcp_servers
                    .iter()
                    .map(|s| {
                        let tools = match &s.allowed_tools {
                            Some(t) => format!("{} tools allowed", t.len()),
                            None => "all reported tools".to_string(),
                        };
                        let timeout = format!("call timeout {}s", s.call_timeout.as_secs());
                        (s.name.clone(), tools, timeout)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut list = div().v_flex().gap_2();
        if servers.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::hairline())
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(
                        "No MCP servers configured — add [[mcp_servers]] entries to \
                         surge.toml. Each run gets its own supervised instances.",
                    ),
            );
        }
        for (name, tools, timeout) in servers {
            list = list.child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .p_4()
                    .rounded_lg()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(name),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(tools),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(timeout),
                    ),
            );
        }

        div().v_flex().gap_8().child(
            div()
                .v_flex()
                .gap_3()
                .child(self.section_title("Configured Servers"))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child(
                            "Per-run scoped, supervised, sandbox-delegated (ADR-0014). \
                                 Inspect live health with `surge mcp probe`.",
                        ),
                )
                .child(list),
        )
    }

    // ── Context & Memory page (real paths + store status) ──────────

    fn render_context_memory(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let context_path = state
            .config
            .as_ref()
            .map(|c| c.init.project_context_path.display().to_string())
            .unwrap_or_else(|| "project.md".into());
        let auto_seed = state
            .config
            .as_ref()
            .is_none_or(|c| c.init.project_context_auto_seed);

        let (memory_db, memory_status) =
            match surge_persistence::memory::MemoryStore::default_path() {
                Ok(path) => {
                    let status = match std::fs::metadata(&path) {
                        Ok(m) => format!("{} KB", m.len() / 1024),
                        Err(_) => "not created yet — runs write it".to_string(),
                    };
                    (path.display().to_string(), status)
                },
                Err(_) => ("~/.surge/memory.db".to_string(), "unavailable".to_string()),
            };

        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Project Context"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(
                                "The stable context generated by `surge project describe`, \
                                 captured into every run at start.",
                            ),
                    )
                    .child(self.setting_row("Context file", &context_path))
                    .child(
                        self.setting_row(
                            "Auto-seed into runs",
                            if auto_seed { "Yes" } else { "No" },
                        ),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Memory Store"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(
                                "SQLite + FTS5 knowledge base (discoveries, patterns, \
                                 gotchas, file contexts). Browse it on the Memory surface.",
                            ),
                    )
                    .child(self.setting_row("Database", &memory_db))
                    .child(self.setting_row("Size", &memory_status)),
            )
    }

    // ── Integrations page (real surge.toml [[task_sources]]) ───────

    fn render_integrations(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let sources: Vec<(String, String, String)> = state
            .config
            .as_ref()
            .map(|c| {
                c.task_sources
                    .iter()
                    .map(|s| match s {
                        surge_core::config::TaskSourceConfig::Linear(l) => (
                            l.id.clone(),
                            format!("Linear · workspace {}", l.workspace_id),
                            format!("poll {}s", l.poll_interval.as_secs()),
                        ),
                        surge_core::config::TaskSourceConfig::GithubIssues(g) => (
                            g.id.clone(),
                            format!("GitHub Issues · {}", g.repo),
                            format!("poll {}s", g.poll_interval.as_secs()),
                        ),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut list = div().v_flex().gap_2();
        if sources.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::hairline())
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(
                        "No task sources configured — add [[task_sources]] entries \
                         (Linear or GitHub Issues) to surge.toml. The daemon polls \
                         them, triages tickets and routes decisions to the Inbox.",
                    ),
            );
        }
        for (id, kind, poll) in sources {
            list = list.child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .p_4()
                    .rounded_lg()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(id),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(kind),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(poll),
                    ),
            );
        }

        // Telegram: configured only as env-var references (never inline
        // secrets); report whether those variables resolve here.
        let telegram = state.config.as_ref().and_then(|c| c.telegram.clone());
        let telegram_row: (String, String, Hsla) = match &telegram {
            None => (
                "Not set up".into(),
                "Add a [telegram] section to surge.toml to approve plans and follow missions from your phone.".into(),
                theme::text_muted(),
            ),
            Some(t) => {
                let var_ok = |v: &Option<String>| v.as_ref().is_some_and(|name| std::env::var_os(name).is_some());
                let token = t.bot_token_env.clone().unwrap_or_else(|| "(not set)".into());
                let chat = t
                    .chat_id
                    .map(|id| id.to_string())
                    .or_else(|| t.chat_id_env.clone().map(|v| format!("${v}")))
                    .unwrap_or_else(|| "(not set)".into());
                let ready = var_ok(&t.bot_token_env) && (t.chat_id.is_some() || var_ok(&t.chat_id_env));
                (
                    if ready { "Ready".into() } else { "Configured, but its variables are missing here".into() },
                    format!("Bot token from ${token} · chat {chat}"),
                    if ready { theme::success() } else { theme::warning() },
                )
            },
        };

        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Task Sources"))
                    .child(list),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Telegram"))
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .p_4()
                            .rounded_lg()
                            .bg(theme::panel_raised())
                            .border_1()
                            .border_color(theme::hairline())
                            .child(crate::ui::status_dot(telegram_row.2))
                            .child(
                                div()
                                    .v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text_primary())
                                            .child(telegram_row.0),
                                    )
                                    .child(div().text_size(px(11.0)).text_color(theme::text_muted()).child(telegram_row.1)),
                            ),
                    ),
            )
    }

    // ── Shared helpers ─────────────────────────────────────────────

    fn section_title(&self, title: &str) -> Div {
        div()
            .text_base()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text_primary())
            .child(title.to_string())
    }

    fn setting_row(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .justify_between()
            .items_center()
            .py(px(6.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(label.to_string()),
            )
            .child(self.value_box(value))
    }

    fn value_box(&self, value: &str) -> Div {
        div()
            .text_size(px(12.0))
            .text_color(theme::text_primary())
            .px_3()
            .py(px(6.0))
            .min_w(px(60.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(theme::hairline())
            .child(value.to_string())
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Plain .flex() row so sidebar + content stretch to full height
        // (h_flex would vertically center them).
        div()
            .size_full()
            .flex()
            .bg(theme::panel_deep())
            .overflow_hidden()
            .child(self.render_settings_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .v_flex()
                    .child(self.render_content(cx))
                    .when(self.is_dirty() || self.save_error.is_some() || self.saved, |el| {
                        el.child(self.render_save_bar(cx))
                    }),
            )
    }
}

#[cfg(test)]
mod save_tests {
    use super::SettingsScreen;
    use crate::app_state::AppState;
    use gpui_kit::{AppContext as _, TestAppContext};

    #[test]
    fn saving_writes_only_the_touched_key_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("surge.toml");
        std::fs::write(&path, "# team defaults\nschema_version = 1\n\n[pipeline]\nmax_parallel = 2 # laptop\n").unwrap();
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let state = cx.new(|_| {
            let mut s = AppState::new();
            s.project_path = Some(dir.path().to_path_buf());
            s.config = surge_core::SurgeConfig::load(&path).ok();
            s
        });
        let screen = cx.new(|cx| SettingsScreen::new(state, cx));
        screen.update(&mut cx, |screen, cx| {
            assert!(screen.changes().is_empty(), "nothing is pending on open");
            screen.max_parallel = 4;
            assert_eq!(screen.changes().len(), 1);
            screen.save_config(cx);
            assert!(screen.save_error.is_none(), "{:?}", screen.save_error);
            assert!(screen.changes().is_empty(), "saved edits are no longer pending");
        });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# team defaults"));
        assert!(text.contains("max_parallel = 4 # laptop"));
        assert!(!text.contains("max_qa_iterations"), "untouched defaults are not written out");
    }

    #[test]
    fn a_failed_save_is_shown_and_keeps_the_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("surge.toml");
        std::fs::write(&path, "schema_version = 1\n").unwrap();
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let state = cx.new(|_| {
            let mut s = AppState::new();
            s.project_path = Some(dir.path().to_path_buf());
            s.config = surge_core::SurgeConfig::load(&path).ok();
            s
        });
        let screen = cx.new(|cx| SettingsScreen::new(state, cx));
        // The file breaks on disk after the screen opened.
        std::fs::write(&path, "[pipeline\n").unwrap();
        screen.update(&mut cx, |screen, cx| {
            screen.max_parallel = 5;
            screen.save_config(cx);
            assert!(screen.save_error.as_deref().is_some_and(|e| e.contains("syntax error")));
            assert_eq!(screen.changes().len(), 1, "the edit is kept for a retry");
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[pipeline\n");
    }
}
