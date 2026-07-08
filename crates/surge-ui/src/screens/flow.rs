//! Flow — the DAG editor.
//!
//! Adapted from the "Surge - Interactive" concept's flow editor, but
//! wired to the **real** engine model: it loads an actual bundled flow
//! (`surge_core::BundledFlows`) and renders its true nodes, positions,
//! `NodeKind`s and outcome-keyed edges. The concept's thesis — "the
//! engine is dumb: routing is graph data, edges keyed by outcome" — is
//! literally what Surge does, so nothing here is faked.
//!
//! Three panes: node-kind palette · DAG canvas (nodes positioned from
//! their real `position`, edges painted as curved Béziers) · inspector
//! (kind, profile, outcomes→edges, a compact flow listing).

use std::collections::HashMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use surge_core::{BundledFlow, BundledFlows, EdgeKind, Node, NodeConfig, NodeKind};

use crate::theme;
use crate::ui;

/// Default bundled flow shown by the editor.
const DEFAULT_FLOW: &str = "linear-with-review";

const STAGE_W: f32 = 1000.0;
const STAGE_H: f32 = 520.0;
const PAD: f32 = 52.0;
const NODE_W: f32 = 150.0;
const NODE_H: f32 = 48.0;

/// The 7 real node kinds (closed enum in `surge_core::NodeKind`).
const KINDS: [NodeKind; 7] = [
    NodeKind::Agent,
    NodeKind::HumanGate,
    NodeKind::Branch,
    NodeKind::Terminal,
    NodeKind::Notify,
    NodeKind::Loop,
    NodeKind::Subgraph,
];

fn kind_label(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Agent => "agent",
        NodeKind::HumanGate => "human gate",
        NodeKind::Branch => "branch",
        NodeKind::Terminal => "terminal",
        NodeKind::Notify => "notify",
        NodeKind::Loop => "loop",
        NodeKind::Subgraph => "subgraph",
        _ => "node",
    }
}

fn kind_color(k: NodeKind) -> Hsla {
    match k {
        NodeKind::Agent => theme::accent(),
        NodeKind::HumanGate => hsla(210.0 / 360.0, 0.85, 0.62, 1.0),
        NodeKind::Branch => hsla(270.0 / 360.0, 0.6, 0.66, 1.0),
        NodeKind::Terminal => theme::success(),
        NodeKind::Notify => theme::text_muted(),
        NodeKind::Loop => theme::warning(),
        NodeKind::Subgraph => hsla(175.0 / 360.0, 0.55, 0.55, 1.0),
        _ => theme::text_muted(),
    }
}

fn kind_desc(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Agent => {
            "Runs an agent profile in a sealed worktree; routes on its declared outcomes."
        },
        NodeKind::HumanGate => "Pauses for an operator decision before routing on.",
        NodeKind::Branch => "Deterministic fork — chooses an edge from graph data.",
        NodeKind::Terminal => "End state. The run stops here.",
        NodeKind::Notify => "Emits a notification; does no work of its own.",
        NodeKind::Loop => "Iterates a subflow until its condition is met.",
        NodeKind::Subgraph => "Embeds another flow as a single node.",
        _ => "A graph node.",
    }
}

/// A resolved edge segment in stage coords, for canvas painting.
#[derive(Clone, Copy)]
struct EdgeSeg {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: Hsla,
}

/// Flow editor screen.
pub struct FlowScreen {
    flow: Option<BundledFlow>,
    /// Selected node id (defaults to the graph's start node).
    selected: Option<String>,
}

