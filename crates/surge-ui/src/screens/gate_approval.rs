use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};
use std::fs;
use std::path::PathBuf;

use crate::theme;

/// Gate approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalDecision {
    Pending,
    Approved,
    Rejected,
}

/// Event emitted when a gate decision is made.
#[derive(Debug, Clone)]
pub struct GateDecision {
    pub task_id: String,
    pub approved: bool,
}

/// Context panels in the gate approval view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextPanel {
    PlanDiff,
    CodeChanges,
    QaResults,
}

impl ContextPanel {
    fn label(self) -> &'static str {
        match self {
            Self::PlanDiff => "Plan Diff",
            Self::CodeChanges => "Code Changes",
            Self::QaResults => "QA Results",
        }
    }

    fn all() -> &'static [ContextPanel] {
        &[Self::PlanDiff, Self::CodeChanges, Self::QaResults]
    }
}

/// A plan diff item.
#[derive(Debug, Clone)]
struct PlanDiffItem {
    category: String,
    before: String,
    after: String,
}

/// A changed file summary.
#[derive(Debug, Clone)]
struct ChangedFile {
    path: String,
    status: String,
    added: u32,
    removed: u32,
}

/// A QA check result.
#[derive(Debug, Clone)]
struct QaCheckResult {
    name: String,
    status: String,
    message: Option<String>,
}

