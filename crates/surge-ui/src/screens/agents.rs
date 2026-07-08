//! Agents — the crew: who can fly, what they may touch, how they're
//! doing.
//!
//! Everything here is real: crew cards come from PATH detection
//! ([`AppState::installed_agents`]), status pills from the
//! [`surge_acp::HealthTracker`], config from the registry entry, and —
//! the piece no competitor surfaces — the **sandbox delegation
//! matrix** ([`surge_core::sandbox_matrix::default_matrix`]) rendered
//! per runtime, so the operator can see exactly which sandbox modes
//! are verified for this agent and with what launch flags. Permissions
//! made legible, not buried in docs.
//!
//! "Add agent" opens the Agent Hub catalog (registry with install
//! instructions). When nothing is installed the screen says so — agent
//! detection always runs, so there is no sample mode here.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use surge_acp::{DetectedAgent, HealthStatus};
use surge_core::sandbox_matrix::{RuntimeSandboxMatrix, default_matrix};

use crate::app_state::AppState;
use crate::theme;
use crate::ui;

/// What the crew screen can ask the app shell to do.
#[derive(Clone)]
pub enum AgentsAction {
    /// Open the Agent Hub catalog (install more agents).
    OpenCatalog,
    /// Open a chat terminal with this agent id.
    OpenTerminal(String),
}

impl EventEmitter<AgentsAction> for AgentsScreen {}

pub struct AgentsScreen {
    state: Entity<AppState>,
    /// Selected agent id; defaults to the first detected.
    selected: Option<String>,
    /// The static runtime × mode delegation table (surge-core).
    matrix: RuntimeSandboxMatrix,
}

fn status_parts(status: HealthStatus) -> (&'static str, Hsla) {
    match status {
        HealthStatus::Healthy => ("online", theme::success()),
        HealthStatus::Degraded => ("degraded", theme::warning()),
        HealthStatus::Offline => ("offline", theme::error()),
    }
}

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

