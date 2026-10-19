#[path = "app_observation.rs"]
mod observation;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::component::StyledExt as _;
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
use crate::screens::project_home::{HomeAction, ProjectHomeScreen};
use crate::screens::roadmap::{RoadmapEvent, RoadmapScreen};
use crate::screens::runs::{RunsEvent, RunsScreen};
use crate::screens::settings::SettingsScreen;
use crate::screens::spec_wizard::SpecWizardScreen;
use crate::screens::welcome::{WelcomeEvent, WelcomeScreen};
use crate::sidebar::{AppSidebar, NavigateTo, ToggleSidebar};
use crate::theme;
use crate::top_bar::{TopBar, TopBarEvent};

/// Application mode — Welcome picker or Main project view.
enum AppMode {
    Welcome(Entity<WelcomeScreen>),
    Project { _path: PathBuf, _name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BootstrapIndexEntry {
    operation_id: surge_core::RunId,
    project_path: PathBuf,
}

/// Reconnect attempts (1s apart) after a live daemon link drops.
const DAEMON_RECONNECT_ATTEMPTS: u32 = 60;

#[derive(Clone)]
enum DispatchOrigin {
    Wizard(Entity<SpecWizardScreen>),
    Fleet(Entity<FleetScreen>),
    Other(Screen),
}

#[cfg(test)]
type ControlledHistoryReads = std::rc::Rc<
    std::cell::RefCell<
        std::collections::VecDeque<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
>;

/// Root application view.
pub struct SurgeApp {
    #[cfg(test)]
    controlled_operation_reads: Option<ControlledHistoryReads>,
    #[cfg(test)]
    controlled_operation_rechecks: Option<ControlledHistoryReads>,
    #[cfg(test)]
    current_read_settlements: Vec<(
        crate::run_observation::Token,
        crate::app_state::ReadDisposition,
    )>,
    #[cfg(test)]
    controlled_history_reads: Option<ControlledHistoryReads>,
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
    // Screen entities (created on demand).
    fleet: Option<Entity<FleetScreen>>,
    home: Option<Entity<ProjectHomeScreen>>,
    fleet_drafts: HashMap<PathBuf, Entity<FleetScreen>>,
    accepted_planning_runs: HashMap<surge_core::RunId, surge_core::RunId>,
    flow: Option<Entity<FlowScreen>>,
    memory: Option<Entity<MemoryScreen>>,
    runs_screen: Option<Entity<RunsScreen>>,
    roadmap: Option<Entity<RoadmapScreen>>,
    inbox: Option<Entity<InboxScreen>>,
    backlog: Option<Entity<BacklogScreen>>,
    agents_screen: Option<Entity<AgentsScreen>>,
    agent_hub: Option<Entity<AgentHubScreen>>,
    spec_wizard: Option<Entity<SpecWizardScreen>>,
    wizard_return_screen: Screen,
    wizard_drafts: HashMap<PathBuf, Entity<SpecWizardScreen>>,
    agent_terminal: Option<Entity<AgentTerminalScreen>>,
    settings: Option<Entity<SettingsScreen>>,
    /// Queued notifications to flush on next render (needs Window access).
    pending_notifications: Vec<gpui_kit::component::notification::Notification>,
    /// Runs we already attached a per-run event subscription for.
    stream_subscribed: HashSet<surge_core::id::RunId>,
    /// Run to focus when the Runs cockpit next renders. Deep links land
    /// here because the RunsScreen entity is created lazily — calling
    /// select_run before it exists would silently drop the selection.
    pending_run_selection: Option<surge_core::id::RunId>,
    pending_run_tab: Option<crate::screens::runs::RunTab>,
    pending_recorded_selection: Option<(surge_core::RunId, crate::screens::runs::RecordedTarget)>,
    pending_home_decision: Option<(surge_core::RunId, u64)>,
    pending_task_selection: Option<(
        surge_core::roadmap::MilestoneId,
        surge_core::roadmap::RoadmapTaskId,
    )>,
    bootstrap_operations: HashMap<
        surge_core::RunId,
        (
            PathBuf,
            surge_core::bootstrap_operation::BootstrapOperationStatus,
        ),
    >,
    bootstrap_index: Vec<BootstrapIndexEntry>,
    bootstrap_index_location: Option<PathBuf>,
    offline_operations: HashMap<(crate::run_observation::Scope, surge_core::RunId), u64>,
    pending_operations:
        HashMap<(crate::run_observation::Scope, surge_core::RunId), observation::OperationDemand>,
    next_operation_request: u64,
    fallback_owners: HashMap<surge_core::RunId, observation::FallbackOwner>,
    offline_retries:
        HashMap<(crate::run_observation::Scope, surge_core::RunId), observation::RetryEpisode>,
    next_offline_retry: u64,
    operation_polls: HashMap<surge_core::RunId, (crate::run_observation::Scope, u64)>,
    next_operation_poll: u64,
    #[cfg(all(test, unix))]
    operation_poll_settlements: HashMap<surge_core::RunId, (u64, u64)>,
}

impl SurgeApp {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut app = Self::new_shell(state, cx);
        app.restore_indexed_operations(cx);
        Self::spawn_daemon_link(&app.state, cx);
        app
    }

    fn new_shell(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::new_shell_at(state, Self::bootstrap_index_path(), cx)
    }

    fn new_shell_at(
        state: Entity<AppState>,
        bootstrap_index_location: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let active_screen = Screen::Home;
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
            |this: &mut Self, _sidebar, _event: &crate::sidebar::CreateTask, cx| {
                this.open_new_task(cx);
            },
        )
        .detach();

        cx.subscribe(
            &sidebar,
            |this: &mut Self, _, event: &crate::sidebar::OpenRecentTask, cx| {
                this.navigate(Screen::Fleet, cx);
                let fleet = this.ensure_fleet(cx);
                fleet.update(cx, |fleet, cx| match event {
                    crate::sidebar::OpenRecentTask::Saved(item) => fleet.open_saved_task(*item, cx),
                    crate::sidebar::OpenRecentTask::Run(run) => fleet.open_run_task(*run, cx),
                });
            },
        )
        .detach();

        cx.subscribe(
            &sidebar,
            |this: &mut Self, _, _: &crate::sidebar::ShowTasks, cx| {
                this.navigate(Screen::Fleet, cx);
                this.ensure_fleet(cx)
                    .update(cx, |fleet, cx| fleet.show_task_list(cx));
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
            #[cfg(test)]
            controlled_history_reads: None,
            #[cfg(test)]
            controlled_operation_reads: None,
            #[cfg(test)]
            controlled_operation_rechecks: None,
            #[cfg(test)]
            current_read_settlements: Vec::new(),
            state,
            focus,
            mode: AppMode::Welcome(welcome),
            active_screen,
            sidebar_collapsed: false,
            sidebar,
            top_bar: None,
            command_palette_open: false,
            command_palette: None,
            fleet: None,
            home: None,
            fleet_drafts: HashMap::new(),
            accepted_planning_runs: HashMap::new(),
            flow: None,
            memory: None,
            runs_screen: None,
            roadmap: None,
            inbox: None,
            backlog: None,
            agents_screen: None,
            agent_terminal: None,
            agent_hub: None,
            spec_wizard: None,
            wizard_return_screen: Screen::Fleet,
            wizard_drafts: HashMap::new(),
            settings: None,
            pending_notifications: Vec::new(),
            stream_subscribed: HashSet::new(),
            pending_run_selection: None,
            pending_run_tab: None,
            pending_recorded_selection: None,
            pending_home_decision: None,
            pending_task_selection: None,
            bootstrap_operations: HashMap::new(),
            bootstrap_index: bootstrap_index_location
                .as_deref()
                .map(Self::load_bootstrap_index_at)
                .unwrap_or_default(),
            bootstrap_index_location,
            offline_operations: HashMap::new(),
            pending_operations: HashMap::new(),
            next_operation_request: 0,
            fallback_owners: HashMap::new(),
            offline_retries: HashMap::new(),
            next_offline_retry: 0,
            operation_polls: HashMap::new(),
            next_operation_poll: 0,
            #[cfg(all(test, unix))]
            operation_poll_settlements: HashMap::new(),
        }
    }

    /// Deep-link into the Runs cockpit, focusing `run_id` if given.
    /// Selection is parked in `pending_run_selection` and applied when
    /// the (lazily created) RunsScreen renders.
    fn open_run_cockpit(&mut self, run_id: Option<surge_core::id::RunId>, cx: &mut Context<Self>) {
        self.pending_task_selection = None;
        self.pending_recorded_selection = None;
        self.pending_run_tab = None;
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

    /// Start an application from a free-form request (Fleet command bar,
    /// Backlog dispatch). Goes through the daemon-supervised bootstrap
    /// operation, which continues from the approved plan into the
    /// implementation run; a plain planning run stopped after the flow gate
    /// and never built anything.
    fn dispatch_run(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.dispatch_bootstrap(
            prompt,
            surge_core::RunId::new(),
            DispatchOrigin::Other(self.active_screen),
            cx,
        );
    }

    /// Submit a stable, daemon-owned application operation and retain its identity locally.
    fn dispatch_bootstrap(
        &mut self,
        prompt: String,
        operation_id: surge_core::RunId,
        origin: DispatchOrigin,
        cx: &mut Context<Self>,
    ) {
        if prompt.trim().is_empty() {
            self.dispatch_failed(
                operation_id,
                &origin,
                "Describe what you want to build before starting.".into(),
                cx,
            );
            return;
        }
        let project_path = self.state.read(cx).project_path.clone();
        let Some(project_path) = project_path else {
            self.dispatch_failed(
                operation_id,
                &origin,
                "Open a configured Git project before starting.".into(),
                cx,
            );
            return;
        };
        if let Some(error) = self.state.read(cx).project_load_error.clone() {
            self.dispatch_failed(operation_id, &origin, error, cx);
            return;
        }
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
                &origin,
                "This build cannot start safely: durable application runs cannot enforce a USD cap across planning and implementation yet. Remove the USD cap or set a token-only limit in project Settings, then retry.".into(),
                cx,
            );
            return;
        }
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.dispatch_failed(
                operation_id,
                &origin,
                "Daemon offline. Start it from the sidebar, then retry.".into(),
                cx,
            );
            return;
        };

