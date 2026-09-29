//! Read-only swimlane diagram of a flow graph (Archify / n8n style).
//!
//! Used where someone must *understand* a plan rather than edit it — the
//! flow approval gate in the Inbox. Positions stored in a generated
//! `flow.toml` are not trustworthy, so layout is computed:
//!
//! - **lanes** (rows) by the role of a step: planning, your approvals,
//!   build, verification, result;
//! - **columns** by execution order from the start node, with a loop's body
//!   placed directly after the loop step;
//! - a loop body is framed ("repeats for each item") across the lanes its
//!   steps occupy;
//! - edges are orthogonal; edges that go backwards are routed underneath.

use std::collections::{BTreeMap, HashMap, VecDeque};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::edge::{Edge, EdgeKind};
use surge_core::graph::Graph;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, NodeKind};
use surge_core::terminal_config::TerminalKind;

use crate::theme;

const NODE_W: f32 = 168.0;
const NODE_H: f32 = 52.0;
const COL_GAP: f32 = 58.0;
const ROW_GAP: f32 = 14.0;
const PAD: f32 = 18.0;
const LANE_LABEL_H: f32 = 24.0;
const LANE_PAD_BOTTOM: f32 = 16.0;
const LANE_GAP: f32 = 10.0;

/// Role lanes, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(crate) enum Lane {
    Plan,
    Approve,
    Build,
    Verify,
    Result,
}

impl Lane {
    fn title(self) -> &'static str {
        match self {
            Self::Plan => "Planning",
            Self::Approve => "Your approvals",
            Self::Build => "Build",
            Self::Verify => "Verification",
            Self::Result => "Result",
        }
    }
}

/// A step flattened out of the graph (top level or a loop body).
struct Step {
    key: NodeKey,
    node: Node,
    col: usize,
    lane: Lane,
    /// Loop steps whose bodies contain this step, outermost first.
    groups: Vec<NodeKey>,
}

#[derive(Debug, Clone)]
struct Placed {
    key: String,
    x: f32,
    y: f32,
    icon: SharedString,
    title: String,
    subtitle: String,
    color: Hsla,
}

#[derive(Clone)]
struct Wire {
    points: Vec<(f32, f32)>,
    color: Hsla,
    label: Option<(f32, f32, String)>,
}

#[derive(Clone)]
struct Band {
    y: f32,
    h: f32,
    title: String,
}

#[derive(Clone)]
struct Frame {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: String,
}

#[derive(Default, Clone)]
struct Diagram {
    nodes: Vec<Placed>,
    wires: Vec<Wire>,
    bands: Vec<Band>,
    frames: Vec<Frame>,
    width: f32,
    height: f32,
}

/// "Why this plan": the archetype the generator chose and its stated reason
/// (`metadata.description`), when present.
pub fn plan_rationale(graph: &Graph) -> Option<String> {
    let shape = graph
        .metadata
        .archetype
        .as_ref()
        .map(|a| humanize(a.name.as_str()));
    let why = graph
        .metadata
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    match (shape, why) {
        (Some(shape), Some(why)) => Some(format!("{shape} — {why}")),
        (Some(shape), None) => Some(shape),
        (None, Some(why)) => Some(why.to_string()),
        (None, None) => None,
    }
}

/// A plan parsed and laid out once. Rendering it is cheap; building it is
/// not (TOML parse + layout), so screens cache this per document instead of
/// rebuilding it every frame — the Inbox re-renders on every caret blink,
/// and rebuilding here per frame pinned the UI thread and dropped typing.
#[derive(Clone)]
pub struct PreparedPlan {
    diagram: std::rc::Rc<Diagram>,
    /// "Why this plan" line, see [`plan_rationale`].
    pub rationale: Option<String>,
    /// Per-step details for the inspector, keyed by node id.
    pub nodes: std::rc::Rc<BTreeMap<String, NodeDetails>>,
}