impl AgentsScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self {
            state,
            selected: None,
            matrix: default_matrix(),
        }
    }

    fn selected_agent<'a>(&self, agents: &'a [DetectedAgent]) -> Option<&'a DetectedAgent> {
        match &self.selected {
            Some(id) => agents
                .iter()
                .find(|a| &a.entry.id == id)
                .or_else(|| agents.first()),
            None => agents.first(),
        }
    }

    // ── crew cards ──────────────────────────────────────────────────

    fn render_crew_card(
        &self,
        agent: &DetectedAgent,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let state = self.state.read(cx);
        let health = state.health.get_health(&agent.entry.id);
        let (status_label, status_color) = health
            .map(|h| status_parts(h.status()))
            .unwrap_or(("unknown", theme::text_muted()));
        let requests = health.map(|h| h.total_requests).unwrap_or(0);
        let err_rate = health.map(|h| h.error_rate()).unwrap_or(0.0);

        let id = agent.entry.id.clone();
        let version = agent
            .detected_version
            .clone()
            .unwrap_or_else(|| agent.entry.version.clone());

        let traffic = if requests == 0 {
            "no traffic this session".to_string()
        } else {
            format!("{requests} requests · {err_rate:.0}% errors")
        };

        div()
            .id(SharedString::from(format!("crew-{}", agent.entry.id)))
            .flex_1()
            .min_w(px(220.0))
            .max_w(px(320.0))
            .v_flex()
            .gap(px(9.0))
            .p(px(13.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(if is_selected {
                theme::accent().opacity(0.5)
            } else {
                theme::hairline()
            })
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.4)))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.selected = Some(id.clone());
                cx.notify();
            }))
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(
                        div()
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded_md()
                            .bg(theme::accent().opacity(0.14))
                            .border_1()
                            .border_color(theme::accent().opacity(0.35))
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
                            .min_w_0()
                            .v_flex()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(agent.entry.display_name.clone()),
                            )
                            .child(ui::meta(version)),
                    )
                    .child(ui::pill(
                        status_label,
                        status_color,
                        status_color.opacity(0.13),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_muted())
                    .overflow_hidden()
                    .child(traffic),
            )
    }

    // ── detail: left column ─────────────────────────────────────────

    fn render_detail_header(&self, agent: &DetectedAgent, cx: &mut Context<Self>) -> Div {
        let state = self.state.read(cx);
        let (status_label, status_color) = state
            .health
            .get_health(&agent.entry.id)
            .map(|h| status_parts(h.status()))
            .unwrap_or(("unknown", theme::text_muted()));

        let id_for_chat = agent.entry.id.clone();

        div()
            .h_flex()
            .gap(px(13.0))
            .items_center()
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(theme::accent().opacity(0.14))
                    .border_1()
                    .border_color(theme::accent().opacity(0.35))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::accent())
                    .child(avatar_initials(&agent.entry.display_name)),
            )
            .child(
                div()
                    .v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(9.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(agent.entry.display_name.clone()),
                            )
                            .child(ui::pill(
                                status_label,
                                status_color,
                                status_color.opacity(0.13),
                            )),
                    )
                    .child(ui::meta(agent.entry.description.clone())),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("agent-open-terminal")
                    .h_flex()
                    .gap(px(7.0))
                    .items_center()
                    .h(px(31.0))
                    .px(px(13.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .text_color(theme::text_primary().opacity(0.85))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.5)))
                    .on_click(cx.listener(move |_this, _e, _w, cx| {
                        cx.emit(AgentsAction::OpenTerminal(id_for_chat.clone()));
                    }))
                    .child("Open terminal"),
            )
    }

    fn render_capabilities(&self, agent: &DetectedAgent) -> Div {
        let mut row = div().h_flex().gap(px(7.0)).flex_wrap();
        for cap in &agent.entry.capabilities {
            row = row.child(
                div()
                    .px(px(9.0))
                    .py(px(3.0))
                    .rounded_md()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .text_size(px(9.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary().opacity(0.8))
                    .child(format!("{cap}")),
            );
        }
        for tag in agent.entry.tags.iter().take(4) {
            row = row.child(
                div()
                    .px(px(9.0))
                    .py(px(3.0))
                    .rounded_md()
                    .text_size(px(9.5))
                    .text_color(theme::text_muted())
                    .child(format!("#{tag}")),
            );
        }

        ui::panel()
            .v_flex()
            .gap(px(10.0))
            .p(px(14.0))
            .child(ui::section_label("CAPABILITIES"))
            .child(row)
    }

    fn render_metrics(&self, agent: &DetectedAgent, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let health = state.health.get_health(&agent.entry.id);

        let (requests, failures, err_rate, p50, p99, uptime, rate_limited) = health
            .map(|h| {
                (
                    h.total_requests,
                    h.total_failures,
                    h.error_rate(),
                    h.latency_p50_ms(),
                    h.latency_p99_ms(),
                    h.uptime().as_secs(),
                    h.rate_limited,
                )
            })
            .unwrap_or((0, 0, 0.0, 0, 0, 0, false));

        let uptime_str = if uptime < 3600 {
            format!("{}m", uptime / 60)
        } else {
            format!("{}h {}m", uptime / 3600, (uptime % 3600) / 60)
        };

        let stat = |label: &'static str, value: String, sub: String, color: Hsla| {
            ui::panel()
                .flex_1()
                .v_flex()
                .gap(px(5.0))
                .p(px(13.0))
                .child(ui::section_label(label))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .child(value),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(theme::text_muted())
                        .child(sub),
                )
        };

        div()
            .h_flex()
            .gap(px(10.0))
            .child(stat(
                "REQUESTS",
                requests.to_string(),
                format!("{failures} failed"),
                theme::text_primary(),
            ))
            .child(stat(
                "ERROR RATE",
                format!("{err_rate:.0}%"),
                if rate_limited {
                    "rate-limited".to_string()
                } else {
                    "healthy threshold <50%".to_string()
                },
                if err_rate >= 50.0 || rate_limited {
                    theme::error()
                } else {
                    theme::success()
                },
            ))
            .child(stat(
                "LATENCY",
                if p50 == 0 {
                    "—".to_string()
                } else {
                    format!("{p50}ms")
                },
                format!("p99 {p99}ms"),
                theme::text_primary(),
            ))
            .child(stat(
                "TRACKED",
                uptime_str,
                "since registration".to_string(),
                theme::text_primary(),
            ))
    }

    /// The differentiator pane: this runtime's rows from the sandbox
    /// delegation matrix — which modes are verified, with what flags.
    fn render_sandbox_matrix(&self, agent: &DetectedAgent) -> Div {
        let mut body = div().v_flex().gap(px(6.0));

        match agent.entry.runtime {
            Some(runtime) => {
                let rows: Vec<_> = self
                    .matrix
                    .rows()
                    .iter()
                    .filter(|r| r.runtime == runtime)
                    .collect();
                if rows.is_empty() {
                    body = body.child(ui::meta("no delegation rows declared for this runtime yet"));
                }
                for row in rows {
                    let mode = format!("{:?}", row.mode);
                    let flags = if row.flags.is_empty() {
                        "—".to_string()
                    } else {
                        row.flags.join(" ")
                    };
                    let (badge, badge_color) = if row.verified {
                        ("verified", theme::success())
                    } else {
                        ("unverified", theme::warning())
                    };

                    body = body.child(
                        div()
                            .v_flex()
                            .gap(px(3.0))
                            .py(px(7.0))
                            .border_b_1()
                            .border_color(theme::hairline().opacity(0.6))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(9.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(130.0))
                                            .flex_shrink_0()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text_primary())
                                            .child(mode),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_size(px(10.5))
                                            .text_color(theme::text_muted())
                                            .child(flags),
                                    )
                                    .child(ui::pill(badge, badge_color, badge_color.opacity(0.13))),
                            )
                            .when(!row.note.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(9.5))
                                        .text_color(theme::text_muted().opacity(0.8))
                                        .child(row.note.clone()),
                                )
                            }),
                    );
                }
            },
            None => {
                body = body.child(ui::meta(
                    "runtime unmapped — no sandbox delegation declared (see docs/sandbox-matrix.md)",
                ));
            },
        }

        ui::panel()
            .v_flex()
            .gap(px(10.0))
            .p(px(14.0))
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(ui::section_label("SANDBOX DELEGATION"))
                    .child(ui::meta("what surge hands this runtime, per mode"))
                    .child(div().flex_1())
                    .child(ui::meta("surge doctor matrix")),
            )
            .child(body)
    }

    // ── detail: right rail ──────────────────────────────────────────

    fn render_config(&self, agent: &DetectedAgent) -> Div {
        let entry = &agent.entry;
        let transport = format!("{:?}", entry.transport);
        let command = agent
            .command_path
            .clone()
            .unwrap_or_else(|| entry.command.clone());
        let args = if entry.default_args.is_empty() {
            "—".to_string()
        } else {
            entry.default_args.join(" ")
        };
        let runtime = entry
            .runtime
            .map(|r| format!("{r:?}"))
            .unwrap_or_else(|| "unmapped".to_string());

        let kv = |k: &'static str, v: String| {
            div()
                .h_flex()
                .gap(px(10.0))
                .items_center()
                .pb(px(8.0))
                .border_b_1()
                .border_color(theme::hairline().opacity(0.6))
                .child(
                    div()
                        .w(px(80.0))
                        .flex_shrink_0()
                        .text_size(px(10.0))
                        .text_color(theme::text_muted())
                        .child(k),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(10.5))
                        .text_color(theme::text_primary().opacity(0.88))
                        .child(v),
                )
        };

        ui::panel()
            .v_flex()
            .gap(px(9.0))
            .p(px(14.0))
            .child(ui::section_label("CONFIG"))
            .child(kv("id", entry.id.clone()))
            .child(kv("command", command))
            .child(kv("args", args))
            .child(kv("transport", transport))
            .child(kv("runtime", runtime))
            .child(kv("license", entry.license.clone()))
            .when(!entry.models.is_empty(), |el| {
                el.child(kv("models", entry.models.join(", ")))
            })
            .when(entry.website.is_some(), |el| {
                el.child(kv("website", entry.website.clone().unwrap_or_default()))
            })
    }

    fn render_about(&self, agent: &DetectedAgent) -> Div {
        let text = if agent.entry.long_description.is_empty() {
            agent.entry.description.clone()
        } else {
            agent.entry.long_description.clone()
        };
        ui::panel()
            .v_flex()
            .gap(px(9.0))
            .p(px(14.0))
            .child(ui::section_label("ABOUT"))
            .child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(17.0))
                    .text_color(theme::text_muted())
                    .child(text),
            )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> Div {
        div().flex_1().flex().items_center().justify_center().child(
            div()
                .v_flex()
                .gap(px(12.0))
                .items_center()
                .max_w(px(420.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child("No agents detected on PATH"),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(18.0))
                        .text_color(theme::text_muted())
                        .text_center()
                        .child(
                            "Surge orchestrates any ACP-speaking coding agent. \
                                 Install one (Claude Code, Codex CLI, Gemini CLI, …) \
                                 and it appears here as crew.",
                        ),
                )
                .child(
                    div()
                        .id("agents-open-catalog")
                        .h(px(34.0))
                        .px(px(16.0))
                        .rounded_lg()
                        .bg(theme::accent())
                        .flex()
                        .items_center()
                        .text_color(hsla(0.0, 0.0, 0.08, 1.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s: StyleRefinement| s.bg(theme::accent().opacity(0.85)))
                        .on_click(cx.listener(|_this, _e, _w, cx| {
                            cx.emit(AgentsAction::OpenCatalog);
                        }))
                        .child("Browse the catalog"),
                ),
        )
    }
}

