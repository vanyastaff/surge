//! Agents — the coding agents Surge can hand work to.
//!
//! For each installed agent: can it work right now (its API key / login
//! resolves, no usage limit on record — the engine's own pre-launch check),
//! what it did for this project (sessions, tokens, reported cost, models —
//! folded from the run logs by [`crate::agent_usage`]), what it is allowed
//! to touch (the sandbox delegation matrix per mode), and how it is set up.
//! "Make default" writes `surge.toml`; "Add agent" opens the catalog.

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_acp::DetectedAgent;
use surge_core::sandbox::SandboxMode;
use surge_core::sandbox_matrix::{RuntimeSandboxMatrix, default_matrix};

use crate::agent_usage::{Readiness, Usage};
use crate::app_state::AppState;
use crate::theme::{self, Semantic};
use crate::ui;

/// What the screen can ask the app shell to do.
#[derive(Clone)]
pub enum AgentsAction {
    /// Open the Agent Hub catalog (install more agents).
    OpenCatalog,
    /// Open a chat terminal with this agent id.
    OpenTerminal(String),
}

impl EventEmitter<AgentsAction> for AgentsScreen {}

/// Two-letter avatar from a display name ("Claude Code" → "CC").
fn avatar_initials(name: &str) -> String {
    let mut it = name.split_whitespace().filter_map(|w| w.chars().next());
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{a}{b}"),
        (Some(a), None) => a.to_string(),
        _ => "?".to_string(),
    }
    .to_uppercase()
}

fn readiness_look(r: Option<&Readiness>) -> (&'static str, Semantic) {
    match r {
        Some(Readiness::Ready) => ("ready", Semantic::Verified),
        Some(Readiness::NotConfigured(_)) => ("needs setup", Semantic::You),
        Some(Readiness::LimitReached(_)) => ("limit reached", Semantic::Failure),
        None => ("checking…", Semantic::External),
    }
}

fn mode_label(mode: SandboxMode) -> (&'static str, &'static str) {
    match mode {
        SandboxMode::ReadOnly => (
            "Read only",
            "Reads files; cannot change anything or go online.",
        ),
        SandboxMode::WorkspaceWrite => (
            "Edit the project",
            "Reads and writes files in the project; no shell, no network.",
        ),
        SandboxMode::WorkspaceNetwork => (
            "Edit + web",
            "Writes project files and may fetch from the web.",
        ),
        SandboxMode::FullAccess => (
            "Full access",
            "No restrictions — the agent's own runtime decides.",
        ),
        _ => ("Other", ""),
    }
}

