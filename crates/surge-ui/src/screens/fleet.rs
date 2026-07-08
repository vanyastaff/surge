//! Fleet — the run constellation (mission-control home).
//!
//! Adapted from the "Surge - Interactive" concept: runs are nodes
//! branching off the `main` trunk. This is *inspired by* the mockup,
//! not a pixel copy — it renders **real** runs from [`AppState::runs`]
//! when the daemon link is live, and falls back to a clearly-labelled
//! sample constellation when there are none, so the shape of the idea
//! is legible offline without ever faking live data.
//!
//! Geometry is a fixed, centered "stage" (robust in flexbox) rather
//! than the concept's freeform bezier canvas — nodes alternate above /
//! below a horizontal trunk with straight connectors. True curved edges
//! via `gpui::canvas` are a later polish.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::AppState;
use crate::theme;
use crate::ui;

/// Stage coordinate space (centered inside the canvas).
const STAGE_W: f32 = 1000.0;
const STAGE_H: f32 = 480.0;
const TRUNK_Y: f32 = 240.0;
const NODE_X0: f32 = 90.0;
const NODE_STEP: f32 = 165.0;
const CARD_W: f32 = 188.0;

/// What the operator can trigger from the Fleet surface.
#[derive(Clone)]
pub enum FleetAction {
    /// Open a run's cockpit (live execution).
    OpenRun(String),
    /// Open the review gate for a run awaiting the operator.
    OpenGate(String),
}

impl EventEmitter<FleetAction> for FleetScreen {}

#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    Merged,
    Failed,
    Active,
    Review,
    Queued,
}

impl NodeKind {
    fn color(self) -> Hsla {
        match self {
            Self::Merged => theme::success(),
            Self::Failed => theme::error(),
            Self::Active | Self::Review => theme::accent(),
            Self::Queued => theme::text_muted(),
        }
    }

    fn status_label(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Failed => "failed",
            Self::Active => "active",
            Self::Review => "review gate",
            Self::Queued => "queued",
        }
    }
}

/// One node on the constellation.
#[derive(Clone)]
struct FleetNode {
    id: String,
    title: String,
    meta: String,
    kind: NodeKind,
    /// Whether this is real daemon data (vs. sample fallback).
    live: bool,
}

/// Fleet screen — reads runs from shared state, renders the constellation.
pub struct FleetScreen {
    state: Entity<AppState>,
}