impl Render for AgentsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agents: Vec<DetectedAgent> = self.state.read(cx).installed_agents.clone();
        let catalog_size = self.state.read(cx).registry.len();

        if agents.is_empty() {
            return div()
                .size_full()
                .v_flex()
                .bg(theme::panel_deep())
                .child(self.render_empty(cx))
                .into_any_element();
        }

        let selected = self.selected_agent(&agents).cloned();
        let selected_id = selected.as_ref().map(|a| a.entry.id.clone());

        let crew: Vec<Stateful<Div>> = agents
            .iter()
            .map(|a| self.render_crew_card(a, selected_id.as_deref() == Some(&a.entry.id), cx))
            .collect();

        let header = div()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child("Agents"),
            )
            .child(ui::meta(format!(
                "{} in crew · {} in catalog",
                agents.len(),
                catalog_size
            )))
            .child(div().flex_1())
            .child(
                div()
                    .id("agents-add")
                    .h_flex()
                    .gap(px(7.0))
                    .items_center()
                    .h(px(31.0))
                    .px(px(14.0))
                    .rounded_lg()
                    .bg(theme::accent())
                    .text_color(hsla(0.0, 0.0, 0.08, 1.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.bg(theme::accent().opacity(0.85)))
                    .on_click(cx.listener(|_this, _e, _w, cx| {
                        cx.emit(AgentsAction::OpenCatalog);
                    }))
                    .child("+ Add agent"),
            );

        let mut detail = div().flex_1().min_h_0().h_flex().gap(px(14.0));
        if let Some(agent) = &selected {
            detail = detail
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .id("agents-detail-left")
                        .overflow_y_scroll()
                        .v_flex()
                        .gap(px(14.0))
                        .child(self.render_detail_header(agent, cx))
                        .child(self.render_metrics(agent, cx))
                        .child(self.render_sandbox_matrix(agent))
                        .child(self.render_capabilities(agent)),
                )
                .child(
                    div()
                        .w(px(330.0))
                        .flex_shrink_0()
                        .id("agents-detail-right")
                        .overflow_y_scroll()
                        .v_flex()
                        .gap(px(14.0))
                        .child(self.render_config(agent))
                        .child(self.render_about(agent)),
                );
        }

        div()
            .size_full()
            .v_flex()
            .gap(px(16.0))
            .p(px(20.0))
            .bg(theme::panel_deep())
            .child(header)
            .child(div().h_flex().gap(px(12.0)).children(crew))
            .child(detail)
            .into_any_element()
    }
}
