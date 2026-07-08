//! Memory — the project-memory knowledge graph.
//!
//! Adapted from the "Surge - Interactive" concept, grounded in Surge's
//! **real** memory model (`surge_persistence::memory`): four categories
//! — Discovery, Pattern, Gotcha, FileContext — carrying tags and run /
//! spec provenance, indexed by FTS5.
//!
//! LIVE SEARCH: the search bar queries the real `MemoryStore`
//! (`~/.surge/memory.db`, SQLite + FTS5) via `search_all` — results
//! replace the graph with actual memories including run/spec
//! provenance. The store exposes FTS search but no "list all", so with
//! an empty query the screen shows a clearly-labelled **preview**
//! dataset in the real category vocabulary. Relations are derived from
//! shared tags (a real signal), not faked wiki-links.

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::input::{Input, InputEvent, InputState};
use surge_persistence::memory::MemoryStore;

use crate::theme;
use crate::ui;

const STAGE_W: f32 = 820.0;
const STAGE_H: f32 = 560.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemKind {
    Discovery,
    Pattern,
    Gotcha,
    FileContext,
}

impl MemKind {
    fn color(self) -> Hsla {
        match self {
            Self::Discovery => theme::accent(),
            Self::Pattern => hsla(210.0 / 360.0, 0.85, 0.62, 1.0),
            Self::Gotcha => theme::error(),
            Self::FileContext => hsla(175.0 / 360.0, 0.55, 0.55, 1.0),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Pattern => "pattern",
            Self::Gotcha => "gotcha",
            Self::FileContext => "file context",
        }
    }

    fn group(self) -> &'static str {
        match self {
            Self::Discovery => "DISCOVERIES",
            Self::Pattern => "PATTERNS",
            Self::Gotcha => "GOTCHAS",
            Self::FileContext => "FILE CONTEXT",
        }
    }
}

#[derive(Clone)]
struct MemNode {
    kind: MemKind,
    title: String,
    desc: String,
    tags: Vec<String>,
    /// Provenance — which run / spec seeded or wrote this.
    seeded: Vec<String>,
}

/// A painted relation edge between two memory nodes (shared tag).
#[derive(Clone, Copy)]
struct EdgeSeg {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    strong: bool,
}

pub struct MemoryScreen {
    nodes: Vec<MemNode>,
    selected: usize,
    /// Real FTS5 store at `~/.surge/memory.db` (None if unopenable).
    store: Option<MemoryStore>,
    search_input: Option<Entity<InputState>>,
    /// Current nodes come from a live store query (vs preview set).
    live: bool,
    query: String,
}