impl FleetScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Re-render when the run list / daemon link changes.
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self { state }
    }

    /// Build nodes from real runs, or a labelled sample when offline.
    fn nodes(&self, cx: &Context<Self>) -> (Vec<FleetNode>, bool) {
        let state = self.state.read(cx);
        if state.runs.is_empty() {
            return (sample_nodes(), false);
        }

        let nodes = state
            .runs
            .iter()
            .take(6)
            .map(|r| {
                let kind = match r.status {
                    RunStatus::Active => NodeKind::Active,
                    RunStatus::Completed => NodeKind::Merged,
                    RunStatus::Failed | RunStatus::Aborted => NodeKind::Failed,
                    _ => NodeKind::Queued,
                };
                let short = r.run_id.short();
                let seq = r
                    .last_event_seq
                    .map(|s| format!("seq {s}"))
                    .unwrap_or_else(|| "—".to_string());
                FleetNode {
                    id: format!("r-{short}"),
                    title: format!("started {}", r.started_at.format("%H:%M")),
                    meta: seq,
                    kind,
                    live: true,
                }
            })
            .collect();
        (nodes, true)
    }

    // ── stage pieces ────────────────────────────────────────────────

    fn node_x(i: usize) -> f32 {
        NODE_X0 + (i as f32) * NODE_STEP
    }

    /// Trunk pieces as direct children of the (sized, relative) stage.
    fn render_trunk(&self) -> Vec<AnyElement> {
        vec![
            // the line
            div()
                .absolute()
                .left(px(60.0))
                .top(px(TRUNK_Y - 1.0))
                .w(px(STAGE_W - 120.0))
                .h(px(2.0))
                .rounded_full()
                .bg(theme::graph_line())
                .into_any_element(),
            // "main" label
            div()
                .absolute()
                .left(px(16.0))
                .top(px(TRUNK_Y - 26.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_muted())
                .child("main")
                .into_any_element(),
            // HEAD chip
            div()
                .absolute()
                .left(px(STAGE_W - 58.0))
                .top(px(TRUNK_Y - 12.0))
                .px(px(6.0))
                .py(px(2.0))
                .rounded_md()
                .bg(theme::panel_raised())
                .border_1()
                .border_color(theme::hairline_strong())
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_muted())
                .child("HEAD")
                .into_any_element(),
        ]
    }

    /// A node's connector + dot + mission card, as direct children of the
    /// stage (so their absolute coords anchor to the stage box).
    fn render_node(&self, node: &FleetNode, i: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let x = Self::node_x(i);
        let above = i.is_multiple_of(2);
        let color = node.kind.color();
        let card_top = if above { 84.0 } else { 316.0 };
        let connector_top = if above { 158.0 } else { TRUNK_Y };

        let id_for_click = node.id.clone();
        let is_review = node.kind == NodeKind::Review;
        let is_failed = node.kind == NodeKind::Failed;

        let connector = div()
            .absolute()
            .left(px(x - 0.75))
            .top(px(connector_top))
            .w(px(1.5))
            .h(px(82.0))
            .bg(color.opacity(0.4))
            .into_any_element();

        let dot = div()
            .absolute()
            .left(px(x - 5.0))
            .top(px(TRUNK_Y - 5.0))
            .w(px(10.0))
            .h(px(10.0))
            .rounded_full()
            .bg(color)
            .border_2()
            .border_color(theme::panel_deep())
            .into_any_element();

        let card = div()
            .id(SharedString::from(format!("fleet-card-{}", node.id)))
            .absolute()
            .left(px(x - CARD_W / 2.0))
            .top(px(card_top))
            .w(px(CARD_W))
            .v_flex()
            .gap(px(6.0))
            .p(px(11.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(if is_review {
                theme::accent().opacity(0.5)
            } else if is_failed {
                theme::error().opacity(0.4)
            } else {
                theme::hairline()
            })
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.6)))
            .on_click(cx.listener(move |_this, _e, _w, cx| {
                if is_review || is_failed {
                    cx.emit(FleetAction::OpenGate(id_for_click.clone()));
                } else {
                    cx.emit(FleetAction::OpenRun(id_for_click.clone()));
                }
            }))
            // header
            .child(
                div()
                    .h_flex()
                    .gap(px(7.0))
                    .items_center()
                    .child(ui::status_dot(color))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child(node.id.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(9.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(node.kind.status_label()),
                    ),
            )
            // title
            .child(
                div()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_primary().opacity(0.9))
                    .child(node.title.clone()),
            )
            // meta footer
            .child(ui::meta(node.meta.clone()))
            .into_any_element();

        vec![connector, dot, card]
    }

    fn render_chips(&self, active: usize, needs: usize, merged: usize, live: bool) -> Div {
        let chip = |dot: Hsla, text: String, fg: Hsla| {
            div()
                .h_flex()
                .gap(px(6.0))
                .items_center()
                .child(ui::status_dot(dot))
                .child(div().text_size(px(11.0)).text_color(fg).child(text))
        };

        div()
            .absolute()
            .top(px(14.0))
            .left(px(18.0))
            .h_flex()
            .gap(px(14.0))
            .items_center()
            .child(chip(
                theme::accent(),
                format!("{active} active"),
                theme::text_muted(),
            ))
            .child(chip(
                theme::warning(),
                format!("{needs} needs you"),
                theme::accent(),
            ))
            .child(chip(
                theme::success(),
                format!("{merged} merged"),
                theme::text_muted(),
            ))
            .when(!live, |el| {
                el.child(
                    ui::pill(
                        "sample · start the daemon for live runs",
                        theme::text_muted(),
                        theme::panel_raised(),
                    )
                    .border_1()
                    .border_color(theme::hairline()),
                )
            })
    }

    fn render_inspector(&self, node: FleetNode, cx: &mut Context<Self>) -> impl IntoElement {
        let id = node.id.clone();
        let live = node.live;
        let primary_label = if live { "Open run" } else { "Approve & merge" };

        // primary button
        let id_primary = id.clone();
        let primary = div()
            .id("fleet-inspector-primary")
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .rounded_lg()
            .bg(theme::accent())
            .text_color(hsla(0.0, 0.0, 0.08, 1.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.bg(theme::accent().opacity(0.85)))
            .on_click(cx.listener(move |_this, _e, _w, cx| {
                cx.emit(FleetAction::OpenGate(id_primary.clone()));
            }))
            .child(primary_label);

        ui::panel()
            .absolute()
            .top(px(16.0))
            .right(px(18.0))
            .w(px(288.0))
            .v_flex()
            .gap(px(11.0))
            .p(px(16.0))
            .shadow_lg()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(ui::status_dot(node.kind.color()))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(node.kind.color())
                            .child(if live { "NEEDS ATTENTION" } else { "REVIEW GATE" }),
                    )
                    .child(div().flex_1())
                    .child(ui::meta(node.id.clone())),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child(node.title.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(17.0))
                    .text_color(theme::text_muted())
                    .child(if live {
                        "This run ended without merging. Open its cockpit to inspect what happened."
                    } else {
                        "Branch ready to merge into main. Awaiting your call."
                    }),
            )
            .child(primary)
    }

    fn render_command_bar(&self, agents: usize) -> Div {
        div()
            .h(px(60.0))
            .flex_shrink_0()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(16.0))
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .h(px(38.0))
                    .px(px(12.0))
                    .rounded_lg()
                    .bg(theme::panel_deep())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .child(ui::pill(
                        "@all",
                        theme::accent(),
                        theme::accent().opacity(0.12),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child("Direct the fleet…  @ to target a run or agent"),
                    )
                    .child(ui::kbd("↵")),
            )
            .child(ui::meta(format!("{agents} agents")))
    }
}

