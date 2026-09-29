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

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
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

/// Flow screen.
pub struct FlowScreen {
    /// Shared state; `None` only in isolated render tests.
    state: Option<Entity<crate::app_state::AppState>>,
    mode: Mode,
    /// Live plan (from a stream) keyed by its artifact path.
    live: Option<(std::path::PathBuf, ProjectPlan)>,
    /// Stored plan from the project's newest planning run.
    stored: Option<ProjectPlan>,
    stored_for: Option<(Option<std::path::PathBuf>, usize)>,
    level: usize,
    /// Step selected in the diagram (inspector subject).
    plan_selected: Option<String>,
    /// Run whose plan is shown (the planning run while its gate waits).
    plan_run: Option<surge_core::RunId>,
    /// Why an installed provider cannot run a step right now (not
    /// configured / quota exhausted), loaded once in the background.
    provider_notes: Option<std::collections::HashMap<String, String>>,
    provider_notes_loading: bool,
    /// Model field of the step editor, and the step it currently edits.
    model_input: Option<Entity<InputState>>,
    model_input_for: Option<String>,
    /// Profile registry for the inspector's Agent section (loaded once).
    profiles: Option<std::rc::Rc<surge_orchestrator::profile_loader::ProfileRegistry>>,
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
            stored: None,
            stored_for: None,
            level: 0,
            plan_selected: None,
            plan_run: None,
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