/// Gate Approval Screen content.
pub struct GateApprovalScreen {
    pub task_id: String,
    title: String,
    gate_type: String,
    description: String,
    current_decision: ApprovalDecision,
    active_panel: ContextPanel,
    plan_diffs: Vec<PlanDiffItem>,
    changed_files: Vec<ChangedFile>,
    qa_checks: Vec<QaCheckResult>,
    rejection_feedback: SharedString,
    show_rejection_input: bool,
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
            active_panel: ContextPanel::PlanDiff,
            rejection_feedback: SharedString::from(""),
            show_rejection_input: false,
            plan_diffs: vec![
                PlanDiffItem {
                    category: "Complexity".into(),
                    before: "Simple".into(),
                    after: "Standard".into(),
                },
                PlanDiffItem {
                    category: "Estimated Subtasks".into(),
                    before: "3".into(),
                    after: "5".into(),
                },
                PlanDiffItem {
                    category: "Files Changed".into(),
                    before: "2".into(),
                    after: "4".into(),
                },
            ],
            changed_files: vec![
                ChangedFile {
                    path: "src/auth/middleware.rs".into(),
                    status: "Added".into(),
                    added: 45,
                    removed: 0,
                },
                ChangedFile {
                    path: "src/routes/mod.rs".into(),
                    status: "Modified".into(),
                    added: 3,
                    removed: 1,
                },
                ChangedFile {
                    path: "src/old_auth.rs".into(),
                    status: "Deleted".into(),
                    added: 0,
                    removed: 22,
                },
                ChangedFile {
                    path: "Cargo.toml".into(),
                    status: "Modified".into(),
                    added: 2,
                    removed: 0,
                },
            ],
            qa_checks: vec![
                QaCheckResult {
                    name: "Build Success".into(),
                    status: "Passed".into(),
                    message: None,
                },
                QaCheckResult {
                    name: "Unit Tests".into(),
                    status: "Passed".into(),
                    message: Some("All 24 tests passed".into()),
                },
                QaCheckResult {
                    name: "Clippy Lints".into(),
                    status: "Passed".into(),
                    message: Some("No warnings".into()),
                },
                QaCheckResult {
                    name: "Code Formatting".into(),
                    status: "Passed".into(),
                    message: None,
                },
                QaCheckResult {
                    name: "Integration Tests".into(),
                    status: "Failed".into(),
                    message: Some("2 tests failed: test_auth_token_refresh, test_invalid_token".into()),
                },
            ],
        }
    }

    /// Write rejection feedback to HUMAN_INPUT.md file.
    fn write_human_input(&self, feedback: &str) -> anyhow::Result<()> {
        // Determine the task worktree path - typically in .auto-claude/worktrees/tasks/{task_id}/
        let human_input_path = PathBuf::from("HUMAN_INPUT.md");

        let content = format!(
            "# Human Review Feedback\n\n\
             Task ID: {}\n\
             Decision: Rejected\n\
             Date: {}\n\n\
             ## Feedback\n\n\
             {}\n\n\
             ## Instructions for Agent\n\n\
             Please address the feedback above and re-run the task.\n",
            self.task_id,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            feedback
        );

        fs::write(&human_input_path, content)?;
        Ok(())
    }

    fn render_header(&self) -> Div {
        let gate_color = match self.gate_type.as_str() {
            "QaReview" => theme::primary(),
            "HumanReview" => theme::warning(),
            _ => theme::text_muted(),
        };

        div()
            .v_flex()
            .gap_2()
            .pb_4()
            .border_b_1()
            .border_color(theme::text_muted().opacity(0.1))
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
                            .bg(theme::text_muted().opacity(0.15))
                            .text_color(theme::text_muted())
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
                    .text_color(theme::text_primary())
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
                    .text_color(theme::text_primary())
                    .child("Description".to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child(self.description.clone()),
            )
    }

    fn render_decision_status(&self) -> Div {
        let (status_text, status_color) = match self.current_decision {
            ApprovalDecision::Pending => ("Awaiting Decision", theme::warning()),
            ApprovalDecision::Approved => ("Approved", theme::success()),
            ApprovalDecision::Rejected => ("Rejected", theme::error()),
        };

        div()
            .v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
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
            .text_color(theme::text_muted())
            .child(msg.to_string())
    }

    fn render_context_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = ContextPanel::all()
            .iter()
            .map(|&panel| {
                let is_active = panel == self.active_panel;
                div()
                    .id(SharedString::from(format!("panel-{}", panel.label())))
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .text_sm()
                    .text_color(if is_active {
                        theme::primary()
                    } else {
                        theme::text_muted()
                    })
                    .bg(if is_active {
                        theme::primary().opacity(0.1)
                    } else {
                        gpui::transparent_black()
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.active_panel = panel;
                        cx.notify();
                    }))
                    .child(panel.label().to_string())
            })
            .collect();

        div().h_flex().gap_1().children(tabs)
    }

    fn render_panel_content(&self) -> Div {
        match self.active_panel {
            ContextPanel::PlanDiff => self.render_plan_diff(),
            ContextPanel::CodeChanges => self.render_code_changes(),
            ContextPanel::QaResults => self.render_qa_results(),
        }
    }

    fn render_plan_diff(&self) -> Div {
        let items: Vec<Div> = self
            .plan_diffs
            .iter()
            .map(|item| {
                div()
                    .v_flex()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(theme::text_muted().opacity(0.05))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(item.category.clone()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(theme::error().opacity(0.1))
                                    .text_sm()
                                    .text_color(theme::text_muted())
                                    .child(item.before.clone()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_muted())
                                    .child("→".to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(theme::success().opacity(0.1))
                                    .text_sm()
                                    .text_color(theme::text_primary())
                                    .child(item.after.clone()),
                            ),
                    )
            })
            .collect();

        div().v_flex().children(items)
    }

    fn render_code_changes(&self) -> Div {
        let total_added: u32 = self.changed_files.iter().map(|f| f.added).sum();
        let total_removed: u32 = self.changed_files.iter().map(|f| f.removed).sum();

        let items: Vec<Div> = self
            .changed_files
            .iter()
            .map(|file| {
                let status_color = match file.status.as_str() {
                    "Added" => theme::success(),
                    "Modified" => theme::warning(),
                    "Deleted" => theme::error(),
                    _ => theme::text_muted(),
                };

                let status_badge = match file.status.as_str() {
                    "Added" => "A",
                    "Modified" => "M",
                    "Deleted" => "D",
                    _ => "?",
                };

                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(theme::text_muted().opacity(0.05))
                    .child(
                        div()
                            .text_xs()
                            .w(px(18.0))
                            .text_color(status_color)
                            .font_weight(FontWeight::BOLD)
                            .child(status_badge.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::text_primary())
                            .child(file.path.clone()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::success())
                                    .child(format!("+{}", file.added)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::error())
                                    .child(format!("-{}", file.removed)),
                            ),
                    )
            })
            .collect();

        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child(format!(
                        "{} files changed  +{}  -{}",
                        self.changed_files.len(),
                        total_added,
                        total_removed
                    )),
            )
            .child(div().v_flex().children(items))
    }

    fn render_qa_results(&self) -> Div {
        let passed = self
            .qa_checks
            .iter()
            .filter(|c| c.status == "Passed")
            .count();
        let failed = self
            .qa_checks
            .iter()
            .filter(|c| c.status == "Failed")
            .count();
        let total = self.qa_checks.len();

        let items: Vec<Div> = self
            .qa_checks
            .iter()
            .map(|check| {
                let (status_icon, status_color) = match check.status.as_str() {
                    "Passed" => ("✓", theme::success()),
                    "Failed" => ("✗", theme::error()),
                    _ => ("○", theme::text_muted()),
                };

                div()
                    .v_flex()
                    .gap_1()
                    .py_3()
                    .border_b_1()
                    .border_color(theme::text_muted().opacity(0.05))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(status_color)
                                    .w(px(16.0))
                                    .child(status_icon.to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(theme::text_primary())
                                    .child(check.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(status_color.opacity(0.15))
                                    .text_color(status_color)
                                    .child(check.status.clone()),
                            ),
                    )
                    .when(check.message.is_some(), |el: Div| {
                        let msg = check.message.as_deref().unwrap_or("");
                        el.child(
                            div()
                                .pl(px(18.0))
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(msg.to_string()),
                        )
                    })
            })
            .collect();

        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child(format!("{} / {} checks passed", passed, total)),
            )
            .when(failed > 0, |el: Div| {
                el.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme::error().opacity(0.1))
                        .text_sm()
                        .text_color(theme::error())
                        .child(format!("{} check(s) failed - review required", failed)),
                )
            })
            .child(div().v_flex().children(items))
    }

    fn render_rejection_feedback(&self, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .gap_2()
            .pt_4()
            .pb_2()
            .border_t_1()
            .border_color(theme::text_muted().opacity(0.1))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .child("Rejection Feedback".to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("This feedback will be written to HUMAN_INPUT.md for the agent to review.".to_string()),
            )
            .child(
                div()
                    .id("rejection-feedback-input")
                    .w_full()
                    .min_h(px(100.0))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::text_muted().opacity(0.2))
                    .text_sm()
                    .text_color(theme::text_primary())
                    .cursor_text()
                    .child(
                        if self.rejection_feedback.is_empty() {
                            div()
                                .text_color(theme::text_muted())
                                .child("Enter your feedback here (e.g., \"Tests are failing\", \"Code needs refactoring\")...".to_string())
                        } else {
                            div().child(self.rejection_feedback.to_string())
                        }
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        // Handle text input
                        let key = &event.keystroke.key;
                        let mut feedback = this.rejection_feedback.to_string();

                        if key == "backspace" {
                            feedback.pop();
                        } else if key == "enter" {
                            feedback.push('\n');
                        } else if key.len() == 1 && !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt {
                            feedback.push_str(key);
                        }

                        this.rejection_feedback = SharedString::from(feedback);
                        cx.notify();
                    }))
                    .on_click(cx.listener(|_this, _event, window, _cx| {
                        // Focus the window to receive keyboard events
                        window.activate_window();
                    })),
            )
    }
}