/// What the inspector shows about one step, straight from the graph.
#[derive(Clone, Debug)]
pub struct NodeDetails {
    pub title: String,
    pub icon: SharedString,
    pub kind: &'static str,
    pub color: Hsla,
    /// `profile@x.y` for agent steps.
    pub profile: Option<String>,
    /// Skills declared on the node itself (on top of the profile's).
    pub node_skills: Vec<String>,
    /// Node-level overrides present (prompt, tools, sandbox, …).
    pub overrides: Vec<&'static str>,
    /// Provider chosen for this step instead of the profile's.
    pub runtime_override: Option<String>,
    /// Model / reasoning level chosen for this step.
    pub model_override: Option<String>,
    pub effort_override: Option<String>,
    /// Declared outcome → next step.
    pub routes: Vec<(String, String)>,
}

impl PreparedPlan {
    /// Parse-free preparation from an already parsed graph.
    pub fn new(graph: &Graph) -> Self {
        Self {
            diagram: std::rc::Rc::new(layout(graph)),
            rationale: plan_rationale(graph),
            nodes: std::rc::Rc::new(node_details(graph)),
        }
    }
}

fn node_details(graph: &Graph) -> BTreeMap<String, NodeDetails> {
    let levels = std::iter::once((&graph.nodes, &graph.edges))
        .chain(graph.subgraphs.values().map(|s| (&s.nodes, &s.edges)));
    let mut out = BTreeMap::new();
    for (nodes, edges) in levels {
        for (key, node) in nodes {
            let (icon, _, color) = describe(node);
            let (runtime_override, model_override, effort_override) = match &node.config {
                NodeConfig::Agent(agent) => (
                    agent.runtime_override().map(str::to_string),
                    agent.model_override().map(str::to_string),
                    agent.effort_override().map(str::to_string),
                ),
                _ => (None, None, None),
            };
            let (profile, node_skills, overrides) = match &node.config {
                NodeConfig::Agent(agent) => {
                    let mut overrides = Vec::new();
                    if agent.prompt_overrides.is_some() {
                        overrides.push("prompt");
                    }
                    if agent.tool_overrides.is_some() {
                        overrides.push("tools / MCP");
                    }
                    if agent.sandbox_override.is_some() {
                        overrides.push("sandbox");
                    }
                    if agent.rules_overrides.is_some() {
                        overrides.push("rules");
                    }
                    let skills = agent
                        .declared_skills()
                        .map(|skills| skills.iter().map(|s| format!("{s:?}")).collect())
                        .unwrap_or_default();
                    (Some(agent.profile.to_string()), skills, overrides)
                },
                _ => (None, Vec::new(), Vec::new()),
            };
            let routes = edges
                .iter()
                .filter(|e| &e.from.node == key)
                .map(|e| {
                    (
                        e.from.outcome.as_str().replace('_', " "),
                        humanize(e.to.as_str()),
                    )
                })
                .collect();
            out.insert(
                key.as_str().to_string(),
                NodeDetails {
                    title: humanize(key.as_str()),
                    icon,
                    kind: kind_label(node),
                    color,
                    profile,
                    node_skills,
                    overrides,
                    runtime_override,
                    model_override,
                    effort_override,
                    routes,
                },
            );
        }
    }
    out
}

fn kind_label(node: &Node) -> &'static str {
    match &node.config {
        NodeConfig::Agent(_) => "Agent step",
        NodeConfig::HumanGate(_) => "Your approval",
        NodeConfig::Branch(_) => "Automatic decision",
        NodeConfig::Notify(_) => "Notification",
        NodeConfig::Loop(_) => "Loop",
        NodeConfig::Subgraph(_) => "Sub-flow",
        NodeConfig::Terminal(_) => "End",
    }
}

/// Click handler for a diagram step (receives the node id).
pub type OnSelect = std::rc::Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Render a [`PreparedPlan`] (no parsing or layout work per frame).
pub fn render_prepared(plan: &PreparedPlan, id: impl Into<ElementId>) -> AnyElement {
    render_interactive(plan, id, None, None)
}