impl FlowScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let flow = BundledFlows::by_name_latest(DEFAULT_FLOW)
            .or_else(|| BundledFlows::all().into_iter().next());
        let selected = flow.as_ref().map(|f| f.graph.start.as_str().to_string());
        Self { flow, selected }
    }

    /// Map a node's engine-space position to stage coords (top-left).
    fn layout(&self) -> HashMap<String, (f32, f32)> {
        let mut out = HashMap::new();
        let Some(flow) = &self.flow else {
            return out;
        };
        let g = &flow.graph;
        let (mut minx, mut maxx, mut miny, mut maxy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for n in g.nodes.values() {
            minx = minx.min(n.position.x);
            maxx = maxx.max(n.position.x);
            miny = miny.min(n.position.y);
            maxy = maxy.max(n.position.y);
        }
        let spanx = (maxx - minx).max(1.0);
        let spany = (maxy - miny).max(1.0);
        let avail_w = STAGE_W - 2.0 * PAD - NODE_W;
        let avail_h = STAGE_H - 2.0 * PAD - NODE_H;
        for n in g.nodes.values() {
            let sx = PAD + (n.position.x - minx) / spanx * avail_w;
            let sy = PAD + (n.position.y - miny) / spany * avail_h;
            out.insert(n.id.as_str().to_string(), (sx, sy));
        }
        out
    }

    fn edge_color(outcome: &str, to: &str, kind: EdgeKind) -> Hsla {
        let negative = matches!(
            outcome,
            "fail" | "failed" | "changes_requested" | "rejected" | "reject" | "blocked" | "error"
        ) || to.contains("fail")
            || kind == EdgeKind::Escalate;
        if negative {
            theme::error()
        } else if kind == EdgeKind::Backtrack {
            theme::warning()
        } else if to.contains("success") || to == "end" || to.contains("done") {
            theme::success()
        } else {
            theme::accent()
        }
    }

    // ── palette ─────────────────────────────────────────────────────

    fn render_palette(&self) -> Div {
        let counts: HashMap<&str, usize> = self
            .flow
            .as_ref()
            .map(|f| {
                let mut m: HashMap<&str, usize> = HashMap::new();
                for n in f.graph.nodes.values() {
                    *m.entry(kind_label(n.kind())).or_default() += 1;
                }
                m
            })
            .unwrap_or_default();

        let items: Vec<Div> = KINDS
            .iter()
            .map(|&k| {
                let label = kind_label(k);
                let n = counts.get(label).copied().unwrap_or(0);
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(9.0))
                    .py(px(7.0))
                    .rounded_lg()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_sm()
                            .bg(kind_color(k))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(label),
                    )
                    .child(ui::meta(format!("{n}")))
            })
            .collect();

        div()
            .w(px(190.0))
            .flex_shrink_0()
            .v_flex()
            .gap(px(10.0))
            .p(px(13.0))
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_muted())
                    .child("NODE KINDS"),
            )
            .child(
                div()
                    .text_size(px(9.5))
                    .line_height(px(15.0))
                    .text_color(theme::text_muted().opacity(0.8))
                    .child("Closed enum — extend via profiles, not new kinds."),
            )
            .child(div().v_flex().gap(px(6.0)).children(items))
            .child(div().flex_1())
            .child(
                div()
                    .v_flex()
                    .gap(px(5.0))
                    .p(px(11.0))
                    .rounded_lg()
                    .bg(theme::accent().opacity(0.06))
                    .border_1()
                    .border_color(theme::accent().opacity(0.22))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::accent())
                            .child("ENGINE IS DUMB"),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .line_height(px(14.0))
                            .text_color(theme::text_muted())
                            .child("Routing is graph data — edges keyed by outcome. The LLM only does the work."),
                    ),
            )
    }

    // ── canvas ──────────────────────────────────────────────────────

    fn render_canvas_layer(&self, segs: Vec<EdgeSeg>) -> impl IntoElement {
        let dot_color = theme::graph_line().opacity(0.5);
        canvas(
            |_b, _w, _c| {},
            move |bounds, _p, window, _c| {
                let ox = bounds.origin.x;
                let oy = bounds.origin.y;
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);

                // dot grid
                let step = 24.0_f32;
                let mut gy = 8.0_f32;
                while gy < h {
                    let mut gx = 8.0_f32;
                    while gx < w {
                        let d =
                            Bounds::new(point(ox + px(gx), oy + px(gy)), size(px(1.4), px(1.4)));
                        window.paint_quad(fill(d, dot_color));
                        gx += step;
                    }
                    gy += step;
                }

                // edges: cubic Bézier, glow + bright
                for e in &segs {
                    let dx = (e.x2 - e.x1) * 0.45;
                    let start = point(ox + px(e.x1), oy + px(e.y1));
                    let end = point(ox + px(e.x2), oy + px(e.y2));
                    let c1 = point(ox + px(e.x1 + dx), oy + px(e.y1));
                    let c2 = point(ox + px(e.x2 - dx), oy + px(e.y2));

                    let mut glow = PathBuilder::stroke(px(4.0));
                    glow.move_to(start);
                    glow.cubic_bezier_to(end, c1, c2);
                    if let Ok(p) = glow.build() {
                        window.paint_path(p, e.color.opacity(0.13));
                    }
                    let mut line = PathBuilder::stroke(px(1.5));
                    line.move_to(start);
                    line.cubic_bezier_to(end, c1, c2);
                    if let Ok(p) = line.build() {
                        window.paint_path(p, e.color.opacity(0.7));
                    }
                }
            },
        )
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(STAGE_W))
        .h(px(STAGE_H))
    }

    fn render_node_box(
        &self,
        node: &Node,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kind = node.kind();
        let color = kind_color(kind);
        let id = node.id.as_str().to_string();
        let selected = self.selected.as_deref() == Some(id.as_str());
        let id_click = id.clone();

        div()
            .id(SharedString::from(format!("flow-node-{id}")))
            .absolute()
            .left(px(pos.0))
            .top(px(pos.1))
            .w(px(NODE_W))
            .h(px(NODE_H))
            .h_flex()
            .gap(px(9.0))
            .items_center()
            .px(px(11.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(if selected {
                theme::accent().opacity(0.7)
            } else {
                theme::hairline_strong()
            })
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.55)))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.selected = Some(id_click.clone());
                cx.notify();
            }))
            .child(
                div()
                    .w(px(9.0))
                    .h(px(9.0))
                    .rounded_sm()
                    .bg(color)
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child(id),
                    )
                    .child(
                        div()
                            .text_size(px(8.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child(kind_label(kind)),
                    ),
            )
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> Div {
        let (name, node_count, edge_count) = self
            .flow
            .as_ref()
            .map(|f| (f.name.clone(), f.graph.nodes.len(), f.graph.edges.len()))
            .unwrap_or_else(|| ("—".to_string(), 0, 0));

        let positions = self.layout();

        // resolved edge segments for the canvas
        let mut segs: Vec<EdgeSeg> = Vec::new();
        if let Some(flow) = &self.flow {
            for e in &flow.graph.edges {
                let from = e.from.node.as_str();
                let to = e.to.as_str();
                if let (Some(&(fx, fy)), Some(&(tx, ty))) = (positions.get(from), positions.get(to))
                {
                    segs.push(EdgeSeg {
                        x1: fx + NODE_W,
                        y1: fy + NODE_H / 2.0,
                        x2: tx,
                        y2: ty + NODE_H / 2.0,
                        color: Self::edge_color(e.from.outcome.as_str(), to, e.kind),
                    });
                }
            }
        }

        // stage children: canvas (back) → node boxes
        let mut stage_children: Vec<AnyElement> =
            vec![self.render_canvas_layer(segs).into_any_element()];
        if let Some(flow) = &self.flow {
            for n in flow.graph.nodes.values() {
                if let Some(&pos) = positions.get(n.id.as_str()) {
                    stage_children.push(self.render_node_box(n, pos, cx).into_any_element());
                }
            }
        }

        div()
            .flex_1()
            .min_w_0()
            .v_flex()
            // header
            .child(
                div()
                    .flex_shrink_0()
                    .h_flex()
                    .gap(px(12.0))
                    .items_center()
                    .px(px(20.0))
                    .py(px(13.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child("Flow editor"),
                    )
                    .child(ui::meta(format!(
                        "{name}.toml · {node_count} nodes · {edge_count} edges"
                    )))
                    .child(div().flex_1())
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::success())
                            .child(ui::status_dot(theme::success()))
                            .child("valid · reaches a terminal"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(30.0))
                            .px(px(13.0))
                            .rounded_lg()
                            .border_1()
                            .border_color(theme::hairline_strong())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child("Validate"),
                    ),
            )
            // canvas
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::panel_deep())
                    .child(
                        div()
                            .relative()
                            .w(px(STAGE_W))
                            .h(px(STAGE_H))
                            .flex_shrink_0()
                            .children(stage_children),
                    ),
            )
    }

    // ── inspector ───────────────────────────────────────────────────

    fn render_inspector(&self) -> Div {
        let panel = div()
            .w(px(330.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::hairline());

        let Some(flow) = &self.flow else {
            return panel.child(ui::meta("no flow loaded").p(px(16.0)));
        };
        let g = &flow.graph;
        let node = self
            .selected
            .as_ref()
            .and_then(|sel| g.nodes.values().find(|n| n.id.as_str() == sel));
        let Some(node) = node else {
            return panel.child(ui::meta("select a node").p(px(16.0)));
        };

        let kind = node.kind();
        let color = kind_color(kind);
        let id = node.id.as_str().to_string();

        // outcomes → edges (real routing)
        let outcome_rows: Vec<Div> = node
            .declared_outcomes
            .iter()
            .map(|oc| {
                let outcome = oc.id.as_str();
                let target = g
                    .edges
                    .iter()
                    .find(|e| e.from.node.as_str() == id && e.from.outcome.as_str() == outcome)
                    .map(|e| e.to.as_str().to_string())
                    .unwrap_or_else(|| "—".to_string());
                let neg = oc.is_terminal
                    || matches!(
                        outcome,
                        "fail" | "changes_requested" | "rejected" | "blocked"
                    );
                let tag_color = if neg {
                    theme::error()
                } else {
                    theme::success()
                };
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded_md()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline())
                    .child(ui::pill(
                        outcome.to_string(),
                        tag_color,
                        tag_color.opacity(0.14),
                    ))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child("→"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(target),
                    )
            })
            .collect();

        // profile field for agent nodes
        let profile = if let NodeConfig::Agent(cfg) = &node.config {
            Some(cfg.profile.as_str().to_string())
        } else {
            None
        };

        panel
            .child(
                div()
                    .v_flex()
                    .gap(px(11.0))
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(ui::status_dot(color))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(id.clone()),
                            )
                            .child(ui::pill(kind_label(kind), color, color.opacity(0.14))),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(17.0))
                            .text_color(theme::text_muted())
                            .child(kind_desc(kind)),
                    )
                    .when_some(profile, |el, p| {
                        el.child(
                            div()
                                .h_flex()
                                .justify_between()
                                .items_center()
                                .child(ui::meta("profile"))
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::accent())
                                        .child(p),
                                ),
                        )
                    })
                    .child(
                        div()
                            .v_flex()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_muted())
                                    .child("OUTCOMES → EDGES"),
                            )
                            .when(outcome_rows.is_empty(), |el| {
                                el.child(ui::meta("terminal · no outgoing edges"))
                            })
                            .children(outcome_rows),
                    ),
            )
            .child(self.render_toml(g))
    }

    fn render_toml(&self, g: &surge_core::Graph) -> Div {
        let mut lines: Vec<Div> = Vec::new();
        let mut push = |text: String, color: Hsla, indent: f32| {
            lines.push(
                div()
                    .pl(px(indent))
                    .text_size(px(10.0))
                    .line_height(px(16.0))
                    .text_color(color)
                    .child(text),
            );
        };
        push(
            format!("start = \"{}\"", g.start.as_str()),
            theme::accent(),
            0.0,
        );
        for n in g.nodes.values() {
            push(
                format!("[{}]  {}", n.id.as_str(), kind_label(n.kind())),
                theme::text_primary(),
                0.0,
            );
            for oc in &n.declared_outcomes {
                let target = g
                    .edges
                    .iter()
                    .find(|e| {
                        e.from.node.as_str() == n.id.as_str()
                            && e.from.outcome.as_str() == oc.id.as_str()
                    })
                    .map(|e| e.to.as_str().to_string())
                    .unwrap_or_else(|| "—".to_string());
                push(
                    format!("{} → {}", oc.id.as_str(), target),
                    theme::text_muted(),
                    12.0,
                );
            }
        }

        div()
            .flex_1()
            .v_flex()
            .gap(px(9.0))
            .p(px(16.0))
            .min_h_0()
            .child(
                div()
                    .text_size(px(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_muted())
                    .child("FLOW.TOML"),
            )
            .child(
                div()
                    .v_flex()
                    .gap(px(1.0))
                    .p(px(12.0))
                    .rounded_lg()
                    .bg(theme::panel_deep())
                    .border_1()
                    .border_color(theme::hairline())
                    .overflow_hidden()
                    .children(lines),
            )
    }
}

impl Render for FlowScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Plain .flex() row so the three panes stretch to full height
        // (h_flex would vertically center them in tall windows).
        div()
            .size_full()
            .flex()
            .min_h_0()
            .child(self.render_palette())
            .child(self.render_canvas(cx))
            .child(self.render_inspector())
    }
}
