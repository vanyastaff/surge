//! Fleet — the run constellation (mission-control home).
//!
//! Displays daemon run summaries only. An empty run list is an onboarding
//! state, never a source of sample activity or invented review requests.

use std::time::Duration;

use gpui_kit::component::StyledExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_orchestrator::engine::handle::RunStatus;

use gpui_kit::component::input::{Input, InputEvent, InputState};

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
    /// Open a run's cockpit.
    OpenRun(Option<surge_core::id::RunId>),
    /// Operator described new work in the command bar — dispatch a
    /// bootstrap run with this prompt.
    Dispatch(String),
    /// Open the review gate for a run awaiting the operator.
    OpenGate(String),
}

impl EventEmitter<FleetAction> for FleetScreen {}

#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    Completed,
    Failed,
    Aborted,
    Active,
    Queued,
}

impl NodeKind {
    fn color(self) -> Hsla {
        match self {
            Self::Completed => theme::success(),
            Self::Failed | Self::Aborted => theme::error(),
            Self::Active => theme::accent(),
            Self::Queued => theme::text_muted(),
        }
    }

    fn status_label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            // Same word the Runs rail and Inbox use for this status —
            // one run must not read "failed" here and "aborted" there.
            Self::Aborted => "aborted",
            Self::Active => "active",
            Self::Queued => "queued",
        }
    }
}

/// One node on the constellation.
#[derive(Clone)]
struct FleetNode {
    id: String,
    /// Real daemon run id.
    run_id: Option<surge_core::id::RunId>,
    title: String,
    meta: String,
    kind: NodeKind,
}

/// A curved branch edge to paint on the canvas (trunk → card anchor).
#[derive(Clone, Copy)]
struct EdgeSpec {
    x: f32,
    card_edge_y: f32,
    color: Hsla,
}

/// Card headline from the operator's request: its first line, cut to fit.
fn card_title(prompt: &str) -> String {
    ui::headline(prompt, 34)
}

/// Fleet screen — reads runs from shared state, renders the constellation.
pub struct FleetScreen {
    state: Entity<AppState>,
    /// Branch the runs fork from, read once from the repository.
    branch: Option<String>,
    /// Command-bar input (created lazily; needs a Window).
    command_input: Option<Entity<InputState>>,
}