    /// The plan a live planning stream is showing (while its gate waits).
    fn live_plan(&mut self, cx: &Context<Self>) -> Option<ProjectPlan> {
        let state = self.state.as_ref()?.read(cx);
        let (run, headline, path) = state.project_runs().into_iter().find_map(|run| {
            let stream = state.run_streams.get(&run.run_id)?;
            let recorded = stream.artifacts.get("flow")?;
            // Seeded artifacts (an implementation run inherits the approved
            // plan) are recorded relative to the run's worktree.
            let path = if recorded.is_absolute() {
                recorded.clone()
            } else {
                stream.run_path.as_ref()?.join(recorded)
            };
            if !path.is_file() {
                return None;
            }
            let headline = state.run_prompt(&run.run_id).map_or_else(
                || format!("run r-{}", run.run_id.short().to_lowercase()),
                |p| ui::headline(p, 90),
            );
            Some((run.run_id, headline, path))
        })?;
        if self.live.as_ref().is_none_or(|(cached, _)| cached != &path) {
            let source = std::fs::read_to_string(&path).ok()?;
            let plans = toml::from_str::<Graph>(&source)
                .map(|graph| LevelPlans::new(&graph))
                .map_err(|error| error.to_string());
            self.live = Some((
                path,
                ProjectPlan {
                    run,
                    headline,
                    plans,
                },
            ));
        }
        self.live.as_ref().map(|(_, plan)| plan.clone())
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
        let key = (root.clone(), ops);
        if self.stored_for.as_ref() == Some(&key) {
            return;
        }
        self.stored_for = Some(key);
        let Some(root) = root else {
            self.stored = None;
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = if tokio::runtime::Handle::try_current().is_ok() {
                match surge_core::home::surge_home_dir() {
                    Some(home) => {
                        let mut found = None;
                        for op in crate::roadmap_source::operations(&root, &home).await {
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
            };
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
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
                std::rc::Rc::new(
                    surge_orchestrator::profile_loader::ProfileRegistry::load().unwrap_or_else(
                        |_| {
                            surge_orchestrator::profile_loader::ProfileRegistry::new(
                                surge_orchestrator::profile_loader::DiskProfileSet::empty(),
                            )
                        },
                    ),
                )
            })
            .clone();
        let resolved = details.profile.as_deref().and_then(|profile| {
            let key = surge_core::profile::keyref::parse_key_ref(profile).ok()?;
            profiles.resolve(&key).ok()
        });
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

        if let Some(resolved) = &resolved {
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
                .child(section("Agent"))
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
                .child(section("Prompt"))
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

        // Session: the newest project run that has executed this step.
        // Only this plan's runs: the planning run and the implementation run
        // that follows it — never an older run with a same-named step.
        let plan_started = state.as_ref().and_then(|s| {
            let plan_run = self.plan_run?;
            s.runs
                .iter()
                .find(|r| r.run_id == plan_run)
                .map(|r| r.started_at)
        });
        let session = state.as_ref().and_then(|s| {
            s.project_runs()
                .into_iter()
                .filter(|run| plan_started.is_none_or(|start| run.started_at >= start))
                .find_map(|run| {
                    let stream = s.run_streams.get(&run.run_id)?;
                    let stage = stream.stages.iter().rev().find(|st| st.node == key)?;
                    Some((run.run_id, stage.clone()))
                })
        });
        panel = panel.child(section("Session"));
        panel = match session {
            Some((run_id, stage)) => panel
                .child(row("Run", format!("r-{}", run_id.short().to_lowercase())))
                .child(row(
                    "State",
                    match stage.phase {
                        crate::run_stream::StagePhase::Running => "running".into(),
                        crate::run_stream::StagePhase::Done => "done".into(),
                        crate::run_stream::StagePhase::Failed => "failed".into(),
                    },
                ))
                .child(row("Attempt", stage.attempt.to_string()))
                .child(row(
                    "Detail",
                    if stage.detail.is_empty() {
                        "—".into()
                    } else {
                        stage.detail
                    },
                )),
            None => panel.child(ui::meta("Not started yet.")),
        };
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
        let (awaiting, current, agents) = {
            let state = state_entity.read(cx);
            let awaiting = state
                .pending_decisions()
                .iter()
                .any(|(run, p)| *run == plan_run && p.node == "flow_gate");
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
            (awaiting, current, agents)
        };
        if !awaiting {
            return None;
        }
        self.load_provider_notes(cx);
        let notes = self.provider_notes.clone().unwrap_or_default();

        // Model input: one per inspector, re-targeted to the selected step.
        if self.model_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Model, e.g. sonnet or haiku (agent default) — Enter to apply")
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, input, event: &InputEvent, _window, cx| {
                    if !matches!(event, InputEvent::PressEnter { .. }) {
                        return;
                    }
                    let (Some(plan_run), Some(node), Some(state)) = (
                        this.plan_run,
                        this.plan_selected.clone(),
                        this.state.clone(),
                    ) else {
                        return;
                    };
                    let value = input.read(cx).value().trim().to_string();
                    state.update(cx, |state, cx| {
                        edit_step(state, plan_run, &node, |edit| {
                            edit.model = (!value.is_empty()).then_some(value);
                        });
                        cx.notify();
                    });
                },
            )
            .detach();
            self.model_input = Some(input);
        }
        if self.model_input_for.as_deref() != Some(key) {
            let text = current.model.clone().unwrap_or_default();
            if let Some(input) = &self.model_input {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
            self.model_input_for = Some(key.to_string());
        }

        let chip = |id: String, label: String, selected: bool| {
            div()
                .id(SharedString::from(id))
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
            providers = providers.child(
                chip(format!("provider-{key}-{index}"), label, selected).on_click(
                    move |_e, _w, cx| {
                        let agent_id = agent_id.clone();
                        state.update(cx, |state, cx| {
                            edit_step(state, plan_run, &node, |edit| edit.agent_id = agent_id);
                            cx.notify();
                        });
                    },
                ),
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
            let label = level.unwrap_or("Default").to_string();
            efforts = efforts.child(
                chip(format!("effort-{key}-{index}"), label, selected).on_click(
                    move |_e, _w, cx| {
                        state.update(cx, |state, cx| {
                            edit_step(state, plan_run, &node, |edit| {
                                edit.effort = level.map(str::to_string);
                            });
                            cx.notify();
                        });
                    },
                ),
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

    /// Level tabs: one per level of the flow, with what happens there.
    fn render_levels_bar(&self, plans: &LevelPlans, cx: &mut Context<Self>) -> Div {
        let mut row = div().h_flex().gap(px(8.0)).flex_wrap();
        for (i, (level, _)) in plans.levels.iter().enumerate() {
            let active = i == self.level;
            let color = match level.depth {
                0 => Semantic::Plan.color(),
                1 => Semantic::Loop.color(),
                _ => Semantic::Agent.color(),
            };
            let note = if level.approvals > 0 {
                format!("{} steps · {} by you", level.work_steps, level.approvals)
            } else {
                format!("{} steps", level.work_steps)
            };
            row = row.child(
                div()
                    .id(("flow-level", i))
                    .role(Role::Tab)
                    .aria_label(level.title.clone())
                    .v_flex()
                    .gap(px(2.0))
                    .min_w(px(150.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(ui::R_CONTROL + 2.0))
                    .border_1()
                    .border_color(if active {
                        theme::stroke(color)
                    } else {
                        theme::hairline()
                    })
                    .bg(if active {
                        theme::tint(color)
                    } else {
                        theme::panel_raised()
                    })
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.border_color(theme::hairline_strong()))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.level = i;
                        this.plan_selected = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .when(level.depth > 0, |el| {
                                el.child(Icon::new(Lucide::Repeat).size(px(11.0)).text_color(color))
                            })
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(level.title.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::text_muted())
                            .child(note),
                    ),
            );
            if i + 1 < plans.levels.len() {
                row = row.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.0))
                        .text_color(theme::text_dim()),
                );
            }
        }
        row
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
                    .text_size(px(11.5))
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
        let plan = self.live_plan(cx).or_else(|| self.stored.clone());
        let Some(plan) = plan else {
            return ui::panel()
                .child(ui::empty_state(
                    "◇",
                    "No plan for this project yet",
                    "When you describe an app, Surge drafts a flow for it — milestones and tasks each get their own steps — and asks you to approve it. Browse the shapes it can choose from in Library.",
                ))
                .into_any_element();
        };
        self.plan_run = Some(plan.run);
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
    plan_run: surge_core::RunId,
    node: &str,
    change: impl FnOnce(&mut surge_core::node_overrides::NodeOverride),
) {
    let edits = state.plan_edits.entry(plan_run).or_default();
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