impl Render for FleetScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (nodes, live) = self.nodes(cx);
        let agents = self.state.read(cx).installed_agents.len();

        let active = nodes.iter().filter(|n| n.kind == NodeKind::Active).count();
        let needs = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Failed | NodeKind::Review))
            .count();
        let merged = nodes.iter().filter(|n| n.kind == NodeKind::Merged).count();

        // The node the inspector focuses on: review gate first, else a
        // failing run needing attention.
        let attention = nodes
            .iter()
            .find(|n| n.kind == NodeKind::Review)
            .or_else(|| nodes.iter().find(|n| n.kind == NodeKind::Failed))
            .cloned();

        // Build the stage (trunk + nodes) as one flat layer of absolute
        // children so their coords anchor to the stage box.
        let mut stage_children: Vec<AnyElement> = self.render_trunk();
        for (i, node) in nodes.iter().enumerate() {
            stage_children.extend(self.render_node(node, i, cx));
        }
        let stage = div()
            .relative()
            .w(px(STAGE_W))
            .h(px(STAGE_H))
            .flex_shrink_0()
            .children(stage_children);

        div()
            .v_flex()
            .size_full()
            .child(
                // canvas
                div()
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::panel_deep())
                    .child(stage)
                    .child(self.render_chips(active, needs, merged, live))
                    .children(attention.map(|n| self.render_inspector(n, cx))),
            )
            .child(self.render_command_bar(agents))
    }
}

/// Clearly-labelled sample constellation shown only when there are no
/// real runs (offline / daemon not started). Mirrors the concept's five
/// missions so the idea reads without a live daemon.
fn sample_nodes() -> Vec<FleetNode> {
    vec![
        FleetNode {
            id: "r-01aa".into(),
            title: "OAuth token refresh".into(),
            meta: "verifier-1 · +98 −12".into(),
            kind: NodeKind::Merged,
            live: false,
        },
        FleetNode {
            id: "r-4f2a".into(),
            title: "Session cache".into(),
            meta: "claude-1 · +214 −40".into(),
            kind: NodeKind::Merged,
            live: false,
        },
        FleetNode {
            id: "r-77b0".into(),
            title: "Config loader refactor".into(),
            meta: "claude-1 · qa 3/6".into(),
            kind: NodeKind::Failed,
            live: false,
        },
        FleetNode {
            id: "r-b2e8".into(),
            title: "Retry logic patch".into(),
            meta: "gpt-runner · qa 5/6".into(),
            kind: NodeKind::Active,
            live: false,
        },
        FleetNode {
            id: "r-9c1e".into(),
            title: "Rate limiter middleware".into(),
            meta: "claude-1 · +142 −18".into(),
            kind: NodeKind::Review,
            live: false,
        },
    ]
}