impl FleetScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Re-render when the run list / daemon link changes.
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        let branch = state
            .read(cx)
            .project_path
            .as_deref()
            .and_then(ui::current_branch);
        Self {
            state,
            branch,
            command_input: None,
        }
    }

    /// Read the command bar, clear it, and emit a dispatch.
    fn submit_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.command_input.clone() else {
            return;
        };
        let prompt = input.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        input.update(cx, |s, cx| s.set_value("", window, cx));
        cx.emit(FleetAction::Dispatch(prompt));
    }

    /// Build nodes exclusively from daemon run summaries.
    fn nodes(&self, cx: &Context<Self>) -> (Vec<FleetNode>, bool) {
        let state = self.state.read(cx);
        let live = state.daemon_state.facade().is_some();

        let nodes = state
            .project_runs()
            .into_iter()
            .take(6)
            .map(|r| {
                let kind = match r.status {
                    RunStatus::Active => NodeKind::Active,
                    RunStatus::Completed => NodeKind::Completed,
                    RunStatus::Failed => NodeKind::Failed,
                    RunStatus::Aborted => NodeKind::Aborted,
                    _ => NodeKind::Queued,
                };
                let short = r.run_id.short().to_lowercase();
                let started = r.started_at.with_timezone(&chrono::Local).format("%H:%M");
                let started = format!("started {started}");
                FleetNode {
                    id: format!("r-{short}"),
                    run_id: Some(r.run_id),
                    title: state.run_prompt(&r.run_id).map_or_else(
                        || {
                            format!(
                                "started {}",
                                r.started_at.with_timezone(&chrono::Local).format("%H:%M")
                            )
                        },
                        card_title,
                    ),
                    meta: started,
                    kind,
                }
            })
            .collect();
        (nodes, live)
    }

    // ── stage pieces ────────────────────────────────────────────────

    fn node_x(i: usize) -> f32 {
        NODE_X0 + (i as f32) * NODE_STEP
    }

    /// Low-level paint layer: curved, glowing branch
    /// edges. Sits behind the trunk / dots / cards. Painted in stage
    /// coordinates offset by the canvas's window-space origin.
    fn render_stage_canvas(&self, edges: Vec<EdgeSpec>) -> impl IntoElement {
        canvas(
            |_bounds, _window, _cx| {},
            move |bounds, _prepaint, window, _cx| {
                let ox = bounds.origin.x;
                let oy = bounds.origin.y;

                // curved branch edges: a wide faint glow pass + a bright
                // thin pass, both following the same cubic Bézier.
                for e in &edges {
                    let start = point(ox + px(e.x), oy + px(TRUNK_Y));
                    let end = point(ox + px(e.x), oy + px(e.card_edge_y));
                    let midy = (TRUNK_Y + e.card_edge_y) / 2.0;
                    let c1 = point(ox + px(e.x + 22.0), oy + px(midy));
                    let c2 = point(ox + px(e.x - 22.0), oy + px(midy));

                    let mut glow = PathBuilder::stroke(px(5.0));
                    glow.move_to(start);
                    glow.cubic_bezier_to(end, c1, c2);
                    if let Ok(p) = glow.build() {
                        window.paint_path(p, e.color.opacity(0.14));
                    }

                    let mut line = PathBuilder::stroke(px(1.6));
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
            // branch label — the repository's, or "HEAD" when unknown
            div()
                .absolute()
                .left(px(16.0))
                .top(px(TRUNK_Y - 26.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_muted())
                .child(self.branch.clone().unwrap_or_else(|| "HEAD".to_string()))
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

        let id_for_click = node.id.clone();
        let run_id_for_click = node.run_id;
        let is_failed = matches!(node.kind, NodeKind::Failed | NodeKind::Aborted);

        // Node dot on the trunk; active dots gently pulse.
        let dot_base = div()
            .absolute()
            .left(px(x - 5.0))
            .top(px(TRUNK_Y - 5.0))
            .w(px(10.0))
            .h(px(10.0))
            .rounded_full()
            .bg(color)
            .border_2()
            .border_color(theme::panel_deep());
        let dot = if matches!(node.kind, NodeKind::Active) {
            dot_base
                .with_animation(
                    SharedString::from(format!("fleet-pulse-{}", node.id)),
                    Animation::new(Duration::from_millis(1600)).repeat(),
                    |el, delta| el.opacity(0.5 + 0.5 * (delta * std::f32::consts::PI).sin()),
                )
                .into_any_element()
        } else {
            dot_base.into_any_element()
        };

        let card = div()
            .id(SharedString::from(format!("fleet-card-{}", node.run_id.map_or_else(|| node.id.clone(), |id| id.to_string()))))
            .role(Role::Button)
            .aria_label(node.title.clone())
            .absolute()
            .left(px(x - CARD_W / 2.0))
            .top(px(card_top))
            .w(px(CARD_W))
            .v_flex()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(ui::R_CONTROL + 2.0))
            .bg(theme::panel_raised())
            .border_1()
            .border_color(if is_failed {
                theme::stroke(color)
            } else {
                theme::hairline()
            })
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::stroke(theme::accent())))
            .on_click(cx.listener(move |_this, _e, _w, cx| {
                if is_failed {
                    cx.emit(FleetAction::OpenGate(id_for_click.clone()));
                } else {
                    cx.emit(FleetAction::OpenRun(run_id_for_click));
                }
            }))
            // What was asked — the part a person recognises.
            .child(
                div()
                    .overflow_hidden()
                    .text_size(px(12.5))
                    .line_height(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .child(node.title.clone()),
            )
            // Status, then when and which run — the part a developer greps.
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(ui::pill(node.kind.status_label(), color, theme::tint(color)))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_dim())
                            .child(node.meta.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_dim())
                    .child(node.id.clone()),
            )
            .into_any_element();

        vec![dot, card]
    }

    fn render_chips(
        &self,
        active: usize,
        needs: usize,
        completed: usize,
        failed: usize,
        live: bool,
    ) -> Div {
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
                if needs > 0 {
                    theme::warning()
                } else {
                    theme::text_muted()
                },
            ))
            .child(chip(
                theme::success(),
                format!("{completed} completed"),
                theme::text_muted(),
            ))
            .when(failed > 0, |el| {
                el.child(chip(theme::error(), format!("{failed} failed"), theme::text_muted()))
            })
            .when(!live, |el| {
                el.child(
                    ui::pill(
                        "Offline · last known runs",
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
        let primary_label = "Open Inbox";

        // primary button
        let id_primary = id.clone();
        let primary = div()
            .id("fleet-inspector-primary")
            .test_support()
            .role(Role::Button)
            .aria_label(primary_label)
            .flex()
            .items_center()
            .justify_center()
            .h(px(36.0))
            .rounded_lg()
            .bg(theme::accent())
            .text_color(theme::on_accent())
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
                            .child("NEEDS YOU"),
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
            .child(primary)
    }

    fn render_command_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if self.command_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    "Describe what to build next…",
                )
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_command(window, cx);
                    }
                },
            )
            .detach();
            self.command_input = Some(input);
        }

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
                    .rounded(px(ui::R_CONTROL + 2.0))
                    .bg(theme::panel_deep())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .child(ui::role_badge("new run", theme::Semantic::Agent))
                    .child(
                        div().flex_1().child(
                            Input::new(self.command_input.as_ref().unwrap())
                                .accessibility_id("fleet-task")
                                .aria_label("Describe a task")
                                .appearance(false),
                        ),
                    )
                    .child(ui::kbd("↵")),
            )
    }
}

