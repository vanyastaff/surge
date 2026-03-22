use std::path::PathBuf;

use gpui::*;
use gpui_component::button::Button;
use gpui_component::WindowExt as _;
use gpui_component::StyledExt as _;

use crate::actions::*;
use crate::command_palette::{CommandPalette, CommandSelected};
use gpui_component::Icon;
use crate::notifications::SurgeNotification;
use crate::project::RecentProjects;
use crate::router::Screen;
use crate::screens::agent_hub::AgentHubScreen;
use crate::screens::dashboard::DashboardScreen;
use crate::screens::diff_viewer::DiffViewerScreen;
use crate::screens::file_explorer::FileExplorerScreen;
use crate::screens::github_prs::GithubPrsScreen;
use crate::screens::insights::InsightsScreen;
use crate::screens::kanban::KanbanScreen;
use crate::screens::live_execution::LiveExecutionScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::spec_explorer::SpecExplorerScreen;
use crate::screens::spec_wizard::SpecWizardScreen;
use crate::screens::welcome::{WelcomeEvent, WelcomeScreen};
use crate::screens::worktrees::WorktreesScreen;
use crate::sidebar::{AppSidebar, NavigateTo, ToggleSidebar};
use crate::theme;
use crate::top_bar::TopBar;

/// Application mode — Welcome picker or Main project view.
enum AppMode {
    Welcome(Entity<WelcomeScreen>),
    Project {
        path: PathBuf,
        name: String,
    },
}

/// Root application view.
pub struct SurgeApp {
    mode: AppMode,
    // Project mode state:
    active_screen: Screen,
    sidebar_collapsed: bool,
    sidebar: Entity<AppSidebar>,
    top_bar: Option<Entity<TopBar>>,
    command_palette_open: bool,
    command_palette: Option<Entity<CommandPalette>>,
    // Screen entities (created on demand).
    dashboard: Option<Entity<DashboardScreen>>,
    kanban: Option<Entity<KanbanScreen>>,
    agent_hub: Option<Entity<AgentHubScreen>>,
    spec_explorer: Option<Entity<SpecExplorerScreen>>,
    spec_wizard: Option<Entity<SpecWizardScreen>>,
    live_execution: Option<Entity<LiveExecutionScreen>>,
    diff_viewer: Option<Entity<DiffViewerScreen>>,
    file_explorer: Option<Entity<FileExplorerScreen>>,
    worktrees: Option<Entity<WorktreesScreen>>,
    github_prs: Option<Entity<GithubPrsScreen>>,
    insights: Option<Entity<InsightsScreen>>,
    settings: Option<Entity<SettingsScreen>>,
}