/// [`render_prepared`] with a highlighted step and clickable steps.
pub fn render_interactive(
    plan: &PreparedPlan,
    id: impl Into<ElementId>,
    selected: Option<&str>,
    on_select: Option<OnSelect>,
) -> AnyElement {
    let d = &*plan.diagram;
    let (width, height) = (d.width, d.height);
    let wires: Vec<(Vec<(f32, f32)>, Hsla)> = d
        .wires
        .iter()
        .map(|w| (w.points.clone(), w.color))
        .collect();

    let mut children: Vec<AnyElement> = Vec::new();
    for band in &d.bands {
        children.push(
            div()
                .absolute()
                .left(px(PAD / 2.0))
                .top(px(band.y))
                .w(px(width - PAD))
                .h(px(band.h))
                .rounded_md()
                .border_1()
                .border_dashed()
                .border_color(theme::hairline_strong())
                .child(
                    div()
                        .absolute()
                        .left(px(10.0))
                        .top(px(6.0))
                        .text_size(px(9.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_muted())
                        .child(band.title.clone()),
                )
                .into_any_element(),
        );
    }
    for frame in &d.frames {
        children.push(
            div()
                .absolute()
                .left(px(frame.x))
                .top(px(frame.y))
                .w(px(frame.w))
                .h(px(frame.h))
                .rounded_lg()
                .border_1()
                .border_dashed()
                .border_color(theme::warning().opacity(0.7))
                .bg(theme::warning().opacity(0.04))
                .child(
                    div()
                        .absolute()
                        .left(px(8.0))
                        .top(px(-9.0))
                        .px(px(5.0))
                        .rounded_sm()
                        .bg(theme::panel_deep())
                        .text_size(px(9.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::warning())
                        .child(frame.title.clone()),
                )
                .into_any_element(),
        );
    }
    children.push(
        canvas(
            |_b, _w, _c| {},
            move |bounds, _p, window, _c| {
                let (ox, oy) = (bounds.origin.x, bounds.origin.y);
                let at = |x: f32, y: f32| point(ox + px(x), oy + px(y));
                for (points, color) in &wires {
                    let mut line = PathBuilder::stroke(px(1.5));
                    for (i, (x, y)) in points.iter().enumerate() {
                        if i == 0 {
                            line.move_to(at(*x, *y));
                        } else {
                            line.line_to(at(*x, *y));
                        }
                    }
                    if let Ok(path) = line.build() {
                        window.paint_path(path, color.opacity(0.8));
                    }
                    if let [.., (px0, py0), (x, y)] = points.as_slice() {
                        let (dx, dy) = ((x - px0).signum(), (y - py0).signum());
                        let (bx, by) = (x - dx * 7.0, y - dy * 7.0);
                        let (nx, ny) = (-dy * 4.0, dx * 4.0);
                        let mut head = PathBuilder::fill();
                        head.move_to(at(*x, *y));
                        head.line_to(at(bx + nx, by + ny));
                        head.line_to(at(bx - nx, by - ny));
                        head.close();
                        if let Ok(path) = head.build() {
                            window.paint_path(path, *color);
                        }
                    }
                }
            },
        )
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(width))
        .h(px(height))
        .into_any_element(),
    );
    for wire in &d.wires {
        let Some((x, y, label)) = &wire.label else {
            continue;
        };
        children.push(
            div()
                .absolute()
                .left(px(x - 44.0))
                .top(px(y - 8.0))
                .w(px(88.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(5.0))
                        .rounded_sm()
                        .bg(theme::panel_deep())
                        .text_size(px(9.0))
                        .text_color(wire.color)
                        .child(label.clone()),
                )
                .into_any_element(),
        );
    }
    for node in &d.nodes {
        let is_selected = selected == Some(node.key.as_str());
        children.push(node_box(node.clone(), is_selected, on_select.clone()).into_any_element());
    }

    div()
        .id(id)
        .w_full()
        .overflow_x_scroll()
        .rounded_lg()
        .bg(theme::panel_deep())
        .border_1()
        .border_color(theme::hairline())
        .child(
            div()
                .relative()
                .w(px(width))
                .h(px(height))
                .flex_shrink_0()
                .children(children),
        )
        .into_any_element()
}

fn node_box(node: Placed, selected: bool, on_select: Option<OnSelect>) -> Stateful<Div> {
    let key = node.key.clone();
    div()
        .id(SharedString::from(format!("plan-node-{}", node.key)))
        .role(Role::Button)
        .aria_label(format!("Inspect step {}", node.title))
        .when_some(on_select, move |el, on_select| {
            el.cursor_pointer()
                .on_click(move |_e, window, cx| on_select(key.clone(), window, cx))
        })
        .absolute()
        .left(px(node.x))
        .top(px(node.y))
        .w(px(NODE_W))
        .h(px(NODE_H))
        .flex()
        .items_center()
        .gap(px(9.0))
        .px(px(10.0))
        .rounded_lg()
        .bg(theme::panel_raised())
        .when(selected, |el| el.shadow_md())
        .map(|el| {
            if selected {
                el.border_2()
            } else {
                el.border_1()
            }
        })
        .border_color(if selected {
            theme::accent()
        } else {
            node.color.opacity(0.75)
        })
        .child(
            div()
                .w(px(24.0))
                .h(px(24.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(node.color.opacity(0.16))
                .text_size(px(12.0))
                .text_color(node.color)
                .child(node.icon.clone()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(1.0))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child(node.title),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(9.5))
                        .text_color(theme::text_muted())
                        .child(node.subtitle),
                ),
        )
}

// ── layout ──────────────────────────────────────────────────────────────

fn layout(graph: &Graph) -> Diagram {
    let steps = flatten(graph);
    let mut d = Diagram::default();

    // Lanes actually used, in fixed order.
    let mut lanes: Vec<Lane> = steps.iter().map(|s| s.lane).collect();
    lanes.sort();
    lanes.dedup();

    // Stack depth per (lane, col) decides each lane's height.
    let mut stack: HashMap<(Lane, usize), usize> = HashMap::new();
    let mut row_of: HashMap<NodeKey, usize> = HashMap::new();
    for step in &steps {
        let slot = stack.entry((step.lane, step.col)).or_insert(0);
        row_of.insert(step.key.clone(), *slot);
        *slot += 1;
    }
    let mut lane_top: HashMap<Lane, f32> = HashMap::new();
    let mut y = PAD;
    for (i, lane) in lanes.iter().enumerate() {
        let rows = stack
            .iter()
            .filter(|((l, _), _)| l == lane)
            .map(|(_, n)| *n)
            .max()
            .unwrap_or(1);
        let h = LANE_LABEL_H + rows as f32 * (NODE_H + ROW_GAP) - ROW_GAP + LANE_PAD_BOTTOM;
        lane_top.insert(*lane, y);
        d.bands.push(Band {
            y,
            h,
            title: format!("{:02} / {}", i + 1, lane.title()),
        });
        y += h + LANE_GAP;
    }
    let lanes_bottom = y - LANE_GAP;

    let mut at: HashMap<NodeKey, (f32, f32)> = HashMap::new();
    for step in &steps {
        let x = PAD + step.col as f32 * (NODE_W + COL_GAP);
        let y = lane_top[&step.lane] + LANE_LABEL_H + row_of[&step.key] as f32 * (NODE_H + ROW_GAP);
        at.insert(step.key.clone(), (x, y));
        let (icon, subtitle, color) = describe(&step.node);
        d.nodes.push(Placed {
            key: step.key.as_str().to_string(),
            x,
            y,
            icon,
            title: humanize(step.key.as_str()),
            subtitle,
            color,
        });
        d.width = d.width.max(x + NODE_W);
    }

    // Loop frames around every step nested in them (inner loops included).
    let mut groups: Vec<&NodeKey> = steps.iter().flat_map(|s| s.groups.iter()).collect();
    groups.sort();
    groups.dedup();
    for group in groups {
        let body: Vec<(f32, f32)> = steps
            .iter()
            .filter(|s| s.groups.contains(group))
            .filter_map(|s| at.get(&s.key).copied())
            .collect();
        let (Some(min_x), Some(min_y)) = (
            body.iter().map(|p| p.0).reduce(f32::min),
            body.iter().map(|p| p.1).reduce(f32::min),
        ) else {
            continue;
        };
        let nesting = steps
            .iter()
            .find(|s| &s.key == group)
            .map_or(0, |s| s.groups.len()) as f32;
        let inset = 10.0 - nesting * 3.0;
        let max_x = body.iter().map(|p| p.0).fold(min_x, f32::max) + NODE_W;
        let max_y = body.iter().map(|p| p.1).fold(min_y, f32::max) + NODE_H;
        d.frames.push(Frame {
            x: min_x - inset,
            y: min_y - inset - 2.0,
            w: max_x - min_x + inset * 2.0,
            h: max_y - min_y + inset * 2.0 + 2.0,
            title: format!("↻ Repeats for each item · {}", humanize(group.as_str())),
        });
    }

    // Edges: every edge of the graph and of the loop bodies.
    let mut all_edges: Vec<&Edge> = graph.edges.iter().collect();
    for step in &steps {
        if let Some(body) = body_of(graph, &step.node)
            && at.contains_key(&body.start)
        {
            all_edges.extend(body.edges.iter());
            // Enter the body from the loop step.
            if let (Some(&from), Some(&to)) = (at.get(&step.key), at.get(&body.start)) {
                d.wires.push(route(
                    from,
                    to,
                    theme::warning(),
                    Some("each item".into()),
                    0,
                ));
            }
        }
    }
    let exits: HashMap<&NodeKey, usize> = all_edges.iter().fold(HashMap::new(), |mut m, e| {
        *m.entry(&e.from.node).or_insert(0) += 1;
        m
    });
    let targets: HashMap<&NodeKey, &Node> = steps.iter().map(|s| (&s.key, &s.node)).collect();
    let mut back_lanes = 0;
    for edge in all_edges {
        let (Some(&from), Some(&to)) = (at.get(&edge.from.node), at.get(&edge.to)) else {
            continue;
        };
        let color = edge_color(edge, targets.get(&edge.to).copied());
        let label = (exits.get(&edge.from.node).copied().unwrap_or(0) > 1)
            .then(|| edge.from.outcome.as_str().replace('_', " "));
        let backwards = to.0 <= from.0;
        if backwards {
            back_lanes += 1;
        }
        d.wires.push(route(
            from,
            to,
            color,
            label,
            if backwards { back_lanes } else { 0 },
        ));
    }

    d.width += PAD;
    d.height = lanes_bottom + PAD + back_lanes as f32 * 10.0;
    d
}

/// Orthogonal route from the right edge of `from` to the left edge of `to`;
/// backwards edges drop below both boxes (`back` staggers parallel ones).
fn route(
    from: (f32, f32),
    to: (f32, f32),
    color: Hsla,
    label: Option<String>,
    back: usize,
) -> Wire {
    let (sx, sy) = (from.0 + NODE_W, from.1 + NODE_H / 2.0);
    let (tx, ty) = (to.0, to.1 + NODE_H / 2.0);
    if back == 0 {
        let mid = sx + (tx - sx) / 2.0;
        let points = vec![(sx, sy), (mid, sy), (mid, ty), (tx, ty)];
        let label = label.map(|l| {
            (
                mid,
                (sy + ty) / 2.0 - if (sy - ty).abs() < 1.0 { 9.0 } else { 0.0 },
                l,
            )
        });
        Wire {
            points,
            color,
            label,
        }
    } else {
        let below = from.1.max(to.1) + NODE_H + 8.0 + back as f32 * 6.0;
        let (bx, by) = (from.0 + NODE_W / 2.0, from.1 + NODE_H);
        let (ex, ey) = (to.0 + NODE_W / 2.0, to.1 + NODE_H);
        let points = vec![(bx, by), (bx, below), (ex, below), (ex, ey)];
        let label = label.map(|l| ((bx + ex) / 2.0, below, l));
        Wire {
            points,
            color,
            label,
        }
    }
}

/// Steps in execution order, each loop's body (recursively) inserted right
/// after its loop step. Columns: breadth-first depth within a level, shifted
/// so a loop's body gets its own columns before the steps that follow it.
fn flatten(graph: &Graph) -> Vec<Step> {
    let mut steps = Vec::new();
    place(
        graph,
        &graph.start,
        &graph.nodes,
        &graph.edges,
        0,
        &[],
        &mut steps,
        0,
    );
    steps
}

/// Columns a level needs: per depth, one for its steps plus the widest
/// loop body started at that depth.
fn span(
    graph: &Graph,
    start: &NodeKey,
    nodes: &BTreeMap<NodeKey, Node>,
    edges: &[Edge],
    level: usize,
) -> usize {
    let depth = bfs_depth(start, nodes, edges);
    let max_depth = depth.values().copied().max().unwrap_or(0);
    let unreachable = nodes.keys().any(|k| !depth.contains_key(k));
    (0..=max_depth + usize::from(unreachable))
        .map(|d| 1 + widest_body(graph, nodes, &depth, d, max_depth, level))
        .sum()
}

fn widest_body(
    graph: &Graph,
    nodes: &BTreeMap<NodeKey, Node>,
    depth: &HashMap<NodeKey, usize>,
    d: usize,
    max_depth: usize,
    level: usize,
) -> usize {
    if level >= MAX_NESTING {
        return 0;
    }
    nodes
        .iter()
        .filter(|(k, _)| depth.get(*k).copied().unwrap_or(max_depth + 1) == d)
        .filter_map(|(_, n)| body_of(graph, n))
        .map(|b| span(graph, &b.start, &b.nodes, &b.edges, level + 1))
        .max()
        .unwrap_or(0)
}

/// Loops nested deeper than this are drawn as a single step.
const MAX_NESTING: usize = 4;

fn body_of<'g>(graph: &'g Graph, node: &Node) -> Option<&'g surge_core::graph::Subgraph> {
    match &node.config {
        NodeConfig::Loop(l) => graph.subgraphs.get(&l.body),
        NodeConfig::Subgraph(s) => graph.subgraphs.get(&s.inner),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn place(
    graph: &Graph,
    start: &NodeKey,
    nodes: &BTreeMap<NodeKey, Node>,
    edges: &[Edge],
    base_col: usize,
    ancestors: &[NodeKey],
    out: &mut Vec<Step>,
    level: usize,
) {
    let depth = bfs_depth(start, nodes, edges);
    let max_depth = depth.values().copied().max().unwrap_or(0);
    let mut ordered: Vec<(&NodeKey, usize)> = nodes
        .keys()
        .map(|k| (k, depth.get(k).copied().unwrap_or(max_depth + 1)))
        .collect();
    ordered.sort_by_key(|(k, d)| (*d, (*k).clone()));

    let mut col_of_depth: HashMap<usize, usize> = HashMap::new();
    let mut cursor = base_col;
    for d in 0..=max_depth + 1 {
        col_of_depth.insert(d, cursor);
        cursor += 1 + widest_body(graph, nodes, &depth, d, max_depth, level);
    }

    for (key, d) in ordered {
        let node = &nodes[key];
        let col = col_of_depth[&d];
        out.push(Step {
            key: key.clone(),
            node: node.clone(),
            col,
            lane: lane_of(node),
            groups: ancestors.to_vec(),
        });
        if level < MAX_NESTING
            && let Some(body) = body_of(graph, node)
        {
            let mut inner = ancestors.to_vec();
            inner.push(key.clone());
            place(
                graph,
                &body.start,
                &body.nodes,
                &body.edges,
                col + 1,
                &inner,
                out,
                level + 1,
            );
        }
    }
}

fn bfs_depth(
    start: &NodeKey,
    nodes: &BTreeMap<NodeKey, Node>,
    edges: &[Edge],
) -> HashMap<NodeKey, usize> {
    let mut depth = HashMap::new();
    let mut queue = VecDeque::from([(start.clone(), 0usize)]);
    while let Some((key, d)) = queue.pop_front() {
        if !nodes.contains_key(&key) || depth.contains_key(&key) {
            continue;
        }
        depth.insert(key.clone(), d);
        for edge in edges
            .iter()
            .filter(|e| e.from.node == key && e.kind != EdgeKind::Backtrack)
        {
            queue.push_back((edge.to.clone(), d + 1));
        }
    }
    depth
}

pub(crate) fn lane_of(node: &Node) -> Lane {
    match &node.config {
        NodeConfig::HumanGate(_) => Lane::Approve,
        NodeConfig::Terminal(_) | NodeConfig::Notify(_) => Lane::Result,
        NodeConfig::Loop(_) | NodeConfig::Subgraph(_) => Lane::Build,
        NodeConfig::Branch(_) => Lane::Plan,
        NodeConfig::Agent(agent) => {
            let profile = agent.profile.to_string().to_lowercase();
            if ["verif", "review", "test", "qa", "security", "audit"]
                .iter()
                .any(|k| profile.contains(k))
            {
                Lane::Verify
            } else if ["implement", "fix", "refactor", "migrat", "build", "code"]
                .iter()
                .any(|k| profile.contains(k))
            {
                Lane::Build
            } else if profile.contains("pr-") || profile.contains("release") {
                Lane::Result
            } else {
                Lane::Plan
            }
        },
    }
}

fn describe(node: &Node) -> (SharedString, String, Hsla) {
    let (icon, subtitle, color) = describe_kind(node);
    match &node.config {
        NodeConfig::Agent(agent) => match profile_look(agent.profile.as_str()) {
            Some(look) => (
                look.icon
                    .map_or(SharedString::from(icon), SharedString::from),
                subtitle,
                look.color.unwrap_or(color),
            ),
            None => (icon.into(), subtitle, color),
        },
        _ => (icon.into(), subtitle, color),
    }
}

/// Raw `(icon, #RRGGBB colour)` a bundled role declares.
type RoleLook = (Option<String>, Option<String>);

/// Display metadata a bundled profile declares for its role.
struct ProfileLook {
    icon: Option<String>,
    color: Option<Hsla>,
}

/// The role's own icon and colour for a `name@version` profile reference, so
/// the diagram tells reviewers from implementers at a glance.
fn profile_look(profile_ref: &str) -> Option<ProfileLook> {
    use std::sync::OnceLock;
    static LOOKS: OnceLock<HashMap<String, RoleLook>> = OnceLock::new();
    let looks = LOOKS.get_or_init(|| {
        surge_core::profile::bundled::BundledRegistry::all()
            .into_iter()
            .map(|p| (p.role.id.as_str().to_string(), (p.role.icon, p.role.color)))
            .collect()
    });
    let name = profile_ref.split('@').next().unwrap_or(profile_ref);
    let (icon, color) = looks.get(name)?;
    Some(ProfileLook {
        icon: icon.clone(),
        color: color.as_deref().and_then(parse_hex_color),
    })
}

/// `#RRGGBB` to a colour; anything else is ignored rather than guessed.
fn parse_hex_color(hex: &str) -> Option<Hsla> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    u32::from_str_radix(digits, 16)
        .ok()
        .map(|rgb_value| rgb(rgb_value).into())
}

