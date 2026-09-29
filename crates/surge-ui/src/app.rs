use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::component::StyledExt as _;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::actions::*;
use crate::app_state::AppState;
use crate::command_palette::{CommandPalette, PaletteCommand, PaletteEvent};
use crate::notifications::SurgeNotification;
use crate::project::RecentProjects;
use crate::router::Screen;
use crate::screens::agent_hub::AgentHubScreen;
use crate::screens::agent_terminal::AgentTerminalScreen;
use crate::screens::agents::{AgentsAction, AgentsScreen};
use crate::screens::backlog::{BacklogAction, BacklogScreen};
use crate::screens::fleet::{FleetAction, FleetScreen};
use crate::screens::flow::FlowScreen;
use crate::screens::inbox::{InboxAction, InboxScreen};
use crate::screens::memory::{MemoryAction, MemoryScreen};
use crate::screens::roadmap::{RoadmapEvent, RoadmapScreen};
use crate::screens::runs::{RunsEvent, RunsScreen};
use crate::screens::settings::SettingsScreen;
use crate::screens::spec_explorer::SpecExplorerScreen;
use crate::screens::spec_wizard::SpecWizardScreen;
use crate::screens::welcome::{WelcomeEvent, WelcomeScreen};
use crate::screens::worktrees::WorktreesScreen;
use crate::sidebar::{AppSidebar, NavigateTo, ToggleSidebar};
use crate::theme;
use crate::top_bar::{TopBar, TopBarEvent};

/// Application mode — Welcome picker or Main project view.
enum AppMode {
    Welcome(Entity<WelcomeScreen>),
    Project { _path: PathBuf, _name: String },
}

struct PlanningRequest {
    run_id: surge_core::RunId,
    graph: surge_core::graph::Graph,
    project_path: PathBuf,
    config: surge_orchestrator::engine::EngineRunConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BootstrapIndexEntry {
    operation_id: surge_core::RunId,
    project_path: PathBuf,
}

/// Reconnect attempts (1s apart) after a live daemon link drops.
const DAEMON_RECONNECT_ATTEMPTS: u32 = 60;

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
    fleet: Option<Entity<FleetScreen>>,
    flow: Option<Entity<FlowScreen>>,
    memory: Option<Entity<MemoryScreen>>,
    runs_screen: Option<Entity<RunsScreen>>,
    roadmap: Option<Entity<RoadmapScreen>>,
    inbox: Option<Entity<InboxScreen>>,
    backlog: Option<Entity<BacklogScreen>>,
    agents_screen: Option<Entity<AgentsScreen>>,
    agent_hub: Option<Entity<AgentHubScreen>>,
    spec_explorer: Option<Entity<SpecExplorerScreen>>,
    spec_wizard: Option<Entity<SpecWizardScreen>>,
    wizard_drafts: HashMap<PathBuf, Entity<SpecWizardScreen>>,
    agent_terminal: Option<Entity<AgentTerminalScreen>>,
    worktrees: Option<Entity<WorktreesScreen>>,
    settings: Option<Entity<SettingsScreen>>,
    /// Queued notifications to flush on next render (needs Window access).
    pending_notifications: Vec<gpui_kit::component::notification::Notification>,
    /// Runs we already attached a per-run event subscription for.
    stream_subscribed: HashSet<surge_core::id::RunId>,
    /// Run to focus when the Runs cockpit next renders. Deep links land
    /// here because the RunsScreen entity is created lazily — calling
    /// select_run before it exists would silently drop the selection.
    pending_run_selection: Option<surge_core::id::RunId>,
    bootstrap_operations: HashMap<
        surge_core::RunId,
        (
            PathBuf,
            surge_core::bootstrap_operation::BootstrapOperationStatus,
        ),
    >,
    bootstrap_index: Vec<BootstrapIndexEntry>,
}

