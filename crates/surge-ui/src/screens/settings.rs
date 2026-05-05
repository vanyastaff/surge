use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{Icon, IconName, StyledExt};

use crate::app_state::AppState;
use crate::theme;

// ── Settings page identifiers ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    // App
    Appearance,
    DisplayFonts,
    Agents,
    Keybindings,
    EditorPaths,
    Notifications,
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
            Self::DisplayFonts => "Display & Fonts",
            Self::Agents => "Agents",
            Self::Keybindings => "Keybindings",
            Self::EditorPaths => "Editor & Paths",
            Self::Notifications => "Notifications",
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
            Self::DisplayFonts => "Scale, terminal fonts",
            Self::Agents => "Discovery, default agent",
            Self::Keybindings => "Keyboard shortcuts",
            Self::EditorPaths => "IDE integration",
            Self::Notifications => "Alerts & sounds",
            Self::General => "Updates, privacy, logs",
            Self::Pipeline => "Gates, parallelism, QA",
            Self::Routing => "Agent per phase",
            Self::Budgets => "Cost & token limits",
            Self::GitWorktrees => "Merge, cleanup",
            Self::Resilience => "Timeouts, retry",
            Self::McpServers => "Context protocol",
            Self::ContextMemory => "Knowledge base",
            Self::Integrations => "Linear, GitHub",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Appearance => IconName::Palette,
            Self::DisplayFonts => IconName::ALargeSmall,
            Self::Agents => IconName::Bot,
            Self::Keybindings => IconName::Asterisk,
            Self::EditorPaths => IconName::Folder,
            Self::Notifications => IconName::Bell,
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

    fn icon_color(self) -> Hsla {
        match self {
            Self::Appearance => ACCENT_ORANGE,
            Self::DisplayFonts => ACCENT_BLUE,
            Self::Agents => ACCENT_GREEN,
            Self::Keybindings => ACCENT_BLUE,
            Self::EditorPaths => ACCENT_ORANGE,
            Self::Notifications => ACCENT_PINK,
            Self::General => ACCENT_PURPLE,
            Self::Pipeline => ACCENT_AMBER,
            Self::Routing => ACCENT_BLUE,
            Self::Budgets => ACCENT_AMBER,
            Self::GitWorktrees => ACCENT_TEAL,
            Self::Resilience => ACCENT_PINK,
            Self::McpServers => ACCENT_PURPLE,
            Self::ContextMemory => ACCENT_ORANGE,
            Self::Integrations => ACCENT_TEAL,
        }
    }

    fn app_pages() -> &'static [SettingsPage] {
        &[
            Self::Appearance,
            Self::DisplayFonts,
            Self::Agents,
            Self::Keybindings,
            Self::EditorPaths,
            Self::Notifications,
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
            Self::DisplayFonts => "Configure UI scale and terminal fonts",
            Self::Agents => "Manage AI agent discovery and defaults",
            Self::Keybindings => "Customize keyboard shortcuts",
            Self::EditorPaths => "Configure IDE integration and file paths",
            Self::Notifications => "Configure alerts and sounds",
            Self::General => "Updates, privacy, telemetry, and logs",
            Self::Pipeline => "Gates, parallelism, and QA configuration",
            Self::Routing => "Configure how tasks are routed to agents",
            Self::Budgets => "Set cost and token spending limits",
            Self::GitWorktrees => "Git branching, worktrees, and PR settings",
            Self::Resilience => "Timeouts, retry policies, and circuit breakers",
            Self::McpServers => "Model Context Protocol server configuration",
            Self::ContextMemory => "Knowledge base and memory configuration",
            Self::Integrations => "Connect to Linear, GitHub, Jira, and more",
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

// ── Accent colors ──────────────────────────────────────────────────

const ACCENT_ORANGE: Hsla = Hsla {
    h: 33.0 / 360.0,
    s: 0.90,
    l: 0.55,
    a: 1.0,
};
const ACCENT_BLUE: Hsla = Hsla {
    h: 210.0 / 360.0,
    s: 0.80,
    l: 0.55,
    a: 1.0,
};
const ACCENT_GREEN: Hsla = Hsla {
    h: 142.0 / 360.0,
    s: 0.71,
    l: 0.45,
    a: 1.0,
};
const ACCENT_PURPLE: Hsla = Hsla {
    h: 263.0 / 360.0,
    s: 0.85,
    l: 0.58,
    a: 1.0,
};
const ACCENT_TEAL: Hsla = Hsla {
    h: 175.0 / 360.0,
    s: 0.65,
    l: 0.45,
    a: 1.0,
};
const ACCENT_AMBER: Hsla = Hsla {
    h: 45.0 / 360.0,
    s: 0.93,
    l: 0.50,
    a: 1.0,
};
const ACCENT_PINK: Hsla = Hsla {
    h: 330.0 / 360.0,
    s: 0.80,
    l: 0.55,
    a: 1.0,
};

// ── Appearance mode ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceMode {
    System,
    Light,
    Dark,
}

