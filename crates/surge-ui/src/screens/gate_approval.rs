use gpui::*;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};

use crate::theme;

/// Gate approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalDecision {
    Pending,
    Approved,
    Rejected,
}

/// Gate Approval Screen content.
pub struct GateApprovalScreen {
    task_id: String,
    title: String,
    gate_type: String,
    description: String,
    current_decision: ApprovalDecision,
}

impl GateApprovalScreen {
    pub fn new(task_id: &str, _cx: &mut Context<Self>) -> Self {
        // Demo data.
        Self {
            task_id: task_id.to_string(),
            title: "QA Review Required".to_string(),
            gate_type: "QaReview".to_string(),
            description: "Task has completed execution and requires quality assurance review before proceeding to merge.".to_string(),
            current_decision: ApprovalDecision::Pending,
        }
    }

    fn render_header(&self) -> Div {
        let gate_color = match self.gate_type.as_str() {
            "QaReview" => theme::PRIMARY,
            "HumanReview" => theme::WARNING,
            _ => theme::TEXT_MUTED,
        };

        div()
            .v_flex()
            .gap_2()
            .pb_4()
            .border_b_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    // ID badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::TEXT_MUTED.opacity(0.15))
                            .text_color(theme::TEXT_MUTED)
                            .child(self.task_id.clone()),
                    )
                    // Gate type badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(gate_color.opacity(0.15))
                            .text_color(gate_color)
                            .child(self.gate_type.clone()),
                    ),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(self.title.clone()),
            )
    }

    fn render_description(&self) -> Div {
        div()
            .v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Description".to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(self.description.clone()),
            )
    }

    fn render_decision_status(&self) -> Div {
        let (status_text, status_color) = match self.current_decision {
            ApprovalDecision::Pending => ("Awaiting Decision", theme::WARNING),
            ApprovalDecision::Approved => ("Approved", theme::SUCCESS),
            ApprovalDecision::Rejected => ("Rejected", theme::ERROR),
        };

        div()
            .v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Status".to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(status_color)
                    .child(status_text.to_string()),
            )
    }

    fn render_placeholder(&self, msg: &str) -> Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(theme::TEXT_MUTED)
            .child(msg.to_string())
    }
}

impl Render for GateApprovalScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            .overflow_hidden()
            .child(self.render_header())
            .child(self.render_description())
            .child(self.render_decision_status())
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Review Details".to_string()),
                    )
                    .child(self.render_placeholder("Review checklist and details will appear here")),
            )
            // Actions
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .child(Button::new("ga-view-changes").ghost().label("View Changes"))
                    .child(Button::new("ga-view-logs").ghost().label("View Logs"))
                    .child(
                        div()
                            .flex_1()
                            .h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("ga-reject")
                                    .ghost()
                                    .label("Reject")
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.current_decision = ApprovalDecision::Rejected;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("ga-approve")
                                    .primary()
                                    .label("Approve")
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.current_decision = ApprovalDecision::Approved;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
}