fn describe_kind(node: &Node) -> (&'static str, String, Hsla) {
    match &node.config {
        NodeConfig::Agent(agent) => {
            let profile = agent.profile.to_string();
            let name = profile
                .split('@')
                .next()
                .unwrap_or(&profile)
                .replace('-', " ");
            ("✦", format!("agent · {name}"), kind_color(NodeKind::Agent))
        },
        NodeConfig::HumanGate(_) => ("✋", "you decide".into(), kind_color(NodeKind::HumanGate)),
        NodeConfig::Branch(_) => (
            "⑂",
            "automatic decision".into(),
            kind_color(NodeKind::Branch),
        ),
        NodeConfig::Notify(_) => ("✉", "notification".into(), kind_color(NodeKind::Notify)),
        NodeConfig::Loop(_) => ("↻", "repeats per item".into(), kind_color(NodeKind::Loop)),
        NodeConfig::Subgraph(_) => ("⧉", "sub-flow".into(), kind_color(NodeKind::Subgraph)),
        NodeConfig::Terminal(t) => match t.kind {
            TerminalKind::Success => ("✓", "done".into(), theme::success()),
            TerminalKind::Failure { .. } => ("✕", "stops as failed".into(), theme::error()),
            TerminalKind::Aborted => ("✕", "stops as aborted".into(), theme::error()),
        },
    }
}

