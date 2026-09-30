//! Flow — how the work is done, level by level.
//!
//! Surge's work nests (see `docs/workflow.md`): the whole project's steps,
//! a loop over milestones whose body is the milestone's own flow, and a loop
//! over tasks whose body is the task's own flow. This screen shows the open
//! project's plan split into those levels ([`crate::flow_levels`]), why the
//! planner chose that shape, and the library of flow shapes Surge ships.
//!
//! The plan comes from the newest planning run of this project: live from
//! its stream while it waits for your approval (steps are editable then),
//! otherwise from its stored `flow` artifact — never a sample template
//! presented as the project's plan.

use std::{collections::HashMap, path::PathBuf, time::SystemTime};

use gpui_kit::base::Selectable;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::graph::Graph;
use surge_core::{BundledFlow, BundledFlows, NodeKind};

use crate::flow_diagram::PreparedPlan;
use crate::flow_levels::{FlowLevel, levels};
use crate::theme::{self, Semantic};
use crate::ui;

/// Node-kind color in the app's semantic vocabulary.
pub(crate) fn kind_color(k: NodeKind) -> Hsla {
    match k {
        NodeKind::Agent => Semantic::Agent.color(),
        NodeKind::HumanGate => Semantic::You.color(),
        NodeKind::Branch | NodeKind::Subgraph => Semantic::Plan.color(),
        NodeKind::Terminal => Semantic::Verified.color(),
        NodeKind::Loop => Semantic::Loop.color(),
        _ => Semantic::External.color(),
    }
}

/// When to reach for a bundled shape (from `docs/archetypes.md`).
fn archetype_blurb(name: &str) -> (&'static str, &'static str) {
    match name {
        "single-task" => ("Size", "A tiny, obvious change — one agent, then a check."),
        "linear-3" => ("Size", "One coherent change: spec → build → check."),
        "linear-with-review" => (
            "Size",
            "One milestone of work, with a review before it ends.",
        ),
        "multi-milestone" => (
            "Size",
            "Several milestones: a loop over milestones, each looping over its tasks.",
        ),
        "feature" => (
            "Kind",
            "New behaviour: acceptance criteria first, then build and check.",
        ),
        "bug-fix" => (
            "Kind",
            "A reproducible failure: reproduce it first, then fix and prove it.",
        ),
        "refactor" => (
            "Kind",
            "No behaviour change: pin current behaviour, then refactor under review.",
        ),
        "migration" => (
            "Kind",
            "Schema, data or API change: plan and rollback before the change.",
        ),
        "performance" => (
            "Kind",
            "A number to beat: baseline first, then compare against it.",
        ),
        "security" => (
            "Kind",
            "Vulnerabilities or secrets: audit, then fix each finding with a test.",
        ),
        "docs" => (
            "Kind",
            "Documentation only: outline, write, check against the code.",
        ),
        "code-review" => (
            "Kind",
            "An existing change to judge: review it, audit security, then verify independently.",
        ),
        "spike" => (
            "Kind",
            "Unknown feasibility: a bounded experiment that answers a question.",
        ),
        "bootstrap" => (
            "Planning",
            "Turns your idea into a description, a roadmap and a flow — each approved by you.",
        ),
        _ => ("Other", "A bundled flow."),
    }
}

/// A plan split into levels, parsed and laid out once.
#[derive(Clone)]
struct LevelPlans {
    levels: Vec<(FlowLevel, PreparedPlan)>,
    archetype: Option<String>,
    rationale: Option<String>,
}

impl LevelPlans {
    fn ancestry(&self, index: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = Some(index);
        while let Some(index) = current {
            let Some((level, _)) = self.levels.get(index) else {
                break;
            };
            path.push(index);
            current = level.parent;
        }
        path.reverse();
        path
    }

    fn new(graph: &Graph) -> Self {
        Self {
            levels: levels(graph)
                .into_iter()
                .map(|level| {
                    let prepared = PreparedPlan::new(&level.graph);
                    (level, prepared)
                })
                .collect(),
            archetype: graph
                .metadata
                .archetype
                .as_ref()
                .map(|a| a.name.as_str().to_string()),
            // The shape is shown as a badge; this is the planner's reason.
            rationale: graph
                .metadata
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string),
        }
    }
}

/// The project's plan.
#[derive(Clone)]
struct ProjectPlan {
    run: surge_core::RunId,
    headline: String,
    plans: Result<LevelPlans, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Project,
    Library,
}

#[derive(Clone, PartialEq, Eq)]
struct PlanTarget {
    project: Option<PathBuf>,
    run: surge_core::RunId,
    decision_seq: Option<u64>,
}

impl PlanTarget {
    fn editable(&self, state: &crate::app_state::AppState) -> bool {
        self.project == state.project_path
            && state.run_in_project(&self.run)
            && state.pending_decisions().iter().any(|(run, decision)| {
                *run == self.run
                    && decision.node == "flow_gate"
                    && self.decision_seq.is_none_or(|seq| seq == decision.seq)
            })
    }
}

#[derive(PartialEq, Eq)]
struct LivePlanKey {
    project: Option<PathBuf>,
    run: surge_core::RunId,
    path: PathBuf,
    sequence: u64,
    length: u64,
    modified: Option<SystemTime>,
}

type StoredPlanKey = (Option<PathBuf>, usize, Option<PlanTarget>);

#[cfg(test)]
type ControlledStoredPlans =
    std::rc::Rc<std::cell::RefCell<Vec<tokio::sync::oneshot::Sender<Option<ProjectPlan>>>>>;