impl MemoryScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let store = MemoryStore::default_path()
            .and_then(|p| MemoryStore::open(&p))
            .map_err(|e| tracing::info!("memory store unavailable: {e}"))
            .ok();
        Self {
            nodes: sample_nodes(),
            selected: 0,
            store,
            search_input: None,
            live: false,
            query: String::new(),
        }
    }

    /// Run the FTS query against the real store; empty query restores
    /// the labelled preview set.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self
            .search_input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.query = query.clone();
        self.selected = 0;

        let Some(store) = &self.store else {
            self.nodes = sample_nodes();
            self.live = false;
            cx.notify();
            return;
        };
        if query.is_empty() {
            self.nodes = sample_nodes();
            self.live = false;
            cx.notify();
            return;
        }

        let prov = |task: &Option<surge_core::TaskId>, spec: &Option<surge_core::SpecId>| {
            let mut v = Vec::new();
            if let Some(t) = task {
                v.push(format!("task {}", t.short().to_lowercase()));
            }
            if let Some(s) = spec {
                v.push(format!("spec {}", s.short().to_lowercase()));
            }
            v
        };

        match store.search_all(&query, Some(6)) {
            Ok(results) => {
                let mut nodes = Vec::new();
                for d in results.discoveries {
                    nodes.push(MemNode {
                        kind: MemKind::Discovery,
                        title: d.title,
                        desc: d.content,
                        tags: d.tags,
                        seeded: prov(&d.task_id, &d.spec_id),
                    });
                }
                for p in results.patterns {
                    nodes.push(MemNode {
                        kind: MemKind::Pattern,
                        title: p.name,
                        desc: p.description,
                        tags: p.tags,
                        seeded: prov(&p.task_id, &p.spec_id),
                    });
                }
                for g in results.gotchas {
                    let desc = match &g.symptom {
                        Some(sym) => format!("{sym} → {}", g.solution),
                        None => format!("{} → {}", g.description, g.solution),
                    };
                    nodes.push(MemNode {
                        kind: MemKind::Gotcha,
                        title: g.title,
                        desc,
                        tags: g.tags,
                        seeded: prov(&g.task_id, &g.spec_id),
                    });
                }
                for f in results.file_contexts {
                    nodes.push(MemNode {
                        kind: MemKind::FileContext,
                        title: f.file_path,
                        desc: f.summary,
                        // FileContext carries key APIs, not tags — real signal.
                        tags: f.key_apis.into_iter().take(4).collect(),
                        seeded: prov(&f.task_id, &f.spec_id),
                    });
                }
                self.nodes = nodes;
                self.live = true;
            },
            Err(e) => {
                tracing::warn!("memory search failed: {e}");
            },
        }
        cx.notify();
    }

    /// Search bar + honest source pill, above the three panes.
    fn render_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.search_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Search project memory — FTS5 over ~/.surge/memory.db…")
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                        this.run_search(cx);
                    }
                },
            )
            .detach();
            self.search_input = Some(input);
        }

        let pill = if self.store.is_none() {
            ui::pill(
                "no memory.db yet — runs write it",
                theme::text_muted(),
                theme::panel_raised(),
            )
        } else if self.live {
            ui::pill(
                format!("memory.db · {} matches", self.nodes.len()),
                theme::success(),
                theme::success().opacity(0.12),
            )
        } else {
            ui::pill(
                "preview · type to search the real store",
                theme::text_muted(),
                theme::panel_raised(),
            )
        };

        div()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(14.0))
            .py(px(9.0))
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .h(px(32.0))
                    .px(px(11.0))
                    .rounded_lg()
                    .bg(theme::panel_deep())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .child(
                        div().flex_1().child(
                            Input::new(self.search_input.as_ref().unwrap()).appearance(false),
                        ),
                    ),
            )
            .child(pill)
    }

    /// Circular layout — node centers in stage coords.
    fn positions(&self) -> Vec<(f32, f32)> {
        let n = self.nodes.len().max(1);
        let cx0 = STAGE_W / 2.0;
        let cy0 = STAGE_H / 2.0;
        let r = 198.0;
        (0..self.nodes.len())
            .map(|i| {
                let a = -PI / 2.0 + i as f32 * TAU / n as f32;
                (cx0 + a.cos() * r, cy0 + a.sin() * r)
            })
            .collect()
    }

    fn shares_tag(a: &MemNode, b: &MemNode) -> bool {
        a.tags.iter().any(|t| b.tags.contains(t))
    }

    fn relation_count(&self) -> usize {
        let mut c = 0;
        for i in 0..self.nodes.len() {
            for j in (i + 1)..self.nodes.len() {
                if Self::shares_tag(&self.nodes[i], &self.nodes[j]) {
                    c += 1;
                }
            }
        }
        c
    }

    // ── left list ───────────────────────────────────────────────────

    fn render_list(&self, cx: &mut Context<Self>) -> Div {
        // group node indices by kind, preserving kind order
        let order = [
            MemKind::Discovery,
            MemKind::Pattern,
            MemKind::Gotcha,
            MemKind::FileContext,
        ];
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (ki, k) in order.iter().enumerate() {
            for (i, n) in self.nodes.iter().enumerate() {
                if n.kind == *k {
                    groups.entry(ki).or_default().push(i);
                }
            }
        }

        let mut sections: Vec<Div> = Vec::new();
        for (ki, idxs) in &groups {
            let kind = order[*ki];
            let items: Vec<Stateful<Div>> = idxs
                .iter()
                .map(|&i| {
                    let selected = i == self.selected;
                    let color = self.nodes[i].kind.color();
                    let title = self.nodes[i].title.clone();
                    div()
                        .id(SharedString::from(format!("mem-row-{i}")))
                        .h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |el| el.bg(theme::accent().opacity(0.1)))
                        .hover(|s: StyleRefinement| s.bg(theme::panel_raised()))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.selected = i;
                            cx.notify();
                        }))
                        .child(ui::status_dot(color))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(11.0))
                                .text_color(if selected {
                                    theme::text_primary()
                                } else {
                                    theme::text_muted()
                                })
                                .child(title),
                        )
                })
                .collect();

            sections.push(
                div()
                    .v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .text_size(px(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_muted().opacity(0.7))
                            .child(kind.group()),
                    )
                    .children(items),
            );
        }

        div()
            .w(px(236.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .v_flex()
                    .gap(px(3.0))
                    .p(px(14.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child("Project memory"),
                    )
                    .child(ui::meta(format!(
                        "{} memories · {} relations · reseeds run context",
                        self.nodes.len(),
                        self.relation_count()
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .v_flex()
                    .gap(px(10.0))
                    .px(px(8.0))
                    .pb(px(10.0))
                    .children(sections),
            )
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(14.0))
                    .py(px(11.0))
                    .border_t_1()
                    .border_color(theme::hairline())
                    .child(ui::meta("Search memory graph…")),
            )
    }

    // ── center graph ────────────────────────────────────────────────

    fn render_canvas_layer(&self, segs: Vec<EdgeSeg>) -> impl IntoElement {
        let dot_color = theme::graph_line().opacity(0.45);
        canvas(
            |_b, _w, _c| {},
            move |bounds, _p, window, _c| {
                let ox = bounds.origin.x;
                let oy = bounds.origin.y;
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);

                // dot grid
                let step = 26.0_f32;
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

                // relation edges (straight lines); highlighted ones on top
                for pass in [false, true] {
                    for e in segs.iter().filter(|e| e.strong == pass) {
                        let color = if e.strong {
                            theme::accent().opacity(0.55)
                        } else {
                            theme::graph_line().opacity(0.8)
                        };
                        let mut line = PathBuilder::stroke(px(if e.strong { 1.6 } else { 1.0 }));
                        line.move_to(point(ox + px(e.x1), oy + px(e.y1)));
                        line.line_to(point(ox + px(e.x2), oy + px(e.y2)));
                        if let Ok(p) = line.build() {
                            window.paint_path(p, color);
                        }
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

    fn render_node_chip(
        &self,
        i: usize,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let node = &self.nodes[i];
        let selected = i == self.selected;
        let color = node.kind.color();
        let title = node.title.clone();
        div()
            .id(SharedString::from(format!("mem-node-{i}")))
            .absolute()
            .left(px(pos.0 - 82.0))
            .top(px(pos.1 - 15.0))
            .w(px(164.0))
            .h_flex()
            .gap(px(7.0))
            .items_center()
            .px(px(9.0))
            .py(px(6.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(if selected {
                theme::accent().opacity(0.7)
            } else {
                theme::hairline_strong()
            })
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.5)))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.selected = i;
                cx.notify();
            }))
            .child(ui::status_dot(color))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_primary())
                    .child(title),
            )
    }

    fn render_graph(&self, cx: &mut Context<Self>) -> Div {
        let positions = self.positions();

        // relation edges
        let mut segs: Vec<EdgeSeg> = Vec::new();
        for i in 0..self.nodes.len() {
            for j in (i + 1)..self.nodes.len() {
                if Self::shares_tag(&self.nodes[i], &self.nodes[j]) {
                    let (x1, y1) = positions[i];
                    let (x2, y2) = positions[j];
                    segs.push(EdgeSeg {
                        x1,
                        y1,
                        x2,
                        y2,
                        strong: i == self.selected || j == self.selected,
                    });
                }
            }
        }

        let mut stage_children: Vec<AnyElement> =
            vec![self.render_canvas_layer(segs).into_any_element()];
        for (i, &pos) in positions.iter().enumerate() {
            stage_children.push(self.render_node_chip(i, pos, cx).into_any_element());
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
                    .py(px(12.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .h_flex()
                            .p(px(2.0))
                            .rounded_lg()
                            .bg(theme::panel_raised())
                            .border_1()
                            .border_color(theme::hairline())
                            .child(
                                div()
                                    .px(px(12.0))
                                    .py(px(4.0))
                                    .rounded_md()
                                    .bg(theme::panel())
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_primary())
                                    .child("Graph"),
                            )
                            .child(
                                div()
                                    .px(px(12.0))
                                    .py(px(4.0))
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .child("List"),
                            ),
                    )
                    .child(ui::meta(
                        "how the fleet remembers this repo — patterns, decisions, gotchas",
                    ))
                    .child(div().flex_1())
                    .child(
                        ui::pill(
                            "preview · live FTS/RAG memory is a phased build",
                            theme::text_muted(),
                            theme::panel_raised(),
                        )
                        .border_1()
                        .border_color(theme::hairline()),
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

    // ── right inspector ─────────────────────────────────────────────

    fn render_inspector(&self) -> Div {
        let node = &self.nodes[self.selected];
        let color = node.kind.color();

        // related (shared-tag) neighbours
        let related: Vec<&MemNode> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| *i != self.selected && Self::shares_tag(node, n))
            .map(|(_, n)| n)
            .collect();

        let related_rows: Vec<Div> = related
            .iter()
            .map(|r| {
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .h(px(30.0))
                    .px(px(10.0))
                    .rounded_md()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline())
                    .child(ui::status_dot(r.kind.color()))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(10.5))
                            .text_color(theme::text_primary())
                            .child(r.title.clone()),
                    )
            })
            .collect();

        let tag_chips: Vec<Div> = node
            .tags
            .iter()
            .map(|t| ui::pill(t.clone(), theme::text_muted(), theme::panel_raised()))
            .collect();

        let seeded_chips: Vec<Div> = node
            .seeded
            .iter()
            .map(|s| {
                div()
                    .h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .px(px(9.0))
                    .py(px(3.0))
                    .rounded_md()
                    .bg(theme::panel_raised())
                    .border_1()
                    .border_color(theme::hairline())
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_muted())
                    .child(ui::status_dot(theme::accent()))
                    .child(s.clone())
            })
            .collect();

        div()
            .w(px(320.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .v_flex()
                    .gap(px(11.0))
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(ui::pill(node.kind.label(), color, color.opacity(0.14)))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .line_height(px(21.0))
                            .text_color(theme::text_primary())
                            .child(node.title.clone()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(12.0))
                            .child(ui::meta(format!("{} relations", related.len())))
                            .child(ui::meta(format!("{} tags", node.tags.len()))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(18.0))
                            .text_color(theme::text_muted())
                            .child(node.desc.clone()),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap(px(16.0))
                    .p(px(16.0))
                    .child(
                        div()
                            .v_flex()
                            .gap(px(7.0))
                            .child(section_head(format!("RELATED · {}", related.len())))
                            .when(related_rows.is_empty(), |el| {
                                el.child(ui::meta("no shared-tag relations"))
                            })
                            .children(related_rows),
                    )
                    .child(
                        div()
                            .v_flex()
                            .gap(px(7.0))
                            .child(section_head("TAGS".to_string()))
                            .child(div().h_flex().gap(px(6.0)).flex_wrap().children(tag_chips)),
                    )
                    .child(
                        div()
                            .v_flex()
                            .gap(px(7.0))
                            .child(section_head("SEEDED INTO".to_string()))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(6.0))
                                    .flex_wrap()
                                    .children(seeded_chips),
                            ),
                    ),
            )
    }
}