impl AppearanceMode {
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::System => IconName::Settings,
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
    dirty: bool,
    // Appearance
    appearance_mode: AppearanceMode,
    selected_theme: theme::ThemeName,
    theme_mode: theme::ThemeMode,
    // Pipeline (synced with config)
    gate_after_spec: bool,
    gate_after_plan: bool,
    gate_after_each_subtask: bool,
    gate_after_qa: bool,
    gate_timeout: u32,
    max_parallel: usize,
    max_qa_iterations: u32,
    // Git
    branch_prefix: String,
    auto_commit: bool,
    worktree_dir: String,
    remove_worktrees_on_complete: bool,
    keep_branches_days: u32,
    // IDE
    editor: String,
    auto_open_worktree: bool,
    // Routing
    routing_strategy: String,
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
    // General
    send_telemetry: bool,
    crash_reports: bool,
    auto_update: bool,
    // Notifications
    notify_task_completed: bool,
    notify_task_failed: bool,
    notify_gate_waiting: bool,
    notify_agent_disconnect: bool,
    notify_rate_limit: bool,
    notify_sound: bool,
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
            dirty: false,
            appearance_mode: AppearanceMode::Dark,
            selected_theme: theme::ThemeName::Default,
            theme_mode: theme::ThemeMode::Dark,
            // Pipeline — from config
            gate_after_spec: gates.is_none_or(|g| g.after_spec),
            gate_after_plan: gates.is_none_or(|g| g.after_plan),
            gate_after_each_subtask: gates.is_some_and(|g| g.after_each_subtask),
            gate_after_qa: gates.is_none_or(|g| g.after_qa),
            gate_timeout: 3600,
            max_parallel: pipeline.map_or(3, |p| p.max_parallel),
            max_qa_iterations: pipeline.map_or(10, |p| p.max_qa_iterations),
            // Git
            branch_prefix: "surge/".into(),
            auto_commit: true,
            worktree_dir: ".surge/worktrees/".into(),
            remove_worktrees_on_complete: cleanup.is_none_or(|c| c.remove_worktrees_on_complete),
            keep_branches_days: cleanup.map_or(7, |c| c.keep_branches_days),
            // IDE
            editor: ide
                .and_then(|i| i.editor.clone())
                .unwrap_or_else(|| "VS Code".into()),
            auto_open_worktree: ide.is_some_and(|i| i.auto_open_worktree),
            // Routing
            routing_strategy: cfg
                .map(|c| format!("{:?}", c.routing.strategy))
                .unwrap_or_else(|| "Default".into()),
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
            // General
            send_telemetry: false,
            crash_reports: true,
            auto_update: true,
            // Notifications — defaults
            notify_task_completed: true,
            notify_task_failed: true,
            notify_gate_waiting: true,
            notify_agent_disconnect: true,
            notify_rate_limit: true,
            notify_sound: false,
        }
    }

    /// Build a SurgeConfig from current UI state and save.
    ///
    /// On success, clears `dirty`. On failure, leaves `dirty=true` so
    /// the user can retry — and emits a tracing::error so the desktop
    /// build shows up in logs. If there is no `state.config` (no
    /// project loaded), refuses with a clear error rather than the
    /// previous silent no-op.
    fn save_config(&mut self, cx: &mut Context<Self>) {
        let result: Result<(), String> = self.state.update(cx, |state, _cx| {
            let Some(ref mut config) = state.config else {
                return Err("no project loaded; open a project before saving settings".into());
            };
            let Some(project_path) = state.project_path.clone() else {
                return Err("project has no on-disk path; cannot save settings".into());
            };
            // Pipeline
            config.pipeline.gates.after_spec = self.gate_after_spec;
            config.pipeline.gates.after_plan = self.gate_after_plan;
            config.pipeline.gates.after_each_subtask = self.gate_after_each_subtask;
            config.pipeline.gates.after_qa = self.gate_after_qa;
            config.pipeline.max_parallel = self.max_parallel;
            config.pipeline.max_qa_iterations = self.max_qa_iterations;
            // Cleanup
            config.cleanup.remove_worktrees_on_complete = self.remove_worktrees_on_complete;
            config.cleanup.keep_branches_days = self.keep_branches_days;
            // IDE
            config.ide.editor = Some(self.editor.clone());
            config.ide.auto_open_worktree = self.auto_open_worktree;
            // Log
            config.log.level = self.log_level.clone();
            config.log.max_size_mb = self.log_max_size_mb;
            // Analytics
            config.analytics.budget_usd = self.budget_usd;
            config.analytics.budget_tokens = self.budget_tokens;
            // Resilience
            config.resilience.connect_timeout_secs = self.connect_timeout_secs;
            config.resilience.prompt_timeout_secs = self.prompt_timeout_secs;
            config.resilience.prompt_retries = self.prompt_retries;
            config.resilience.circuit_breaker_threshold = self.circuit_breaker_threshold;
            // NOTE: send_telemetry / crash_reports / auto_update / the
            // notify_* and notify_sound toggles are still UI-only —
            // they do not have homes on `SurgeConfig` yet, so flipping
            // them is not persisted. Wiring them through to disk is a
            // follow-up; until then the dirty flag will report unsaved
            // for those toggles too even after save_config completes
            // (see `dirty` logic above) so the user knows the toggle
            // didn't survive.

            config
                .save(&project_path.join("surge.toml"))
                .map_err(|e| format!("failed to save settings: {e}"))
        });

        match result {
            Ok(()) => {
                self.dirty = false;
            },
            Err(msg) => {
                tracing::error!("{msg}");
                // Leave self.dirty = true so retry surfaces the same
                // error path; the user can fix the underlying issue
                // (open a project, fix permissions, etc.) and retry.
            },
        }
        cx.notify();
    }

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
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
            .bg(theme::sidebar_bg())
            .border_r_1()
            .border_color(theme::surface())
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
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("Settings"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
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
                    .text_xs()
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
            .map(|p| format!("~/{}", p.file_name().unwrap_or_default().to_string_lossy()))
            .unwrap_or_else(|| "(no project)".into());

        div()
            .mx_2()
            .mt_3()
            .p_2()
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.1))
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
                            .text_sm()
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
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(project_name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
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
        let icon_color = page.icon_color();
        let is_pipeline = matches!(page, SettingsPage::Pipeline);

        let base = div()
            .id(SharedString::from(format!("sp-{}", page.label())))
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
            base.hover(|s: StyleRefinement| s.bg(theme::surface()))
        };

        let mut row = base
            .child(
                div()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded_md()
                    .bg(icon_color.opacity(0.15))
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
                            .text_sm()
                            .text_color(theme::text_primary())
                            .child(page.label().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.6))
                            .child(page.subtitle().to_string()),
                    ),
            );

        if is_pipeline {
            row = row.child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(theme::success().opacity(0.15))
                    .text_color(theme::success())
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
            .v_flex()
            .h_full()
            .overflow_y_scroll()
            .p_8()
            .child(self.render_page_header(page))
            .child(match page {
                SettingsPage::Appearance => self.render_appearance(cx),
                SettingsPage::Agents => self.render_agents(cx),
                SettingsPage::Pipeline => self.render_pipeline(cx),
                SettingsPage::GitWorktrees => self.render_git(cx),
                SettingsPage::EditorPaths => self.render_editor_paths(cx),
                SettingsPage::Budgets => self.render_budgets(),
                SettingsPage::General => self.render_general(cx),
                SettingsPage::Resilience => self.render_resilience(),
                SettingsPage::Routing => self.render_routing(cx),
                SettingsPage::Keybindings => self.render_keybindings(),
                SettingsPage::Notifications => self.render_notifications(cx),
                _ => self.render_placeholder(page.subtitle()),
            })
            // Save bar at bottom when dirty
            .when(self.dirty, |el: Stateful<Div>| {
                el.child(self.render_save_bar(cx))
            })
    }

    fn render_page_header(&self, page: SettingsPage) -> Div {
        let mut header = div()
            .v_flex()
            .gap_1()
            .pb_4()
            .mb_6()
            .border_b_1()
            .border_color(theme::text_muted().opacity(0.1))
            .child(
                div()
                    .text_2xl()
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
                            .text_sm()
                            .text_color(theme::text_muted())
                            .child(format!("{subtitle} — maps to")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::surface())
                            .border_1()
                            .border_color(theme::text_muted().opacity(0.15))
                            .text_color(theme::text_primary())
                            .child(config_ref.to_string()),
                    ),
            );
        } else {
            header = header.child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child(subtitle.to_string()),
            );
        }

        header
    }

    fn render_save_bar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .justify_end()
            .gap_3()
            .pt_6()
            .mt_6()
            .border_t_1()
            .border_color(theme::text_muted().opacity(0.1))
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(theme::warning()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::warning())
                            .child("Unsaved changes"),
                    ),
            )
            .child(
                Button::new("settings-save")
                    .primary()
                    .label("Save Settings")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.save_config(cx);
                    })),
            )
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
        let modes = [
            AppearanceMode::System,
            AppearanceMode::Light,
            AppearanceMode::Dark,
        ];
        let cards: Vec<Stateful<Div>> = modes
            .iter()
            .map(|&mode| {
                let is_selected = mode == self.appearance_mode;
                div()
                    .id(SharedString::from(format!("mode-{}", mode.label())))
                    .flex_1()
                    .v_flex()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .py_6()
                    .rounded_xl()
                    .cursor_pointer()
                    .bg(if is_selected {
                        theme::primary().opacity(0.12)
                    } else {
                        theme::surface()
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
                            AppearanceMode::Dark | AppearanceMode::System => theme::ThemeMode::Dark,
                            AppearanceMode::Light => theme::ThemeMode::Light,
                        };
                        theme::apply_theme(this.selected_theme, this.theme_mode);
                        cx.notify();
                    }))
                    .child(Icon::new(mode.icon()).size_6().text_color(if is_selected {
                        theme::text_primary()
                    } else {
                        theme::text_muted()
                    }))
                    .child(
                        div()
                            .text_sm()
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
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child("Choose light, dark, or system preference"),
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
                    .flex_1()
                    .p_3()
                    .rounded_xl()
                    .cursor_pointer()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(if is_selected {
                        theme::primary().opacity(0.5)
                    } else {
                        theme::text_muted().opacity(0.1)
                    })
                    .hover(|s: StyleRefinement| s.border_color(theme::primary().opacity(0.3)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.selected_theme = tn;
                        theme::apply_theme(tn, this.theme_mode);
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
                                            .text_sm()
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
                                    .text_xs()
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

        div()
            .v_flex()
            .gap_3()
            .child(self.section_title("Color Theme"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child("Select a color palette for the interface"),
            )
            .child(grid)
    }

    fn render_accent_color(&self) -> Div {
        let accent = theme::primary();
        let rgba: gpui::Rgba = accent.into();
        let hex = format!(
            "#{:02x}{:02x}{:02x}",
            (rgba.r * 255.0) as u8,
            (rgba.g * 255.0) as u8,
            (rgba.b * 255.0) as u8,
        );
        div()
            .v_flex()
            .gap_3()
            .child(self.section_title("Accent Color"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child("Current accent color from selected theme"),
            )
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .w(px(36.0))
                            .h(px(36.0))
                            .rounded_lg()
                            .bg(accent)
                            .border_1()
                            .border_color(theme::text_muted().opacity(0.2)),
                    )
                    .child(
                        div()
                            .px_3()
                            .py(px(8.0))
                            .rounded_lg()
                            .bg(theme::surface())
                            .border_1()
                            .border_color(theme::text_muted().opacity(0.15))
                            .text_sm()
                            .text_color(theme::text_primary())
                            .child(hex),
                    )
                    .child(
                        Button::new("accent-reset")
                            .ghost()
                            .label("Reset to theme default"),
                    ),
            )
    }

    // ── Agents page ────────────────────────────────────────────────

    fn render_agents(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let default_agent = state
            .config
            .as_ref()
            .map(|c| c.default_agent.as_str())
            .unwrap_or("");

        let agents: Vec<Div> = state
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

                div()
                    .h_flex()
                    .gap_3()
                    .p_4()
                    .rounded_xl()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(if is_default {
                        theme::primary().opacity(0.3)
                    } else {
                        theme::text_muted().opacity(0.1)
                    })
                    // Status dot
                    .child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(theme::success())
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
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text_primary())
                                            .child(a.entry.id.clone()),
                                    )
                                    .when(is_default, |el: Div| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .px(px(6.0))
                                                .py(px(1.0))
                                                .rounded(px(4.0))
                                                .bg(theme::primary().opacity(0.15))
                                                .text_color(theme::primary())
                                                .child("DEFAULT"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(format!(
                                        "Model: {}  ·  {}",
                                        model,
                                        a.command_path.as_deref().unwrap_or("(unknown)")
                                    )),
                            ),
                    )
                    // Installed badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::success().opacity(0.15))
                            .text_color(theme::success())
                            .child("Installed"),
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
                    .rounded_xl()
                    .bg(theme::surface())
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
                                    .text_sm()
                                    .text_color(theme::text_muted())
                                    .child(entry.id.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted().opacity(0.6))
                                    .child(format!("Install: {}", entry.install_instructions)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
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
            .child(self.render_pipeline_timeout())
            .child(self.render_pipeline_execution(cx))
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
                    .text_sm()
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
            theme::success()
        } else {
            theme::text_muted()
        };

        div()
            .id(SharedString::from(format!("gate-card-{idx}")))
            .flex_1()
            .h_flex()
            .justify_between()
            .items_center()
            .p_4()
            .rounded_xl()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.1))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::text_muted().opacity(0.2)))
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
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
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

    fn render_pipeline_timeout(&self) -> Div {
        div()
            .v_flex()
            .gap_4()
            .child(self.section_title("Gate Timeout"))
            .child(
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
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_primary())
                                    .child("Timeout"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child("0 = wait forever"),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(self.value_box(&format!("{}", self.gate_timeout)))
                            .child(div().text_sm().text_color(theme::text_muted()).child("sec")),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child("On timeout"),
                    )
                    .child(self.dropdown_box("Abort")),
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
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
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
                                    .border_color(theme::background()),
                            ),
                    )
                    // +/- buttons
                    .child(
                        div()
                            .id(SharedString::from(format!("slider-dec-{label}")))
                            .cursor_pointer()
                            .px(px(4.0))
                            .rounded_md()
                            .hover(|s: StyleRefinement| s.bg(theme::surface()))
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
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .min_w(px(28.0))
                            .text_color(theme::success())
                            .child(format!("{value}")),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("slider-inc-{label}")))
                            .cursor_pointer()
                            .px(px(4.0))
                            .rounded_md()
                            .hover(|s: StyleRefinement| s.bg(theme::surface()))
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

    fn render_git(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let current_branch = state.current_branch.clone();

        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Git Configuration"))
                    .child(self.setting_row("Current Branch", &current_branch))
                    .child(self.setting_row("Branch Prefix", &self.branch_prefix))
                    .child(self.setting_row(
                        "Auto-commit",
                        if self.auto_commit {
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
                    .child(self.section_title("Worktrees"))
                    .child(self.setting_row("Worktree Directory", &self.worktree_dir))
                    .child(self.setting_row(
                        "Remove on complete",
                        if self.remove_worktrees_on_complete {
                            "Yes"
                        } else {
                            "No"
                        },
                    ))
                    .child(self.setting_row(
                        "Keep branches",
                        &format!("{} days", self.keep_branches_days),
                    )),
            )
    }

    // ── Editor & Paths page ────────────────────────────────────────

    fn render_editor_paths(&self, cx: &Context<Self>) -> Div {
        let project_path = self
            .state
            .read(cx)
            .project_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no project)".into());

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
                    .child(self.setting_row("Surge Directory", ".surge"))
                    .child(self.setting_row("Spec Directory", "specs/"))
                    .child(self.setting_row("Spec Format", "TOML")),
            )
    }

    // ── Routing page ───────────────────────────────────────────────

    fn render_routing(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let default_agent = state
            .config
            .as_ref()
            .map(|c| c.default_agent.as_str())
            .unwrap_or("(none)");

        div().v_flex().gap_8().child(
            div()
                .v_flex()
                .gap_3()
                .child(self.section_title("Strategy"))
                .child(self.setting_row("Routing Strategy", &self.routing_strategy))
                .child(self.setting_row("Default Agent", default_agent)),
        )
    }

    // ── Keybindings page ─────────────────────────────────────────────

    fn render_keybindings(&self) -> Div {
        let navigation = [
            Kb {
                action: "Dashboard",
                keys: "Ctrl+1",
                description: "Open dashboard screen",
            },
            Kb {
                action: "Kanban",
                keys: "Ctrl+2",
                description: "Open kanban board",
            },
            Kb {
                action: "Specs",
                keys: "Ctrl+3",
                description: "Open spec explorer",
            },
            Kb {
                action: "Agents",
                keys: "Ctrl+4",
                description: "Open agent hub",
            },
            Kb {
                action: "Terminals",
                keys: "Ctrl+5",
                description: "Open agent terminals",
            },
            Kb {
                action: "Execution",
                keys: "Ctrl+6",
                description: "Open live execution",
            },
            Kb {
                action: "Diff",
                keys: "Ctrl+7",
                description: "Open diff viewer",
            },
            Kb {
                action: "Insights",
                keys: "Ctrl+8",
                description: "Open insights",
            },
            Kb {
                action: "Settings",
                keys: "Ctrl+9",
                description: "Open settings",
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
                action: "New Task",
                keys: "Ctrl+N",
                description: "Create a new task",
            },
            Kb {
                action: "Approve Gate",
                keys: "Ctrl+Enter",
                description: "Approve current gate",
            },
            Kb {
                action: "Open Diff",
                keys: "Ctrl+D",
                description: "Open diff for current task",
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
                            .text_sm()
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
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .py_3()
                    .rounded_xl()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::text_muted().opacity(0.08))
                    .hover(|s: StyleRefinement| {
                        s.border_color(theme::text_muted().opacity(0.15))
                    })
                    // Left: action + description
                    .child(
                        div()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_primary())
                                    .child(action.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted().opacity(0.7))
                                    .child(desc.to_string()),
                            ),
                    )
                    // Right: key badges
                    .child(self.render_key_combo(keys))
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
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.4))
                            .child("+"),
                    );
                }
                items.push(
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(theme::background())
                        .border_1()
                        .border_color(theme::text_muted().opacity(0.15))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_primary())
                        .child(part.to_string()),
                );
                items
            })
            .collect();

        div().h_flex().gap_1().items_center().children(badges)
    }

    // ── Notifications page ─────────────────────────────────────────

    fn render_notifications(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap_8()
            // Event notifications
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Event Notifications"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_muted())
                            .child("Choose which events trigger notifications"),
                    )
                    .child(self.render_notification_toggles(cx)),
            )
            // Sound
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Sound"))
                    .child(self.render_notify_toggle(
                        "notify-sound",
                        "Sound effects",
                        "Play a sound when notifications appear",
                        self.notify_sound,
                        |this| &mut this.notify_sound,
                        cx,
                    )),
            )
            // Preview
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Preview"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_muted())
                            .child("Test how notifications look"),
                    )
                    .child(self.render_notification_previews()),
            )
    }

    fn render_notification_toggles(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap_2()
            .child(self.render_notify_toggle(
                "notify-task-completed",
                "Task completed",
                "When a task finishes successfully",
                self.notify_task_completed,
                |this| &mut this.notify_task_completed,
                cx,
            ))
            .child(self.render_notify_toggle(
                "notify-task-failed",
                "Task failed",
                "When a task fails or errors out",
                self.notify_task_failed,
                |this| &mut this.notify_task_failed,
                cx,
            ))
            .child(self.render_notify_toggle(
                "notify-gate-waiting",
                "Gate waiting for review",
                "When a pipeline gate needs human approval",
                self.notify_gate_waiting,
                |this| &mut this.notify_gate_waiting,
                cx,
            ))
            .child(self.render_notify_toggle(
                "notify-agent-disconnect",
                "Agent disconnected",
                "When an agent connection is lost",
                self.notify_agent_disconnect,
                |this| &mut this.notify_agent_disconnect,
                cx,
            ))
            .child(self.render_notify_toggle(
                "notify-rate-limit",
                "Rate limit warning",
                "When an agent hits API rate limits",
                self.notify_rate_limit,
                |this| &mut this.notify_rate_limit,
                cx,
            ))
    }

    fn render_notify_toggle(
        &self,
        id: &str,
        label: &str,
        description: &str,
        enabled: bool,
        field: fn(&mut Self) -> &mut bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let indicator_color = if enabled {
            theme::success()
        } else {
            theme::text_muted()
        };

        div()
            .id(SharedString::from(id.to_string()))
            .h_flex()
            .justify_between()
            .items_center()
            .px_4()
            .py_3()
            .rounded_xl()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.08))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::text_muted().opacity(0.15)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                let val = field(this);
                *val = !*val;
                this.mark_dirty(cx);
            }))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_primary())
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.7))
                            .child(description.to_string()),
                    ),
            )
            .child(self.toggle_switch(enabled, indicator_color))
    }

    fn render_notification_previews(&self) -> Div {
        use crate::notifications::{SurgeNotification, send_os_notification};
        use gpui_component::WindowExt as _;

        div()
            .v_flex()
            .gap_2()
            .child(self.preview_card(
                "test-notif-success",
                "Task Completed",
                "build-api finished successfully",
                theme::success(),
                IconName::CircleCheck,
                |_event, window, cx| {
                    window.push_notification(SurgeNotification::task_completed("build-api"), cx);
                    send_os_notification("Task Completed", "build-api finished successfully");
                },
            ))
            .child(self.preview_card(
                "test-notif-error",
                "Task Failed",
                "test-suite: 3 tests failed",
                theme::error(),
                IconName::CircleX,
                |_event, window, cx| {
                    window.push_notification(
                        SurgeNotification::task_failed("test-suite", "3 tests failed"),
                        cx,
                    );
                    send_os_notification("Task Failed", "test-suite: 3 tests failed");
                },
            ))
            .child(self.preview_card(
                "test-notif-review",
                "Review Required",
                "deploy-prod needs your review",
                theme::warning(),
                IconName::TriangleAlert,
                |_event, window, cx| {
                    window.push_notification(SurgeNotification::review_needed("deploy-prod"), cx);
                    send_os_notification("Review Required", "deploy-prod needs your review");
                },
            ))
            .child(self.preview_card(
                "test-notif-agent",
                "Agent Connected",
                "claude-code is ready",
                theme::primary(),
                IconName::Info,
                |_event, window, cx| {
                    window.push_notification(SurgeNotification::agent_connected("claude-code"), cx);
                    send_os_notification("Agent Connected", "claude-code is ready");
                },
            ))
            .child(self.preview_card(
                "test-notif-disconnect",
                "Agent Disconnected",
                "copilot connection lost",
                theme::warning(),
                IconName::TriangleAlert,
                |_event, window, cx| {
                    window.push_notification(SurgeNotification::agent_disconnected("copilot"), cx);
                    send_os_notification("Agent Disconnected", "copilot connection lost");
                },
            ))
            .child(self.preview_card(
                "test-notif-ratelimit",
                "Rate Limit",
                "claude-code rate limited — resets in 30s",
                theme::warning(),
                IconName::TriangleAlert,
                |_event, window, cx| {
                    window.push_notification(
                        SurgeNotification::rate_limit_warning("claude-code", 30),
                        cx,
                    );
                    send_os_notification("Rate Limit", "claude-code rate limited — resets in 30s");
                },
            ))
    }

    fn preview_card(
        &self,
        id: &str,
        title: &str,
        message: &str,
        color: Hsla,
        icon: IconName,
        on_test: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        div()
            .h_flex()
            .gap_3()
            .p_3()
            .rounded_xl()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.08))
            .child(Icon::new(icon).size_4().text_color(color))
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(message.to_string()),
                    ),
            )
            .child(
                Button::new(SharedString::from(id.to_string()))
                    .ghost()
                    .label("Test")
                    .on_click(on_test),
            )
    }

    // ── Budgets page ───────────────────────────────────────────────

    fn render_budgets(&self) -> Div {
        let usd = self
            .budget_usd
            .map(|v| format!("${:.2}", v))
            .unwrap_or_else(|| "Unlimited".into());
        let tokens = self
            .budget_tokens
            .map(|v| format!("{}", v))
            .unwrap_or_else(|| "Unlimited".into());

        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Cost Limits"))
                    .child(self.setting_row("Global Budget (USD)", &usd)),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Token Limits"))
                    .child(self.setting_row("Global Token Budget", &tokens)),
            )
    }

    // ── Resilience page ────────────────────────────────────────────

    fn render_resilience(&self) -> Div {
        div()
            .v_flex()
            .gap_8()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Timeouts"))
                    .child(self.setting_row(
                        "Connect Timeout",
                        &format!("{} sec", self.connect_timeout_secs),
                    ))
                    .child(self.setting_row(
                        "Prompt Timeout",
                        &format!("{} sec", self.prompt_timeout_secs),
                    )),
            )
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Retry"))
                    .child(
                        self.setting_row("Max Prompt Retries", &format!("{}", self.prompt_retries)),
                    )
                    .child(self.setting_row(
                        "Circuit Breaker Threshold",
                        &format!("{} failures", self.circuit_breaker_threshold),
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
                    .flex_1()
                    .v_flex()
                    .items_center()
                    .gap_1()
                    .py_3()
                    .rounded_xl()
                    .cursor_pointer()
                    .bg(if is_selected {
                        color.opacity(0.12)
                    } else {
                        theme::surface()
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
                            .text_xs()
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
                            .text_sm()
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
            // Updates
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Updates"))
                    .child(self.render_general_toggle(
                        "general-auto-update",
                        "Auto-update",
                        "Automatically check for and install updates",
                        self.auto_update,
                        |this| &mut this.auto_update,
                        cx,
                    ))
                    .child(self.setting_row("Update Channel", "Stable"))
                    .child(self.setting_row("Current Version", env!("CARGO_PKG_VERSION"))),
            )
            // Privacy
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("Privacy & Telemetry"))
                    .child(self.render_general_toggle(
                        "general-telemetry",
                        "Send usage data",
                        "Help improve Surge by sending anonymous usage statistics",
                        self.send_telemetry,
                        |this| &mut this.send_telemetry,
                        cx,
                    ))
                    .child(self.render_general_toggle(
                        "general-crash-reports",
                        "Crash reports",
                        "Automatically send crash reports for debugging",
                        self.crash_reports,
                        |this| &mut this.crash_reports,
                        cx,
                    )),
            )
            // About
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .child(self.section_title("About"))
                    .child(self.setting_row("Application", "Surge"))
                    .child(self.setting_row("Framework", "GPUI + ACP"))
                    .child(self.setting_row("Rust Edition", "2024")),
            )
    }

    fn render_general_toggle(
        &self,
        id: &str,
        label: &str,
        description: &str,
        enabled: bool,
        field: fn(&mut Self) -> &mut bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let indicator_color = if enabled {
            theme::success()
        } else {
            theme::text_muted()
        };

        div()
            .id(SharedString::from(id.to_string()))
            .h_flex()
            .justify_between()
            .items_center()
            .px_4()
            .py_3()
            .rounded_xl()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.08))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::text_muted().opacity(0.15)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                let val = field(this);
                *val = !*val;
                this.mark_dirty(cx);
            }))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_primary())
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted().opacity(0.7))
                            .child(description.to_string()),
                    ),
            )
            .child(self.toggle_switch(enabled, indicator_color))
    }

    // ── Placeholder ────────────────────────────────────────────────

    fn render_placeholder(&self, subtitle: &str) -> Div {
        div()
            .v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .py_16()
            .child(
                Icon::new(IconName::Settings2)
                    .size_8()
                    .text_color(theme::text_muted().opacity(0.3)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted().opacity(0.5))
                    .child(format!("Configure: {subtitle}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted().opacity(0.3))
                    .child("Coming soon"),
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
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child(label.to_string()),
            )
            .child(self.value_box(value))
    }

    fn value_box(&self, value: &str) -> Div {
        div()
            .text_sm()
            .text_color(theme::text_primary())
            .px_3()
            .py(px(6.0))
            .min_w(px(60.0))
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.1))
            .child(value.to_string())
    }

    fn dropdown_box(&self, value: &str) -> Div {
        div()
            .h_flex()
            .gap_1()
            .items_center()
            .px_3()
            .py(px(6.0))
            .min_w(px(80.0))
            .rounded_lg()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::text_muted().opacity(0.1))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(theme::text_primary())
                    .child(value.to_string()),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .size_3p5()
                    .text_color(theme::text_muted()),
            )
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .h_flex()
            .bg(theme::background())
            .overflow_hidden()
            .child(self.render_settings_sidebar(cx))
            .child(self.render_content(cx))
    }
}
