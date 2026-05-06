use std::path::PathBuf;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt as _;
use gpui_component::WindowExt as _;
use gpui_component::button::Button;

use crate::actions::*;
use crate::app_state::AppState;
use crate::command_palette::{CommandPalette, CommandSelected};
use crate::notifications::SurgeNotification;
use crate::project::RecentProjects;
use crate::router::Screen;
use crate::screens::agent_hub::AgentHubScreen;
use crate::screens::agent_terminal::AgentTerminalScreen;
use crate::screens::dashboard::DashboardScreen;
use crate::screens::diff_viewer::DiffViewerScreen;
use crate::screens::file_explorer::FileExplorerScreen;
use crate::screens::gate_approval::{GateApprovalScreen, GateDecision};
use crate::screens::github_prs::GithubPrsScreen;
use crate::screens::insights::InsightsScreen;
use crate::screens::kanban::{KanbanScreen, TaskClicked};
use crate::screens::live_execution::LiveExecutionScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::spec_explorer::SpecExplorerScreen;
use crate::screens::spec_wizard::SpecWizardScreen;
use crate::screens::welcome::{WelcomeEvent, WelcomeScreen};
use crate::screens::worktrees::WorktreesScreen;
use crate::sidebar::{AppSidebar, NavigateTo, ToggleSidebar};
use crate::theme;
use crate::top_bar::TopBar;
use gpui_component::Icon;

/// Application mode — Welcome picker or Main project view.
enum AppMode {
    Welcome(Entity<WelcomeScreen>),
    Project { _path: PathBuf, _name: String },
}

/// Root application view.
pub struct SurgeApp {
    state: Entity<AppState>,
    focus: FocusHandle,
    mode: AppMode,
    // Project mode state:
    active_screen: Screen,
    sidebar_collapsed: bool,
    sidebar: Entity<AppSidebar>,
    top_bar: Option<Entity<TopBar>>,
    command_palette_open: bool,
    command_palette: Option<Entity<CommandPalette>>,
    /// Task detail overlay — set when a kanban card is clicked.
    task_detail_id: Option<String>,
    // Screen entities (created on demand).
    dashboard: Option<Entity<DashboardScreen>>,
    kanban: Option<Entity<KanbanScreen>>,
    agent_hub: Option<Entity<AgentHubScreen>>,
    spec_explorer: Option<Entity<SpecExplorerScreen>>,
    spec_wizard: Option<Entity<SpecWizardScreen>>,
    live_execution: Option<Entity<LiveExecutionScreen>>,
    agent_terminal: Option<Entity<AgentTerminalScreen>>,
    diff_viewer: Option<Entity<DiffViewerScreen>>,
    file_explorer: Option<Entity<FileExplorerScreen>>,
    worktrees: Option<Entity<WorktreesScreen>>,
    github_prs: Option<Entity<GithubPrsScreen>>,
    insights: Option<Entity<InsightsScreen>>,
    settings: Option<Entity<SettingsScreen>>,
    gate_approval: Option<Entity<GateApprovalScreen>>,
    /// Queued notifications to flush on next render (needs Window access).
    pending_notifications: Vec<gpui_component::notification::Notification>,
}

impl SurgeApp {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let active_screen = Screen::Dashboard;
        let sidebar = cx.new(|cx| AppSidebar::new(active_screen, false, cx));

        cx.subscribe(
            &sidebar,
            |this: &mut Self, _sidebar, event: &NavigateTo, cx| {
                this.navigate(event.0, cx);
            },
        )
        .detach();

        cx.subscribe(
            &sidebar,
            |this: &mut Self, _sidebar, _event: &ToggleSidebar, cx| {
                this.toggle_sidebar(cx);
            },
        )
        .detach();

        // Subscribe to SurgeEvents from AppState → queue notifications.
        // (Currently dormant — no in-process emitter; kept for the
        // in-process orchestrator path that may surface later.)
        cx.subscribe(
            &state,
            |this, _state, event: &surge_core::SurgeEvent, cx| {
                this.queue_notification_for_event(event);
                cx.notify(); // trigger re-render to flush
            },
        )
        .detach();