        let entry = BootstrapIndexEntry {
            operation_id,
            project_path: project_path.clone(),
        };
        self.bootstrap_index
            .retain(|existing| existing.operation_id != operation_id);
        self.bootstrap_index.push(entry);
        if let Err(error) = self
            .bootstrap_index_location
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Surge home is unavailable"))
            .and_then(|path| Self::save_bootstrap_index_at(path, &self.bootstrap_index))
        {
            tracing::warn!(%operation_id, %error, "could not persist desktop bootstrap index");
        }
        let dispatch_scope = self.state.update(cx, |state, _| state.observation_scope());
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
                        if app.state.update(cx, |state, _| state.observation_scope())
                            != dispatch_scope
                            || status.operation_id != operation_id
                        {
                            return;
                        }
                        app.update_bootstrap_view(operation_id, status.clone(), &project_path, cx);
                        app.dispatch_accepted(
                            operation_id,
                            status.planning_run,
                            &project_path,
                            &origin,
                            cx,
                        );
                        app.track_bootstrap_operation(
                            facade.clone(),
                            operation_id,
                            project_path.clone(),
                            cx,
                        );
                    });
                },
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        if app.state.update(cx, |state, _| state.observation_scope())
                            == dispatch_scope
                        {
                            app.dispatch_failed(operation_id, &origin, error, cx);
                        }
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
        cx: &mut Context<Self>,
    ) {
        let scope = self.state.update(cx, |state, _| state.observation_scope());
        let Some(scope) = scope else {
            return;
        };
        if self
            .operation_polls
            .get(&operation_id)
            .is_some_and(|(current, _)| current == &scope)
        {
            return;
        }
        self.next_operation_poll = self.next_operation_poll.saturating_add(1);
        let poll = (scope.clone(), self.next_operation_poll);
        self.operation_polls.insert(operation_id, poll.clone());
        #[cfg(all(test, unix))]
        self.operation_poll_settlements
            .insert(operation_id, (poll.1, 0));
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let status = facade.bootstrap_status(operation_id).await;
                match status {
                    Ok(status) => {
                        let is_terminal = status.state.phase().is_none()
                            && !status.continuations.iter().any(|attempt| {
                                matches!(attempt.state,
                                surge_core::bootstrap_attempt::BootstrapAttemptState::Reserved
                                | surge_core::bootstrap_attempt::BootstrapAttemptState::ForkReady
                                | surge_core::bootstrap_attempt::BootstrapAttemptState::Running)
                            });
                        let accepted = this
                            .update(cx, |app, cx| {
                                if app.operation_polls.get(&operation_id) != Some(&poll)
                                    || app.state.update(cx, |state, _| {
                                        state.observation_scope().as_ref() != Some(&scope)
                                    })
                                {
                                    return None;
                                }
                                app.update_bootstrap_view(
                                    operation_id,
                                    status.clone(),
                                    &display_project,
                                    cx,
                                );
                                #[cfg(all(test, unix))]
                                if let Some((generation, completed)) =
                                    app.operation_poll_settlements.get_mut(&operation_id)
                                    && *generation == poll.1
                                {
                                    *completed = completed.saturating_add(1);
                                }
                                Some(
                                    app.bootstrap_operations
                                        .get(&operation_id)
                                        .is_some_and(|(_, snapshot)| snapshot == &status),
                                )
                            })
                            .ok()
                            .flatten();
                        if accepted.is_none() || (is_terminal && accepted == Some(true)) {
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
            let _ = this.update(cx, |app, cx| {
                if app.operation_polls.get(&operation_id) == Some(&poll)
                    && app.state.update(cx, |state, _| {
                        state.observation_scope().as_ref() == Some(&scope)
                    })
                {
                    app.operation_polls.remove(&operation_id);
                }
            });
        })
        .detach();
    }

    fn accept_bootstrap_continuation(
        &mut self,
        operation: surge_core::RunId,
        status: &surge_core::bootstrap_operation::BootstrapOperationStatus,
        scope: &crate::run_observation::Scope,
        cx: &mut Context<Self>,
    ) {
        if status.operation_id != operation
            || self
                .state
                .update(cx, |state, _| state.observation_scope())
                .as_ref()
                != Some(scope)
        {
            return;
        }
        self.update_bootstrap_view(operation, status.clone(), &scope.project, cx);
        if !self
            .bootstrap_operations
            .get(&operation)
            .is_some_and(|(_, current)| current == status)
        {
            return;
        }
        if let Some(facade) = self.state.read(cx).daemon_state.facade() {
            // Replace any older terminal poll lease before it can retire this
            // continuation's observation. Old callbacks remain generation fenced.
            self.operation_polls.remove(&operation);
            self.track_bootstrap_operation(facade, operation, scope.project.clone(), cx);
        }
    }

    fn update_bootstrap_view(
        &mut self,
        operation_id: surge_core::RunId,
        status: surge_core::bootstrap_operation::BootstrapOperationStatus,
        project_path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let root = state
            .project_scope
            .as_ref()
            .map(|scope| scope.root())
            .or(state.project_path.as_deref());
        let canonical_project = project_path
            .canonicalize()
            .unwrap_or_else(|_| project_path.to_path_buf());
        let canonical_root =
            root.map(|root| root.canonicalize().unwrap_or_else(|_| root.to_path_buf()));
        if status.operation_id != operation_id
            || !(canonical_root.as_deref() == Some(canonical_project.as_path())
                || root == Some(project_path)
                || state.project_path.as_deref() == Some(project_path))
        {
            return;
        }
        let project_path = root
            .unwrap_or(project_path)
            .canonicalize()
            .unwrap_or_else(|_| root.unwrap_or(project_path).to_path_buf());
        let project_path = project_path.as_path();
        if let Some((project, previous)) = self.bootstrap_operations.get(&operation_id) {
            if project != project_path
                || status.planning_run != previous.planning_run
                || status.implementation_run != previous.implementation_run
                || status.revision < previous.revision
                || (status.revision == previous.revision && status != *previous)
            {
                return;
            }
            if status == *previous {
                let restored = self.state.update(cx, |state, cx| {
                    if state.bootstrap_operations.contains_key(&operation_id) {
                        return false;
                    }
                    state
                        .bootstrap_operations
                        .insert(operation_id, status.clone());
                    cx.notify();
                    true
                });
                if restored {
                    self.hydrate_related(operation_id, cx);
                }
                return;
            }
        }
        let snapshot = status;
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
        if let Err(error) = self
            .bootstrap_index_location
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Surge home is unavailable"))
            .and_then(|path| Self::save_bootstrap_index_at(path, &self.bootstrap_index))
        {
            tracing::warn!(%operation_id, %error, "could not update desktop bootstrap index");
        }
        self.state.update(cx, |state, cx| {
            state
                .bootstrap_operations
                .insert(operation_id, snapshot.clone());
            cx.notify();
        });
        self.hydrate_related(operation_id, cx);
    }

    fn bootstrap_index_path() -> Option<PathBuf> {
        surge_core::home::surge_home_dir()
            .map(|home| home.join("desktop").join("bootstrap-operations.json"))
    }

    fn load_bootstrap_index_at(path: &std::path::Path) -> Vec<BootstrapIndexEntry> {
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
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
            if !self.operation_polls.contains_key(&entry.operation_id) {
                self.track_bootstrap_operation(
                    facade.clone(),
                    entry.operation_id,
                    current_project
                        .as_ref()
                        .filter(|path| ***path == entry.project_path)
                        .map_or_else(|| entry.project_path.clone(), Clone::clone),
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
                if self.active_screen != Screen::SpecWizard
                    || self.spec_wizard.as_ref() != Some(&wizard)
                    || self.state.read(cx).project_path.as_ref()
                        != Some(&wizard.read(cx).project_path)
                {
                    return;
                }
                self.dispatch_bootstrap(
                    description.clone(),
                    *run_id,
                    DispatchOrigin::Wizard(wizard.clone()),
                    cx,
                );
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
                self.open_run_cockpit(
                    Some(
                        self.accepted_planning_runs
                            .get(run_id)
                            .copied()
                            .unwrap_or(*run_id),
                    ),
                    cx,
                );
            },
            SpecWizardEvent::Cancel => {
                if self.active_screen == Screen::SpecWizard
                    && self.spec_wizard.as_ref() == Some(&wizard)
                    && self.state.read(cx).project_path.as_ref()
                        == Some(&wizard.read(cx).project_path)
                {
                    self.navigate(self.wizard_return_screen, cx);
                }
            },
        }
    }

    fn dispatch_failed(
        &mut self,
        run_id: surge_core::RunId,
        origin: &DispatchOrigin,
        error: String,
        cx: &mut Context<Self>,
    ) {
        self.operation_polls.remove(&run_id);
        if let DispatchOrigin::Wizard(wizard) = origin {
            wizard.update(cx, |wizard, cx| {
                wizard.finish_submission(run_id, Err(error), cx);
            });
        } else if let DispatchOrigin::Fleet(fleet) = origin {
            fleet.update(cx, |fleet, cx| {
                fleet.finish_submission(run_id, Err(error), cx);
            });
        } else {
            self.pending_notifications
                .push(SurgeNotification::task_failed("planning", &error));
        }
        cx.notify();
    }

    fn dispatch_accepted(
        &mut self,
        operation_id: surge_core::RunId,
        run_id: surge_core::RunId,
        project_path: &std::path::Path,
        origin: &DispatchOrigin,
        cx: &mut Context<Self>,
    ) {
        self.accepted_planning_runs.insert(operation_id, run_id);
        let same_project = self.state.read(cx).project_path.as_deref() == Some(project_path);
        let show_run = if let DispatchOrigin::Wizard(wizard) = origin {
            let applied = wizard.update(cx, |wizard, cx| {
                wizard.finish_submission(operation_id, Ok(()), cx)
            });
            applied
                && same_project
                && self.active_screen == Screen::SpecWizard
                && self.spec_wizard.as_ref() == Some(wizard)
        } else if let DispatchOrigin::Fleet(fleet) = origin {
            let applied = fleet.update(cx, |fleet, cx| {
                fleet.finish_submission(operation_id, Ok(()), cx)
            });
            applied
                && same_project
                && self.active_screen == Screen::Fleet
                && self.fleet.as_ref() == Some(fleet)
        } else if let DispatchOrigin::Other(screen) = origin {
            same_project && self.active_screen == *screen
        } else {
            false
        };
        if show_run {
            if matches!(origin, DispatchOrigin::Wizard(_)) {
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
    fn finish_run_pump(&mut self, token: &crate::run_observation::Token, cx: &mut Context<Self>) {
        let retired = self.state.update(cx, |state, cx| {
            if !state.accepts_observation(token, false) {
                return false;
            }
            if let Some(stream) = state.run_streams.get_mut(&token.run) {
                stream.live = false;
            }
            let retired = state.observations.retire(token);
            cx.notify();
            retired
        });
        if retired {
            self.stream_subscribed.remove(&token.run);
            self.drain_pending_operations(cx);
        }
    }

    fn receive_global_event(
        &mut self,
        facade: &std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
        scope: &Option<crate::run_observation::Scope>,
        event: &surge_orchestrator::engine::ipc::GlobalDaemonEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.current_global_connection(facade, cx)
            || self.state.update(cx, |state, _| state.observation_scope()) != *scope
        {
            return;
        }
        if let surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted { run_id } = event
            && let Some(owner) = self.fallback_owners.get_mut(run_id)
            && self.state.update(cx, |state, _| {
                state.accepts_observation(&owner.token, false)
            })
        {
            owner.renewed = true;
        }
        self.state.update(cx, |state, cx| {
            state.apply_global_event(event);
            cx.notify();
        });
        self.queue_notification_for_global(event);
        self.sync_run_subscriptions(cx);
        cx.notify();
    }

    fn current_global_connection(
        &self,
        facade: &std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.state
            .read(cx)
            .daemon_state
            .facade()
            .is_some_and(|current| std::sync::Arc::ptr_eq(&current, facade))
    }

    fn sync_run_subscriptions(&mut self, cx: &mut Context<Self>) {
        use surge_orchestrator::engine::handle::RunStatus;

        // Prune bookkeeping for runs that vanished from the daemon's list.
        let known: HashSet<surge_core::id::RunId> =
            {
                let state = self.state.read(cx);
                state
                    .runs
                    .iter()
                    .map(|r| r.run_id)
                    .chain(state.bootstrap_operations.values().flat_map(|operation| {
                        [operation.planning_run, operation.implementation_run]
                    }))
                    .collect()
            };
        self.stream_subscribed.retain(|id| known.contains(id));
        self.state.update(cx, |s, cx| {
            s.observations.retain_runs(&known);
            let before = s.run_streams.len();
            s.run_streams.retain(|id, _| known.contains(id));
            if s.run_streams.len() != before {
                cx.notify();
            }
        });

        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            return;
        };
        self.fallback_owners.retain(|_, owner| {
            self.state.update(cx, |state, _| {
                state.accepts_observation(&owner.token, false)
            })
        });
        let to_subscribe: Vec<surge_core::id::RunId> = self
            .state
            .read(cx)
            .runs
            .iter()
            .filter(|r| matches!(r.status, RunStatus::Active))
            .map(|r| r.run_id)
            .filter(|id| {
                !self.stream_subscribed.contains(id)
                    && self.fallback_owners.get(id).is_none_or(|owner| {
                        owner.renewed
                            && !matches!(
                                owner.completion,
                                observation::FallbackCompletion::Terminal
                            )
                            && !self.state.read(cx).observations.is_reading(*id)
                    })
            })
            .collect();

        for run_id in to_subscribe {
            let token = self.state.update(cx, |state, _| {
                let scope = state.observation_scope()?;
                state.observations.claim(&scope, run_id, true)
            });
            let Some(token) = token else {
                continue;
            };
            self.fallback_owners.remove(&run_id);
            self.stream_subscribed.insert(run_id);
            // Weak handles only — an immortal pump must not pin the app
            // state alive, and a dead entity ends the loop naturally.
            let state = self.state.downgrade();
            #[cfg(test)]
            let controlled_reads = self.controlled_history_reads.clone();
            let facade = facade.clone();
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let unmark = |cx: &mut AsyncApp, this: &WeakEntity<Self>| {
                    cx.update(|cx| {
                        let _ = this.update(cx, |t, cx| {
                            t.finish_run_pump(&token, cx);
                        });
                    });
                };

                let mut rx = match facade.subscribe_to_run(run_id).await {
                    Ok(rx) => rx,
                    Err(e) => {
                        tracing::debug!(run_id = %run_id, "subscribe_to_run failed: {e}");
                        let _=this.update(cx,|app,cx|app.recover_rejected_subscription(token.clone(),cx));
                        return;
                    },
                };
                tracing::info!(run_id = %run_id, "per-run event stream attached");
                // One owner publishes both live traffic and detached read candidates.
                // A read never blocks receipt of newer persisted events.
                let start_hydration = |cx: &AsyncApp| {
                    let mut read_cx = cx.clone();
                    let state = state.clone();
                    let token = token.clone();
                    #[cfg(test)]
                    let barrier = controlled_reads.as_ref().and_then(|reads| reads.borrow_mut().pop_front());
                    Box::pin(async move {
                        #[cfg(test)]
                        if let Some((loaded,release)) = barrier {
                            return hydrate_run_history_observed(&state,&token,&mut read_cx,async move { let _=loaded.send(());let _=release.await; }).await;
                        }
                        hydrate_run_history(&state, &token, &mut read_cx).await
                    })
                };
                let mut hydration_attempt = 0_u32;
                let mut hydrated = false;
                let mut hydration = Some(start_hydration(cx));
                let mut next_retry = std::time::Instant::now() + history_retry_delay(0);
                cx.update(|cx| {
                    let _ = state.update(cx, |s, cx| {
                        if s.accepts_observation(&token, false) {
                            s.run_streams.entry(run_id).or_default().attach_live_owner();
                            cx.notify();
                        }
                    });
                });
                loop {
                    // A fixed retry instant: live traffic must not keep
                    // pushing the next attempt back.
                    let retry = cx
                        .background_executor()
                        .timer(next_retry.saturating_duration_since(std::time::Instant::now()));
                    let (received, history) = tokio::select! {
                        received = rx.recv() => (Some(received), None),
                        result = async { match hydration.as_mut() { Some(read) => read.await, None => crate::app_state::ReadCompletion::obsolete(&token) } }, if hydration.is_some() => (None, Some(result)),
                        () = retry, if !hydrated && hydration.is_none() => (None, None),
                    };
                    if let Some(completion) = history {
                        let _=this.update(cx,|app,cx| { app.settle_current_reads(std::slice::from_ref(&completion),cx); app.drain_pending_operations(cx); });
                        hydration = None;
                        hydrated = completion.confirmed();
                        if !hydrated { next_retry = std::time::Instant::now() + history_retry_delay(hydration_attempt); }
                        continue;
                    }
                    let Some(received) = received else {
                        hydration_attempt = hydration_attempt.saturating_add(1);
                        hydration = Some(start_hydration(cx));
                        continue;
                    };
                    match received {
                        Ok(event) => {
                            let alive = cx.update(|cx| {
                                state
                                    .update(cx, |s, cx| {
                                        if !s.accepts_observation(&token, false) { return false; }
                                        s.run_streams.entry(run_id).or_default().apply(&event);
                                        cx.notify();
                                        true
                                    })
                                    .unwrap_or(false)
                            });
                            if !alive { break; }
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(run_id = %run_id, dropped = n, "run stream lagged; restoring history");
                            cx.update(|cx| { let _ = state.update(cx, |s, cx| {
                                if !s.accepts_observation(&token, false) { return; }
                                s.run_streams.entry(run_id).or_default().history_unconfirmed("Live feed lost events; restoring history");
                                cx.notify();
                            }); });
                            // Dropped events are durable: rebuild from the journal.
                            hydration_attempt = 0;
                            hydrated = false;
                            next_retry = std::time::Instant::now();
                        },
                    }
                }
                // Settle the bounded read before releasing this owner. A revoked
                // candidate still completes without publishing through its old token.
                if let Some(read) = hydration { let completion = read.await; let _=this.update(cx,|app,cx| { app.settle_current_reads(std::slice::from_ref(&completion),cx); app.drain_pending_operations(cx); }); }
                cx.update(|cx| {
                    let _ = state.update(cx, |s, cx| {
                        if !s.accepts_observation(&token, false) { return; }
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
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        // Update recent projects.
        let mut recent = RecentProjects::load();
        recent.touch(&name, path);
        let _ = recent.save();

        if let (Some(project), Some(fleet)) =
            (self.state.read(cx).project_path.clone(), self.fleet.take())
        {
            self.fleet_drafts.insert(project, fleet);
        }
        self.home = None;
        self.pending_home_decision = None;
        // Load project data into AppState.
        self.state.update(cx, |state, _cx| {
            state.load_project(path);
        });

        self.stream_subscribed.clear();
        self.operation_polls.clear();
        self.offline_retries.clear();
        self.pending_operations.clear();
        self.fallback_owners.clear();
        self.restore_indexed_operations(cx);

        // Create top bar.
        self.install_top_bar(&name, path, cx);

        // Reset screen entities so they re-read from AppState.
        self.fleet = self.fleet_drafts.remove(path);
        self.flow = None;
        self.memory = None;
        self.runs_screen = None;
        self.roadmap = None;
        self.inbox = None;
        self.backlog = None;
        self.agents_screen = None;
        self.agent_hub = None;
        self.agent_terminal = None;
        self.spec_wizard = None;
        self.wizard_return_screen = Screen::Fleet;
        self.settings = None;

        self.mode = AppMode::Project {
            _path: path.to_path_buf(),
            _name: name,
        };
        self.active_screen = Screen::Home;
        self.sidebar
            .update(cx, |sb, cx| sb.set_active(Screen::Home, cx));
        cx.notify();
    }

    fn install_top_bar(&mut self, name: &str, path: &std::path::Path, cx: &mut Context<Self>) {
        let path = path.to_path_buf();
        let top_bar = cx.new(|cx| TopBar::new(name, Some(path), Screen::Home, cx));
        cx.subscribe(
            &top_bar,
            |this, _bar, event: &TopBarEvent, cx| match event {
                TopBarEvent::ProjectSwitcherOpened => {},
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

    fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if screen == Screen::Flow
            && let Some(flow) = &self.flow
        {
            flow.update(cx, |flow, cx| flow.show_project_plan(cx));
        }
        if screen == Screen::SpecWizard && self.active_screen != Screen::SpecWizard {
            self.wizard_return_screen = self.active_screen;
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

    /// Queue a notification for a `GlobalDaemonEvent` (run lifecycle).
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
                    RunOutcome::SettlementFailed { .. } => {
                        Some(SurgeNotification::run_unconfirmed(&short))
                    },
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

            // The same exact-run publisher owns cold hydration and live replacement.
            if let Some(home) = surge_core::home::surge_home_dir() {
                let limit = std::num::NonZeroU32::new(100).unwrap_or(std::num::NonZeroU32::MIN);
                if let Ok(summaries) =
                    surge_persistence::runs::Storage::inspect_existing_run_summaries(home, limit)
                        .await
                {
                    for summary in summaries {
                        hydrate_cold_summary(
                            &this,
                            &state_for_task,
                            summary,
                            cx,
                            std::future::ready(()),
                        )
                        .await;
                    }
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
            let rx = match facade.subscribe_global().await {
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
            let _ = this.update(cx, |app, cx| {
                app.watch_global_events(facade, rx, true, cx).detach()
            });
        })
        .detach();
    }

    fn watch_global_events(
        &mut self,
        facade: std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
        mut rx: tokio::sync::broadcast::Receiver<
            surge_orchestrator::engine::ipc::GlobalDaemonEvent,
        >,
        reconnect: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Task<()> {
        let state_for_task = self.state.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            tracing::info!("daemon link: subscribed to global events");

            loop {
                match rx.recv().await {
                    Ok(event) => {
                        cx.update(|cx| {
                            let _ = this.update(cx, |this, cx| {
                                if !this.current_global_connection(&facade, cx) {
                                    return;
                                }
                                let scope =
                                    this.state.update(cx, |state, _| state.observation_scope());
                                this.receive_global_event(&facade, &scope, &event, cx);
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
                let current = this
                    .update(cx, |app, cx| app.current_global_connection(&facade, cx))
                    .unwrap_or(false);
                if !current {
                    return;
                }
                let _ = state_for_task.update(cx, |state, cx| {
                    state.daemon_state = crate::daemon_link::ConnectionState::Disconnected;
                    state.observations.clear();
                    for stream in state.run_streams.values_mut() {
                        stream.live = false;
                    }
                    cx.notify();
                });
                if let Some(state) = state_for_task.upgrade() {
                    let _ = this.update(cx, |app, cx| {
                        app.stream_subscribed.clear();
                        app.operation_polls.clear();
                        tracing::info!("daemon link dropped; reconnecting");
                        if reconnect {
                            Self::spawn_daemon_link_with_retries(
                                &state,
                                DAEMON_RECONNECT_ATTEMPTS,
                                cx,
                            );
                        }
                    });
                }
            });
        })
    }

    /// Flush queued notifications (called from render where Window is available).
    fn flush_notifications(&mut self, window: &mut Window, cx: &mut App) {
        use gpui_kit::component::WindowExt as _;
        for notif in self.pending_notifications.drain(..) {
            window.push_notification(notif, cx);
        }
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

    /// Open the durable task form from any project screen.
    fn open_new_task(&mut self, cx: &mut Context<Self>) {
        self.navigate(Screen::Fleet, cx);
        self.ensure_fleet(cx)
            .update(cx, |fleet, cx| fleet.start_new_task(cx));
    }

    fn ensure_fleet(&mut self, cx: &mut Context<Self>) -> Entity<FleetScreen> {
        let state = self.state.clone();
        self.fleet
            .get_or_insert_with(|| {
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
                        FleetAction::OpenResult(run, tab) => {
                            this.open_run_cockpit(Some(*run), cx);
                            this.pending_run_tab = Some(*tab);
                        },
                        FleetAction::NewTask => this.navigate(Screen::SpecWizard, cx),
                        FleetAction::Dispatch(operation_id, prompt) => {
                            this.dispatch_bootstrap(
                                prompt.clone(),
                                *operation_id,
                                DispatchOrigin::Fleet(_f.clone()),
                                cx,
                            );
                        },
                    }
                })
                .detach();
                f
            })
            .clone()
    }

    fn render_screen_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active_screen {
            Screen::Home => {
                if self.home.is_none() {
                    let state = self.state.clone();
                    let home = cx.new(|cx| ProjectHomeScreen::new(state, cx));
                    cx.subscribe(
                        &home,
                        |this: &mut Self, _, event: &HomeAction, cx| match event {
                            HomeAction::PlanApplication => {
                                this.wizard_return_screen = Screen::Home;
                                if this.spec_wizard.as_ref().is_some_and(|wizard| {
                                    Some(&wizard.read(cx).project_path)
                                        != this.state.read(cx).project_path.as_ref()
                                }) {
                                    this.spec_wizard = None;
                                }
                                this.navigate(Screen::SpecWizard, cx);
                            },
                            HomeAction::CreateTask => this.open_new_task(cx),
                            HomeAction::OpenRecorded { run, target } => {
                                this.open_run_cockpit(Some(*run), cx);
                                this.pending_recorded_selection = Some((*run, *target));
                            },
                            HomeAction::OpenTask(item) => {
                                this.navigate(Screen::Fleet, cx);
                                this.ensure_fleet(cx)
                                    .update(cx, |fleet, cx| fleet.open_saved_task(*item, cx));
                            },
                            HomeAction::OpenRun(run) => this.open_run_cockpit(Some(*run), cx),
                            HomeAction::OpenPlan => this.navigate(Screen::Roadmap, cx),
                            HomeAction::OpenDecision { run, request_seq } => {
                                this.navigate(Screen::Inbox, cx);
                                if let Some(inbox) = &this.inbox {
                                    inbox.update(cx, |inbox, cx| {
                                        inbox.select_decision(*run, *request_seq, cx)
                                    });
                                } else {
                                    this.pending_home_decision = Some((*run, *request_seq));
                                }
                            },
                        },
                    )
                    .detach();
                    self.home = Some(home);
                }
                self.home
                    .as_ref()
                    .map(|home| home.clone().into_any_element())
                    .unwrap_or_else(|| div().into_any_element())
            },
            Screen::Fleet => self.ensure_fleet(cx).into_any_element(),
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
                                    RunsEvent::BootstrapContinuationAccepted {
                                        operation,
                                        status,
                                        scope,
                                    } => {
                                        this.accept_bootstrap_continuation(
                                            *operation, status, scope, cx,
                                        );
                                    },
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
                if let Some((milestone, task)) = self.pending_task_selection.take() {
                    s.update(cx, |screen, cx| screen.select_task(milestone, task, cx));
                }
                if let Some((run, target)) = self.pending_recorded_selection.take() {
                    s.update(cx, |screen, cx| screen.select_recorded(run, target, cx));
                }
                if let Some(tab) = self.pending_run_tab.take() {
                    s.update(cx, |screen, cx| screen.select_tab(tab, cx));
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
                            RoadmapEvent::OpenTask {
                                run,
                                milestone,
                                task,
                            } => {
                                this.open_run_cockpit(Some(*run), cx);
                                this.pending_task_selection =
                                    Some((milestone.clone(), task.clone()));
                            },
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
                            InboxAction::OpenPlan {
                                run_id,
                                decision_seq,
                            } => {
                                this.navigate(Screen::Flow, cx);
                                let state = this.state.clone();
                                let flow = this.flow.get_or_insert_with(|| {
                                    cx.new(|cx| FlowScreen::with_state(state, cx))
                                });
                                flow.update(cx, |flow, cx| {
                                    flow.select_plan(*run_id, Some(*decision_seq), cx)
                                });
                            },
                        },
                    )
                    .detach();
                    i
                });
                if let Some((run, seq)) = self.pending_home_decision.take() {
                    inbox.update(cx, |inbox, cx| inbox.select_decision(run, seq, cx));
                }
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
            Screen::Settings => {
                let state = self.state.clone();
                let s = self
                    .settings
                    .get_or_insert_with(|| cx.new(|cx| SettingsScreen::new(state, cx)));
                s.clone().into_any_element()
            },
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
                    .font_family(crate::ui::BODY)
                    .child(welcome.clone())
                    .into_any_element(),
                AppMode::Project { .. } => {
                    div()
                        .key_context("SurgeApp")
                        .track_focus(&self.focus)
                        .size_full()
                        .font_family(crate::ui::BODY)
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
                        .on_action(cx.listener(|this, _: &NewTask, _w, cx| this.open_new_task(cx)))
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
                        .into_any_element()
                },
            };

        div()
            .size_full()
            .v_flex()
            .font_family(crate::ui::BODY)
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

/// Backoff before history-restore attempt `attempt + 1` (#121): 0.5 s,
/// doubling, capped at 30 s.
fn history_retry_delay(attempt: u32) -> std::time::Duration {
    const FIRST_MS: u64 = 500;
    const CAP_MS: u64 = 30_000;
    let factor = 1_u64.checked_shl(attempt.min(16)).unwrap_or(u64::MAX);
    std::time::Duration::from_millis(FIRST_MS.saturating_mul(factor).min(CAP_MS))
}

/// Replay an attributed durable prefix without inferring that an attached feed is current.
/// Read failure retains recorded history and keeps current observation unconfirmed.
async fn hydrate_cold_summary(
    app: &WeakEntity<SurgeApp>,
    state: &WeakEntity<AppState>,
    summary: surge_persistence::runs::RunSummary,
    cx: &mut AsyncApp,
    before_publish: impl std::future::Future<Output = ()>,
) {
    let token = cx.update(|cx| {
        state
            .update(cx, |state, _| {
                let scope = state.observation_scope()?;
                let owned = summary.project_path == scope.project
                    || scope.repository.as_ref().is_some_and(|repository| {
                        crate::project::git_common_dir(&summary.project_path).as_ref()
                            == Some(repository)
                    });
                if !owned {
                    return None;
                }
                let owner = state.observations.claim(&scope, summary.id, false)?;
                if !state.runs.iter().any(|run| run.run_id == summary.id)
                    && let Some(started_at) =
                        chrono::DateTime::from_timestamp_millis(summary.started_at_ms)
                {
                    state.runs.push(crate::app_state::UiRun {
                        run_id: summary.id,
                        status: surge_orchestrator::engine::handle::RunStatus::Active,
                        started_at,
                        ended_at: None,
                        last_event_seq: None,
                    });
                }
                state.run_streams.entry(summary.id).or_insert_with(|| {
                    crate::run_stream::RunStreamState::registry_hint(
                        summary.id,
                        summary.status,
                        summary.project_path.clone(),
                    )
                });
                state.begin_history_read(&owner)
            })
            .ok()
            .flatten()
    });
    if let Some(token) = token {
        let result = observation::read_history(&token.scope.home, token.run).await;
        before_publish.await;
        let completion = observation::publish_read(state, &token, result, cx);
        let _ = app.update(cx, |app, cx| {
            app.settle_current_reads(std::slice::from_ref(&completion), cx);
            app.drain_pending_operations(cx);
        });
    }
}

async fn hydrate_run_history(
    state: &WeakEntity<AppState>,
    token: &crate::run_observation::Token,
    cx: &mut AsyncApp,
) -> crate::app_state::ReadCompletion {
    hydrate_run_history_observed(state, token, cx, std::future::ready(())).await
}

async fn hydrate_run_history_observed(
    state: &WeakEntity<AppState>,
    token: &crate::run_observation::Token,
    cx: &mut AsyncApp,
    before_publish: impl std::future::Future<Output = ()>,
) -> crate::app_state::ReadCompletion {
    let read = cx.update(|cx| {
        state
            .update(cx, |state, _| {
                if !state.accepts_observation(token, false) {
                    return None;
                }
                state.begin_history_read(token)
            })
            .ok()
            .flatten()
    });
    let Some(read) = read else {
        return crate::app_state::ReadCompletion::obsolete(token);
    };
    let result = observation::read_history(&read.scope.home, read.run).await;
    before_publish.await;
    observation::publish_read(state, &read, result, cx)
}

#[cfg(test)]
mod history_retry_tests {
    use super::history_retry_delay;
    use std::time::Duration;

    #[test]
    fn history_retries_back_off_to_a_cap() {
        assert_eq!(history_retry_delay(0), Duration::from_millis(500));
        assert_eq!(history_retry_delay(1), Duration::from_secs(1));
        assert_eq!(history_retry_delay(3), Duration::from_secs(4));
        assert_eq!(history_retry_delay(6), Duration::from_secs(30));
        assert_eq!(history_retry_delay(u32::MAX), Duration::from_secs(30));
    }
}
