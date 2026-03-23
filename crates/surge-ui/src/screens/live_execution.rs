use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Pending,
    Running,
    Done,
    Failed,
}

impl NodeState {
    fn color(self) -> Hsla {
        match self {
            Self::Pending => theme::TEXT_MUTED,
            Self::Running => theme::WARNING,
            Self::Done => theme::SUCCESS,
            Self::Failed => theme::ERROR,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Done => "Done",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionNode {
    pub id: String,
    pub title: String,
    pub state: NodeState,
    pub agent: Option<String>,
    pub lane: usize,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
    pub node_id: Option<String>,
}

pub struct LiveExecutionScreen {
    spec_title: String,
    nodes: Vec<ExecutionNode>,
    logs: Vec<LogEntry>,
    completed: usize,
    total: usize,
    tokens_used: u64,
    paused: bool,
}

impl LiveExecutionScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            spec_title: "Auth Middleware".into(),
            nodes: vec![
                ExecutionNode { id: "n1".into(), title: "Create JWT module".into(), state: NodeState::Done, agent: Some("claude-acp".into()), lane: 0 },
                ExecutionNode { id: "n2".into(), title: "Add middleware".into(), state: NodeState::Done, agent: Some("claude-acp".into()), lane: 0 },
                ExecutionNode { id: "n3".into(), title: "Role-based guards".into(), state: NodeState::Running, agent: Some("claude-acp".into()), lane: 0 },
                ExecutionNode { id: "n4".into(), title: "Token refresh".into(), state: NodeState::Pending, agent: None, lane: 1 },
                ExecutionNode { id: "n5".into(), title: "Integration tests".into(), state: NodeState::Pending, agent: None, lane: 1 },
            ],
            logs: vec![
                LogEntry { timestamp: "10:42:01".into(), message: "Started: Create JWT module".into(), node_id: Some("n1".into()) },
                LogEntry { timestamp: "10:42:15".into(), message: "Writing src/auth/jwt.rs".into(), node_id: Some("n1".into()) },
                LogEntry { timestamp: "10:42:30".into(), message: "Completed: Create JWT module (29s)".into(), node_id: Some("n1".into()) },
                LogEntry { timestamp: "10:42:31".into(), message: "Started: Add middleware".into(), node_id: Some("n2".into()) },
                LogEntry { timestamp: "10:43:00".into(), message: "Completed: Add middleware (29s)".into(), node_id: Some("n2".into()) },
                LogEntry { timestamp: "10:43:01".into(), message: "Started: Role-based guards".into(), node_id: Some("n3".into()) },
                LogEntry { timestamp: "10:43:10".into(), message: "Writing src/auth/guards.rs".into(), node_id: Some("n3".into()) },
            ],
            completed: 2,
            total: 5,
            tokens_used: 45_200,
            paused: false,
        }
    }

    fn render_progress_bar(&self) -> Div {
        let pct = if self.total > 0 { self.completed as f32 / self.total as f32 } else { 0.0 };

        div()
            .h_flex()
            .gap_4()
            .items_center()
            .p_3()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY)
                    .child(self.spec_title.clone()),
            )
            .child(
                div().flex_1().h(px(6.0)).rounded_full().bg(theme::TEXT_MUTED.opacity(0.15))
                    .child(div().h_full().rounded_full().bg(theme::PRIMARY).w(relative(pct))),
            )
            .child(
                div().text_sm().text_color(theme::TEXT_MUTED)
                    .child(format!("{}/{}", self.completed, self.total)),
            )
            .child(
                div().text_xs().text_color(theme::TEXT_MUTED)
                    .child(format!("{}K tokens", self.tokens_used / 1000)),
            )
    }

    fn render_graph(&self) -> Div {
        // Simplified lane-based visualization.
        let max_lane = self.nodes.iter().map(|n| n.lane).max().unwrap_or(0);
        let lanes: Vec<Div> = (0..=max_lane)
            .map(|lane| {
                let lane_nodes: Vec<Div> = self.nodes.iter()
                    .filter(|n| n.lane == lane)
                    .map(|node| {
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(node.state.color().opacity(0.1))
                            .border_1()
                            .border_color(node.state.color().opacity(0.3))
                            .child(
                                div().w(px(8.0)).h(px(8.0)).rounded_full().bg(node.state.color()),
                            )
                            .child(
                                div().text_sm().text_color(theme::TEXT_PRIMARY).child(node.title.clone()),
                            )
                            .child(
                                div().text_xs().text_color(node.state.color()).child(node.state.label().to_string()),
                            )
                    })
                    .collect();

                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().text_xs().text_color(theme::TEXT_MUTED).w(px(50.0))
                            .child(format!("Lane {}", lane + 1)),
                    )
                    .children(lane_nodes)
            })
            .collect();

        div()
            .v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY)
                    .child("Execution Graph".to_string()),
            )
            .children(lanes)
    }

    fn render_log(&self) -> Div {
        let entries: Vec<Div> = self.logs.iter().map(|entry| {
            let color = if entry.message.starts_with("Completed") {
                theme::SUCCESS
            } else if entry.message.starts_with("Started") {
                theme::WARNING
            } else {
                theme::TEXT_MUTED
            };

            div()
                .h_flex()
                .gap_2()
                .py(px(2.0))
                .child(div().text_xs().text_color(theme::TEXT_MUTED.opacity(0.5)).w(px(60.0)).child(entry.timestamp.clone()))
                .child(div().text_xs().text_color(color).child(entry.message.clone()))
        }).collect();

        div()
            .v_flex()
            .flex_1()
            .gap_1()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT_PRIMARY)
                    .child("Execution Log".to_string()),
            )
            .children(entries)
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .gap_2()
            .child(
                if self.paused {
                    Button::new("exec-resume").primary().label("▶ Resume")
                        .on_click(cx.listener(|this, _e, _w, cx| { this.paused = false; cx.notify(); }))
                } else {
                    Button::new("exec-pause").label("⏸ Pause")
                        .on_click(cx.listener(|this, _e, _w, cx| { this.paused = true; cx.notify(); }))
                },
            )
            .child(Button::new("exec-cancel").ghost().label("✕ Cancel"))
    }
}

impl Render for LiveExecutionScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            // Header
            .child(
                div().h_flex().justify_between().items_center()
                    .child(
                        div().text_2xl().font_weight(FontWeight::BOLD).text_color(theme::TEXT_PRIMARY)
                            .child("Live Execution".to_string()),
                    )
                    .child(self.render_controls(cx)),
            )
            // Progress
            .child(self.render_progress_bar())
            // Graph + Log side by side
            .child(
                div().flex_1().h_flex().gap_4().overflow_hidden()
                    .child(div().flex_1().child(self.render_graph()))
                    .child(div().w(px(400.0)).child(self.render_log())),
            )
    }
}