impl EventEmitter<GateDecision> for GateApprovalScreen {}

impl Render for GateApprovalScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            .overflow_hidden()
            .child(self.render_header())
            .child(
                div()
                    .h_flex()
                    .gap_4()
                    .child(self.render_description())
                    .child(self.render_decision_status()),
            )
            .child(self.render_context_tabs(cx))
            .child(
                div()
                    .flex_1()
                    .v_flex()
                    .overflow_hidden()
                    .child(self.render_panel_content()),
            )
            // Rejection feedback input (shown when user clicks reject)
            .when(self.show_rejection_input, |el: Div| {
                el.child(self.render_rejection_feedback(cx))
            })
            // Actions
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::text_muted().opacity(0.1))
                    .child(Button::new("ga-view-changes").ghost().label("View Changes"))
                    .child(Button::new("ga-view-logs").ghost().label("View Logs"))
                    .child(
                        div()
                            .flex_1()
                            .h_flex()
                            .gap_2()
                            .justify_end()
                            .when(!self.show_rejection_input, |el: Div| {
                                el.child(
                                    Button::new("ga-reject")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.show_rejection_input = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(self.show_rejection_input, |el: Div| {
                                el.child(
                                    Button::new("ga-cancel-reject")
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.show_rejection_input = false;
                                            this.rejection_feedback = SharedString::from("");
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("ga-confirm-reject")
                                        .primary()
                                        .label("Confirm Rejection")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            // Write feedback to HUMAN_INPUT.md
                                            if let Err(e) = this.write_human_input(&this.rejection_feedback) {
                                                eprintln!("Failed to write HUMAN_INPUT.md: {}", e);
                                            }

                                            this.current_decision = ApprovalDecision::Rejected;
                                            cx.emit(GateDecision {
                                                task_id: this.task_id.clone(),
                                                approved: false,
                                            });
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(!self.show_rejection_input, |el: Div| {
                                el.child(
                                    Button::new("ga-approve")
                                        .primary()
                                        .label("Approve")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.current_decision = ApprovalDecision::Approved;
                                            cx.emit(GateDecision {
                                                task_id: this.task_id.clone(),
                                                approved: true,
                                            });
                                            cx.notify();
                                        })),
                                )
                            }),
                    ),
            )
    }
}
