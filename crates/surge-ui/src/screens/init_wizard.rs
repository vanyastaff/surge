use std::path::PathBuf;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::button::{Button, ButtonVariants};

use crate::theme;

/// Steps of the init wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Directory,
    ProjectInfo,
    AgentSetup,
    PipelineConfig,
    Confirm,
}

impl Step {
    fn index(self) -> usize {
        match self {
            Self::Directory => 0,
            Self::ProjectInfo => 1,
            Self::AgentSetup => 2,
            Self::PipelineConfig => 3,
            Self::Confirm => 4,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Directory => "Directory",
            Self::ProjectInfo => "Project Info",
            Self::AgentSetup => "Agent Setup",
            Self::PipelineConfig => "Pipeline",
            Self::Confirm => "Confirm",
        }
    }

    fn all() -> &'static [Step] {
        &[
            Self::Directory,
            Self::ProjectInfo,
            Self::AgentSetup,
            Self::PipelineConfig,
            Self::Confirm,
        ]
    }

    fn next(self) -> Option<Step> {
        Step::all().get(self.index() + 1).copied()
    }

    fn prev(self) -> Option<Step> {
        if self.index() == 0 {
            None
        } else {
            Step::all().get(self.index() - 1).copied()
        }
    }
}

/// Pipeline preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipelinePreset {
    Careful,
    Fast,
    Custom,
}

/// Events emitted by the wizard.
#[derive(Clone, PartialEq)]
pub enum InitWizardEvent {
    /// Wizard completed — create the project.
    Create {
        path: PathBuf,
        name: String,
        agent: String,
        preset: String,
    },
    /// Wizard cancelled.
    Cancel,
}

impl EventEmitter<InitWizardEvent> for InitWizard {}

/// Multi-step project init wizard.
pub struct InitWizard {
    step: Step,
    // Step 1: Directory
    directory: String,
    has_surge: bool,
    has_git: bool,
    // Step 2: Project Info
    project_name: String,
    description: String,
    detected_language: Option<String>,
    // Step 3: Agent Setup
    agent_name: String,
    agent_command: String,
    // Step 4: Pipeline
    pipeline_preset: PipelinePreset,
}

