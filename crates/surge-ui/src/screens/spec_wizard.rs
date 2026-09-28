//! Prompt entry for a daemon-hosted planning run.
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{Disableable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::id::RunId;

use crate::theme;

#[derive(Clone, PartialEq)]
pub enum SpecWizardEvent {
    Create { run_id: RunId, description: String },
    OpenRun(RunId),
    Cancel,
}

impl EventEmitter<SpecWizardEvent> for SpecWizardScreen {}

#[derive(Debug, PartialEq)]
enum SubmissionState {
    Editing { error: Option<String> },
    Submitting(RunId),
    Accepted(RunId),
}

pub struct SpecWizardScreen {
    pub project_path: PathBuf,
    input: Option<Entity<TextareaState>>,
    submission: SubmissionState,
}

impl SpecWizardScreen {
    pub fn new(project_path: PathBuf, _cx: &mut Context<Self>) -> Self {
        Self {
            project_path,
            input: None,
            submission: SubmissionState::Editing { error: None },
        }
    }

    pub fn prompt(&self, cx: &App) -> String {
        self.input
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string())
    }

    pub fn error(&self) -> Option<&str> {
        match &self.submission {
            SubmissionState::Editing { error } => error.as_deref(),
            _ => None,
        }
    }

    pub fn is_submitting(&self, run_id: RunId) -> bool {
        self.submission == SubmissionState::Submitting(run_id)
    }

    pub fn is_accepted(&self, run_id: RunId) -> bool {
        self.submission == SubmissionState::Accepted(run_id)
    }

    /// A response belongs to precisely the attempt that emitted its run id.
    pub fn finish_submission(
        &mut self,
        run_id: RunId,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_submitting(run_id) {
            return false;
        }
        self.submission = match result {
            Ok(()) => SubmissionState::Accepted(run_id),
            Err(error) => SubmissionState::Editing { error: Some(error) },
        };
        cx.notify();
        true
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.submission, SubmissionState::Editing { .. }) {
            return;
        }
        let description = self.prompt(cx);
        if description.trim().is_empty() {
            self.submission = SubmissionState::Editing {
                error: Some("Describe the work before starting a planning run.".into()),
            };
            cx.notify();
            return;
        }
        let run_id = RunId::new();
        self.submission = SubmissionState::Submitting(run_id);
        cx.emit(SpecWizardEvent::Create {
            run_id,
            description,
        });
        cx.notify();
    }
}

impl Render for SpecWizardScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self
            .input
            .get_or_insert_with(|| {
                let input = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .rows(7)
                        .placeholder("Describe the feature, bugfix, or refactor…")
                });
                cx.subscribe(&input, |_: &mut Self, _, _: &InputEvent, cx| cx.notify())
                    .detach();
                input
            })
            .clone();
        let editing = matches!(self.submission, SubmissionState::Editing { .. });
        let pending = matches!(self.submission, SubmissionState::Submitting(_));
        let accepted = match self.submission {
            SubmissionState::Accepted(id) => Some(id),
            _ => None,
        };
        div().size_full().v_flex().p_6().gap_4().child(
            div().v_flex().max_w(px(700.0)).w_full().gap_4().p_6()
                .bg(theme::surface()).rounded_xl()
                .child(div().text_lg().text_color(theme::text_primary()).child("Plan a task"))
                .child(div().text_sm().text_color(theme::text_muted())
                    .child(format!("Project: {}", self.project_path.display())))
                .child(div().text_sm().text_color(theme::text_muted()).child(
                    "Send your request to the daemon to prepare a description, roadmap, and flow. Review its decisions in Inbox. This starts planning; it does not start the generated implementation."))
                .child(div().id("planning-prompt-region").min_h(px(200.0)).flex_shrink_0().test_support().debug_selector(|| "planning-prompt".into())
                    .child(Textarea::new(&input).h(px(200.0)).accessibility_id("planning-prompt").aria_label("Task description").disabled(!editing)))
                .when_some(self.error().map(str::to_owned), |el, error| {
                    el.child(div().id("planning-error").role(Role::Label).aria_label(error.clone()).test_support().debug_selector(|| "planning-error".into()).text_sm()
                        .text_color(theme::error()).child(error))
                })
                .when(pending, |el| el.child(div().id("pending-status").role(Role::Label).aria_label("Submitting planning request…").child("Submitting planning request…")))
                .when_some(accepted, |el, id| el.child(div().id("accepted-status").role(Role::Label).aria_label(format!("Planning request {id} accepted or queued by the daemon.")).text_sm()
                    .child(format!("Planning request {id} accepted or queued by the daemon."))))
                .child(div().h_flex().gap_3()
                    .child(Button::new("planning-back").ghost().label("Back").accessibility_id("planning-back")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SpecWizardEvent::Cancel))))
                    .child(if let Some(id) = accepted {
                        Button::new("planning-open").primary().label("Open planning run").accessibility_id("planning-open")
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(SpecWizardEvent::OpenRun(id))))
                    } else {
                        Button::new("planning-start").primary().label("Start planning").accessibility_id("planning-start")
                            .debug_selector(|| "planning-submit".into())
                            .disabled(pending || self.prompt(cx).trim().is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                    }))
        )
    }
}

#[cfg(test)]
mod accessibility_tests {
    use super::{SpecWizardEvent, SpecWizardScreen};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext, WindowOptions, px};
    use std::{cell::Cell, path::PathBuf, rc::Rc};

    #[gpui_kit::test]
    fn enter_inserts_newline_and_pending_prompt_cannot_be_edited(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| SpecWizardScreen::new(PathBuf::from("/tmp/textarea-test"), cx))
            })
            .unwrap()
        });
        let submissions = Rc::new(Cell::new(0));
        let copy = submissions.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &SpecWizardEvent, _| {
                if matches!(event, SpecWizardEvent::Create { .. }) {
                    copy.set(copy.get() + 1);
                }
            })
            .detach();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let input_id = ("input", view.read(cx).input.as_ref().unwrap().entity_id());
            assert!(
                window.find(input_id).bounds().size.height >= px(180.0),
                "multiline input must visibly fit seven lines"
            );
            window.click("planning-prompt-region", cx);
            window.input("  first", cx);
            window.press("enter", cx);
            window.input("second  ", cx);
            assert_eq!(view.read(cx).prompt(cx), "  first\nsecond  ");
        })
        .unwrap();
        cx.update(|_| assert_eq!(submissions.get(), 0));
        cx.update_window(handle, |_, window, cx| {
            window.click("planning-start", cx);
            window.click("planning-prompt-region", cx);
            window.input("unexpected edit", cx);
            assert_eq!(view.read(cx).prompt(cx), "  first\nsecond  ");
        })
        .unwrap();
        cx.update(|_| assert_eq!(submissions.get(), 1));
    }
}