fn section_head(text: String) -> Div {
    div()
        .text_size(px(9.0))
        .font_weight(FontWeight::BOLD)
        .text_color(theme::text_muted())
        .child(text)
}

impl Render for MemoryScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let search_bar = self.render_search_bar(window, cx);

        let body: AnyElement = if self.nodes.is_empty() {
            // Live query with zero matches — honest empty state.
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::panel_deep())
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child(format!("No memories match “{}”.", self.query)),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .h_flex()
                .child(self.render_list(cx))
                .child(self.render_graph(cx))
                .child(self.render_inspector())
                .into_any_element()
        };

        div().size_full().v_flex().child(search_bar).child(body)
    }
}

/// Clearly-labelled preview memories using the real category vocabulary
/// (Discovery / Pattern / Gotcha / FileContext) + tags + provenance.
fn sample_nodes() -> Vec<MemNode> {
    vec![
        MemNode {
            kind: MemKind::Discovery,
            title: "ACP-only agent transport".into(),
            desc: "All agents connect over ACP; no per-CLI stdout parsers. See ADR-0006.".into(),
            tags: vec!["architecture".into(), "acp".into()],
            seeded: vec!["spec · transport".into(), "r-4f2a".into()],
        },
        MemNode {
            kind: MemKind::Discovery,
            title: "Event-log is source of truth".into(),
            desc: "Crash recovery scans the per-run SQLite event log; no other state is authoritative.".into(),
            tags: vec!["persistence".into(), "recovery".into()],
            seeded: vec!["r-01aa".into()],
        },
        MemNode {
            kind: MemKind::Discovery,
            title: "Per-run MCP, sandbox-delegated".into(),
            desc: "MCP servers are per-run scoped and supervised; sandbox is delegated to the runtime.".into(),
            tags: vec!["architecture".into(), "mcp".into()],
            seeded: vec!["spec · mcp".into()],
        },
        MemNode {
            kind: MemKind::Pattern,
            title: "thiserror for libs, anyhow for CLI".into(),
            desc: "Library crates use thiserror; the CLI uses anyhow. No unwrap in library code.".into(),
            tags: vec!["rust".into(), "errors".into()],
            seeded: vec!["r-4f2a".into(), "r-9c1e".into()],
        },
        MemNode {
            kind: MemKind::Pattern,
            title: "ULID for all ids".into(),
            desc: "SpecId / TaskId / RunId use ULID (ulid crate) for sortable unique ids.".into(),
            tags: vec!["rust".into(), "ids".into()],
            seeded: vec!["spec · core".into()],
        },
        MemNode {
            kind: MemKind::Gotcha,
            title: "Mutex across await deadlocks".into(),
            desc: "Holding a std Mutex guard across an await point deadlocks; use tokio::sync::Mutex.".into(),
            tags: vec!["concurrency".into(), "tokio".into()],
            seeded: vec!["r-b2e8".into()],
        },
        MemNode {
            kind: MemKind::Gotcha,
            title: "Worktree lost → mark failed".into(),
            desc: "If a run's git worktree is gone at recovery, the policy marks it failed, not resumed.".into(),
            tags: vec!["recovery".into(), "git".into()],
            seeded: vec!["r-77b0".into()],
        },
        MemNode {
            kind: MemKind::FileContext,
            title: "surge-core/src/node.rs".into(),
            desc: "Closed NodeKind enum (Agent/HumanGate/Branch/Terminal/Notify/Loop/Subgraph) + configs.".into(),
            tags: vec!["core".into(), "flow".into()],
            seeded: vec!["spec · flow".into()],
        },
        MemNode {
            kind: MemKind::FileContext,
            title: "surge-acp/src/client.rs".into(),
            desc: "ACP client trait implementation; AgentPool + AgentConnection wiring.".into(),
            tags: vec!["acp".into(), "transport".into()],
            seeded: vec!["r-4f2a".into()],
        },
        MemNode {
            kind: MemKind::FileContext,
            title: "surge-persistence/memory/store.rs".into(),
            desc: "SQLite + FTS5 memory store: discoveries, patterns, gotchas, file contexts.".into(),
            tags: vec!["persistence".into(), "memory".into()],
            seeded: vec!["spec · memory".into()],
        },
    ]
}
