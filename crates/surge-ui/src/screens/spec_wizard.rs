use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardStep {
    Describe,
    Analysis,
    ReviewPlan,
    Criteria,
    Confirm,
}

impl WizardStep {
    fn index(self) -> usize {
        match self {
            Self::Describe => 0,
            Self::Analysis => 1,
            Self::ReviewPlan => 2,
            Self::Criteria => 3,
            Self::Confirm => 4,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Describe => "Describe",
            Self::Analysis => "AI Analysis",
            Self::ReviewPlan => "Review Plan",
            Self::Criteria => "Criteria",
            Self::Confirm => "Confirm",
        }
    }

    fn all() -> &'static [WizardStep] {
        &[
            Self::Describe,
            Self::Analysis,
            Self::ReviewPlan,
            Self::Criteria,
            Self::Confirm,
        ]
    }

    fn next(self) -> Option<Self> {
        Self::all().get(self.index() + 1).copied()
    }
    fn prev(self) -> Option<Self> {
        if self.index() == 0 {
            None
        } else {
            Self::all().get(self.index() - 1).copied()
        }
    }
}

#[derive(Debug, Clone)]
struct PlannedSubtask {
    title: String,
    agent: String,
}

#[derive(Clone, PartialEq)]
pub enum SpecWizardEvent {
    Create { title: String, description: String },
    Cancel,
}

impl EventEmitter<SpecWizardEvent> for SpecWizardScreen {}

pub struct SpecWizardScreen {
    step: WizardStep,
    description: String,
    title: String,
    planned_subtasks: Vec<PlannedSubtask>,
    criteria: Vec<String>,
}

impl SpecWizardScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            step: WizardStep::Describe,
            description: String::new(),
            title: String::new(),
            planned_subtasks: vec![
                PlannedSubtask {
                    title: "Parse requirements".into(),
                    agent: "claude-acp".into(),
                },
                PlannedSubtask {
                    title: "Create data models".into(),
                    agent: "claude-acp".into(),
                },
                PlannedSubtask {
                    title: "Implement core logic".into(),
                    agent: "claude-acp".into(),
                },
                PlannedSubtask {
                    title: "Write tests".into(),
                    agent: "claude-acp".into(),
                },
            ],
            criteria: vec![
                "All endpoints return correct status codes".into(),
                "Test coverage above 80%".into(),
                "No clippy warnings".into(),
            ],
        }
    }

    fn render_stepper(&self) -> Div {
        let steps: Vec<Div> = WizardStep::all()
            .iter()
            .map(|&s| {
                let is_current = s == self.step;
                let is_done = s.index() < self.step.index();
                let color = if is_current {
                    theme::PRIMARY
                } else if is_done {
                    theme::SUCCESS
                } else {
                    theme::TEXT_MUTED.opacity(0.3)
                };

                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(color.opacity(0.2))
                            .text_color(color)
                            .text_xs()
                            .child(if is_done {
                                "✓".to_string()
                            } else {
                                format!("{}", s.index() + 1)
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(color)
                            .child(s.label().to_string()),
                    )
            })
            .collect();

        div().h_flex().gap_4().justify_center().children(steps)
    }

    fn render_step_content(&self) -> Div {
        match self.step {
            WizardStep::Describe => div()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT_PRIMARY)
                        .child("What do you want to build?".to_string()),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme::BACKGROUND)
                        .border_1()
                        .border_color(theme::TEXT_MUTED.opacity(0.2))
                        .min_h(px(120.0))
                        .text_sm()
                        .text_color(if self.description.is_empty() {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT_PRIMARY
                        })
                        .child(if self.description.is_empty() {
                            "Describe the feature, bugfix, or refactor...".to_string()
                        } else {
                            self.description.clone()
                        }),
                ),

            WizardStep::Analysis => div()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT_PRIMARY)
                        .child("AI Analysis".to_string()),
                )
                .child(
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(theme::PRIMARY.opacity(0.05))
                        .border_1()
                        .border_color(theme::PRIMARY.opacity(0.2))
                        .v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::TEXT_PRIMARY)
                                .child("Analyzing your description...".to_string()),
                        )
                        .child(div().text_xs().text_color(theme::TEXT_MUTED).child(
                            "The AI will break down your request into subtasks.".to_string(),
                        )),
                ),

            WizardStep::ReviewPlan => {
                let subtasks: Vec<Div> = self
                    .planned_subtasks
                    .iter()
                    .enumerate()
                    .map(|(i, st)| {
                        div()
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .py(px(6.0))
                            .border_b_1()
                            .border_color(theme::TEXT_MUTED.opacity(0.05))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_MUTED)
                                    .w(px(20.0))
                                    .child(format!("{}", i + 1)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(st.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::PRIMARY)
                                    .child(st.agent.clone()),
                            )
                    })
                    .collect();

                div()
                    .v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Review Plan".to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child("Drag to reorder. Edit subtasks as needed.".to_string()),
                    )
                    .child(div().v_flex().children(subtasks))
            }

            WizardStep::Criteria => {
                let items: Vec<Div> = self
                    .criteria
                    .iter()
                    .map(|c| {
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .py(px(4.0))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::SUCCESS)
                                    .child("✓".to_string()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(c.clone()),
                            )
                    })
                    .collect();

                div()
                    .v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Acceptance Criteria".to_string()),
                    )
                    .child(div().v_flex().gap_1().children(items))
            }

            WizardStep::Confirm => div()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT_PRIMARY)
                        .child("Ready to create".to_string()),
                )
                .child(self.summary_row("Subtasks", &format!("{}", self.planned_subtasks.len())))
                .child(self.summary_row("Criteria", &format!("{}", self.criteria.len())))
                .child(self.summary_row("Agent", "claude-acp")),
        }
    }

    fn summary_row(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .justify_between()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(value.to_string()),
            )
    }
}

impl Render for SpecWizardScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_last = self.step == WizardStep::Confirm;
        let has_prev = self.step.prev().is_some();

        div().size_full().v_flex().p_6().gap_6().child(
            div()
                .v_flex()
                .max_w(px(700.0))
                .gap_6()
                .p_6()
                .bg(theme::SURFACE)
                .rounded_xl()
                .border_1()
                .border_color(theme::TEXT_MUTED.opacity(0.15))
                .child(self.render_stepper())
                .child(div().min_h(px(200.0)).child(self.render_step_content()))
                .child(
                    div()
                        .h_flex()
                        .justify_between()
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(Button::new("sw-cancel").ghost().label("Cancel").on_click(
                                    cx.listener(|_this, _e, _w, cx| {
                                        cx.emit(SpecWizardEvent::Cancel)
                                    }),
                                ))
                                .when(has_prev, |el: Div| {
                                    el.child(Button::new("sw-back").ghost().label("Back").on_click(
                                        cx.listener(|this, _e, _w, cx| {
                                            if let Some(prev) = this.step.prev() {
                                                this.step = prev;
                                                cx.notify();
                                            }
                                        }),
                                    ))
                                }),
                        )
                        .child(if is_last {
                            Button::new("sw-create")
                                .primary()
                                .label("Create & Start")
                                .on_click(cx.listener(|this, _e, _w, cx| {
                                    cx.emit(SpecWizardEvent::Create {
                                        title: this.title.clone(),
                                        description: this.description.clone(),
                                    });
                                }))
                        } else {
                            Button::new("sw-next")
                                .primary()
                                .label("Next")
                                .on_click(cx.listener(|this, _e, _w, cx| {
                                    if let Some(next) = this.step.next() {
                                        this.step = next;
                                        cx.notify();
                                    }
                                }))
                        }),
                ),
        )
    }
}