/// Flow screen.
pub struct FlowScreen {
    /// Shared state; `None` only in isolated render tests.
    state: Option<Entity<crate::app_state::AppState>>,
    mode: Mode,
    /// Live plan keyed by project, run, artifact path/revision and content hash.
    live: Option<(LivePlanKey, surge_core::ContentHash, ProjectPlan)>,
    requested: Option<PlanTarget>,
    /// Stored plan from the project's newest planning run.
    stored: Option<ProjectPlan>,
    stored_for: Option<StoredPlanKey>,
    stored_generation: u64,
    #[cfg(test)]
    controlled_stored_plans: Option<ControlledStoredPlans>,
    level: usize,
    /// Step selected in the diagram (inspector subject).
    plan_selected: Option<String>,
    /// Run whose plan is shown (the planning run while its gate waits).
    plan_run: Option<surge_core::RunId>,
    selection_run: Option<surge_core::RunId>,
    /// Why an installed provider cannot run a step right now (not
    /// configured / quota exhausted), loaded once in the background.
    provider_notes: Option<std::collections::HashMap<String, String>>,
    provider_notes_loading: bool,
    /// Model field of the step editor, and the step it currently edits.
    model_input: Option<Entity<InputState>>,
    model_input_for: Option<(PlanTarget, String)>,
    /// Profile defaults for the template inspector (loaded once, including errors).
    profiles:
        Option<Result<std::rc::Rc<surge_orchestrator::profile_loader::ProfileRegistry>, String>>,
    library: Vec<BundledFlow>,
    library_selected: usize,
    library_cache: HashMap<usize, LevelPlans>,
}