/// Operator environment variables the agent needs and this process lacks
/// (required `from` injections with no default).
fn missing_env(agent: &DetectedAgent) -> Vec<String> {
    use surge_core::config::AgentEnvValue;
    let mut missing: Vec<String> = agent
        .entry
        .env
        .values()
        .filter_map(|value| match value {
            AgentEnvValue::Inject {
                from,
                default: None,
                required: true,
            } if std::env::var_os(from).is_none() => Some(from.clone()),
            _ => None,
        })
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Agents screen.
pub struct AgentsScreen {
    state: Entity<AppState>,
    selected: Option<String>,
    matrix: RuntimeSandboxMatrix,
    usage: HashMap<String, Usage>,
    readiness: HashMap<String, Readiness>,
    loaded_for: Option<Vec<String>>,
    note: Option<String>,
}

impl AgentsScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this: &mut Self, _state, cx| {
            this.reload_if_changed(cx);
            cx.notify();
        })
        .detach();
        let mut this = Self {
            state,
            selected: None,
            matrix: default_matrix(),
            usage: HashMap::new(),
            readiness: HashMap::new(),
            loaded_for: None,
            note: None,
        };
        this.reload_if_changed(cx);
        this
    }

    fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        let (key, runs, agents) = {
            let state = self.state.read(cx);
            let runs = state.project_runs();
            let mut ids: Vec<surge_core::RunId> = runs.iter().map(|r| r.run_id).collect();
            for op in state.bootstrap_operations.values() {
                if ids.contains(&op.implementation_run) && !ids.contains(&op.planning_run) {
                    ids.push(op.planning_run);
                }
            }
            let mut key: Vec<String> = runs
                .iter()
                .map(|r| format!("{}:{:?}", r.run_id, r.status))
                .collect();
            key.extend(state.installed_agents.iter().map(|a| a.entry.id.clone()));
            let agents: Vec<_> = state
                .installed_agents
                .iter()
                .map(|a| (a.entry.id.clone(), a.entry.env.clone()))
                .collect();
            (key, ids, agents)
        };
        if self.loaded_for.as_ref() == Some(&key) {
            return;
        }
        self.loaded_for = Some(key);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (usage, readiness) = crate::agent_usage::load(runs, agents).await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.usage = usage;
                    screen.readiness = readiness;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn default_agent(&self, cx: &Context<Self>) -> Option<String> {
        self.state
            .read(cx)
            .config
            .as_ref()
            .map(|c| c.default_agent.clone())
    }

    fn make_default(&mut self, agent: &DetectedAgent, cx: &mut Context<Self>) {
        let entry = agent.entry.clone();
        let result = self.state.update(cx, |state, cx| {
            let mut config = state.config.clone().unwrap_or_default();
            config.default_agent = entry.id.clone();
            config
                .agents
                .entry(entry.id.clone())
                .or_insert_with(|| entry.to_agent_config());
            let result = state.update_config(config);
            cx.notify();
            result
        });
        self.note = Some(match result {
            Ok(()) => format!(
                "{} is now the default for new missions.",
                entry.display_name
            ),
            Err(error) => format!("Could not save surge.toml: {error}"),
        });
        tracing::info!(agent = %entry.id, ok = self.note.as_deref().is_some_and(|n| !n.starts_with("Could")), "default agent changed from the UI");
        cx.notify();
    }

    fn render_row(
        &self,
        agent: &DetectedAgent,
        selected: bool,
        is_default: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = agent.entry.id.clone();
        let (label, role) = readiness_look(self.readiness.get(&agent.entry.id));
        let color = role.color();
        let usage = self.usage.get(&agent.entry.id);
        let summary = match usage {
            Some(u) if u.sessions > 0 => match u.cost_usd {
                Some(cost) => format!("{} sessions · ${cost:.2}", u.sessions),
                None => format!("{} sessions", u.sessions),
            },
            _ => "not used in this project yet".into(),
        };
        div()
            .id(SharedString::from(format!("crew-{}", agent.entry.id)))
            .role(Role::Button)
            .aria_label(format!("Select agent {}", agent.entry.id))
            .h_flex()
            .gap(px(10.0))
            .items_center()
            .px(px(10.0))
            .py(px(9.0))
            .rounded(px(ui::R_CONTROL + 2.0))
            .border_1()
            .border_color(if selected {
                theme::hairline_strong()
            } else {
                transparent_black()
            })
            .when(selected, |el| el.bg(theme::surface()))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.bg(theme::surface()))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.selected = Some(id.clone());
                this.note = None;
                cx.notify();
            }))
            .child(
                div()
                    .size(px(30.0))
                    .flex_none()
                    .rounded(px(ui::R_CONTROL + 2.0))
                    .bg(theme::tint(theme::accent()))
                    .border_1()
                    .border_color(theme::stroke(theme::accent()).opacity(0.6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::accent())
                    .child(avatar_initials(&agent.entry.display_name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_primary())
                                    .truncate()
                                    .child(agent.entry.display_name.clone()),
                            )
                            .when(is_default, |el| {
                                el.child(ui::role_badge("default", Semantic::Agent))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::text_dim())
                            .truncate()
                            .child(summary),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h_flex()
                    .gap(px(5.0))
                    .items_center()
                    .text_size(px(10.5))
                    .text_color(color)
                    .child(ui::status_dot(color))
                    .child(label),
            )
    }

    fn render_readiness(&self, agent: &DetectedAgent) -> Option<Div> {
        let (headline, detail, fix, role) = match self.readiness.get(&agent.entry.id)? {
            Readiness::Ready => return None,
            Readiness::NotConfigured(why) => {
                let missing = missing_env(agent);
                let fix = if missing.is_empty() {
                    "Sign in with the agent's own CLI, then come back.".to_string()
                } else {
                    format!(
                        "Set {} in your environment, then restart Surge.",
                        missing.join(", ")
                    )
                };
                ("Needs setup before it can work", why.clone(), fix, Semantic::You)
            },
            Readiness::LimitReached(why) => (
                "Usage limit reached",
                why.clone(),
                "Missions that need this agent wait until the limit resets, or pick another agent for those steps in Flow."
                    .to_string(),
                Semantic::Failure,
            ),
        };
        let color = role.color();
        Some(
            ui::node_card(color)
                .h_flex()
                .gap(px(12.0))
                .items_start()
                .px(px(14.0))
                .py(px(11.0))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(15.0))
                        .text_color(color),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .v_flex()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text_primary())
                                .child(headline),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme::text_primary())
                                .child(fix),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme::text_muted())
                                .child(detail),
                        ),
                ),
        )
    }

    fn render_usage(&self, agent: &DetectedAgent) -> Div {
        let usage = self.usage.get(&agent.entry.id).cloned().unwrap_or_default();
        let tile = |label: &'static str, value: String, note: String| {
            ui::panel()
                .flex_1()
                .min_w(px(0.0))
                .v_flex()
                .gap(px(4.0))
                .px(px(14.0))
                .py(px(11.0))
                .child(ui::section_label(label))
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child(value),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme::text_dim())
                        .child(note),
                )
        };
        let last = usage
            .last_used_ms
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("last %b %d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "never".into());
        div()
            .v_flex()
            .gap(px(8.0))
            .child(ui::section_label("In this project"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .gap(px(10.0))
                    .child(tile("Sessions", usage.sessions.to_string(), last))
                    .child(if usage.tokens_in + usage.tokens_out == 0 {
                        tile(
                            "Tokens",
                            "—".into(),
                            if usage.sessions > 0 {
                                "not reported by this agent".into()
                            } else {
                                "no sessions yet".into()
                            },
                        )
                    } else {
                        tile(
                            "Tokens",
                            fmt_tokens(usage.tokens_in + usage.tokens_out),
                            format!(
                                "{} in · {} out",
                                fmt_tokens(usage.tokens_in),
                                fmt_tokens(usage.tokens_out)
                            ),
                        )
                    })
                    .child(tile(
                        "Cost",
                        usage
                            .cost_usd
                            .map_or_else(|| "—".into(), |c| format!("${c:.2}")),
                        if usage.cost_usd.is_some() {
                            "as reported by the agent".into()
                        } else if usage.sessions > 0 {
                            "not reported by this agent".into()
                        } else {
                            "no sessions yet".into()
                        },
                    )),
            )
            .when(!usage.models.is_empty(), |el| {
                el.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme::text_dim())
                                .child("Models used"),
                        )
                        .children(usage.models.iter().map(|m| {
                            ui::pill(m.clone(), theme::text_muted(), theme::panel_deep())
                        })),
                )
            })
    }

    fn render_permissions(&self, agent: &DetectedAgent, cx: &Context<Self>) -> Div {
        let project_mode = self
            .state
            .read(cx)
            .config
            .as_ref()
            .map(|c| c.init.sandbox_default);
        let mut body = div().v_flex();
        match agent.entry.runtime {
            Some(runtime) => {
                let rows: Vec<_> = self
                    .matrix
                    .rows()
                    .iter()
                    .filter(|r| r.runtime == runtime)
                    .collect();
                if rows.is_empty() {
                    body = body.child(ui::meta(
                        "No permission modes are declared for this agent yet.",
                    ));
                }
                for row in rows {
                    let (name, plain) = mode_label(row.mode);
                    let is_project = project_mode == Some(row.mode);
                    body = body.child(
                        div()
                            .v_flex()
                            .gap(px(4.0))
                            .py(px(9.0))
                            .border_b_1()
                            .border_color(theme::hairline().opacity(0.6))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text_primary())
                                            .child(name),
                                    )
                                    .when(is_project, |el| {
                                        el.child(ui::role_badge("this project", Semantic::Agent))
                                    })
                                    .child(div().flex_1())
                                    .child(if row.verified {
                                        ui::role_badge("verified", Semantic::Verified)
                                    } else {
                                        ui::role_badge("unverified", Semantic::You)
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .child(plain),
                            )
                            .when(!row.flags.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(theme::text_dim())
                                        .child(row.flags.join(" ")),
                                )
                            }),
                    );
                }
            },
            None => {
                body = body.child(ui::meta(
                    "Surge does not know how to restrict this agent — it runs with the agent's own settings.",
                ));
            },
        }
        ui::panel()
            .v_flex()
            .gap(px(4.0))
            .p(px(14.0))
            .child(ui::section_label("What it may touch"))
            .child(body)
    }

    fn render_setup(&self, agent: &DetectedAgent) -> Div {
        let entry = &agent.entry;
        let command = agent
            .command_path
            .clone()
            .unwrap_or_else(|| entry.command.clone());
        let kv = |k: &'static str, v: String| {
            div()
                .h_flex()
                .gap(px(10.0))
                .items_start()
                .py(px(5.0))
                .child(
                    div()
                        .w(px(80.0))
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .text_color(theme::text_dim())
                        .child(k),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.0))
                        .text_color(theme::text_primary())
                        .child(v),
                )
        };
        ui::panel()
            .v_flex()
            .p(px(14.0))
            .child(ui::section_label("Setup"))
            .child(kv("id", entry.id.clone()))
            .child(kv("command", command))
            .when(!entry.default_args.is_empty(), |el| {
                el.child(kv("args", entry.default_args.join(" ")))
            })
            .child(kv("transport", format!("{:?}", entry.transport)))
            .when(!entry.env.is_empty(), |el| {
                el.child(kv(
                    "needs",
                    entry.env.keys().cloned().collect::<Vec<_>>().join(", "),
                ))
            })
            .when(!entry.models.is_empty(), |el| {
                el.child(kv("models", entry.models.join(", ")))
            })
            .child(kv("license", entry.license.clone()))
            .when_some(entry.website.clone(), |el, w| el.child(kv("website", w)))
    }

    fn render_detail(
        &self,
        agent: &DetectedAgent,
        is_default: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (label, role) = readiness_look(self.readiness.get(&agent.entry.id));
        let color = role.color();
        let id_for_chat = agent.entry.id.clone();
        let agent_for_default = agent.clone();
        let version = agent
            .detected_version
            .clone()
            .unwrap_or_else(|| agent.entry.version.clone());
        div()
            .flex_1()
            .min_w(px(0.0))
            .v_flex()
            .gap(px(16.0))
            .child(
                div()
                    .h_flex()
                    .gap(px(14.0))
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .v_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::text_primary())
                                            .child(agent.entry.display_name.clone()),
                                    )
                                    .child(ui::pill(label, color, theme::tint(color)))
                                    .when(is_default, |el| el.child(ui::role_badge("default", Semantic::Agent))),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme::text_muted())
                                    .child(format!("{} · {version}", agent.entry.description)),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .flex_none()
                            .when(!is_default, |el| {
                                el.child(
                                    Button::new("agent-make-default")
                                        .outline()
                                        .small()
                                        .icon(IconName::Check)
                                        .label("Make default")
                                        .tooltip("New missions use this agent unless a step says otherwise")
                                        .on_click(cx.listener(move |this, _e, _w, cx| {
                                            this.make_default(&agent_for_default, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("agent-open-terminal")
                                    .ghost()
                                    .small()
                                    .icon(IconName::SquareTerminal)
                                    .label("Chat")
                                    .on_click(cx.listener(move |_this, _e, _w, cx| {
                                        cx.emit(AgentsAction::OpenTerminal(id_for_chat.clone()));
                                    })),
                            ),
                    ),
            )
            .children(self.note.clone().map(|n| {
                div().text_size(px(11.5)).text_color(theme::text_muted()).child(n)
            }))
            .children(self.render_readiness(agent))
            .child(self.render_usage(agent))
            .child(
                div()
                    .flex()
                    .gap(px(14.0))
                    .items_start()
                    .child(div().flex_1().min_w(px(0.0)).child(self.render_permissions(agent, cx)))
                    .child(div().w(px(320.0)).flex_none().child(self.render_setup(agent))),
            )
    }
}

impl Render for AgentsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agents: Vec<DetectedAgent> = self.state.read(cx).installed_agents.clone();
        let default = self.default_agent(cx);
        let ready = agents
            .iter()
            .filter(|a| matches!(self.readiness.get(&a.entry.id), Some(Readiness::Ready)))
            .count();
        let subtitle = (!agents.is_empty()).then(|| {
            SharedString::from(format!(
                "{} installed · {ready} ready to work{}",
                agents.len(),
                default
                    .as_ref()
                    .and_then(|d| agents.iter().find(|a| &a.entry.id == d))
                    .map(|a| format!(" · default: {}", a.entry.display_name))
                    .unwrap_or_default()
            ))
        });
        let header = ui::page_header(
            "Agents",
            subtitle,
            Button::new("agents-open-catalog")
                .outline()
                .small()
                .icon(IconName::Plus)
                .label("Add agent")
                .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(AgentsAction::OpenCatalog))),
        );

        let body: AnyElement = if agents.is_empty() {
            ui::panel()
                .child(
                    ui::empty_state(
                        "◍",
                        "No coding agents found",
                        "Surge hands work to coding agents that speak ACP — Claude Code, Codex, Gemini and others. Install one and it appears here.",
                    )
                    .child(
                        Button::new("agents-empty-catalog")
                            .primary()
                            .icon(Lucide::Bot)
                            .label("Browse the catalog")
                            .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(AgentsAction::OpenCatalog))),
                    ),
                )
                .into_any_element()
        } else {
            let selected = self
                .selected
                .as_ref()
                .and_then(|id| agents.iter().find(|a| &a.entry.id == id))
                .or_else(|| {
                    default
                        .as_ref()
                        .and_then(|d| agents.iter().find(|a| &a.entry.id == d))
                })
                .or_else(|| agents.first())
                .cloned();
            let selected_id = selected.as_ref().map(|a| a.entry.id.clone());
            let rows: Vec<Stateful<Div>> = agents
                .iter()
                .map(|a| {
                    self.render_row(
                        a,
                        selected_id.as_deref() == Some(a.entry.id.as_str()),
                        default.as_deref() == Some(a.entry.id.as_str()),
                        cx,
                    )
                })
                .collect();
            div()
                .flex()
                .gap(px(20.0))
                .items_start()
                .child(
                    ui::panel()
                        .w(px(320.0))
                        .flex_none()
                        .v_flex()
                        .gap(px(2.0))
                        .p(px(6.0))
                        .children(rows),
                )
                .children(selected.map(|a| {
                    let is_default = default.as_deref() == Some(a.entry.id.as_str());
                    self.render_detail(&a, is_default, cx)
                }))
                .into_any_element()
        };

        div()
            .id("agents-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::background())
            .px(px(28.0))
            .pt(px(22.0))
            .pb(px(32.0))
            .child(div().v_flex().gap(px(18.0)).child(header).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::{fmt_tokens, readiness_look};
    use crate::agent_usage::Readiness;
    use crate::theme::Semantic;

    #[test]
    fn readiness_is_never_optimistic_before_it_is_known() {
        assert_eq!(readiness_look(None), ("checking…", Semantic::External));
        assert_eq!(
            readiness_look(Some(&Readiness::NotConfigured("x".into()))).0,
            "needs setup"
        );
        assert_eq!(readiness_look(Some(&Readiness::Ready)).0, "ready");
    }

    #[test]
    fn token_counts_stay_readable() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(12_300), "12.3k");
        assert_eq!(fmt_tokens(4_200_000), "4.2M");
    }
}