impl InitWizard {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            step: Step::Directory,
            directory: String::new(),
            has_surge: false,
            has_git: false,
            project_name: String::new(),
            description: String::new(),
            detected_language: None,
            agent_name: "claude-acp".to_string(),
            agent_command: "claude".to_string(),
            pipeline_preset: PipelinePreset::Fast,
        }
    }

    pub fn with_directory(mut self, dir: &str) -> Self {
        self.directory = dir.to_string();
        self.detect_directory();
        self
    }

    fn detect_directory(&mut self) {
        let path = PathBuf::from(&self.directory);
        self.has_surge = path.join(".surge").exists();
        self.has_git = path.join(".git").exists();

        // Auto-detect name from directory.
        if self.project_name.is_empty() {
            if let Some(name) = path.file_name() {
                self.project_name = name.to_string_lossy().to_string();
            }
        }

        // Auto-detect language.
        if path.join("Cargo.toml").exists() {
            self.detected_language = Some("Rust".to_string());
        } else if path.join("package.json").exists() {
            self.detected_language = Some("JavaScript/TypeScript".to_string());
        } else if path.join("pyproject.toml").exists() {
            self.detected_language = Some("Python".to_string());
        } else if path.join("go.mod").exists() {
            self.detected_language = Some("Go".to_string());
        }
    }

    fn render_stepper(&self) -> Div {
        let steps: Vec<Div> = Step::all()
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
            Step::Directory => self.render_step_directory(),
            Step::ProjectInfo => self.render_step_project_info(),
            Step::AgentSetup => self.render_step_agent(),
            Step::PipelineConfig => self.render_step_pipeline(),
            Step::Confirm => self.render_step_confirm(),
        }
    }

    fn render_step_directory(&self) -> Div {
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Choose Directory".to_string()),
            )
            .child(div().text_sm().text_color(theme::TEXT_MUTED).child(
                "Select an existing folder or create a new one for your project.".to_string(),
            ))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.2))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(if self.directory.is_empty() {
                                theme::TEXT_MUTED
                            } else {
                                theme::TEXT_PRIMARY
                            })
                            .child(if self.directory.is_empty() {
                                "No directory selected".to_string()
                            } else {
                                self.directory.clone()
                            }),
                    ),
            )
            .when(self.has_surge, |el: Div| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme::WARNING)
                        .child("⚠ This directory already has a .surge/ folder".to_string()),
                )
            })
            .when(!self.has_git && !self.directory.is_empty(), |el: Div| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .child("ℹ No .git/ found — git init will be suggested".to_string()),
                )
            })
    }

    fn render_step_project_info(&self) -> Div {
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Project Info".to_string()),
            )
            .child(self.render_field("Name", &self.project_name))
            .child(self.render_field("Description", &self.description))
            .when(self.detected_language.is_some(), |el: Div| {
                let lang = self.detected_language.as_deref().unwrap_or("");
                el.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child("Detected:".to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(theme::PRIMARY.opacity(0.15))
                                .text_color(theme::PRIMARY)
                                .child(lang.to_string()),
                        ),
                )
            })
    }

    fn render_step_agent(&self) -> Div {
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Agent Setup".to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child("Configure the default coding agent.".to_string()),
            )
            .child(self.render_field("Agent Name", &self.agent_name))
            .child(self.render_field("Command", &self.agent_command))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("You can skip this and configure later in Settings.".to_string()),
            )
    }

    fn render_step_pipeline(&self) -> Div {
        let presets = [
            (
                PipelinePreset::Careful,
                "Careful",
                "All gates enabled, max 3 parallel, QA strict",
            ),
            (
                PipelinePreset::Fast,
                "Fast",
                "Only Human Review gate, max 5 parallel, QA lenient",
            ),
            (
                PipelinePreset::Custom,
                "Custom",
                "Configure all options manually",
            ),
        ];

        let preset_items: Vec<Div> = presets
            .iter()
            .map(|(preset, label, desc)| {
                let is_selected = self.pipeline_preset == *preset;
                div()
                    .h_flex()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_selected {
                        theme::PRIMARY
                    } else {
                        theme::TEXT_MUTED.opacity(0.2)
                    })
                    .bg(if is_selected {
                        theme::PRIMARY.opacity(0.1)
                    } else {
                        theme::SURFACE
                    })
                    .child(
                        div()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(label.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(desc.to_string()),
                            ),
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
                    .child("Pipeline Configuration".to_string()),
            )
            .children(preset_items)
    }

    fn render_step_confirm(&self) -> Div {
        let preset_label = match self.pipeline_preset {
            PipelinePreset::Careful => "Careful",
            PipelinePreset::Fast => "Fast",
            PipelinePreset::Custom => "Custom",
        };

        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Confirm".to_string()),
            )
            .child(self.render_summary_row("Directory", &self.directory))
            .child(self.render_summary_row("Project", &self.project_name))
            .child(self.render_summary_row("Agent", &self.agent_name))
            .child(self.render_summary_row("Pipeline", preset_label))
            .when(self.detected_language.is_some(), |el: Div| {
                let lang = self.detected_language.as_deref().unwrap_or("");
                el.child(self.render_summary_row("Language", lang))
            })
    }

    fn render_field(&self, label: &str, value: &str) -> Div {
        div()
            .v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.2))
                    .text_sm()
                    .text_color(if value.is_empty() {
                        theme::TEXT_MUTED
                    } else {
                        theme::TEXT_PRIMARY
                    })
                    .child(if value.is_empty() {
                        format!("Enter {}", label.to_lowercase())
                    } else {
                        value.to_string()
                    }),
            )
    }

    fn render_summary_row(&self, label: &str, value: &str) -> Div {
        div()
            .h_flex()
            .justify_between()
            .px_3()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::TEXT_PRIMARY)
                    .child(value.to_string()),
            )
    }
}

impl Render for InitWizard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _has_next = self.step.next().is_some();
        let has_prev = self.step.prev().is_some();
        let is_last = self.step == Step::Confirm;

        div()
            .v_flex()
            .w(px(550.0))
            .gap_6()
            .p_6()
            .bg(theme::SURFACE)
            .rounded_xl()
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.15))
            // Stepper
            .child(self.render_stepper())
            // Content
            .child(
                div()
                    .v_flex()
                    .min_h(px(200.0))
                    .child(self.render_step_content()),
            )
            // Navigation buttons
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("wizard-cancel")
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(InitWizardEvent::Cancel);
                                    })),
                            )
                            .when(has_prev, |el: Div| {
                                el.child(Button::new("wizard-back").ghost().label("Back").on_click(
                                    cx.listener(|this, _event, _window, cx| {
                                        if let Some(prev) = this.step.prev() {
                                            this.step = prev;
                                            cx.notify();
                                        }
                                    }),
                                ))
                            }),
                    )
                    .child(if is_last {
                        Button::new("wizard-create")
                            .primary()
                            .label("Create Project")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                let preset = match this.pipeline_preset {
                                    PipelinePreset::Careful => "careful",
                                    PipelinePreset::Fast => "fast",
                                    PipelinePreset::Custom => "custom",
                                };
                                cx.emit(InitWizardEvent::Create {
                                    path: PathBuf::from(&this.directory),
                                    name: this.project_name.clone(),
                                    agent: this.agent_name.clone(),
                                    preset: preset.to_string(),
                                });
                            }))
                    } else {
                        Button::new("wizard-next")
                            .primary()
                            .label("Next")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                if let Some(next) = this.step.next() {
                                    this.step = next;
                                    cx.notify();
                                }
                            }))
                    }),
            )
    }
}