        // Spawn the daemon connect + global-event subscription task.
        // The runtime UI is a daemon client (per
        // `docs/revision/rfcs/0008-ui-architecture.md`): it watches runs
        // hosted by `surge-daemon` rather than running them in-process.
        // This task: try_connect → list_runs → subscribe_global → loop
        // pumping `GlobalDaemonEvent` into AppState + UI notifications.
        Self::spawn_daemon_link(&state, cx);

        // Start in Welcome mode.
        let welcome = cx.new(WelcomeScreen::new);
        cx.subscribe(
            &welcome,
            |this: &mut Self, _welcome, event: &WelcomeEvent, cx| {
                this.handle_welcome_event(event.clone(), cx);
            },
        )
        .detach();

        Self {
            state,
            focus,
            mode: AppMode::Welcome(welcome),
            active_screen,
            sidebar_collapsed: false,
            sidebar,
            top_bar: None,
            command_palette_open: false,
            command_palette: None,
            task_detail_id: None,
            dashboard: None,
            agent_terminal: None,
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
            gate_approval: None,
            pending_notifications: Vec::new(),
        }
    }

    fn handle_welcome_event(&mut self, event: WelcomeEvent, cx: &mut Context<Self>) {
        match event {
            WelcomeEvent::OpenProject(path) => {
                self.open_project(&path, cx);
            },
            WelcomeEvent::BrowseProject => {
                // Native directory picker dialog.
                let receiver = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Select project directory".into()),
                });
                cx.spawn(async |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(Ok(Some(paths))) = receiver.await
                        && let Some(path) = paths.first()
                    {
                        let path = path.clone();
                        cx.update(|cx| {
                            this.update(cx, |this: &mut Self, cx| {
                                this.open_project(&path, cx);
                            })
                        })
                        .ok();
                    }
                })
                .detach();
            },
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
                    if let Ok(Ok(Some(paths))) = receiver.await
                        && let Some(path) = paths.first()
                    {
                        let path = path.clone();
                        cx.update(|cx| {
                            this.update(cx, |this: &mut Self, cx| {
                                this.open_project(&path, cx);
                            })
                        })
                        .ok();
                    }
                })
                .detach();
            },
            WelcomeEvent::RemoveProject(path) => {
                let mut recent = RecentProjects::load();
                recent.remove(&path);
                let _ = recent.save();
                self.refresh_welcome(cx);
            },
            WelcomeEvent::TogglePin(path) => {
                let mut recent = RecentProjects::load();
                recent.toggle_pin(&path);
                let _ = recent.save();
                self.refresh_welcome(cx);
            },
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

        // Load project data into AppState.
        self.state.update(cx, |state, _cx| {
            state.load_project(path);
        });

        // Create top bar.
        let name_clone = name.clone();
        let top_bar = cx.new(|cx| TopBar::new(&name_clone, Screen::Dashboard, cx));
        self.top_bar = Some(top_bar);

        // Reset screen entities so they re-read from AppState.
        self.dashboard = None;
        self.kanban = None;
        self.agent_hub = None;
        self.agent_terminal = None;
        self.spec_explorer = None;
        self.spec_wizard = None;
        self.live_execution = None;
        self.diff_viewer = None;
        self.file_explorer = None;
        self.worktrees = None;
        self.github_prs = None;
        self.insights = None;
        self.settings = None;
        self.gate_approval = None;

        self.mode = AppMode::Project {
            _path: path.to_path_buf(),
            _name: name,
        };
        self.active_screen = Screen::Dashboard;
        self.sidebar
            .update(cx, |sb, cx| sb.set_active(Screen::Dashboard, cx));
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
        self.sidebar
            .update(cx, |sb, cx| sb.set_collapsed(collapsed, cx));
        cx.notify();
    }

    /// Queue a notification for a SurgeEvent (flushed during render when Window is available).
    fn queue_notification_for_event(&mut self, event: &surge_core::SurgeEvent) {
        // Also send OS-level notification
        crate::notifications::os_notify_event(event);

        use surge_core::SurgeEvent;

        let notification = match event {
            SurgeEvent::TaskStateChanged {
                task_id, new_state, ..
            } => {
                let id_short = task_id.short();
                match new_state {
                    surge_core::TaskState::Completed => {
                        Some(SurgeNotification::task_completed(&id_short))
                    },
                    surge_core::TaskState::Failed { .. } => {
                        Some(SurgeNotification::task_failed(&id_short, "task failed"))
                    },
                    _ => None,
                }
            },
            SurgeEvent::GateAwaitingApproval {
                task_id, gate_name, ..
            } => {
                let label = format!("{} ({})", gate_name, task_id.short());
                Some(SurgeNotification::review_needed(&label))
            },
            SurgeEvent::AgentConnected { agent_name } => {
                Some(SurgeNotification::agent_connected(agent_name))
            },
            SurgeEvent::AgentDisconnected { agent_name } => {
                Some(SurgeNotification::agent_disconnected(agent_name))
            },
            SurgeEvent::AgentRateLimited {
                agent_name,
                retry_after_secs,
            } => Some(SurgeNotification::rate_limit_warning(
                agent_name,
                *retry_after_secs,
            )),
            SurgeEvent::CircuitBreakerOpened {
                agent_name, reason, ..
            } => Some(SurgeNotification::task_failed(
                agent_name,
                &format!("circuit breaker: {reason}"),
            )),
            _ => None,
        };

        if let Some(notif) = notification {
            self.pending_notifications.push(notif);
        }
    }

    /// Queue a notification for a `GlobalDaemonEvent` (run lifecycle).
    /// Mirrors `queue_notification_for_event` for the SurgeEvent path
    /// but consumes daemon-side run lifecycle events instead.
    fn queue_notification_for_global(
        &mut self,
        event: &surge_orchestrator::engine::ipc::GlobalDaemonEvent,
    ) {
        // Also send OS-level notification.
        crate::notifications::os_notify_global(event);

        use surge_orchestrator::engine::handle::RunOutcome;
        use surge_orchestrator::engine::ipc::GlobalDaemonEvent as G;

        let notification = match event {
            G::RunAccepted { run_id } => Some(SurgeNotification::run_accepted(&run_id.short())),
            G::RunFinished { run_id, outcome } => {
                let short = run_id.short();
                match outcome {
                    RunOutcome::Completed { .. } => Some(SurgeNotification::run_completed(&short)),
                    RunOutcome::Failed { error } => {
                        Some(SurgeNotification::run_failed(&short, error))
                    },
                    RunOutcome::Aborted { reason } => {
                        Some(SurgeNotification::run_aborted(&short, reason))
                    },
                    _ => None,
                }
            },
            G::DaemonShuttingDown => Some(SurgeNotification::daemon_shutting_down()),
            _ => None,
        };

        if let Some(notif) = notification {
            self.pending_notifications.push(notif);
        }
    }

    /// Spawn the daemon connection + global-event subscription task.
    ///
    /// Lifecycle on success:
    ///   1. State flips to `Connecting`.
    ///   2. `try_connect` opens the local socket; on failure flips
    ///      to `Failed(reason)` and exits without retrying. (User-driven
    ///      retry is a phase-2 affordance.)
    ///   3. State flips to `Connected(facade)`. `list_runs` populates
    ///      the run list once.
    ///   4. `subscribe_global` opens the lifecycle event stream; the
    ///      task loops on `recv` until the channel closes (daemon
    ///      shutdown / connection drop).
    ///   5. Each event lands in three places: `apply_global_event`
    ///      mutates the run list, `queue_notification_for_global`
    ///      queues an in-app banner, and `os_notify_global` fires an
    ///      OS toast.
    fn spawn_daemon_link(state: &Entity<AppState>, cx: &mut Context<Self>) {
        let state_for_task = state.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Set Connecting.
            let _ = cx.update(|cx| {
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state = crate::daemon_link::ConnectionState::Connecting;
                    cx.notify();
                });
            });

            // Try to connect.
            let facade = match crate::daemon_link::try_connect().await {
                Ok(f) => f,
                Err(e) => {
                    tracing::info!("daemon not reachable on startup ({e}); UI continues offline");
                    let _ = cx.update(|cx| {
                        let _ = state_for_task.update(cx, |state, cx| {
                            state.daemon_state =
                                crate::daemon_link::ConnectionState::Failed(e.to_string());
                            cx.notify();
                        });
                    });
                    return;
                },
            };

            // Connected — flip state, then list runs once.
            let facade_for_state = facade.clone();
            let _ = cx.update(|cx| {
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state =
                        crate::daemon_link::ConnectionState::Connected(facade_for_state);
                    cx.notify();
                });
            });

            {
                use surge_orchestrator::engine::facade::EngineFacade as _;
                match facade.list_runs().await {
                    Ok(summaries) => {
                        let _ = cx.update(|cx| {
                            let _ = state_for_task.update(cx, |state, cx| {
                                state.set_runs_from_summaries(&summaries);
                                cx.notify();
                            });
                        });
                    },
                    Err(e) => {
                        tracing::warn!("daemon list_runs failed: {e}");
                    },
                }
            }

            // Subscribe to the global lifecycle event stream.
            let mut rx = match facade.subscribe_global().await {
                Ok(rx) => rx,
                Err(e) => {
                    // The Connected state we set above implied an active
                    // subscription; without it the UI would lie about
                    // receiving events. Roll back to Failed so the
                    // status pill is honest.
                    tracing::warn!("daemon subscribe_global failed: {e}");
                    let reason = e.to_string();
                    let _ = cx.update(|cx| {
                        let _ = state_for_task.update(cx, |state, cx| {
                            state.daemon_state =
                                crate::daemon_link::ConnectionState::Failed(reason);
                            cx.notify();
                        });
                    });
                    return;
                },
            };
            tracing::info!("daemon link: subscribed to global events");

            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let event_for_state = event.clone();
                        let event_for_app = event.clone();
                        let _ = cx.update(|cx| {
                            let _ = state_for_task.update(cx, |state, cx| {
                                state.apply_global_event(&event_for_state);
                                cx.notify();
                            });
                            let _ = this.update(cx, |this, cx| {
                                this.queue_notification_for_global(&event_for_app);
                                cx.notify();
                            });
                        });
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!(
                            "daemon link: global event channel closed; ending subscription"
                        );
                        break;
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(dropped = n, "daemon link: global event subscriber lagged");
                    },
                }
            }

            // Channel closed — flip back to Disconnected so the UI can
            // surface a "reconnect" affordance later.
            let _ = cx.update(|cx| {
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state = crate::daemon_link::ConnectionState::Disconnected;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Flush queued notifications (called from render where Window is available).
    fn flush_notifications(&mut self, window: &mut Window, cx: &mut App) {
        use gpui_component::WindowExt as _;
        for notif in self.pending_notifications.drain(..) {
            window.push_notification(notif, cx);
        }
    }

    /// Push a single notification immediately (used from UI button handlers).
    pub fn push_notification(
        &mut self,
        notif: gpui_component::notification::Notification,
        window: &mut Window,
        cx: &mut App,
    ) {
        use gpui_component::WindowExt as _;
        window.push_notification(notif, cx);
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
        cx.subscribe(
            &palette,
            |this: &mut Self, _palette, event: &CommandSelected, cx| {
                if let Some(screen) = event.0 {
                    this.navigate(screen, cx);
                } else {
                    this.close_palette(cx);
                }
            },
        )
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

    fn handle_gate_decision(&mut self, decision: GateDecision, cx: &mut Context<Self>) {
        let task_id = decision.task_id.clone();
        let approved = decision.approved;

        // Get project path from AppMode
        let project_path = match &self.mode {
            AppMode::Project { _path, .. } => _path.clone(),
            _ => return,
        };

        // Write gate decision file
        let gate_dir = project_path.join(".surge").join("gates");
        let decision_file = gate_dir.join(format!("{}.json", task_id));

        // Spawn async task to write decision
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = std::fs::create_dir_all(&gate_dir) {
                eprintln!("Failed to create gates directory: {}", e);
                return;
            }

            let decision_data = format!(
                r#"{{"task_id":"{}","approved":{},"timestamp":"{}"}}"#,
                task_id,
                approved,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            );

            if let Err(e) = std::fs::write(&decision_file, decision_data) {
                eprintln!("Failed to write gate decision: {}", e);
                return;
            }

            println!(
                "Gate decision written: {} {}",
                task_id,
                if approved { "approved" } else { "rejected" }
            );
        })
        .detach();

        // Navigate back to dashboard after decision
        self.navigate(Screen::Dashboard, cx);
    }

    pub fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            // Navigation: Ctrl+1..9
            KeyBinding::new("ctrl-1", GoToDashboard, None),
            KeyBinding::new("ctrl-2", GoToKanban, None),
            KeyBinding::new("ctrl-3", GoToSpecs, None),
            KeyBinding::new("ctrl-4", GoToAgents, None),
            KeyBinding::new("ctrl-5", GoToTerminals, None),
            KeyBinding::new("ctrl-6", GoToExecution, None),
            KeyBinding::new("ctrl-7", GoToDiff, None),
            KeyBinding::new("ctrl-8", GoToInsights, None),
            KeyBinding::new("ctrl-9", GoToSettings, None),
            // UI toggles
            KeyBinding::new("ctrl-b", ToggleSidebarAction, None),
            KeyBinding::new("ctrl-k", ToggleCommandPalette, None),
            // Project
            KeyBinding::new("ctrl-shift-p", SwitchProject, None),
            // Tasks
            KeyBinding::new("ctrl-n", NewTask, None),
            KeyBinding::new("ctrl-enter", ApproveGate, None),
            // Diff
            KeyBinding::new("ctrl-d", OpenDiffViewer, None),
        ]);
    }

    fn render_screen_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active_screen {
            Screen::Dashboard => {
                let state = self.state.clone();
                let dashboard = self
                    .dashboard
                    .get_or_insert_with(|| cx.new(|cx| DashboardScreen::new(state, cx)));
                dashboard.clone().into_any_element()
            },
            Screen::Kanban => {
                let state = self.state.clone();
                let kanban = self.kanban.get_or_insert_with(|| {
                    let k = cx.new(|cx| KanbanScreen::new(state, cx));
                    cx.subscribe(&k, |this: &mut Self, _kanban, event: &TaskClicked, cx| {
                        this.task_detail_id = Some(event.0.clone());
                        cx.notify();
                    })
                    .detach();
                    k
                });
                kanban.clone().into_any_element()
            },
            Screen::AgentHub => {
                let state = self.state.clone();
                let agent_hub = self
                    .agent_hub
                    .get_or_insert_with(|| cx.new(|cx| AgentHubScreen::new(state, cx)));
                agent_hub.clone().into_any_element()
            },
            Screen::SpecExplorer => {
                let state = self.state.clone();
                let spec_explorer = self
                    .spec_explorer
                    .get_or_insert_with(|| cx.new(|cx| SpecExplorerScreen::new(state, cx)));
                spec_explorer.clone().into_any_element()
            },
            Screen::AgentTerminals => {
                let state = self.state.clone();
                let terminal = self
                    .agent_terminal
                    .get_or_insert_with(|| cx.new(|cx| AgentTerminalScreen::new(state, cx)));
                terminal.clone().into_any_element()
            },
            Screen::SpecWizard => {
                let spec_wizard = self
                    .spec_wizard
                    .get_or_insert_with(|| cx.new(SpecWizardScreen::new));
                spec_wizard.clone().into_any_element()
            },
            Screen::LiveExecution => {
                let live_exec = self
                    .live_execution
                    .get_or_insert_with(|| cx.new(LiveExecutionScreen::new));
                live_exec.clone().into_any_element()
            },
            Screen::DiffViewer => {
                let s = self
                    .diff_viewer
                    .get_or_insert_with(|| cx.new(DiffViewerScreen::new));
                s.clone().into_any_element()
            },
            Screen::FileExplorer => {
                let s = self
                    .file_explorer
                    .get_or_insert_with(|| cx.new(FileExplorerScreen::new));
                s.clone().into_any_element()
            },
            Screen::Worktrees => {
                let state = self.state.clone();
                let s = self
                    .worktrees
                    .get_or_insert_with(|| cx.new(|cx| WorktreesScreen::new(state, cx)));
                s.clone().into_any_element()
            },
            Screen::GitHubPRs => {
                let s = self
                    .github_prs
                    .get_or_insert_with(|| cx.new(GithubPrsScreen::new));
                s.clone().into_any_element()
            },
            Screen::Insights => {
                let s = self
                    .insights
                    .get_or_insert_with(|| cx.new(InsightsScreen::new));
                s.clone().into_any_element()
            },
            Screen::Settings => {
                let state = self.state.clone();
                let s = self
                    .settings
                    .get_or_insert_with(|| cx.new(|cx| SettingsScreen::new(state, cx)));
                s.clone().into_any_element()
            },
            Screen::GateApproval => {
                // Use task_detail_id or default demo task
                let task_id = self.task_detail_id.as_deref().unwrap_or("task-001");
                let gate_approval = self.gate_approval.get_or_insert_with(|| {
                    let ga = cx.new(|cx| GateApprovalScreen::new(task_id, cx));
                    cx.subscribe(&ga, |this: &mut Self, _ga, event: &GateDecision, cx| {
                        this.handle_gate_decision(event.clone(), cx);
                    })
                    .detach();
                    ga
                });
                gate_approval.clone().into_any_element()
            },
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
                            .child(Icon::new(icon).size_6().text_color(theme::primary()))
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(label.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_color(theme::text_muted())
                            .child(format!("{} — coming soon", label)),
                    )
                    .child(
                        div().h_flex().gap_2().mt_4().child(
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
            },
        }
    }

    fn render_task_detail_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(task_id) = &self.task_detail_id {
            let task = self
                .state
                .read(cx)
                .tasks
                .iter()
                .find(|t| t.id.to_string() == *task_id)
                .cloned();

            let card_content = if let Some(task) = task {
                let status_label = format!("{:?}", task.state);
                let (sub_done, sub_total) = match &task.state {
                    surge_core::TaskState::Executing { completed, total } => (*completed, *total),
                    _ => (0, 0),
                };

                div()
                    .id("task-detail-card")
                    .v_flex()
                    .gap_3()
                    .p_5()
                    .w(px(500.0))
                    .max_h(px(500.0))
                    .rounded_xl()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::text_muted().opacity(0.1))
                    .on_click(|_e, _w, _cx| {}) // absorb click
                    // Header: title + close
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(task.title.clone()),
                            )
                            .child(
                                div()
                                    .id("task-detail-close")
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(theme::text_muted())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .hover(|s| s.bg(theme::text_muted().opacity(0.1)))
                                    .on_click(cx.listener(|this, _e, _w, cx| {
                                        this.task_detail_id = None;
                                        cx.notify();
                                    }))
                                    .child("X"),
                            ),
                    )
                    // Badges row: ID + status
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(theme::primary().opacity(0.15))
                                    .text_color(theme::primary())
                                    .child(format!("#{}", task.id)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(theme::warning().opacity(0.15))
                                    .text_color(theme::warning())
                                    .child(status_label),
                            ),
                    )
                    // Description
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_muted())
                                    .child("Description"),
                            )
                            .child(div().text_sm().text_color(theme::text_primary()).child(
                                if task.description.is_empty() {
                                    "(no description)".to_string()
                                } else {
                                    task.description.clone()
                                },
                            )),
                    )
                    // Subtask progress (if executing)
                    .when(sub_total > 0, |el: Stateful<Div>| {
                        let pct = sub_done as f32 / sub_total as f32;
                        el.child(
                            div()
                                .v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::text_muted())
                                        .child(format!("Subtasks: {sub_done}/{sub_total}")),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(4.0))
                                        .rounded_full()
                                        .bg(theme::text_muted().opacity(0.1))
                                        .child(
                                            div()
                                                .h_full()
                                                .rounded_full()
                                                .bg(theme::primary())
                                                .w(relative(pct)),
                                        ),
                                ),
                        )
                    })
                    // Agent + Complexity
                    .child(
                        div()
                            .h_flex()
                            .gap_4()
                            .child(
                                div()
                                    .v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child("Agent"),
                                    )
                                    .child(div().text_sm().text_color(theme::text_primary()).child(
                                        task.agent.unwrap_or_else(|| "unassigned".to_string()),
                                    )),
                            )
                            .child(
                                div()
                                    .v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child("Complexity"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme::text_primary())
                                            .child(task.complexity.clone()),
                                    ),
                            ),
                    )
            } else {
                // Task not found
                div()
                    .id("task-detail-card")
                    .v_flex()
                    .gap_3()
                    .p_5()
                    .w(px(500.0))
                    .rounded_xl()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::text_muted().opacity(0.1))
                    .on_click(|_e, _w, _cx| {})
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("Task not found"),
                            )
                            .child(
                                div()
                                    .id("task-detail-close-nf")
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(theme::text_muted())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .hover(|s| s.bg(theme::text_muted().opacity(0.1)))
                                    .on_click(cx.listener(|this, _e, _w, cx| {
                                        this.task_detail_id = None;
                                        cx.notify();
                                    }))
                                    .child("X"),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_muted())
                            .child(format!("Task ID: {}", task_id)),
                    )
            };

            div()
                .id("task-detail-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0.0, 0.0, 0.0, 0.5))
                .on_click(cx.listener(|this, _e, _w, cx| {
                    this.task_detail_id = None;
                    cx.notify();
                }))
                .child(card_content)
                .into_any_element()
        } else {
            div().into_any_element()
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
        // Flush any queued notifications now that we have Window access.
        self.flush_notifications(window, cx);

        match &self.mode {
            AppMode::Welcome(welcome) => div()
                .key_context("SurgeApp")
                .track_focus(&self.focus)
                .size_full()
                .child(welcome.clone())
                .into_any_element(),
            AppMode::Project { .. } => {
                div()
                    .key_context("SurgeApp")
                    .track_focus(&self.focus)
                    .size_full()
                    .bg(theme::background())
                    .text_color(theme::text_primary())
                    .on_action(cx.listener(|this, _: &GoToDashboard, _w, cx| {
                        this.navigate(Screen::Dashboard, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &GoToKanban, _w, cx| {
                            this.navigate(Screen::Kanban, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &GoToSpecs, _w, cx| {
                        this.navigate(Screen::SpecExplorer, cx)
                    }))
                    .on_action(cx.listener(|this, _: &GoToAgents, _w, cx| {
                        this.navigate(Screen::AgentHub, cx)
                    }))
                    .on_action(cx.listener(|this, _: &GoToTerminals, _w, cx| {
                        this.navigate(Screen::AgentTerminals, cx)
                    }))
                    .on_action(cx.listener(|this, _: &GoToExecution, _w, cx| {
                        this.navigate(Screen::LiveExecution, cx)
                    }))
                    .on_action(cx.listener(|this, _: &GoToDiff, _w, cx| {
                        this.navigate(Screen::DiffViewer, cx)
                    }))
                    .on_action(cx.listener(|this, _: &GoToInsights, _w, cx| {
                        this.navigate(Screen::Insights, cx)
                    }))
                    .on_action(cx.listener(|this, _: &GoToSettings, _w, cx| {
                        this.navigate(Screen::Settings, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &ToggleSidebarAction, _w, cx| {
                            this.toggle_sidebar(cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &ToggleCommandPalette, _w, cx| {
                            this.toggle_palette(cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &SwitchProject, _w, cx| {
                        // Toggle project switcher in top bar.
                        if let Some(top_bar) = &this.top_bar {
                            top_bar.update(cx, |tb, cx| tb.toggle_switcher(cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &NewTask, _w, cx| {
                        this.navigate(Screen::SpecWizard, cx)
                    }))
                    .on_action(cx.listener(|this, _: &OpenDiffViewer, _w, cx| {
                        this.navigate(Screen::DiffViewer, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ApproveGate, _w, cx| {
                        // If on gate approval screen, approve the current gate
                        if this.active_screen == Screen::GateApproval
                            && let Some(gate_approval) = &this.gate_approval
                        {
                            gate_approval.update(cx, |ga, cx| {
                                // Trigger approve button click programmatically
                                cx.emit(GateDecision {
                                    task_id: ga.task_id.clone(),
                                    approved: true,
                                });
                                cx.notify();
                            });
                        }
                    }))
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
                    .child(self.render_task_detail_overlay(cx))
                    .children(gpui_component::Root::render_notification_layer(window, cx))
                    .into_any_element()
            },
        }
    }
}