fn edge_color(edge: &Edge, target: Option<&Node>) -> Hsla {
    let to_failure = target.is_some_and(|n| {
        matches!(&n.config, NodeConfig::Terminal(t) if !matches!(t.kind, TerminalKind::Success))
    });
    if to_failure || edge.kind == EdgeKind::Escalate {
        theme::error()
    } else if edge.kind == EdgeKind::Backtrack {
        theme::warning()
    } else {
        theme::text_muted()
    }
}

/// `spec_1` → "Spec 1", `implement_feature` → "Implement feature".
fn humanize(id: &str) -> String {
    let spaced = id.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn kind_color(kind: NodeKind) -> Hsla {
    crate::screens::flow::kind_color(kind)
}

#[cfg(test)]
mod tests {
    use super::{Lane, NODE_H, NODE_W, flatten, humanize, layout, parse_hex_color, profile_look};

    fn graphs() -> Vec<(&'static str, surge_core::graph::Graph)> {
        [
            (
                "bug_fix",
                include_str!("../../../examples/flow_bug_fix.toml"),
            ),
            (
                "refactor",
                include_str!("../../../examples/flow_refactor.toml"),
            ),
            ("spike", include_str!("../../../examples/flow_spike.toml")),
            // A real Claude-generated plan: milestone loop → task loop.
            ("nested", include_str!("../testdata/nested_loops_flow.toml")),
        ]
        .into_iter()
        .map(|(name, src)| (name, toml::from_str(src).expect("example parses")))
        .collect()
    }

    #[test]
    fn every_step_is_drawn_once_without_overlap() {
        for (name, graph) in graphs() {
            let steps = flatten(&graph);
            assert!(steps.len() >= graph.nodes.len(), "{name}: top level drawn");
            let d = layout(&graph);
            assert_eq!(d.nodes.len(), steps.len(), "{name}");
            for (i, a) in d.nodes.iter().enumerate() {
                for b in d.nodes.iter().skip(i + 1) {
                    let apart = (a.x - b.x).abs() >= NODE_W || (a.y - b.y).abs() >= NODE_H;
                    assert!(apart, "{name}: {} overlaps {}", a.title, b.title);
                }
            }
            assert!(d.width > 0.0 && d.height > 0.0);
        }
    }

    #[test]
    fn start_step_is_leftmost_and_lanes_follow_roles() {
        for (name, graph) in graphs() {
            let steps = flatten(&graph);
            let start = steps
                .iter()
                .find(|s| s.key == graph.start)
                .expect("start drawn");
            assert_eq!(start.col, 0, "{name}");
            for s in &steps {
                if matches!(s.node.config, surge_core::node::NodeConfig::HumanGate(_)) {
                    assert_eq!(s.lane, Lane::Approve, "{name}");
                }
            }
        }
    }

    #[test]
    fn nested_loop_bodies_are_expanded_and_framed() {
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).expect("parses");
        let steps = flatten(&graph);
        let agents = steps
            .iter()
            .filter(|s| matches!(s.node.config, surge_core::node::NodeConfig::Agent(_)))
            .count();
        assert!(agents > 0, "agents inside the inner loop must be drawn");
        assert!(
            steps.iter().any(|s| s.groups.len() >= 2),
            "two levels of nesting"
        );
        let d = layout(&graph);
        assert!(d.frames.len() >= 2, "a frame per loop");
    }

    #[test]
    fn identifiers_read_as_words() {
        assert_eq!(humanize("spec_1"), "Spec 1");
        assert_eq!(humanize("final-verify"), "Final verify");
    }

    #[test]
    fn bundled_roles_bring_their_own_icon_and_colour() {
        let look = profile_look("security-reviewer@1.0").expect("bundled role");
        assert!(look.icon.is_some());
        assert!(look.color.is_some());
        assert!(profile_look("no-such-role@1.0").is_none());
    }

    #[test]
    fn hex_colours_are_parsed_strictly() {
        assert!(parse_hex_color("#EF4444").is_some());
        assert!(parse_hex_color("EF4444").is_none());
        assert!(parse_hex_color("#EF44").is_none());
        assert!(parse_hex_color("#GGGGGG").is_none());
    }
}