impl FlowScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut library = BundledFlows::all();
        // Size shapes in order of scale, then kinds, then planning.
        const SIZE_ORDER: [&str; 4] = [
            "single-task",
            "linear-3",
            "linear-with-review",
            "multi-milestone",
        ];
        library.sort_by_key(|f| {
            let (group, _) = archetype_blurb(&f.name);
            let rank = match group {
                "Size" => 0,
                "Kind" => 1,
                "Planning" => 2,
                _ => 3,
            };
            let scale = SIZE_ORDER.iter().position(|n| *n == f.name).unwrap_or(0);
            (rank, scale, f.name.clone())
        });
        Self {
            state: None,
            mode: Mode::Project,
            live: None,
            requested: None,
            stored: None,
            stored_for: None,
            stored_generation: 0,
            #[cfg(test)]
            controlled_stored_plans: None,
            level: 0,
            plan_selected: None,
            plan_run: None,
            selection_run: None,
            provider_notes: None,
            provider_notes_loading: false,
            model_input: None,
            model_input_for: None,
            profiles: None,
            library,
            library_selected: 0,
            library_cache: HashMap::new(),
        }
    }

    /// Flow screen bound to the open project: shows its current plan.
    pub fn with_state(state: Entity<crate::app_state::AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this: &mut Self, _state, cx| {
            this.reload_stored_if_changed(cx);
            cx.notify();
        })
        .detach();
        let mut this = Self {
            state: Some(state),
            ..Self::new(cx)
        };
        this.reload_stored_if_changed(cx);
        this
    }

    #[cfg(test)]
    pub(crate) fn displayed_plan_run(&self) -> Option<surge_core::RunId> {
        self.plan_run
    }

    pub(crate) fn select_plan(
        &mut self,
        run: surge_core::RunId,
        decision_seq: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let target = PlanTarget {
            project: self
                .state
                .as_ref()
                .and_then(|state| state.read(cx).project_path.clone()),
            run,
            decision_seq,
        };
        if self.requested.as_ref() != Some(&target) {
            self.reset_plan_selection();
            self.live = None;
            self.stored = None;
            self.requested = Some(target);
            self.reload_stored_if_changed(cx);
        }
        self.mode = Mode::Project;
        cx.notify();
    }

    pub(crate) fn show_project_plan(&mut self, cx: &mut Context<Self>) {
        if self.requested.take().is_some() {
            self.reset_plan_selection();
            self.live = None;
            self.stored = None;
            self.reload_stored_if_changed(cx);
        }
        cx.notify();
    }

    fn reset_plan_selection(&mut self) {
        self.level = 0;
        self.plan_selected = None;
        self.plan_run = None;
        self.selection_run = None;
        self.model_input = None;
        self.model_input_for = None;
    }

    fn unavailable(run: surge_core::RunId, message: impl Into<String>) -> ProjectPlan {
        ProjectPlan {
            run,
            headline: format!("Requested plan · {}", run.short()),
            plans: Err(message.into()),
        }
    }

    /// The exact requested run, or the project's newest live plan in ordinary navigation.
    fn live_plan(&mut self, cx: &Context<Self>) -> Option<ProjectPlan> {
        let state = self.state.as_ref()?.read(cx);
        if let Some(target) = &self.requested
            && (target.project != state.project_path
                || !state.run_in_project(&target.run)
                || target.decision_seq.is_some() && !target.editable(state))
        {
            return Some(Self::unavailable(
                target.run,
                "The requested plan decision is no longer available in this project.",
            ));
        }
        let runs = self.requested.as_ref().map_or_else(
            || {
                state
                    .project_runs()
                    .iter()
                    .map(|run| run.run_id)
                    .collect::<Vec<_>>()
            },
            |target| vec![target.run],
        );
        let (run, headline, path, sequence) = runs.into_iter().find_map(|run| {
            let stream = state.run_streams.get(&run)?;
            let recorded = stream.artifacts.get("flow")?;
            let path = if recorded.is_absolute() {
                recorded.clone()
            } else {
                stream.run_path.as_ref()?.join(recorded)
            };
            if self.requested.is_none() && !path.is_file() {
                return None;
            }
            let headline = state.run_prompt(&run).map_or_else(
                || format!("run r-{}", run.short().to_lowercase()),
                |prompt| ui::headline(prompt, 90),
            );
            Some((run, headline, path, stream.last_seq))
        })?;
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                return Some(Self::unavailable(
                    run,
                    "The recorded plan artifact is unavailable. Refresh the run's artifact before opening it.",
                ));
            },
        };
        let key = LivePlanKey {
            project: state.project_path.clone(),
            run,
            path: path.clone(),
            sequence,
            length: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if self
            .live
            .as_ref()
            .is_some_and(|(cached, _, _)| cached == &key)
        {
            return self.live.as_ref().map(|(_, _, plan)| plan.clone());
        }
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(_) => {
                return Some(Self::unavailable(
                    run,
                    "The recorded plan artifact could not be read.",
                ));
            },
        };
        let fingerprint = surge_core::ContentHash::compute(source.as_bytes());
        let plans = if let Some((cached, old_hash, plan)) = &self.live
            && cached.project == key.project
            && cached.run == run
            && *old_hash == fingerprint
        {
            plan.plans.clone()
        } else {
            self.reset_plan_selection();
            toml::from_str::<Graph>(&source)
                .map(|graph| LevelPlans::new(&graph))
                .map_err(|_| "The requested plan artifact is not a valid workflow.".into())
        };
        self.live = Some((
            key,
            fingerprint,
            ProjectPlan {
                run,
                headline,
                plans,
            },
        ));
        self.live.as_ref().map(|(_, _, plan)| plan.clone())
    }

    /// Reload the stored plan when the project or its operations changed.
    fn reload_stored_if_changed(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let (root, ops) = {
            let state = state.read(cx);
            (state.project_path.clone(), state.bootstrap_operations.len())
        };
        let requested = self.requested.clone();
        let key = (root.clone(), ops, requested.clone());
        if self.stored_for.as_ref() == Some(&key) {
            return;
        }
        self.stored = None;
        self.stored_for = Some(key.clone());
        self.stored_generation = self.stored_generation.saturating_add(1);
        let generation = self.stored_generation;
        let Some(root) = root else {
            self.stored = None;
            return;
        };
        #[cfg(test)]
        let controlled = self.controlled_stored_plans.as_ref().map(|requests| {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            requests.borrow_mut().push(sender);
            receiver
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let load = async move {
                if tokio::runtime::Handle::try_current().is_ok() {
                    match surge_core::home::surge_home_dir() {
                        Some(home) => {
                            let mut found = None;
                            for op in crate::roadmap_source::operations(&root, &home).await {
                                if requested
                                    .as_ref()
                                    .is_some_and(|target| target.run != op.planning_run)
                                {
                                    continue;
                                }
                                let Some(bytes) = crate::roadmap_source::read_run_artifact(
                                    &home,
                                    op.planning_run,
                                    "flow",
                                ) else {
                                    continue;
                                };
                                let plans = String::from_utf8(bytes)
                                    .map_err(|e| e.to_string())
                                    .and_then(|text| {
                                        toml::from_str::<Graph>(&text).map_err(|e| e.to_string())
                                    })
                                    .map(|graph| LevelPlans::new(&graph));
                                found = Some(ProjectPlan {
                                    run: op.planning_run,
                                    headline: ui::headline(&op.prompt, 90),
                                    plans,
                                });
                                break;
                            }
                            found
                        },
                        None => None,
                    }
                } else {
                    None
                }
            };
            #[cfg(test)]
            let found = if let Some(receiver) = controlled {
                receiver.await.ok().flatten()
            } else {
                load.await
            };
            #[cfg(not(test))]
            let found = load.await;

            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    if screen.stored_for.as_ref() != Some(&key)
                        || screen.stored_generation != generation
                    {
                        return;
                    }
                    screen.stored = found;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Node → Agent → Session panel for the selected plan step.
    fn render_node_inspector(
        &mut self,
        key: &str,
        details: &crate::flow_diagram::NodeDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let profiles = self
            .profiles
            .get_or_insert_with(|| {
                surge_orchestrator::profile_loader::ProfileRegistry::load()
                    .map(std::rc::Rc::new)
                    .map_err(|error| error.to_string())
            })
            .clone();
        let resolution = details.profile.as_deref().map(|profile| {
            let registry = profiles.as_ref().map_err(Clone::clone)?;
            let key = surge_core::profile::keyref::parse_key_ref(profile)
                .map_err(|error| error.to_string())?;
            registry.resolve(&key).map_err(|error| error.to_string())
        });
        let resolved = resolution.as_ref().and_then(|result| result.as_ref().ok());
        let provider_editor = self.render_provider_editor(key, details, window, cx);
        let state = self.state.as_ref().map(|s| s.read(cx));

        let row = |label: &str, value: String| {
            div()
                .flex()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(78.0))
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .text_color(theme::text_muted())
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.5))
                        .text_color(theme::text_primary())
                        .child(value),
                )
        };
        let section = |title: &str| {
            div()
                .pt(px(12.0))
                .pb(px(4.0))
                .text_size(px(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_muted())
                .child(title.to_uppercase())
        };

        let mut panel = div()
            .w(px(340.0))
            .flex_shrink_0()
            .v_flex()
            .gap(px(5.0))
            .p(px(14.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(theme::hairline_strong())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(
                        div()
                            .w(px(28.0))
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(details.color.opacity(0.16))
                            .text_color(details.color)
                            .child(details.icon.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(details.title.clone()),
                            )
                            .child(ui::meta(format!("{} · {key}", details.kind))),
                    ),
            );

        if let Some(resolved) = resolved {
            let profile = &resolved.profile;
            panel = panel.child(
                div()
                    .pt(px(6.0))
                    .text_size(px(11.5))
                    .text_color(theme::text_primary().opacity(0.85))
                    .child(profile.role.description.clone()),
            );
            let agent_id = details
                .runtime_override
                .clone()
                .unwrap_or_else(|| profile.runtime.agent_id.clone());
            let provider = state
                .as_ref()
                .and_then(|s| s.registry.find_normalized(&agent_id))
                .map_or_else(
                    || agent_id.clone(),
                    |entry| format!("{} ({})", entry.display_name, entry.id),
                );
            let provider = if details.runtime_override.is_some() {
                format!("{provider} · overridden on this step")
            } else {
                provider
            };
            let model = if let Some(model) = &details.model_override {
                format!("{model} · set on this step")
            } else if profile.runtime.recommended_model.trim().is_empty() {
                "provider default".to_string()
            } else {
                profile.runtime.recommended_model.clone()
            };
            let mut skills = profile.tools.default_skills.clone();
            skills.extend(details.node_skills.iter().cloned());
            let prompt: String = profile.prompt.system.chars().take(420).collect();
            panel = panel
                .child(section("Profile defaults and step overrides"))
                .child(row("Profile", details.profile.clone().unwrap_or_default()))
                .child(row("Role", profile.role.display_name.clone()))
                .child(row("Provider", provider))
                .child(row("Model", model))
                .children(
                    details
                        .effort_override
                        .clone()
                        .map(|effort| row("Reasoning", format!("{effort} · set on this step"))),
                )
                .child(row(
                    "Sandbox",
                    format!("{:?}", profile.sandbox.mode).to_lowercase(),
                ))
                .child(row(
                    "Skills",
                    if skills.is_empty() {
                        "—".into()
                    } else {
                        skills.join(", ")
                    },
                ))
                .child(row(
                    "MCP",
                    if profile.tools.default_mcp.is_empty() {
                        "—".into()
                    } else {
                        profile.tools.default_mcp.join(", ")
                    },
                ))
                .child(row(
                    "Overrides",
                    if details.overrides.is_empty() {
                        "none (profile defaults)".into()
                    } else {
                        details.overrides.join(", ")
                    },
                ))
                .children(provider_editor)
                .child(section("System template preview"))
                .child(
                    div()
                        .p(px(8.0))
                        .rounded_md()
                        .bg(theme::panel_deep())
                        .text_size(px(10.5))
                        .text_color(theme::text_muted())
                        .child(format!("{prompt}…")),
                );
        }

        if !details.routes.is_empty() {
            panel = panel.child(section("Next"));
            for (outcome, to) in &details.routes {
                panel = panel.child(row(outcome, format!("→ {to}")));
            }
        }

        if let Some(Err(error)) = resolution {
            panel = panel.child(section("Profile unavailable")).child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme::error())
                    .child(error),
            );
        }
        panel = panel.child(section("Plan template")).child(ui::meta(
            "Profile defaults and step overrides. No run or loop instance is selected.",
        ));
        panel
    }

    /// Step settings (provider, reasoning level, model) for a plan that is
    /// still awaiting approval. Stored as unapproved edits and sent with the
    /// approval; the engine applies them to that step only.
    fn render_provider_editor(
        &mut self,
        key: &str,
        details: &crate::flow_diagram::NodeDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        details.profile.as_ref()?;
        let plan_run = self.plan_run?;
        let state_entity = self.state.clone()?;
        let (target, current, agents) = {
            let state = state_entity.read(cx);
            let target = self.requested.clone().unwrap_or_else(|| PlanTarget {
                project: state.project_path.clone(),
                run: plan_run,
                decision_seq: state
                    .pending_decisions()
                    .iter()
                    .find(|(run, decision)| *run == plan_run && decision.node == "flow_gate")
                    .map(|(_, decision)| decision.seq),
            });
            let current = state
                .plan_edits
                .get(&plan_run)
                .and_then(|edits| edits.0.get(key))
                .cloned()
                .unwrap_or_default();
            let agents: Vec<(String, String)> = state
                .installed_agents
                .iter()
                .map(|a| (a.entry.id.clone(), a.entry.display_name.clone()))
                .collect();
            (target, current, agents)
        };
        if !target.editable(state_entity.read(cx)) {
            return None;
        }
        self.load_provider_notes(cx);
        let notes = self.provider_notes.clone().unwrap_or_default();

        // A fresh entity per step rejects queued input events from an old inspector.
        let input_owner = (target.clone(), key.to_string());
        if self.model_input_for.as_ref() != Some(&input_owner) {
            self.model_input = None;
        }
        if self.model_input.is_none() {
            let owner = target.clone();
            let node = key.to_string();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Model, e.g. sonnet or haiku (agent default) — Enter to apply")
            });
            cx.subscribe_in(
                &input,
                window,
                move |this: &mut Self, input, event: &InputEvent, _window, cx| {
                    if !matches!(event, InputEvent::PressEnter { .. })
                        || this.model_input.as_ref() != Some(input)
                        || this.plan_run != Some(owner.run)
                        || this.plan_selected.as_deref() != Some(node.as_str())
                    {
                        return;
                    }
                    let Some(state) = this.state.clone() else {
                        return;
                    };
                    let value = input.read(cx).value().trim().to_string();
                    state.update(cx, |state, cx| {
                        edit_step(state, &owner, &node, |edit| {
                            edit.model = (!value.is_empty()).then_some(value);
                        });
                        cx.notify();
                    });
                },
            )
            .detach();
            self.model_input = Some(input);
        }
        if self.model_input_for.as_ref() != Some(&input_owner) {
            let text = current.model.clone().unwrap_or_default();
            if let Some(input) = &self.model_input {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
            self.model_input_for = Some(input_owner);
        }

        let chip = |id: String, label: String, selected: bool| {
            let selector = id.clone();
            div()
                .id(SharedString::from(id))
                .test_support()
                .debug_selector(move || selector.clone())
                .role(Role::Button)
                .aria_label(label.clone())
                .px(px(8.0))
                .py(px(3.0))
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .text_size(px(10.5))
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::hairline_strong()
                })
                .text_color(if selected {
                    theme::accent()
                } else {
                    theme::text_primary()
                })
                .child(label)
        };

        let mut providers = div().flex().flex_wrap().gap(px(6.0));
        let provider_options = std::iter::once((None, "Profile default".to_string()))
            .chain(agents.into_iter().map(|(id, name)| (Some(id), name)));
        let mut unavailable = Vec::new();
        for (index, (agent_id, label)) in provider_options.enumerate() {
            let selected = agent_id == current.agent_id;
            let note = agent_id.as_ref().and_then(|id| notes.get(id));
            if let Some(note) = note {
                unavailable.push(format!("{label}: {note}"));
            }
            let label = if note.is_some() {
                format!("{label} ⚠")
            } else {
                label
            };
            let state = state_entity.clone();
            let node = key.to_string();
            let owner = target.clone();
            providers = providers.child(
                chip(format!("provider-{key}-{index}"), label, selected).on_click(cx.listener(
                    move |this, _e, _w, cx| {
                        if this.plan_run != Some(owner.run)
                            || this.plan_selected.as_deref() != Some(node.as_str())
                        {
                            return;
                        }
                        let agent_id = agent_id.clone();
                        state.update(cx, |state, cx| {
                            edit_step(state, &owner, &node, |edit| edit.agent_id = agent_id);
                            cx.notify();
                        });
                    },
                )),
            );
        }

        let mut efforts = div().flex().flex_wrap().gap(px(6.0));
        for (index, level) in [None, Some("low"), Some("medium"), Some("high")]
            .into_iter()
            .enumerate()
        {
            let selected = current.effort.as_deref() == level;
            let state = state_entity.clone();
            let node = key.to_string();
            let owner = target.clone();
            let label = level.unwrap_or("Default").to_string();
            efforts = efforts.child(
                chip(format!("effort-{key}-{index}"), label, selected).on_click(cx.listener(
                    move |this, _e, _w, cx| {
                        if this.plan_run != Some(owner.run)
                            || this.plan_selected.as_deref() != Some(node.as_str())
                        {
                            return;
                        }
                        state.update(cx, |state, cx| {
                            edit_step(state, &owner, &node, |edit| {
                                edit.effort = level.map(str::to_string);
                            });
                            cx.notify();
                        });
                    },
                )),
            );
        }

        let model_field = self.model_input.as_ref().map(|input| {
            div()
                .h(px(30.0))
                .px(px(8.0))
                .rounded_md()
                .bg(theme::panel_deep())
                .border_1()
                .border_color(theme::hairline_strong())
                .child(
                    Input::new(input)
                        .appearance(false)
                        .accessibility_id("step-model")
                        .aria_label("Model for this step"),
                )
        });

        Some(
            div()
                .v_flex()
                .gap(px(6.0))
                .pt(px(8.0))
                .child(ui::meta(
                    "Edit this step — applied when you approve the plan",
                ))
                .child(ui::meta("Provider"))
                .child(providers)
                .children(
                    (!unavailable.is_empty())
                        .then(|| ui::meta(format!("Unavailable now — {}", unavailable.join("; ")))),
                )
                .child(ui::meta(
                    "Reasoning level — only for agents that offer it (the step will not start otherwise)",
                ))
                .child(efforts)
                .child(ui::meta("Model"))
                .children(model_field),
        )
    }

    /// Load provider availability once: required environment present
    /// (the same check the engine does at launch) and no exhausted quota
    /// window in the capacity ledger.
    fn load_provider_notes(&mut self, cx: &mut Context<Self>) {
        if self.provider_notes.is_some() || self.provider_notes_loading {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let agents: Vec<(
            String,
            std::collections::BTreeMap<String, surge_core::config::AgentEnvValue>,
        )> = state
            .read(cx)
            .installed_agents
            .iter()
            .map(|a| (a.entry.id.clone(), a.entry.env.clone()))
            .collect();
        self.provider_notes_loading = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut notes = std::collections::HashMap::new();
            let storage = match surge_core::home::surge_home_dir() {
                Some(home) => surge_persistence::runs::Storage::open(home).await.ok(),
                None => None,
            };
            for (id, env) in agents {
                if let Err(error) = surge_acp::agent_env::resolve(&id, &env) {
                    notes.insert(id, format!("not configured ({error})"));
                    continue;
                }
                if let Some(storage) = &storage
                    && let Ok(status) = storage.capacity_status(&id).await
                    && let Some(why) = surge_orchestrator::engine::capacity::exhausted_reason(
                        &status,
                        chrono::Utc::now(),
                    )
                {
                    notes.insert(id, why);
                }
            }
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.provider_notes = Some(notes);
                    screen.provider_notes_loading = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_mode_switch(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .h_flex()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(ui::R_CONTROL + 1.0))
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel_deep());
        for (mode, label) in [(Mode::Project, "This project"), (Mode::Library, "Library")] {
            let active = self.mode == mode;
            row = row.child(
                div()
                    .id(SharedString::from(format!("flow-mode-{label}")))
                    .role(Role::Tab)
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(ui::R_CONTROL))
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .when(active, |el| {
                        el.bg(theme::surface()).text_color(theme::text_primary())
                    })
                    .when(!active, |el| {
                        el.text_color(theme::text_muted())
                            .hover(|s: StyleRefinement| s.text_color(theme::text_primary()))
                    })
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.mode = mode;
                        this.level = 0;
                        this.plan_selected = None;
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        row
    }

    /// Ancestor breadcrumbs and direct children, scoped to this occurrence.
    fn render_levels_bar(&self, plans: &LevelPlans, cx: &mut Context<Self>) -> Div {
        let index = self.level.min(plans.levels.len().saturating_sub(1));
        let ancestors = plans.ancestry(index);
        let mut breadcrumb = div().h_flex().gap(px(8.0)).flex_wrap();
        for (position, i) in ancestors.iter().copied().enumerate() {
            if position > 0 {
                breadcrumb = breadcrumb.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.0))
                        .text_color(theme::text_dim()),
                );
            }
            breadcrumb = breadcrumb.child(self.level_button(plans, i, cx));
        }
        let mut children = div().h_flex().gap(px(8.0)).flex_wrap();
        for (i, (level, _)) in plans.levels.iter().enumerate() {
            if level.parent == Some(index) {
                children = children.child(self.level_button(plans, i, cx));
            }
        }
        div()
            .v_flex()
            .gap(px(8.0))
            .child(breadcrumb)
            .child(children)
    }

    fn level_button(
        &self,
        plans: &LevelPlans,
        i: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let level = &plans.levels[i].0;
        let label = format!(
            "{} · {} steps · {} approvals",
            level.title, level.work_steps, level.approvals
        );
        div()
            .id(("flow-level-region", i))
            .test_support()
            .debug_selector(move || format!("flow-level-{i}"))
            .child(
                gpui_kit::component::button::Button::new(SharedString::from(format!(
                    "flow-level-{i}"
                )))
                .accessibility_id(SharedString::from(format!("flow-level-{i}")))
                .label(label)
                .selected(i == self.level)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.level = i;
                    this.plan_selected = None;
                    cx.notify();
                })),
            )
    }

    /// Diagram of the selected level + the step inspector.
    fn render_level_diagram(
        &mut self,
        plans: &LevelPlans,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let index = self.level.min(plans.levels.len().saturating_sub(1));
        let Some((level, prepared)) = plans.levels.get(index) else {
            return div();
        };
        let inspector = if interactive {
            self.plan_selected
                .as_ref()
                .and_then(|key| prepared.nodes.get(key).map(|d| (key.clone(), d.clone())))
                .map(|(key, details)| self.render_node_inspector(&key, &details, window, cx))
        } else {
            None
        };
        let this = cx.entity().downgrade();
        // Clicking a loop opens the level it repeats; any other step opens
        // the inspector.
        let openers: Vec<(String, usize)> = plans
            .levels
            .iter()
            .enumerate()
            .filter(|(_, (level, _))| level.parent == Some(index))
            .filter_map(|(i, (l, _))| l.opened_by.clone().map(|k| (k, i)))
            .collect();
        let on_select: crate::flow_diagram::OnSelect = std::rc::Rc::new(move |key, _w, cx| {
            let _ = this.update(cx, |screen, cx| {
                if let Some((_, level)) = openers.iter().find(|(k, _)| *k == key) {
                    screen.level = *level;
                    screen.plan_selected = None;
                } else if interactive {
                    screen.plan_selected = Some(key);
                }
                cx.notify();
            });
        });
        div()
            .v_flex()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme::text_muted())
                    .child(match level.depth {
                    0 if plans.levels.len() > 1 => {
                        "Runs once for the whole request. Click a repeating step to open its level."
                            .to_string()
                    },
                    0 => "Runs once for the whole request.".to_string(),
                    _ => format!("Runs for {} — its own flow.", level.title.to_lowercase()),
                }),
            )
            .children(level.unopened.iter().map(|link| {
                div()
                    .text_size(px(11.5))
                    .text_color(theme::warning())
                    .child(format!(
                        "{} → {}: {}. Preview is incomplete.",
                        link.opened_by,
                        link.body,
                        link.reason.description()
                    ))
            }))
            .child(
                div()
                    .flex()
                    .gap(px(14.0))
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(crate::flow_diagram::render_interactive(
                                prepared,
                                SharedString::from(format!("flow-level-diagram-{index}")),
                                self.plan_selected.as_deref(),
                                Some(on_select),
                            )),
                    )
                    .children(inspector),
            )
    }

    fn render_why(&self, plans: &LevelPlans) -> Option<Div> {
        let why = plans.rationale.clone()?;
        Some(
            ui::node_card(Semantic::Plan.color())
                .v_flex()
                .gap(px(4.0))
                .px(px(14.0))
                .py(px(10.0))
                .child(ui::section_label("Why this shape"))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(theme::text_primary())
                        .child(why),
                ),
        )
    }

    fn render_project(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.plan_run = None;
        let plan = self.live_plan(cx).or_else(|| self.stored.clone()).or_else(|| self.requested.as_ref().map(|target| Self::unavailable(target.run, "The requested plan artifact is unavailable or still loading. No other run's plan is shown.")));
        let Some(plan) = plan else {
            return ui::panel()
                .child(ui::empty_state(
                    "◇",
                    "No plan for this project yet",
                    "When you describe an app, Surge drafts a flow for it — milestones and tasks each get their own steps — and asks you to approve it. Browse the shapes it can choose from in Library.",
                ))
                .into_any_element();
        };
        if plan.plans.is_ok() {
            if self.selection_run != Some(plan.run) {
                self.reset_plan_selection();
                self.selection_run = Some(plan.run);
            }
            self.plan_run = Some(plan.run);
        }
        match &plan.plans {
            Err(error) => ui::panel()
                .child(ui::empty_state(
                    "◌",
                    "This plan could not be read",
                    error.clone(),
                ))
                .into_any_element(),
            Ok(plans) => {
                let plans = plans.clone();
                div()
                    .v_flex()
                    .gap(px(14.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(ui::section_label("Planned for"))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::text_primary())
                                    .truncate()
                                    .child(plan.headline.clone()),
                            )
                            .when_some(plans.archetype.clone(), |el, a| {
                                el.child(ui::role_badge(a.replace('-', " "), Semantic::Plan))
                            }),
                    )
                    .children(self.render_why(&plans))
                    .child(self.render_levels_bar(&plans, cx))
                    .child(self.render_level_diagram(&plans, true, window, cx))
                    .into_any_element()
            },
        }
    }

    fn render_library(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected = self
            .library_selected
            .min(self.library.len().saturating_sub(1));
        let mut list = div().v_flex().gap(px(2.0)).w(px(300.0)).flex_none();
        let mut last_group = "";
        for (i, flow) in self.library.iter().enumerate() {
            let (group, blurb) = archetype_blurb(&flow.name);
            if group != last_group {
                last_group = group;
                list = list.child(
                    div()
                        .pt(px(if i == 0 { 0.0 } else { 10.0 }))
                        .pb(px(4.0))
                        .child(ui::section_label(group)),
                );
            }
            let active = i == selected;
            list = list.child(
                div()
                    .id(("flow-library", i))
                    .role(Role::Button)
                    .aria_label(flow.name.clone())
                    .v_flex()
                    .gap(px(2.0))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(ui::R_CONTROL))
                    .cursor_pointer()
                    .when(active, |el| el.bg(theme::surface()))
                    .hover(|s: StyleRefinement| s.bg(theme::surface()))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.library_selected = i;
                        this.level = 0;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if active {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .child(flow.name.replace('-', " ")),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(px(15.0))
                            .text_color(theme::text_dim())
                            .child(blurb),
                    ),
            );
        }
        let plans = match self.library.get(selected) {
            Some(flow) => self
                .library_cache
                .entry(selected)
                .or_insert_with(|| LevelPlans::new(&flow.graph))
                .clone(),
            None => return list.into_any_element(),
        };
        div()
            .flex()
            .gap(px(18.0))
            .items_start()
            .child(list)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .v_flex()
                    .gap(px(14.0))
                    .child(self.render_levels_bar(&plans, cx))
                    .child(self.render_level_diagram(&plans, false, window, cx)),
            )
            .into_any_element()
    }
}