impl SurgeApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let active_screen = Screen::Dashboard;
        let sidebar = cx.new(|cx| AppSidebar::new(active_screen, false, cx));

        cx.subscribe(&sidebar, |this: &mut Self, _sidebar, event: &NavigateTo, cx| {
            this.navigate(event.0, cx);
        })
        .detach();

        cx.subscribe(&sidebar, |this: &mut Self, _sidebar, _event: &ToggleSidebar, cx| {
            this.toggle_sidebar(cx);
        })
        .detach();

        // Start in Welcome mode.
        let welcome = cx.new(WelcomeScreen::new);
        cx.subscribe(&welcome, |this: &mut Self, _welcome, event: &WelcomeEvent, cx| {
            this.handle_welcome_event(event.clone(), cx);
        })
        .detach();

        Self {
            mode: AppMode::Welcome(welcome),
            active_screen,
            sidebar_collapsed: false,
            sidebar,
            top_bar: None,
            command_palette_open: false,
            command_palette: None,
            dashboard: None,
            kanban: None,
            agent_hub: None,
            spec_explorer: None,
            spec_wizard: None,
            live_execution: None,
            diff_viewer: None,
            file_explorer: None,
            worktrees: None,
            github_prs: None,
            insights: None,
            settings: None,
        }
    }

    fn handle_welcome_event(&mut self, event: WelcomeEvent, cx: &mut Context<Self>) {
        match event {
            WelcomeEvent::OpenProject(path) => {
                self.open_project(&path, cx);
            }
            WelcomeEvent::BrowseProject => {
                // Native directory picker dialog.
                let receiver = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Select project directory".into()),
                });
                cx.spawn(async |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(Ok(Some(paths))) = receiver.await {
                        if let Some(path) = paths.first() {
                            let path = path.clone();
                            cx.update(|cx| {
                                this.update(cx, |this: &mut Self, cx| {
                                    this.open_project(&path, cx);
                                })
                            })
                            .ok();
                        }
                    }
                })
                .detach();
            }
            WelcomeEvent::InitProject => {
                // For now, open the directory picker and then create a project.
                // TODO: show full init wizard dialog.
                let receiver = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Select project directory".into()),
                });
                cx.spawn(async |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(Ok(Some(paths))) = receiver.await {
                        if let Some(path) = paths.first() {
                            let path = path.clone();
                            cx.update(|cx| {
                                this.update(cx, |this: &mut Self, cx| {
                                    this.open_project(&path, cx);
                                })
                            })
                            .ok();
                        }
                    }
                })
                .detach();
            }
            WelcomeEvent::RemoveProject(path) => {
                let mut recent = RecentProjects::load();
                recent.remove(&path);
                let _ = recent.save();
                self.refresh_welcome(cx);
            }
            WelcomeEvent::TogglePin(path) => {
                let mut recent = RecentProjects::load();
                recent.toggle_pin(&path);
                let _ = recent.save();
                self.refresh_welcome(cx);
            }
        }
    }

    fn refresh_welcome(&mut self, cx: &mut Context<Self>) {
        if let AppMode::Welcome(welcome) = &self.mode {
            welcome.update(cx, |w, cx| w.reload(cx));
        }
    }

    fn open_project(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        // Update recent projects.
        let mut recent = RecentProjects::load();
        recent.touch(&name, path);
        let _ = recent.save();

        // Create top bar.
        let name_clone = name.clone();
        let top_bar = cx.new(|cx| TopBar::new(&name_clone, Screen::Dashboard, cx));
        self.top_bar = Some(top_bar);

        self.mode = AppMode::Project {
            path: path.to_path_buf(),
            name,
        };
        self.active_screen = Screen::Dashboard;
        self.sidebar.update(cx, |sb, cx| sb.set_active(Screen::Dashboard, cx));
        cx.notify();
    }

    fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.active_screen = screen;
        self.sidebar.update(cx, |sb, cx| sb.set_active(screen, cx));
        if let Some(top_bar) = &self.top_bar {
            top_bar.update(cx, |tb, cx| tb.set_screen(screen, cx));
        }
        self.close_palette(cx);
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        let collapsed = self.sidebar_collapsed;
        self.sidebar.update(cx, |sb, cx| sb.set_collapsed(collapsed, cx));
        cx.notify();
    }

    fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.command_palette_open {
            self.close_palette(cx);
        } else {
            self.open_palette(cx);
        }
    }

    fn open_palette(&mut self, cx: &mut Context<Self>) {
        let palette = cx.new(CommandPalette::new);
        cx.subscribe(&palette, |this: &mut Self, _palette, event: &CommandSelected, cx| {
            if let Some(screen) = event.0 {
                this.navigate(screen, cx);
            } else {
                this.close_palette(cx);
            }
        })
        .detach();
        self.command_palette = Some(palette);
        self.command_palette_open = true;
        cx.notify();
    }

    fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette = None;
        self.command_palette_open = false;
        cx.notify();
    }

    pub fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("ctrl-1", GoToDashboard, None),
            KeyBinding::new("ctrl-2", GoToKanban, None),
            KeyBinding::new("ctrl-3", GoToSpecs, None),
            KeyBinding::new("ctrl-4", GoToAgents, None),
            KeyBinding::new("ctrl-5", GoToTerminals, None),
            KeyBinding::new("ctrl-6", GoToExecution, None),
            KeyBinding::new("ctrl-7", GoToDiff, None),
            KeyBinding::new("ctrl-8", GoToInsights, None),
            KeyBinding::new("ctrl-9", GoToSettings, None),
            KeyBinding::new("ctrl-b", ToggleSidebarAction, None),
            KeyBinding::new("ctrl-k", ToggleCommandPalette, None),
        ]);
    }

    fn render_screen_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active_screen {
            Screen::Dashboard => {
                let dashboard = self.dashboard.get_or_insert_with(|| {
                    cx.new(DashboardScreen::new)
                });
                dashboard.clone().into_any_element()
            }
            Screen::Kanban => {
                let kanban = self.kanban.get_or_insert_with(|| {
                    cx.new(KanbanScreen::new)
                });
                kanban.clone().into_any_element()
            }
            Screen::AgentHub => {
                let agent_hub = self.agent_hub.get_or_insert_with(|| {
                    cx.new(AgentHubScreen::new)
                });
                agent_hub.clone().into_any_element()
            }
            Screen::SpecExplorer => {
                let spec_explorer = self.spec_explorer.get_or_insert_with(|| {
                    cx.new(SpecExplorerScreen::new)
                });
                spec_explorer.clone().into_any_element()
            }
            Screen::SpecWizard => {
                let spec_wizard = self.spec_wizard.get_or_insert_with(|| {
                    cx.new(SpecWizardScreen::new)
                });
                spec_wizard.clone().into_any_element()
            }
            Screen::LiveExecution => {
                let live_exec = self.live_execution.get_or_insert_with(|| {
                    cx.new(LiveExecutionScreen::new)
                });
                live_exec.clone().into_any_element()
            }
            Screen::DiffViewer => {
                let s = self.diff_viewer.get_or_insert_with(|| cx.new(DiffViewerScreen::new));
                s.clone().into_any_element()
            }
            Screen::FileExplorer => {
                let s = self.file_explorer.get_or_insert_with(|| cx.new(FileExplorerScreen::new));
                s.clone().into_any_element()
            }
            Screen::Worktrees => {
                let s = self.worktrees.get_or_insert_with(|| cx.new(WorktreesScreen::new));
                s.clone().into_any_element()
            }
            Screen::GitHubPRs => {
                let s = self.github_prs.get_or_insert_with(|| cx.new(GithubPrsScreen::new));
                s.clone().into_any_element()
            }
            Screen::Insights => {
                let s = self.insights.get_or_insert_with(|| cx.new(InsightsScreen::new));
                s.clone().into_any_element()
            }
            Screen::Settings => {
                let s = self.settings.get_or_insert_with(|| cx.new(SettingsScreen::new));
                s.clone().into_any_element()
            }
            _ => {
                // Placeholder for screens not yet implemented.
                let label = self.active_screen.label();
                let icon = self.active_screen.icon();
                div()
                    .flex_1()
                    .p_6()
                    .v_flex()
                    .gap_4()
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                Icon::new(icon).size_6().text_color(theme::PRIMARY),
                            )
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(label.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{} — coming soon", label)),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .mt_4()
                            .child(
                                Button::new("test-notif")
                                    .label("Test Notification")
                                    .on_click(|_event, window, cx| {
                                        window.push_notification(
                                            SurgeNotification::agent_connected("Claude Code"),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_palette_overlay(&self) -> AnyElement {
        if let Some(palette) = &self.command_palette {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(80.0))
                .bg(hsla(0.0, 0.0, 0.0, 0.5))
                .child(palette.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }
}

impl Render for SurgeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.mode {
            AppMode::Welcome(welcome) => {
                div()
                    .key_context("SurgeApp")
                    .size_full()
                    .child(welcome.clone())
                    .into_any_element()
            }
            AppMode::Project { .. } => {
                div()
                    .key_context("SurgeApp")
                    .size_full()
                    .bg(theme::BACKGROUND)
                    .text_color(theme::TEXT_PRIMARY)
                    .on_action(cx.listener(|this, _: &GoToDashboard, _w, cx| this.navigate(Screen::Dashboard, cx)))
                    .on_action(cx.listener(|this, _: &GoToKanban, _w, cx| this.navigate(Screen::Kanban, cx)))
                    .on_action(cx.listener(|this, _: &GoToSpecs, _w, cx| this.navigate(Screen::SpecExplorer, cx)))
                    .on_action(cx.listener(|this, _: &GoToAgents, _w, cx| this.navigate(Screen::AgentHub, cx)))
                    .on_action(cx.listener(|this, _: &GoToTerminals, _w, cx| this.navigate(Screen::AgentTerminals, cx)))
                    .on_action(cx.listener(|this, _: &GoToExecution, _w, cx| this.navigate(Screen::LiveExecution, cx)))
                    .on_action(cx.listener(|this, _: &GoToDiff, _w, cx| this.navigate(Screen::DiffViewer, cx)))
                    .on_action(cx.listener(|this, _: &GoToInsights, _w, cx| this.navigate(Screen::Insights, cx)))
                    .on_action(cx.listener(|this, _: &GoToSettings, _w, cx| this.navigate(Screen::Settings, cx)))
                    .on_action(cx.listener(|this, _: &ToggleSidebarAction, _w, cx| this.toggle_sidebar(cx)))
                    .on_action(cx.listener(|this, _: &ToggleCommandPalette, _w, cx| this.toggle_palette(cx)))
                    .child(
                        div()
                            .size_full()
                            .v_flex()
                            // Top bar
                            .children(self.top_bar.clone())
                            // Main content area: sidebar + screen
                            .child(
                                div()
                                    .flex_1()
                                    .h_flex()
                                    .overflow_hidden()
                                    .child(self.sidebar.clone())
                                    .child(
                                        div()
                                            .flex_1()
                                            .h_full()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .child(self.render_screen_content(cx)),
                                    ),
                            ),
                    )
                    .child(self.render_palette_overlay())
                    .into_any_element()
            }
        }
    }
}