impl Render for FleetScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (nodes, live) = self.nodes(cx);

        // Counts cover every project run, not just the cards that fit.
        let (active, needs, completed, failed) = {
            let state = self.state.read(cx);
            let runs = state.project_runs();
            (
                runs.iter()
                    .filter(|r| r.status == RunStatus::Active)
                    .count(),
                state.needs_you_count(),
                runs.iter()
                    .filter(|r| r.status == RunStatus::Completed)
                    .count(),
                runs.iter()
                    .filter(|r| matches!(r.status, RunStatus::Failed | RunStatus::Aborted))
                    .count(),
            )
        };

        // Only failures the operator has not dismissed ask for attention.
        let attention = {
            let state = self.state.read(cx);
            let open: Vec<_> = state
                .unacknowledged_failures()
                .into_iter()
                .map(|r| r.run_id)
                .collect();
            nodes
                .iter()
                .find(|n| n.run_id.is_some_and(|id| open.contains(&id)))
                .cloned()
        };

        // Branch edges painted on the canvas layer (behind everything).
        let edges: Vec<EdgeSpec> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let above = i.is_multiple_of(2);
                EdgeSpec {
                    x: Self::node_x(i),
                    card_edge_y: if above { 158.0 } else { 316.0 },
                    color: n.kind.color(),
                }
            })
            .collect();

        // Build the stage as one flat layer of absolute children so their
        // coords anchor to the stage box: canvas (back) → trunk → nodes.
        let mut stage_children: Vec<AnyElement> =
            vec![self.render_stage_canvas(edges).into_any_element()];
        if !nodes.is_empty() {
            stage_children.extend(self.render_trunk());
        }
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
                    .child(ui::grid_backdrop(24.0))
                    .when(nodes.is_empty(), |el| {
                        el.child(
                            div()
                                .id("fleet-empty")
                                .test_support()
                                .aria_label("Create your first application")
                                .v_flex()
                                .gap(px(12.0))
                                .max_w(px(480.0))
                                .p(px(32.0))
                                .child(div().text_size(px(24.0)).font_weight(FontWeight::BOLD).text_color(theme::text_primary()).child("Create your first application"))
                                .child(ui::meta(if live {
                                    "Describe what you want to build below. Surge will prepare a plan and bring decisions to your Inbox."
                                } else {
                                    "Connect the daemon to start work. Your actual runs will appear here."
                                })),
                        )
                    })
                    .when(!nodes.is_empty(), |el| el.child(stage))
                    .child(self.render_chips(active, needs, completed, failed, live))
                    .children(attention.map(|n| self.render_inspector(n, cx))),
            )
            .child(self.render_command_bar(window, cx))
    }
}

#[cfg(test)]
mod accessibility_tests {
    use super::{FleetAction, FleetScreen};
    use crate::app_state::AppState;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext, WindowOptions};
    use std::{cell::Cell, rc::Rc};

    #[gpui_kit::test]
    fn empty_fleet_contains_no_invented_runs(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| FleetScreen::new(state, cx))
            })
            .unwrap()
        });
        view.update(cx, |view, cx| {
            let (nodes, live) = view.nodes(cx);
            assert!(nodes.is_empty(), "empty daemon data must stay empty");
            assert!(!live);
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("fleet-empty").label(),
                Some("Create your first application")
            );
        })
        .unwrap();
    }

    #[test]
    fn card_title_uses_the_first_request_line_and_fits_the_card() {
        assert_eq!(
            super::card_title("Add dark mode\nand tests"),
            "Add dark mode"
        );
        let long =
            super::card_title("Create and implement a complete offline countdown timer web app");
        assert_eq!(long.chars().count(), 34);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn workflow_completion_does_not_claim_a_git_merge() {
        assert_eq!(super::NodeKind::Completed.status_label(), "completed");
    }

    #[gpui_kit::test]
    fn inspector_names_the_inbox_action_it_dispatches(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.runs.push(crate::app_state::UiRun {
                    run_id: surge_core::id::RunId::new(),
                    status: surge_orchestrator::engine::handle::RunStatus::Failed,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: None,
                });
                state
            });
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| FleetScreen::new(state, cx))
            })
            .unwrap()
        });
        let inbox_actions = Rc::new(Cell::new(0));
        let captured = inbox_actions.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &FleetAction, _| {
                assert!(matches!(event, FleetAction::OpenGate(_)));
                captured.set(captured.get() + 1);
            })
            .detach();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("fleet-inspector-primary").label(),
                Some("Open Inbox")
            );
            window.click("fleet-inspector-primary", cx);
        })
        .unwrap();
        cx.update(|_| assert_eq!(inbox_actions.get(), 1));
    }
}