fn edit_step(
    state: &mut crate::app_state::AppState,
    target: &PlanTarget,
    node: &str,
    change: impl FnOnce(&mut surge_core::node_overrides::NodeOverride),
) {
    if !target.editable(state) {
        return;
    }
    let edits = state.plan_edits.entry(target.run).or_default();
    let edit = edits.0.entry(node.to_string()).or_default();
    change(edit);
    if edit.agent_id.is_none() && edit.model.is_none() && edit.effort.is_none() {
        edits.0.remove(node);
    }
}

impl Render for FlowScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.mode {
            Mode::Project => self.render_project(window, cx),
            Mode::Library => self.render_library(window, cx),
        };
        div()
            .id("flow-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::background())
            .px(px(28.0))
            .pt(px(22.0))
            .pb(px(32.0))
            .child(
                div()
                    .v_flex()
                    .gap(px(18.0))
                    .child(ui::page_header(
                        "Flow",
                        Some(
                            "How the work is done — each level of the plan runs its own steps."
                                .into(),
                        ),
                        self.render_mode_switch(cx),
                    ))
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{FlowScreen, LevelPlans, Mode};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, SharedString, TestAppContext};

    #[test]
    fn same_path_plan_artifact_refreshes_on_new_stream_revision() {
        let mut cx = TestAppContext::single();
        let root = tempfile::tempdir().unwrap();
        let run = surge_core::RunId::new();
        let path = root.path().join("flow.toml");
        let source = include_str!("../../testdata/four_level_flow.toml").replace(
            "[metadata]",
            "[metadata]\ndescription = \"Original rationale\"",
        );
        std::fs::write(&path, &source).unwrap();
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: None,
                ended_at: None,
            });
            let mut stream = crate::run_stream::RunStreamState::default();
            stream.artifacts.insert("flow".into(), path.clone());
            state.run_streams.insert(run, stream);
            state
        });
        let screen = cx.new(|cx| FlowScreen::with_state(state.clone(), cx));
        screen.update(&mut cx, |screen, cx| {
            assert_eq!(
                screen
                    .live_plan(cx)
                    .unwrap()
                    .plans
                    .unwrap()
                    .rationale
                    .as_deref(),
                Some("Original rationale")
            )
        });
        std::fs::write(
            &path,
            source.replace("Original rationale", "Updated source rationale"),
        )
        .unwrap();
        state.update(&mut cx, |state, _| {
            state.run_streams.get_mut(&run).unwrap().last_seq = 2
        });
        screen.update(&mut cx, |screen, cx| {
            assert_eq!(
                screen
                    .live_plan(cx)
                    .unwrap()
                    .plans
                    .unwrap()
                    .rationale
                    .as_deref(),
                Some("Updated source rationale")
            )
        });
    }

    #[test]
    fn exact_target_refuses_missing_gate_artifact_and_other_project() {
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let root = tempfile::tempdir().unwrap();
        let older = surge_core::RunId::new();
        let newer = surge_core::RunId::new();
        let path = root.path().join("flow.toml");
        std::fs::write(&path, include_str!("../../testdata/four_level_flow.toml")).unwrap();
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            for (run, seq) in [(older, 1), (newer, 2)] {
                state.runs.push(crate::app_state::UiRun {
                    run_id: run,
                    status: surge_orchestrator::engine::handle::RunStatus::Active,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: None,
                });
                let mut stream = crate::run_stream::RunStreamState::default();
                stream.artifacts.insert("flow".into(), path.clone());
                stream.pending.push(crate::run_stream::PendingDecision {
                    seq,
                    time: String::new(),
                    node: "flow_gate".into(),
                    kind: crate::run_stream::DecisionKind::HumanInput {
                        call_id: None,
                        prompt: "Plan".into(),
                        schema: None,
                    },
                });
                state.run_streams.insert(run, stream);
            }
            state
        });
        let screen = cx.new(|cx| FlowScreen::with_state(state.clone(), cx));
        screen.update(&mut cx, |screen, cx| {
            screen.select_plan(older, Some(1), cx);
            assert_eq!(screen.live_plan(cx).unwrap().run, older);
            screen.level = 2;
            screen.plan_selected = Some("old-node".into());
            screen.select_plan(newer, Some(2), cx);
            assert_eq!(screen.level, 0);
            assert!(screen.plan_selected.is_none());
            assert!(screen.model_input.is_none());
            assert_eq!(screen.live_plan(cx).unwrap().run, newer);
            screen.select_plan(older, Some(1), cx);
        });
        state.update(&mut cx, |state, _| {
            state.run_streams.get_mut(&older).unwrap().pending.clear()
        });
        screen.update(&mut cx, |screen, cx| {
            let unavailable = screen.live_plan(cx).unwrap();
            assert_eq!(unavailable.run, older);
            assert!(unavailable.plans.is_err());
        });
        screen.update(&mut cx, |screen, cx| screen.select_plan(older, None, cx));
        std::fs::remove_file(&path).unwrap();
        screen.update(&mut cx, |screen, cx| {
            assert!(screen.live_plan(cx).unwrap().plans.is_err())
        });
        std::fs::write(&path, include_str!("../../testdata/four_level_flow.toml")).unwrap();
        state.update(&mut cx, |state, _| {
            state.project_path = Some(root.path().join("different-project"))
        });
        screen.update(&mut cx, |screen, cx| {
            assert!(screen.live_plan(cx).unwrap().plans.is_err())
        });
    }

    #[test]
    fn model_editor_rejects_detached_input_and_accepts_the_replacement_gate() {
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let root = tempfile::tempdir().unwrap();
        let run = surge_core::RunId::new();
        let path = root.path().join("flow.toml");
        std::fs::write(&path, include_str!("../../testdata/four_level_flow.toml")).unwrap();
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: None,
                ended_at: None,
            });
            let mut stream = crate::run_stream::RunStreamState::default();
            stream.artifacts.insert("flow".into(), path);
            stream.pending.push(crate::run_stream::PendingDecision {
                seq: 1,
                time: String::new(),
                node: "flow_gate".into(),
                kind: crate::run_stream::DecisionKind::HumanInput {
                    call_id: None,
                    prompt: "Plan".into(),
                    schema: None,
                },
            });
            state.run_streams.insert(run, stream);
            state
        });
        let screen = cx.new(|cx| FlowScreen::with_state(state.clone(), cx));
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        window.update(|window, cx| {
            screen.update(cx, |screen, cx| {
                screen.plan_selected = Some("final_spec".into());
                screen.provider_notes = Some(std::collections::HashMap::new());
                screen.profiles = Some(Ok(std::rc::Rc::new(
                    surge_orchestrator::profile_loader::ProfileRegistry::new(
                        surge_orchestrator::profile_loader::DiskProfileSet::empty(),
                    ),
                )));
                cx.notify();
            });
            window.render_frame(cx);
        });
        let old_input = window.update(|_, cx| screen.read(cx).model_input.clone().unwrap());
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.run_streams.get_mut(&run).unwrap().pending[0].seq = 2;
                cx.notify();
            })
        });
        window.run_until_parked();
        let current_input = window.update(|_, cx| screen.read(cx).model_input.clone().unwrap());
        assert_ne!(old_input, current_input);
        window.update(|window, cx| {
            old_input.update(cx, |input, cx| {
                input.set_value("stale-model", window, cx);
                cx.emit(gpui_kit::component::input::InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                });
            })
        });
        window.run_until_parked();
        window.update(|_, cx| assert!(!state.read(cx).plan_edits.contains_key(&run)));
        window.update(|window, cx| {
            current_input.update(cx, |input, cx| {
                input.set_value("current-model", window, cx);
                cx.emit(gpui_kit::component::input::InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                });
            })
        });
        window.run_until_parked();
        window.update(|_, cx| {
            assert_eq!(
                state.read(cx).plan_edits[&run].0["final_spec"]
                    .model
                    .as_deref(),
                Some("current-model")
            )
        });
    }

    #[test]
    fn changed_project_clears_the_previous_stored_plan_before_loading() {
        let mut cx = TestAppContext::single();
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.project_path = Some(std::path::PathBuf::from("/project-a"));
            state
        });
        let screen = cx.new(|cx| FlowScreen::with_state(state.clone(), cx));
        screen.update(&mut cx, |screen, _| {
            screen.stored = Some(super::FlowScreen::unavailable(
                surge_core::RunId::new(),
                "Old plan",
            ))
        });
        state.update(&mut cx, |state, _| {
            state.project_path = Some(std::path::PathBuf::from("/project-b"))
        });
        screen.update(&mut cx, |screen, cx| {
            let old_generation = screen.stored_generation;
            screen.reload_stored_if_changed(cx);
            assert!(screen.stored.is_none());
            assert!(screen.stored_generation > old_generation);
        });
    }

    #[test]
    fn delayed_stored_loader_rejects_a_b_a_stale_completions() {
        let mut cx = TestAppContext::single();
        let a = std::path::PathBuf::from("/project-a");
        let b = std::path::PathBuf::from("/project-b");
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.project_path = Some(a.clone());
            state
        });
        let requests = super::ControlledStoredPlans::default();
        let screen = cx.new(|cx| FlowScreen::with_state(state.clone(), cx));
        screen.update(&mut cx, |screen, cx| {
            screen.controlled_stored_plans = Some(requests.clone());
            screen.stored_for = None;
            screen.reload_stored_if_changed(cx);
        });
        for project in [&b, &a] {
            state.update(&mut cx, |state, _| {
                state.project_path = Some(project.clone())
            });
            screen.update(&mut cx, |screen, cx| screen.reload_stored_if_changed(cx));
        }
        assert_eq!(requests.borrow().len(), 3);
        let old_a = requests.borrow_mut().remove(0);
        let old_b = requests.borrow_mut().remove(0);
        let current_a = requests.borrow_mut().remove(0);
        assert!(
            old_a
                .send(Some(FlowScreen::unavailable(
                    surge_core::RunId::new(),
                    "Stale A"
                )))
                .is_ok()
        );
        assert!(
            old_b
                .send(Some(FlowScreen::unavailable(
                    surge_core::RunId::new(),
                    "Stale B"
                )))
                .is_ok()
        );
        cx.run_until_parked();
        screen.update(&mut cx, |screen, _| assert!(screen.stored.is_none()));
        let current_run = surge_core::RunId::new();
        assert!(
            current_a
                .send(Some(FlowScreen::unavailable(current_run, "Current A")))
                .is_ok()
        );
        cx.run_until_parked();
        screen.update(&mut cx, |screen, _| {
            assert_eq!(screen.stored.as_ref().unwrap().run, current_run)
        });
    }

    #[test]
    fn stored_fallback_run_change_resets_the_previous_inspector() {
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../testdata/four_level_flow.toml")).unwrap();
        let older = surge_core::RunId::new();
        let newer = surge_core::RunId::new();
        let screen = cx.new(|cx| {
            let mut screen = FlowScreen::new(cx);
            screen.stored = Some(super::ProjectPlan {
                run: older,
                headline: "Older".into(),
                plans: Ok(LevelPlans::new(&graph)),
            });
            screen
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        window.update(|_, cx| {
            screen.update(cx, |screen, cx| {
                screen.level = 2;
                screen.plan_selected = Some("old-node".into());
                screen.stored = Some(super::ProjectPlan {
                    run: newer,
                    headline: "Newer".into(),
                    plans: Ok(LevelPlans::new(&graph)),
                });
                cx.notify();
            })
        });
        window.run_until_parked();
        window.update(|_, cx| {
            let screen = screen.read(cx);
            assert_eq!(screen.plan_run, Some(newer));
            assert_eq!(screen.level, 0);
            assert!(screen.plan_selected.is_none());
            assert!(screen.model_input.is_none());
        });
    }

    #[gpui_kit::test]
    fn reused_occurrences_navigate_with_scoped_children_and_breadcrumbs(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init();
            crate::theme::sync_component_theme(cx);
        });
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../testdata/four_level_flow.toml")).unwrap();
        let mut second =
            graph.nodes[&surge_core::keys::NodeKey::try_from("milestone_loop").unwrap()].clone();
        second.id = surge_core::keys::NodeKey::try_from("second_milestone_loop").unwrap();
        graph.nodes.insert(second.id.clone(), second);
        let plans = LevelPlans::new(&graph);
        assert_eq!(plans.ancestry(6), [0, 2, 4, 6]);
        let screen = cx.new(|cx| {
            let mut screen = FlowScreen::new(cx);
            screen.mode = Mode::Library;
            screen.library_cache.insert(0, plans);
            screen
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        let second = window.debug_bounds("flow-level-2").unwrap();
        window.simulate_click(second.center(), gpui_kit::Modifiers::default());
        window.update(|window, cx| {
            assert_eq!(screen.read(cx).level, 2);
            window.render_frame(cx);
            window.click(SharedString::from("plan-node-task_loop"), cx);
        });
        window.update(|_, cx| assert_eq!(screen.read(cx).level, 4));
        assert!(window.debug_bounds("flow-level-2").is_some());
        assert!(
            window.debug_bounds("flow-level-1").is_none(),
            "unrelated sibling is not an ancestor"
        );
        let subtask = window.debug_bounds("flow-level-6").unwrap();
        window.simulate_click(subtask.center(), gpui_kit::Modifiers::default());
        window.update(|_, cx| assert_eq!(screen.read(cx).level, 6));
        let root = window.debug_bounds("flow-level-0").unwrap();
        window.simulate_click(root.center(), gpui_kit::Modifiers::default());
        window.update(|window, cx| {
            assert_eq!(screen.read(cx).level, 0);
            window.focus_next(cx);
            window.focus_next(cx);
            window.render_frame(cx);
            window.press("enter", cx);
        });
        window.update(|_, cx| assert_eq!(screen.read(cx).level, 1));
    }
}