impl SurgeApp {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let app = Self::new_shell(state, cx);
        Self::spawn_daemon_link(&app.state, cx);
        app
    }

    fn new_shell(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let active_screen = Screen::Fleet;
        let sidebar = cx.new(|cx| AppSidebar::new(active_screen, false, state.clone(), cx));

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

        cx.subscribe(
            &sidebar,
            |this: &mut Self, _sidebar, _event: &crate::sidebar::StartDaemon, cx| {
                this.start_daemon(cx);
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
            fleet: None,
            flow: None,
            memory: None,
            runs_screen: None,
            roadmap: None,
            inbox: None,
            backlog: None,
            agents_screen: None,
            agent_terminal: None,
            agent_hub: None,
            spec_explorer: None,
            spec_wizard: None,
            wizard_drafts: HashMap::new(),
            worktrees: None,
            settings: None,
            pending_notifications: Vec::new(),
            stream_subscribed: HashSet::new(),
            pending_run_selection: None,
            bootstrap_operations: HashMap::new(),
            bootstrap_index: Self::load_bootstrap_index(),
        }
    }

    /// Deep-link into the Runs cockpit, focusing `run_id` if given.
    /// Selection is parked in `pending_run_selection` and applied when
    /// the (lazily created) RunsScreen renders.
    fn open_run_cockpit(&mut self, run_id: Option<surge_core::id::RunId>, cx: &mut Context<Self>) {
        self.pending_run_selection = run_id;
        self.navigate(Screen::Runs, cx);
    }

    /// Spawn the daemon via the `surge` CLI (which owns detachment,
    /// pidfile and readiness polling), then retry the connection.
    /// Falls back to launching `surge-daemon` directly when the CLI
    /// binary is not next to us.
    fn start_daemon(&mut self, cx: &mut Context<Self>) {
        fn sibling(name: &str) -> Option<std::path::PathBuf> {
            let exe = std::env::current_exe().ok()?;
            let candidate = exe.parent()?.join(name);
            candidate.exists().then_some(candidate)
        }

        let spawn_result = if let Some(surge) = sibling("surge") {
            std::process::Command::new(surge)
                .args(["daemon", "start"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| ())
        } else if let Some(daemon) = sibling("surge-daemon") {
            std::process::Command::new(daemon)
                .arg("--detached")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| ())
        } else {
            // PATH fallback — the CLI handles "already running" itself.
            std::process::Command::new("surge")
                .args(["daemon", "start"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| ())
        };

        match spawn_result {
            Ok(()) => {
                self.pending_notifications
                    .push(SurgeNotification::agent_connected("surge-daemon starting"));
                // Give the daemon up to ~8s to bind its socket.
                Self::spawn_daemon_link_with_retries(&self.state.clone(), 8, cx);
            },
            Err(e) => {
                tracing::error!("failed to spawn daemon: {e}");
                self.pending_notifications
                    .push(SurgeNotification::task_failed(
                        "surge-daemon",
                        &format!("could not spawn: {e} (is `surge` on PATH?)"),
                    ));
            },
        }
        cx.notify();
    }

    /// Submit planning through the existing daemon protocol; the daemon owns the run.
    /// Start an application from a free-form request (Fleet command bar,
    /// Backlog dispatch). Goes through the daemon-supervised bootstrap
    /// operation, which continues from the approved plan into the
    /// implementation run; a plain planning run stopped after the flow gate
    /// and never built anything.
    fn dispatch_run(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.dispatch_bootstrap(prompt, surge_core::RunId::new(), None, cx);
    }

    fn dispatch_planning(
        &mut self,
        prompt: String,
        template: &'static str,
        run_id: surge_core::RunId,
        origin: Option<Entity<SpecWizardScreen>>,
        cx: &mut Context<Self>,
    ) {
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.dispatch_failed(
                run_id,
                origin.as_ref(),
                "Daemon offline. Start it from the sidebar, then retry.".into(),
                cx,
            );
            return;
        };
        self.dispatch_with(
            prompt,
            template,
            run_id,
            origin,
            move |request| async move {
                use surge_orchestrator::engine::facade::EngineFacade as _;
                facade
                    .start_run(
                        request.run_id,
                        request.graph,
                        request.project_path,
                        request.config,
                    )
                    .await
            },
            cx,
        );
    }

    /// The injected start operation is the same submission boundary in production and tests.
    fn dispatch_with<F, Fut>(
        &mut self,
        prompt: String,
        template: &str,
        run_id: surge_core::RunId,
        origin: Option<Entity<SpecWizardScreen>>,
        start: F,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce(PlanningRequest) -> Fut + 'static,
        Fut: std::future::Future<
                Output = Result<
                    surge_orchestrator::engine::RunHandle,
                    surge_orchestrator::engine::EngineError,
                >,
            > + 'static,
    {
        if self.stream_subscribed.contains(&run_id) {
            return;
        }
        let request = (|| {
            if prompt.trim().is_empty() {
                return Err("Describe the work before starting a planning run.".to_string());
            }
            let state = self.state.read(cx);
            let project_path = state.project_path.clone().ok_or("Open a project first.")?;
            if let Some(wizard) = &origin {
                let wizard = wizard.read(cx);
                if wizard.project_path != project_path || !wizard.is_submitting(run_id) {
                    return Err(
                        "The project changed. Return to this draft's project to submit it.".into(),
                    );
                }
            }
            let graph = surge_orchestrator::archetype_registry::ArchetypeRegistry::load()
                .and_then(|registry| registry.resolve(template))
                .map_err(|error| format!("Could not load planning flow: {error}"))?
                .graph;
            let app_config = state.config.clone().unwrap_or_default();
            let mut config = surge_orchestrator::project_context::with_project_context_seed(
                surge_orchestrator::engine::EngineRunConfig::default(),
                &project_path,
                &app_config,
            );
            config.budget = app_config.analytics.budget_guard();
            config.initial_prompt = prompt;
            Ok(PlanningRequest {
                run_id,
                graph,
                project_path,
                config,
            })
        })();
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                self.dispatch_failed(run_id, origin.as_ref(), error, cx);
                return;
            },
        };
        let project_path = request.project_path.clone();
        self.stream_subscribed.insert(run_id);
        let state = self.state.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            match start(request).await {
                Ok(handle) => {
                    let mut rx = handle.events;
                    let _ = this.update(cx, |app, cx| {
                        app.dispatch_accepted(run_id, &project_path, origin.as_ref(), cx);
                    });
                    loop {
                        match rx.recv().await {
                            Ok(event) => {
                                if state.update(cx, |state, cx| {
                                    state.run_streams.entry(run_id).or_default().apply(&event);
                                    cx.notify();
                                }).is_err() {
                                    return;
                                }
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(run_id = %run_id, dropped = n, "run stream lagged");
                            },
                        }
                    }
                    let _ = state.update(cx, |state, cx| {
                        if let Some(stream) = state.run_streams.get_mut(&run_id) {
                            stream.live = false;
                        }
                        cx.notify();
                    });
                    let _ = this.update(cx, |app, _| { app.stream_subscribed.remove(&run_id); });
                },
                Err(error) => {
                    let message = format!("Could not confirm planning request {run_id}: {error}. Check Runs before retrying; the daemon may have received the request.");
                    let _ = this.update(cx, |app, cx| {
                        app.dispatch_failed(run_id, origin.as_ref(), message, cx);
                    });
                },
            }
        }).detach();
        cx.notify();
    }

    /// Submit a stable, daemon-owned application operation and retain its identity locally.
    fn dispatch_bootstrap(
        &mut self,
        prompt: String,
        operation_id: surge_core::RunId,
        origin: Option<Entity<SpecWizardScreen>>,
        cx: &mut Context<Self>,
    ) {
        if prompt.trim().is_empty() {
            self.dispatch_failed(
                operation_id,
                origin.as_ref(),
                "Describe what you want to build before starting.".into(),
                cx,
            );
            return;
        }
        let project_path = self.state.read(cx).project_path.clone();
        let Some(project_path) = project_path else {
            self.dispatch_failed(
                operation_id,
                origin.as_ref(),
                "Open a configured Git project before starting.".into(),
                cx,
            );
            return;
        };
        let budget = self
            .state
            .read(cx)
            .config
            .clone()
            .unwrap_or_default()
            .analytics
            .budget_guard();
        if budget.limits.usd.is_some() {
            self.dispatch_failed(
                operation_id,
                origin.as_ref(),
                "This build cannot start safely: durable application runs cannot enforce a USD cap across planning and implementation yet. Remove the USD cap or set a token-only limit in project Settings, then retry.".into(),
                cx,
            );
            return;
        }
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.dispatch_failed(
                operation_id,
                origin.as_ref(),
                "Daemon offline. Start it from the sidebar, then retry.".into(),
                cx,
            );
            return;
        };
        self.stream_subscribed.insert(operation_id);
        let entry = BootstrapIndexEntry {
            operation_id,
            project_path: project_path.clone(),
        };
        self.bootstrap_index
            .retain(|existing| existing.operation_id != operation_id);
        self.bootstrap_index.push(entry);
        if let Err(error) = Self::save_bootstrap_index(&self.bootstrap_index) {
            tracing::warn!(%operation_id, %error, "could not persist desktop bootstrap index");
        }
        let state = self.state.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = async {
                let intent = surge_core::bootstrap_operation::BootstrapIntent::new(
                    project_path.clone(),
                    prompt,
                    budget,
                )
                .map_err(|error| error.to_string())?;
                facade
                    .start_bootstrap(operation_id, intent)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            match result {
                Ok(status) => {
                    let _ = this.update(cx, |app, cx| {
                        app.bootstrap_operations
                            .insert(operation_id, (project_path.clone(), status.clone()));
                        app.dispatch_accepted(
                            status.planning_run,
                            &project_path,
                            origin.as_ref(),
                            cx,
                        );
                        app.track_bootstrap_operation(
                            facade.clone(),
                            operation_id,
                            project_path.clone(),
                            project_path.clone(),
                            cx,
                        );
                    });
                },
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.dispatch_failed(operation_id, origin.as_ref(), error, cx);
                    });
                },
            }
        })
        .detach();
        cx.notify();
    }

    fn track_bootstrap_operation(
        &mut self,
        facade: std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
        operation_id: surge_core::RunId,
        display_project: PathBuf,
        project_path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let status = facade.bootstrap_status(operation_id).await;
                match status {
                    Ok(status) => {
                        let is_terminal = status.state.phase().is_none();
                        let _ = this.update(cx, |app, cx| {
                            app.update_bootstrap_view(
                                operation_id,
                                status.clone(),
                                &display_project,
                                cx,
                            );
                        });
                        if is_terminal {
                            break;
                        }
                    },
                    Err(error) => {
                        tracing::warn!(%operation_id, %error, "bootstrap status refresh failed");
                        break;
                    },
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
            let _ = state.update(cx, |state, cx| {
                if let Some(stream) = state.run_streams.get_mut(&operation_id) {
                    stream.live = false;
                }
                cx.notify();
            });
            let _ = this.update(cx, |app, _| {
                app.stream_subscribed.remove(&operation_id);
            });
        })
        .detach();
    }

    fn update_bootstrap_view(
        &mut self,
        operation_id: surge_core::RunId,
        status: surge_core::bootstrap_operation::BootstrapOperationStatus,
        project_path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        use surge_core::bootstrap_operation::{BootstrapPhase, BootstrapState};
        use surge_orchestrator::engine::handle::RunStatus;
        let snapshot = status.clone();
        let run_id = match status.state.phase() {
            Some(
                BootstrapPhase::QueuedImplementation
                | BootstrapPhase::PreparingImplementation
                | BootstrapPhase::Implementing,
            ) => status.implementation_run,
            _ => status.planning_run,
        };
        let run_status = match status.state {
            BootstrapState::Completed => RunStatus::Completed,
            BootstrapState::Failed | BootstrapState::NeedsAttention { .. } => RunStatus::Failed,
            BootstrapState::Cancelled => RunStatus::Aborted,
            BootstrapState::Pending { .. } | BootstrapState::Cancelling { .. } => RunStatus::Active,
        };
        self.bootstrap_operations
            .insert(operation_id, (project_path.to_path_buf(), snapshot.clone()));
        if let Some(entry) = self
            .bootstrap_index
            .iter_mut()
            .find(|entry| entry.operation_id == operation_id)
        {
            entry.project_path = project_path.to_path_buf();
        } else {
            self.bootstrap_index.push(BootstrapIndexEntry {
                operation_id,
                project_path: project_path.to_path_buf(),
            });
        }
        if let Err(error) = Self::save_bootstrap_index(&self.bootstrap_index) {
            tracing::warn!(%operation_id, %error, "could not update desktop bootstrap index");
        }
        self.state.update(cx, |state, cx| {
            state
                .bootstrap_operations
                .insert(operation_id, snapshot.clone());
            state.run_streams.entry(run_id).or_default().live = !status.state.phase().is_none();
            if !state.runs.iter().any(|run| run.run_id == run_id) {
                state.runs.push(crate::app_state::UiRun {
                    run_id,
                    status: run_status,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: status.state.phase().is_none().then_some(chrono::Utc::now()),
                });
            } else if let Some(run) = state.runs.iter_mut().find(|run| run.run_id == run_id) {
                run.status = run_status;
                run.ended_at = status.state.phase().is_none().then_some(chrono::Utc::now());
            }
            cx.notify();
        });
    }

    fn bootstrap_index_path() -> Option<PathBuf> {
        surge_core::home::surge_home_dir()
            .map(|home| home.join("desktop").join("bootstrap-operations.json"))
    }

    fn load_bootstrap_index() -> Vec<BootstrapIndexEntry> {
        let Some(path) = Self::bootstrap_index_path() else {
            return Vec::new();
        };
        Self::load_bootstrap_index_at(&path)
    }

    fn load_bootstrap_index_at(path: &std::path::Path) -> Vec<BootstrapIndexEntry> {
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    fn save_bootstrap_index(entries: &[BootstrapIndexEntry]) -> anyhow::Result<()> {
        let Some(path) = Self::bootstrap_index_path() else {
            anyhow::bail!("Surge home is unavailable");
        };
        Self::save_bootstrap_index_at(path, entries)
    }

    fn save_bootstrap_index_at(
        path: PathBuf,
        entries: &[BootstrapIndexEntry],
    ) -> anyhow::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid Surge home path"))?;
        std::fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(entries)?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }

    fn recover_bootstrap_operations(
        &mut self,
        facade: std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
        cx: &mut Context<Self>,
    ) {
        let entries = self.bootstrap_index.clone();
        let current_project = self.state.read(cx).project_path.clone();
        for entry in entries {
            if self.stream_subscribed.insert(entry.operation_id) {
                self.track_bootstrap_operation(
                    facade.clone(),
                    entry.operation_id,
                    current_project
                        .as_ref()
                        .filter(|path| ***path == entry.project_path)
                        .map_or_else(|| entry.project_path.clone(), Clone::clone),
                    entry.project_path,
                    cx,
                );
            }
        }
    }

    fn handle_wizard_event(
        &mut self,
        wizard: Entity<SpecWizardScreen>,
        event: &crate::screens::spec_wizard::SpecWizardEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::screens::spec_wizard::SpecWizardEvent;
        match event {
            SpecWizardEvent::Create {
                run_id,
                description,
            } => {
                self.dispatch_bootstrap(description.clone(), *run_id, Some(wizard.clone()), cx);
            },
            SpecWizardEvent::OpenRun(run_id) => {
                let draft = wizard.read(cx);
                if !draft.is_accepted(*run_id)
                    || self.spec_wizard.as_ref() != Some(&wizard)
                    || self.state.read(cx).project_path.as_ref() != Some(&draft.project_path)
                {
                    return;
                }
                let project_path = draft.project_path.clone();
                if self.wizard_drafts.get(&project_path) == Some(&wizard) {
                    self.wizard_drafts.remove(&project_path);
                }
                self.spec_wizard = None;
                self.open_run_cockpit(Some(*run_id), cx);
            },
            SpecWizardEvent::Cancel => self.navigate(Screen::Backlog, cx),
        }
    }

    fn dispatch_failed(
        &mut self,
        run_id: surge_core::RunId,
        origin: Option<&Entity<SpecWizardScreen>>,
        error: String,
        cx: &mut Context<Self>,
    ) {
        self.stream_subscribed.remove(&run_id);
        if let Some(wizard) = origin {
            wizard.update(cx, |wizard, cx| {
                wizard.finish_submission(run_id, Err(error), cx);
            });
        } else {
            self.pending_notifications
                .push(SurgeNotification::task_failed("planning", &error));
        }
        cx.notify();
    }

    fn dispatch_accepted(
        &mut self,
        run_id: surge_core::RunId,
        project_path: &std::path::Path,
        origin: Option<&Entity<SpecWizardScreen>>,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.run_streams.entry(run_id).or_default().live = true;
            cx.notify();
        });
        let same_project = self.state.read(cx).project_path.as_deref() == Some(project_path);
        let show_run = if let Some(wizard) = origin {
            let applied = wizard.update(cx, |wizard, cx| {
                wizard.finish_submission(run_id, Ok(()), cx)
            });
            applied
                && same_project
                && self.active_screen == Screen::SpecWizard
                && self.spec_wizard.as_ref() == Some(wizard)
        } else {
            same_project
        };
        if show_run {
            if origin.is_some() {
                self.wizard_drafts.remove(project_path);
                self.spec_wizard = None;
            }
            self.open_run_cockpit(Some(run_id), cx);
        }
        cx.notify();
    }

    /// Attach per-run event subscriptions for every active run we are
    /// not already streaming. The per-run broadcast has no history
    /// replay, so the earlier we attach, the more the cockpit sees —
    /// we sync right after `list_runs` and on every `RunAccepted`.
    ///
    /// Lifecycle: `stream_subscribed` marks in-flight/attached pumps.
    /// The mark is REMOVED when a subscribe attempt fails or the pump
    /// exits, so the next sync (any global event / list_runs) retries a
    /// run that is still Active — a transient failure must not silence
    /// a run for the whole session. Orphan folds for runs the daemon no
    /// longer lists are pruned so their pending decisions can't inflate
    /// the Inbox forever.
    fn sync_run_subscriptions(&mut self, cx: &mut Context<Self>) {
        use surge_orchestrator::engine::handle::RunStatus;

        // Prune bookkeeping for runs that vanished from the daemon's list.
        let known: HashSet<surge_core::id::RunId> =
            self.state.read(cx).runs.iter().map(|r| r.run_id).collect();
        self.stream_subscribed.retain(|id| known.contains(id));
        self.state.update(cx, |s, cx| {
            let before = s.run_streams.len();
            s.run_streams.retain(|id, _| known.contains(id));
            if s.run_streams.len() != before {
                cx.notify();
            }
        });

        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            return;
        };
        let to_subscribe: Vec<surge_core::id::RunId> = self
            .state
            .read(cx)
            .runs
            .iter()
            .filter(|r| matches!(r.status, RunStatus::Active))
            .map(|r| r.run_id)
            .filter(|id| !self.stream_subscribed.contains(id))
            .collect();

        for run_id in to_subscribe {
            self.stream_subscribed.insert(run_id);
            // Weak handles only — an immortal pump must not pin the app
            // state alive, and a dead entity ends the loop naturally.
            let state = self.state.downgrade();
            let facade = facade.clone();
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let unmark = |cx: &mut AsyncApp, this: &WeakEntity<Self>| {
                    cx.update(|cx| {
                        let _ = this.update(cx, |t, _| {
                            t.stream_subscribed.remove(&run_id);
                        });
                    });
                };

                let mut rx = match facade.subscribe_to_run(run_id).await {
                    Ok(rx) => rx,
                    Err(e) => {
                        tracing::debug!(run_id = %run_id, "subscribe_to_run failed: {e}");
                        // Allow the next sync pass to retry.
                        unmark(cx, &this);
                        return;
                    },
                };
                tracing::info!(run_id = %run_id, "per-run event stream attached");
                // Subscribe first, then replay a read-only durable snapshot.
                // Live events buffered during hydration are deduplicated by seq.
                if let Some(home) = surge_core::home::surge_home_dir() {
                    match surge_persistence::runs::Storage::inspect_existing_run_events(
                        home.join("runs"), run_id,
                    ).await {
                        Ok(events) => cx.update(|cx| {
                            let _ = state.update(cx, |s, cx| {
                                let stream = s.run_streams.entry(run_id).or_default();
                                for event in events {
                                    stream.apply_recorded(&surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                                        seq: event.seq.as_u64(), payload: Box::new(event.payload.payload),
                                    }, event.timestamp_ms);
                                }
                                cx.notify();
                            });
                        }),
                        Err(error) => tracing::warn!(%run_id, %error, "could not restore run history"),
                    }
                }
                cx.update(|cx| {
                    let _ = state.update(cx, |s, cx| {
                        s.run_streams.entry(run_id).or_default().live = true;
                        cx.notify();
                    });
                });
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let alive = cx.update(|cx| {
                                state
                                    .update(cx, |s, cx| {
                                        s.run_streams.entry(run_id).or_default().apply(&event);
                                        cx.notify();
                                    })
                                    .is_ok()
                            });
                            if !alive {
                                return;
                            }
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(run_id = %run_id, dropped = n, "run stream lagged");
                        },
                    }
                }
                cx.update(|cx| {
                    let _ = state.update(cx, |s, cx| {
                        if let Some(st) = s.run_streams.get_mut(&run_id) {
                            st.live = false;
                        }
                        cx.notify();
                    });
                });
                // Channel closed — if the run is somehow still listed as
                // active, the next sync may re-attach.
                unmark(cx, &this);
            })
            .detach();
        }
    }

    fn handle_welcome_event(&mut self, event: WelcomeEvent, cx: &mut Context<Self>) {
        match event {
            WelcomeEvent::OpenProject(path) => {
                self.open_project(&path, cx);
            },
            WelcomeEvent::NewProject => self.create_project(cx),
            WelcomeEvent::BrowseProject => {
                self.close_run_preview(cx);
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
        // A recent entry can outlive its folder (tmp dirs, deleted
        // checkouts). Opening it would load an empty, broken project.
        if !path.is_dir() {
            tracing::warn!(path = %path.display(), "project folder not found");
            self.pending_notifications
                .push(SurgeNotification::project_missing(
                    &path.display().to_string(),
                ));
            if let AppMode::Welcome(welcome) = &self.mode {
                welcome.update(cx, |w, cx| w.reload(cx));
            }
            cx.notify();
            return;
        }
        self.close_run_preview(cx);
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
        self.install_top_bar(&name, path, cx);

        // Reset screen entities so they re-read from AppState.
        self.fleet = None;
        self.flow = None;
        self.memory = None;
        self.runs_screen = None;
        self.roadmap = None;
        self.inbox = None;
        self.backlog = None;
        self.agents_screen = None;
        self.agent_hub = None;
        self.agent_terminal = None;
        self.spec_explorer = None;
        self.spec_wizard = None;
        self.worktrees = None;
        self.settings = None;

        self.mode = AppMode::Project {
            _path: path.to_path_buf(),
            _name: name,
        };
        self.active_screen = Screen::Fleet;
        self.sidebar
            .update(cx, |sb, cx| sb.set_active(Screen::Fleet, cx));
        cx.notify();
    }

    fn install_top_bar(&mut self, name: &str, path: &std::path::Path, cx: &mut Context<Self>) {
        let path = path.to_path_buf();
        let top_bar = cx.new(|cx| TopBar::new(name, Some(path), Screen::Fleet, cx));
        cx.subscribe(
            &top_bar,
            |this, _bar, event: &TopBarEvent, cx| match event {
                TopBarEvent::ProjectSwitcherOpened => this.close_run_preview(cx),
                TopBarEvent::SwitchProject(path) => this.open_project(path, cx),
                TopBarEvent::OpenOther => {
                    this.handle_welcome_event(WelcomeEvent::BrowseProject, cx);
                },
                TopBarEvent::NewProject => this.create_project(cx),
                TopBarEvent::OpenPalette => this.toggle_palette(cx),
            },
        )
        .detach();
        self.top_bar = Some(top_bar);
    }

    fn create_project(&mut self, cx: &mut Context<Self>) {
        self.close_run_preview(cx);
        // Reuse the open project's agent settings; on a first launch there is
        // none, so fall back to the same detected-agent default as
        // `surge init --default` rather than refusing to create anything.
        let config = self.state.read(cx).config.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose an empty folder for your new application".into()),
        });
        cx.spawn(async |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let destination = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let config = config.unwrap_or_else(surge_acp::onboarding::default_config);
                    crate::project_init::initialize(&destination, &config)
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.open_project(&path, cx),
                Err(error) => {
                    this.pending_notifications
                        .push(SurgeNotification::task_failed(
                            "new project",
                            &format!("Could not create project: {error}"),
                        ));
                    cx.notify();
                },
            });
        })
        .detach();
    }

    fn close_run_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(runs) = &self.runs_screen {
            runs.update(cx, |screen, cx| screen.close_preview(cx));
        }
    }

    fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if screen != Screen::Runs {
            self.close_run_preview(cx);
        }
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
        Self::spawn_daemon_link_with_retries(state, 1, cx);
    }

    /// Connect (or reconnect) to the daemon, retrying `attempts` times
    /// one second apart — enough to cover a daemon that was *just*
    /// spawned by [`Self::start_daemon`] and is still binding its
    /// socket. Safe to call again after a `Failed`/`Disconnected`
    /// state; a link that is already `Connecting`/`Connected` is left
    /// alone.
    fn spawn_daemon_link_with_retries(
        state: &Entity<AppState>,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        {
            let current = &state.read(cx).daemon_state;
            if matches!(
                current,
                crate::daemon_link::ConnectionState::Connecting
                    | crate::daemon_link::ConnectionState::Connected(_)
            ) {
                return;
            }
        }
        let state_for_task = state.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Set Connecting.
            cx.update(|cx| {
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state = crate::daemon_link::ConnectionState::Connecting;
                    cx.notify();
                });
            });

            // Durable results remain available after daemon/UI restarts.
            if let Some(home) = surge_core::home::surge_home_dir() {
                let limit = std::num::NonZeroU32::new(100).unwrap_or(std::num::NonZeroU32::MIN);
                match surge_persistence::runs::Storage::inspect_existing_run_summaries(home.clone(), limit).await {
                    Ok(summaries) => {
                        for mut summary in summaries {
                            match surge_persistence::runs::Storage::inspect_existing_run_events(home.join("runs"), summary.id).await {
                                Ok(events) => cx.update(|cx| { let _ = state_for_task.update(cx, |state, cx| {
                                    if let Some(event) = events.iter().rev().find(|event| matches!(event.payload.payload,
                                        surge_core::EventPayload::RunCompleted { .. } | surge_core::EventPayload::RunFailed { .. } | surge_core::EventPayload::RunAborted { .. })) {
                                        summary.status = match event.payload.payload {
                                            surge_core::EventPayload::RunCompleted { .. } => surge_core::RunStatus::Completed,
                                            surge_core::EventPayload::RunFailed { .. } => surge_core::RunStatus::Failed,
                                            _ => surge_core::RunStatus::Aborted,
                                        };
                                        summary.ended_at_ms = Some(event.timestamp_ms);
                                    }
                                    state.restore_finished_runs(std::slice::from_ref(&summary));
                                    let stream = state.run_streams.entry(summary.id).or_default();
                                    for event in events {
                                        stream.apply_recorded(&surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                                            seq: event.seq.as_u64(), payload: Box::new(event.payload.payload),
                                        }, event.timestamp_ms);
                                    }
                                    stream.live = false;
                                    cx.notify();
                                }); }),
                                Err(error) => tracing::warn!(%error, run_id = %summary.id, "could not read finished run history"),
                            }
                        }
                    },
                    Err(error) => tracing::warn!(%error, "could not restore finished runs"),
                }
            }

            // Try to connect (with retries for freshly spawned daemons).
            let mut last_err = String::new();
            let mut facade = None;
            for attempt in 0..attempts.max(1) {
                if attempt > 0 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                }
                match crate::daemon_link::try_connect().await {
                    Ok(f) => {
                        facade = Some(f);
                        break;
                    },
                    Err(e) => last_err = e.to_string(),
                }
            }
            let Some(facade) = facade else {
                tracing::info!("daemon not reachable ({last_err}); UI continues offline");
                cx.update(|cx| {
                    let _ = state_for_task.update(cx, |state, cx| {
                        state.daemon_state =
                            crate::daemon_link::ConnectionState::Failed(last_err.clone());
                        cx.notify();
                    });
                });
                return;
            };

            // Connected — flip state, then list runs once.
            let facade_for_state = facade.clone();
            cx.update(|cx| {
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state =
                        crate::daemon_link::ConnectionState::Connected(facade_for_state);
                    cx.notify();
                });
                let _ = this.update(cx, |this, cx| {
                    this.recover_bootstrap_operations(facade.clone(), cx);
                });
            });

            {
                use surge_orchestrator::engine::facade::EngineFacade as _;
                match facade.list_runs().await {
                    Ok(summaries) => {
                        cx.update(|cx| {
                            let _ = state_for_task.update(cx, |state, cx| {
                                state.set_runs_from_summaries(&summaries);
                                cx.notify();
                            });
                            let _ = this.update(cx, |this, cx| {
                                this.sync_run_subscriptions(cx);
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
                    cx.update(|cx| {
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
                        cx.update(|cx| {
                            let _ = state_for_task.update(cx, |state, cx| {
                                state.apply_global_event(&event_for_state);
                                cx.notify();
                            });
                            let _ = this.update(cx, |this, cx| {
                                this.queue_notification_for_global(&event_for_app);
                                // A RunAccepted may have added an active run —
                                // attach its event stream immediately.
                                this.sync_run_subscriptions(cx);
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

            // Channel closed — the daemon restarted or went away. Flip to
            // Disconnected, then reconnect on our own: a restart (upgrade,
            // `surge daemon restart`, crash recovery) must not leave the app
            // saying "offline" until someone clicks. Bounded: ~1 minute.
            cx.update(|cx| {
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state = crate::daemon_link::ConnectionState::Disconnected;
                    cx.notify();
                });
                if let Some(state) = state_for_task.upgrade() {
                    let _ = this.update(cx, |_this, cx| {
                        tracing::info!("daemon link dropped; reconnecting");
                        Self::spawn_daemon_link_with_retries(&state, DAEMON_RECONNECT_ATTEMPTS, cx);
                    });
                }
            });
        })
        .detach();
    }

    /// Flush queued notifications (called from render where Window is available).
    fn flush_notifications(&mut self, window: &mut Window, cx: &mut App) {
        use gpui_kit::component::WindowExt as _;
        for notif in self.pending_notifications.drain(..) {
            window.push_notification(notif, cx);
        }
    }

    /// Push a single notification immediately (used from UI button handlers).
    pub fn push_notification(
        &mut self,
        notif: gpui_kit::component::notification::Notification,
        window: &mut Window,
        cx: &mut App,
    ) {
        use gpui_kit::component::WindowExt as _;
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
        self.close_run_preview(cx);
        let palette = cx.new(CommandPalette::new);
        cx.subscribe(
            &palette,
            |this: &mut Self, _palette, event: &PaletteEvent, cx| {
                this.close_palette(cx);
                match *event {
                    PaletteEvent::Dismiss => {},
                    PaletteEvent::Run(PaletteCommand::Navigate(screen)) => {
                        this.navigate(screen, cx)
                    },
                    PaletteEvent::Run(PaletteCommand::ToggleSidebar) => this.toggle_sidebar(cx),
                    PaletteEvent::Run(PaletteCommand::OpenProject) => {
                        this.handle_welcome_event(WelcomeEvent::BrowseProject, cx);
                    },
                    PaletteEvent::Run(PaletteCommand::NewApp) => this.create_project(cx),
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

    /// Persist an operator gate decision to `.surge/gates/<task>.json`
    /// where the engine's gate poller picks it up. Called by the Inbox.
    /// Synchronous on purpose: it is a tiny local write, and the operator
    /// must SEE a failure — a fire-and-forget task that only logs would
    /// silently lose the decision. Does not navigate; callers decide what
    /// happens next.
    fn write_gate_decision(&mut self, task_id: String, approved: bool, cx: &mut Context<Self>) {
        let project_path = match &self.mode {
            AppMode::Project { _path, .. } => _path.clone(),
            _ => return,
        };

        let gate_dir = project_path.join(".surge").join("gates");
        let decision_file = gate_dir.join(format!("{task_id}.json"));

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let decision_data = serde_json::json!({
            "task_id": task_id,
            "approved": approved,
            "timestamp": timestamp.to_string(),
        });

        let result = std::fs::create_dir_all(&gate_dir)
            .and_then(|()| std::fs::write(&decision_file, decision_data.to_string()));

        let notification = match result {
            Ok(()) => SurgeNotification::gate_decision_recorded(&task_id, approved),
            Err(e) => {
                tracing::error!("failed to write gate decision for {task_id}: {e}");
                SurgeNotification::gate_decision_failed(&task_id, &e.to_string())
            },
        };
        self.pending_notifications.push(notification);
        cx.notify();
    }

    pub fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            // Surface navigation: ⌘1..9 on macOS, Ctrl+1..9 elsewhere
            KeyBinding::new("secondary-1", GoToFleet, None),
            KeyBinding::new("secondary-2", GoToRoadmap, None),
            KeyBinding::new("secondary-3", GoToRuns, None),
            KeyBinding::new("secondary-4", GoToFlow, None),
            KeyBinding::new("secondary-5", GoToInbox, None),
            KeyBinding::new("secondary-6", GoToBacklog, None),
            KeyBinding::new("secondary-7", GoToAgents, None),
            KeyBinding::new("secondary-8", GoToMemory, None),
            KeyBinding::new("secondary-9", GoToSettings, None),
            // UI toggles
            KeyBinding::new("secondary-b", ToggleSidebarAction, None),
            KeyBinding::new("secondary-k", ToggleCommandPalette, None),
            // Project
            KeyBinding::new("secondary-shift-p", SwitchProject, None),
            // Tasks
            KeyBinding::new("secondary-n", NewTask, None),
            KeyBinding::new("secondary-o", OpenProjectDialog, None),
        ]);
    }

    fn render_screen_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active_screen {
            Screen::Fleet => {
                let state = self.state.clone();
                let fleet = self.fleet.get_or_insert_with(|| {
                    let f = cx.new(|cx| FleetScreen::new(state, cx));
                    cx.subscribe(&f, |this: &mut Self, _f, event: &FleetAction, cx| {
                        match event {
                            FleetAction::OpenGate(_id) => {
                                // Decisions live in the Inbox — the queue picks
                                // the most urgent item automatically.
                                this.navigate(Screen::Inbox, cx);
                            },
                            FleetAction::OpenRun(run_id) => {
                                this.open_run_cockpit(*run_id, cx);
                            },
                            FleetAction::Dispatch(prompt) => {
                                this.dispatch_run(prompt.clone(), cx);
                            },
                        }
                    })
                    .detach();
                    f
                });
                fleet.clone().into_any_element()
            },
            Screen::Flow => {
                let state = self.state.clone();
                let s = self
                    .flow
                    .get_or_insert_with(|| cx.new(|cx| FlowScreen::with_state(state, cx)));
                s.clone().into_any_element()
            },
            Screen::Runs => {
                let state = self.state.clone();
                let s =
                    self.runs_screen
                        .get_or_insert_with(|| {
                            let r = cx.new(|cx| RunsScreen::new(state, cx));
                            cx.subscribe(&r, |this: &mut Self, _r, event: &RunsEvent, cx| {
                                match event {
                                    RunsEvent::DescribeApp => this.navigate(Screen::Fleet, cx),
                                }
                            })
                            .detach();
                            r
                        })
                        .clone();
                // Apply a parked deep-link selection now that the screen
                // entity is guaranteed to exist.
                if let Some(run_id) = self.pending_run_selection.take() {
                    s.update(cx, |screen, cx| screen.select_run(run_id, cx));
                }
                s.into_any_element()
            },
            Screen::ContextMemory => {
                let state = self.state.clone();
                let s = self.memory.get_or_insert_with(|| {
                    let m = cx.new(|cx| MemoryScreen::new(state, cx));
                    cx.subscribe(
                        &m,
                        |this: &mut Self, _m, event: &MemoryAction, cx| match event {
                            MemoryAction::OpenRun(run_id) => {
                                this.open_run_cockpit(Some(*run_id), cx)
                            },
                        },
                    )
                    .detach();
                    m
                });
                s.clone().into_any_element()
            },
            Screen::Roadmap => {
                let state = self.state.clone();
                let s = self.roadmap.get_or_insert_with(|| {
                    let r = cx.new(|cx| RoadmapScreen::new(state, cx));
                    cx.subscribe(
                        &r,
                        |this: &mut Self, _r, event: &RoadmapEvent, cx| match event {
                            RoadmapEvent::DescribeApp => this.navigate(Screen::Fleet, cx),
                        },
                    )
                    .detach();
                    r
                });
                s.clone().into_any_element()
            },
            Screen::Inbox => {
                let state = self.state.clone();
                let inbox = self.inbox.get_or_insert_with(|| {
                    let i = cx.new(|cx| InboxScreen::new(state, cx));
                    cx.subscribe(
                        &i,
                        |this: &mut Self, _i, event: &InboxAction, cx| match event {
                            InboxAction::OpenRun(run_id) => {
                                this.open_run_cockpit(Some(*run_id), cx);
                            },
                            InboxAction::OpenPlan => this.navigate(Screen::Flow, cx),
                            InboxAction::TaskDecision { task_id, approved } => {
                                this.write_gate_decision(task_id.clone(), *approved, cx);
                            },
                        },
                    )
                    .detach();
                    i
                });
                inbox.clone().into_any_element()
            },
            Screen::Backlog => {
                let state = self.state.clone();
                let backlog = self.backlog.get_or_insert_with(|| {
                    let b = cx.new(|cx| BacklogScreen::new(state, cx));
                    cx.subscribe(
                        &b,
                        |this: &mut Self, _b, event: &BacklogAction, cx| match event {
                            BacklogAction::OpenMission(run_id) => {
                                this.open_run_cockpit(Some(*run_id), cx);
                            },
                            BacklogAction::NewTask => {
                                this.navigate(Screen::SpecWizard, cx);
                            },
                            BacklogAction::Dispatch { prompt } => {
                                this.dispatch_run(prompt.clone(), cx);
                            },
                        },
                    )
                    .detach();
                    b
                });
                backlog.clone().into_any_element()
            },
            Screen::Agents => {
                let state = self.state.clone();
                let agents = self.agents_screen.get_or_insert_with(|| {
                    let a = cx.new(|cx| AgentsScreen::new(state, cx));
                    cx.subscribe(
                        &a,
                        |this: &mut Self, _a, event: &AgentsAction, cx| match event {
                            AgentsAction::OpenCatalog => {
                                this.navigate(Screen::AgentHub, cx);
                            },
                            AgentsAction::OpenTerminal(_id) => {
                                this.navigate(Screen::AgentTerminals, cx);
                            },
                        },
                    )
                    .detach();
                    a
                });
                agents.clone().into_any_element()
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
                let project_path = self.state.read(cx).project_path.clone().unwrap_or_default();
                let drafts = &mut self.wizard_drafts;
                let spec_wizard = self.spec_wizard.get_or_insert_with(|| {
                    if let Some(draft) = drafts.get(&project_path) {
                        return draft.clone();
                    }
                    let wizard = cx.new(|cx| SpecWizardScreen::new(project_path.clone(), cx));
                    cx.subscribe(
                        &wizard,
                        |this: &mut Self,
                         wizard,
                         event: &crate::screens::spec_wizard::SpecWizardEvent,
                         cx| {
                            this.handle_wizard_event(wizard, event, cx);
                        },
                    )
                    .detach();
                    drafts.insert(project_path, wizard.clone());
                    wizard
                });
                spec_wizard.clone().into_any_element()
            },
            Screen::Worktrees => {
                let state = self.state.clone();
                let s = self
                    .worktrees
                    .get_or_insert_with(|| cx.new(|cx| WorktreesScreen::new(state, cx)));
                s.clone().into_any_element()
            },
            Screen::Settings => {
                let state = self.state.clone();
                let s = self
                    .settings
                    .get_or_insert_with(|| cx.new(|cx| SettingsScreen::new(state, cx)));
                s.clone().into_any_element()
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
                                    .role(Role::Button)
                                    .aria_label("Close task details")
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
                                    .role(Role::Button)
                                    .aria_label("Close task details")
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

    fn render_palette_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(palette) = &self.command_palette {
            div()
                .id("palette-overlay")
                .occlude()
                // Clicking the dimmed backdrop closes the palette; clicks on
                // the palette itself stop before they get here.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.close_palette(cx)),
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(80.0))
                .bg(hsla(0.0, 0.0, 0.0, if theme::is_dark() { 0.6 } else { 0.3 }))
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            cx.stop_propagation();
                        })
                        .child(palette.clone()),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }

    /// Client-side titlebar: app mark + drag region + min/max/close.
    /// Styled to our dark chrome; window controls come from gpui-component.
    fn render_title_bar(&self) -> impl IntoElement {
        gpui_kit::component::TitleBar::new()
            .bg(theme::panel())
            .border_color(theme::hairline())
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(crate::ui::brand_mark(13.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child("Surge"),
                    ),
            )
    }
}

impl Render for SurgeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Flush any queued notifications now that we have Window access.
        self.flush_notifications(window, cx);

        // Key bindings dispatch along the focus path. With nothing focused
        // (startup, after a dialog closes) the root's action handlers are
        // off that path and ⌘1…9 / ⌘K / ⌘N / ⌘O silently do nothing.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }

        let content: AnyElement =
            match &self.mode {
                AppMode::Welcome(welcome) => div()
                    .key_context("SurgeApp")
                    .track_focus(&self.focus)
                    // The start page's two ways in, on the keys its buttons show.
                    .on_action(cx.listener(|this, _: &NewTask, _w, cx| {
                        this.handle_welcome_event(WelcomeEvent::NewProject, cx)
                    }))
                    .on_action(cx.listener(|this, _: &OpenProjectDialog, _w, cx| {
                        this.handle_welcome_event(WelcomeEvent::BrowseProject, cx)
                    }))
                    .size_full()
                    .font_family(crate::ui::MONO)
                    .child(welcome.clone())
                    .into_any_element(),
                AppMode::Project { .. } => {
                    div()
                        .key_context("SurgeApp")
                        .track_focus(&self.focus)
                        .size_full()
                        .font_family(crate::ui::MONO)
                        .bg(theme::background())
                        .text_color(theme::text_primary())
                        .on_action(cx.listener(|this, _: &GoToFleet, _w, cx| {
                            this.navigate(Screen::Fleet, cx)
                        }))
                        .on_action(cx.listener(|this, _: &GoToRoadmap, _w, cx| {
                            this.navigate(Screen::Roadmap, cx)
                        }))
                        .on_action(
                            cx.listener(|this, _: &GoToRuns, _w, cx| {
                                this.navigate(Screen::Runs, cx)
                            }),
                        )
                        .on_action(
                            cx.listener(|this, _: &GoToFlow, _w, cx| {
                                this.navigate(Screen::Flow, cx)
                            }),
                        )
                        .on_action(cx.listener(|this, _: &GoToInbox, _w, cx| {
                            this.navigate(Screen::Inbox, cx)
                        }))
                        .on_action(cx.listener(|this, _: &GoToBacklog, _w, cx| {
                            this.navigate(Screen::Backlog, cx)
                        }))
                        .on_action(cx.listener(|this, _: &GoToAgents, _w, cx| {
                            this.navigate(Screen::Agents, cx)
                        }))
                        .on_action(cx.listener(|this, _: &GoToMemory, _w, cx| {
                            this.navigate(Screen::ContextMemory, cx)
                        }))
                        .on_action(cx.listener(|this, _: &GoToSettings, _w, cx| {
                            this.navigate(Screen::Settings, cx)
                        }))
                        .on_action(cx.listener(|this, _: &ToggleSidebarAction, _w, cx| {
                            this.toggle_sidebar(cx)
                        }))
                        .on_action(cx.listener(|this, _: &ToggleCommandPalette, _w, cx| {
                            this.toggle_palette(cx)
                        }))
                        .on_action(cx.listener(|this, _: &SwitchProject, _w, cx| {
                            // Toggle project switcher in top bar.
                            if let Some(top_bar) = &this.top_bar {
                                top_bar.update(cx, |tb, cx| tb.toggle_switcher(cx));
                            }
                        }))
                        .on_action(cx.listener(|this, _: &NewTask, _w, cx| {
                            this.navigate(Screen::SpecWizard, cx)
                        }))
                        .on_action(cx.listener(|this, _: &OpenProjectDialog, _w, cx| {
                            this.handle_welcome_event(WelcomeEvent::BrowseProject, cx)
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
                        .child(self.render_palette_overlay(cx))
                        .child(self.render_task_detail_overlay(cx))
                        .into_any_element()
                },
            };

        div()
            .size_full()
            .v_flex()
            .font_family(crate::ui::MONO)
            .bg(theme::background())
            .child(self.render_title_bar())
            .child(div().flex_1().min_h_0().child(content))
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

#[cfg(test)]
mod bootstrap_index_tests {
    use super::{BootstrapIndexEntry, SurgeApp};
    use std::path::PathBuf;

    #[test]
    fn operation_index_round_trips_for_ui_restart_recovery() {
        let root = std::env::temp_dir().join(format!(
            "surge-bootstrap-index-{}",
            surge_core::RunId::new()
        ));
        let path = root.join("desktop/bootstrap-operations.json");
        let entries = vec![BootstrapIndexEntry {
            operation_id: surge_core::RunId::new(),
            project_path: PathBuf::from("/tmp/application"),
        }];
        SurgeApp::save_bootstrap_index_at(path.clone(), &entries).unwrap();
        assert_eq!(SurgeApp::load_bootstrap_index_at(&path), entries);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_operation_index_does_not_invent_work() {
        let path = std::env::temp_dir().join(format!(
            "surge-bootstrap-corrupt-{}.json",
            surge_core::RunId::new()
        ));
        std::fs::write(&path, b"{bad").unwrap();
        assert!(SurgeApp::load_bootstrap_index_at(&path).is_empty());
        std::fs::remove_file(path).unwrap();
    }
}
