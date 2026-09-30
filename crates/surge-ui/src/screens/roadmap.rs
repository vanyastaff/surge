//! Roadmap — what this project is, what is planned, and how far it got.
//!
//! - **About**: the first paragraph of the project's `project.md`, with a
//!   button that writes (or refreshes) it — the same deterministic scan as
//!   `surge project describe`.
//! - **Progress**: done / verified / in progress / needs attention, counted
//!   from the shown plan.
//! - **Milestones**: a timeline; each milestone expands into its tasks, and
//!   a task expands into its description, acceptance criteria and
//!   dependencies.
//!
//! The plan comes from [`crate::roadmap_source`]: the project's own
//! `roadmap.toml`, else the newest bootstrap plan for this project with its
//! implementation run's task ledger applied — never invented.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Icon, IconName, Selectable, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::roadmap::{
    MilestoneId, RoadmapArtifact, RoadmapMilestone, RoadmapStage, RoadmapStatus, RoadmapTask,
    RoadmapTaskId, TaskPriority, TaskSize,
};

use crate::app_state::AppState;
use crate::roadmap_source::{self, LoadedRoadmap, RoadmapOrigin};
use crate::theme::{self, Semantic};
use crate::ui;

/// What the Roadmap screen asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum RoadmapEvent {
    /// Go to Fleet to describe the app (empty state).
    DescribeApp,
    /// Inspect this plan item in its owning implementation run.
    OpenTask {
        run: surge_core::RunId,
        milestone: MilestoneId,
        task: RoadmapTaskId,
    },
}

impl EventEmitter<RoadmapEvent> for RoadmapScreen {}

/// Which tasks are listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Filter {
    All,
    Open,
    Done,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Open => "Open",
            Self::Done => "Done",
        }
    }

    fn keeps(self, task: &RoadmapTask) -> bool {
        match self {
            Self::All => true,
            Self::Done => is_done(task.status),
            Self::Open => !is_done(task.status),
        }
    }
}

fn is_done(status: RoadmapStatus) -> bool {
    matches!(status, RoadmapStatus::Completed | RoadmapStatus::Skipped)
}

/// Plain-language status and its semantic role.
fn status_look(status: RoadmapStatus) -> (&'static str, Semantic) {
    match status {
        RoadmapStatus::Pending => ("to do", Semantic::External),
        RoadmapStatus::Running => ("in progress", Semantic::Agent),
        RoadmapStatus::Paused => ("paused", Semantic::You),
        RoadmapStatus::ReadyForVerification => ("checking", Semantic::Agent),
        RoadmapStatus::FailedVerification => ("failed check", Semantic::Failure),
        RoadmapStatus::Completed => ("done", Semantic::Verified),
        RoadmapStatus::Failed => ("failed", Semantic::Failure),
        RoadmapStatus::Skipped => ("skipped", Semantic::External),
    }
}

fn size_label(size: TaskSize) -> &'static str {
    match size {
        TaskSize::S => "S",
        TaskSize::M => "M",
        TaskSize::L => "L",
    }
}

/// Counts across the whole plan.
#[derive(Default, Debug, PartialEq, Eq)]
struct Tally {
    total: usize,
    done: usize,
    verified: usize,
    in_progress: usize,
    attention: usize,
}

fn tally(artifact: &RoadmapArtifact) -> Tally {
    let mut t = Tally::default();
    for task in artifact.milestones.iter().flat_map(|m| &m.tasks) {
        t.total += 1;
        if is_done(task.status) {
            t.done += 1;
        }
        if task.verified {
            t.verified += 1;
        }
        match task.status {
            RoadmapStatus::Running | RoadmapStatus::ReadyForVerification => t.in_progress += 1,
            RoadmapStatus::Failed | RoadmapStatus::FailedVerification | RoadmapStatus::Paused => {
                t.attention += 1;
            },
            _ => {},
        }
    }
    t
}

/// First real paragraph of a markdown document, flattened to one line —
/// headings, front matter, code fences and list markers skipped.
fn summary_of(markdown: &str) -> Option<String> {
    let mut lines = markdown.lines().peekable();
    // YAML front matter.
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    let mut paragraph: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        let decoration =
            trimmed.starts_with('<') || trimmed.starts_with("![") || trimmed.starts_with("[![");
        if in_fence
            || decoration
            || trimmed.starts_with('#')
            || trimmed.starts_with('|')
            || trimmed.starts_with('>')
        {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        let text = trimmed
            .trim_start_matches(['-', '*', ' '])
            .replace("**", "")
            .replace('`', "");
        paragraph.push(text);
    }
    let joined = paragraph.join(" ");
    let joined = joined.trim();
    if joined.is_empty() {
        return None;
    }
    Some(ui::headline(joined, 360))
}

/// Detected facts from a `project.md` fact sheet: each `## Section` and its
/// first lines, minus anything the scanner could not detect.
fn facts_of(markdown: &str) -> Vec<(String, String)> {
    let mut facts = Vec::new();
    let mut section: Option<String> = None;
    let mut values: Vec<String> = Vec::new();
    let mut flush = |section: &mut Option<String>, values: &mut Vec<String>| {
        if let Some(title) = section.take() {
            let value = values.join(", ");
            let lower = value.to_lowercase();
            let unknown = value.is_empty()
                || lower == "unknown"
                || lower.contains("not detected")
                || lower.starts_with("no ");
            if !unknown && !title.eq_ignore_ascii_case("project name") {
                facts.push((title, ui::headline(&value, 80)));
            }
        }
        values.clear();
    };
    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("## ") {
            flush(&mut section, &mut values);
            section = Some(title.trim().to_string());
        } else if section.is_some() && !trimmed.is_empty() && !trimmed.starts_with("<!--") {
            values.push(trimmed.trim_start_matches(['-', '*', ' ']).replace('`', ""));
        }
    }
    flush(&mut section, &mut values);
    facts
}

/// Roadmap screen.
pub struct RoadmapScreen {
    state: Entity<AppState>,
    roadmap: Option<LoadedRoadmap>,
    loading: bool,
    about: Option<String>,
    /// Detected stack facts from `project.md`; `None` when it does not exist.
    facts: Option<Vec<(String, String)>>,
    describing: bool,
    describe_error: Option<String>,
    expanded: HashSet<MilestoneId>,
    open_task: Option<(MilestoneId, RoadmapTaskId)>,
    filter: Filter,
    /// What the last load was for; a state change that leaves it equal
    /// (streaming events, UI toggles) does not touch the disk again.
    loaded_for: Option<(Option<PathBuf>, Vec<String>)>,
}

impl RoadmapScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this: &mut Self, _state, cx| {
            this.reload_if_changed(cx)
        })
        .detach();
        let mut this = Self {
            state,
            roadmap: None,
            loading: false,
            about: None,
            facts: None,
            describing: false,
            describe_error: None,
            expanded: HashSet::new(),
            open_task: None,
            filter: Filter::All,
            loaded_for: None,
        };
        this.reload_if_changed(cx);
        this
    }

    /// Project path + every run's status: the plan or its ledger can only
    /// have moved when one of these did.
    fn fingerprint(&self, cx: &Context<Self>) -> (Option<PathBuf>, Vec<String>) {
        let state = self.state.read(cx);
        let runs = state
            .project_runs()
            .iter()
            .map(|r| format!("{}:{:?}:{:?}", r.run_id, r.status, r.last_event_seq))
            .collect();
        (state.project_path.clone(), runs)
    }

    fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        let fingerprint = self.fingerprint(cx);
        if self.loaded_for.as_ref() == Some(&fingerprint) {
            return;
        }
        self.loaded_for = Some(fingerprint);
        self.reload(cx);
    }

    fn project_md_path(&self, cx: &Context<Self>) -> Option<PathBuf> {
        let state = self.state.read(cx);
        let root = state.project_path.clone()?;
        let configured = state.config.as_ref().map_or_else(
            || PathBuf::from("project.md"),
            |c| c.init.project_context_path.clone(),
        );
        Some(if configured.is_absolute() {
            configured
        } else {
            root.join(configured)
        })
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.state.read(cx).project_path.clone() else {
            self.roadmap = None;
            self.about = None;
            self.facts = None;
            cx.notify();
            return;
        };
        self.about = ["README.md", "readme.md", "README"]
            .iter()
            .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
            .as_deref()
            .and_then(summary_of);
        self.facts = self
            .project_md_path(cx)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|md| facts_of(&md));
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let loaded = match roadmap_source::project_file(&root) {
                Some(found) => Some(found),
                None => match surge_core::home::surge_home_dir() {
                    Some(home) => roadmap_source::planned(&root, &home).await,
                    None => None,
                },
            };
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.loading = false;
                    // Open the first milestone that is not finished yet.
                    if screen.expanded.is_empty()
                        && let Some(m) = loaded.as_ref().and_then(|l| {
                            l.artifact
                                .milestones
                                .iter()
                                .find(|m| !is_done(m.status))
                                .or_else(|| l.artifact.milestones.first())
                        })
                    {
                        screen.expanded.insert(m.id.clone());
                    }
                    screen.roadmap = loaded;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Write `project.md` with the deterministic scanner, off the UI thread.
    fn describe_project(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(output)) = (
            self.state.read(cx).project_path.clone(),
            self.project_md_path(cx),
        ) else {
            return;
        };
        self.describing = true;
        self.describe_error = None;
        cx.notify();
        let task = cx.background_spawn(async move {
            let mut options =
                surge_orchestrator::project_context::ProjectContextOptions::new(root, output);
            options.refresh = true;
            surge_orchestrator::project_context::describe_project(options)
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = task.await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.describing = false;
                    match result {
                        Ok(outcome) => {
                            tracing::info!(
                                status = outcome.status.as_str(),
                                "project.md described from the UI"
                            );
                            screen.facts = std::fs::read_to_string(&outcome.output_path)
                                .ok()
                                .map(|md| facts_of(&md));
                        },
                        Err(error) => {
                            tracing::warn!(%error, "project describe failed");
                            screen.describe_error = Some(error.to_string());
                        },
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    // ── panes ───────────────────────────────────────────────────────

    fn render_filter(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .h_flex()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(ui::R_CONTROL + 1.0))
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel_deep());
        for filter in [Filter::All, Filter::Open, Filter::Done] {
            let active = self.filter == filter;
            row = row.child(
                Button::new(SharedString::from(format!(
                    "roadmap-filter-{}",
                    filter.label()
                )))
                .ghost()
                .small()
                .selected(active)
                .label(filter.label())
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.filter = filter;
                    cx.notify();
                })),
            );
        }
        row
    }

    fn render_stats(&self, tally: &Tally) -> Div {
        let pct = if tally.total == 0 {
            0.0
        } else {
            tally.done as f32 / tally.total as f32
        };
        let tile = |label: &'static str, value: String, note: String, color: Hsla| {
            ui::panel()
                .flex_1()
                .min_w(px(0.0))
                .v_flex()
                .gap(px(4.0))
                .px(px(14.0))
                .py(px(12.0))
                .child(ui::section_label(label))
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .child(value),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme::text_dim())
                        .child(note),
                )
        };
        div()
            .flex()
            .flex_row()
            .items_stretch()
            .gap(px(10.0))
            .child(
                tile(
                    "Progress",
                    format!("{:.0}%", pct * 100.0),
                    format!("{} of {} tasks done", tally.done, tally.total),
                    theme::text_primary(),
                )
                .child(progress_bar(pct, theme::success())),
            )
            .child(tile(
                "Verified",
                tally.verified.to_string(),
                "proven by a verifier".into(),
                theme::success(),
            ))
            .child(tile(
                "In progress",
                tally.in_progress.to_string(),
                "agents working now".into(),
                theme::accent(),
            ))
            .child(tile(
                "Needs you",
                tally.attention.to_string(),
                "failed or paused".into(),
                if tally.attention > 0 {
                    theme::error()
                } else {
                    theme::text_muted()
                },
            ))
    }

    fn render_about(&self, cx: &mut Context<Self>) -> Div {
        let described = self.facts.is_some();
        let facts = self.facts.clone().unwrap_or_default();
        ui::panel()
            .h_flex()
            .gap(px(16.0))
            .items_start()
            .p(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(10.0))
                    .child(ui::section_label("About this project"))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(19.0))
                            .text_color(if self.about.is_some() {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .child(self.about.clone().unwrap_or_else(|| {
                                "No README description yet.".to_string()
                            })),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .children(facts.into_iter().map(|(k, v)| {
                                div()
                                    .h_flex()
                                    .gap(px(6.0))
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(ui::R_CONTROL))
                                    .border_1()
                                    .border_color(theme::hairline())
                                    .bg(theme::panel_deep())
                                    .text_size(px(10.5))
                                    .child(div().text_color(theme::text_dim()).child(k))
                                    .child(div().text_color(theme::text_primary()).child(v))
                            }))
                            .when(described && self.facts.as_ref().is_some_and(Vec::is_empty), |el| {
                                el.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(theme::text_dim())
                                        .child("No stack detected yet — it appears once there is code."),
                                )
                            })
                            .when(!described, |el| {
                                el.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(theme::text_dim())
                                        .child("Scan the repository so every new run starts with its stack and commands."),
                                )
                            }),
                    )
                    .children(self.describe_error.clone().map(|e| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::error())
                            .child(format!("Could not scan the project: {e}"))
                    })),
            )
            .child(
                Button::new("roadmap-describe")
                    .outline()
                    .small()
                    .icon(Lucide::Sparkles)
                    .label(if described { "Rescan" } else { "Scan project" })
                    .loading(self.describing)
                    .disabled(self.describing)
                    .on_click(cx.listener(|this, _e, _w, cx| this.describe_project(cx))),
            )
    }

    fn render_task(
        &self,
        milestone_id: &MilestoneId,
        task: &RoadmapTask,
        cx: &mut Context<Self>,
    ) -> Div {
        let (label, role) = status_look(task.status);
        let color = role.color();
        let key = (milestone_id.clone(), task.id.clone());
        let open = self.open_task.as_ref() == Some(&key);
        let toggle_key = key.clone();
        let done_unverified = task.status == RoadmapStatus::Completed && !task.verified;

        let row = div()
            .id(SharedString::from(format!(
                "task-{milestone_id}-{}",
                task.id
            )))
            .role(Role::Button)
            .aria_label(task.title.clone())
            .h_flex()
            .gap(px(10.0))
            .items_center()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(ui::R_CONTROL))
            .cursor_pointer()
            .when(open, |el| el.bg(theme::surface()))
            .hover(|s: StyleRefinement| s.bg(theme::surface()))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                this.open_task = if this.open_task.as_ref() == Some(&toggle_key) {
                    None
                } else {
                    Some(toggle_key.clone())
                };
                cx.notify();
            }))
            .child(ui::status_dot(if done_unverified {
                theme::slate()
            } else {
                color
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .text_color(if is_done(task.status) {
                        theme::text_muted()
                    } else {
                        theme::text_primary()
                    })
                    .truncate()
                    .child(task.title.clone()),
            )
            .children(task.priority.map(priority_chip))
            .children(task.parallel_group.as_deref().map(group_chip))
            .child(if task.verified {
                div()
                    .h_flex()
                    .gap(px(4.0))
                    .items_center()
                    .text_size(px(10.0))
                    .text_color(theme::success())
                    .child(Icon::new(Lucide::ShieldCheck).size(px(12.0)))
                    .child("verified")
            } else if done_unverified {
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_dim())
                    .child("done · unverified")
            } else {
                ui::pill(label, color, theme::tint(color))
            })
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(12.0))
                .text_color(theme::text_dim()),
            );

        div().v_flex().child(row).when(open, |el| {
            let mut details = el;
            if let Some(LoadedRoadmap {
                origin:
                    RoadmapOrigin::Planned {
                        implementation_run, ..
                    },
                ..
            }) = &self.roadmap
            {
                let event = RoadmapEvent::OpenTask {
                    run: *implementation_run,
                    milestone: milestone_id.clone(),
                    task: task.id.clone(),
                };
                details = details.child(
                    div().px(px(14.0)).pb(px(12.0)).child(
                        Button::new(SharedString::from(format!(
                            "process-{milestone_id}-{}",
                            task.id
                        )))
                        .label("Open task process")
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(event.clone()))),
                    ),
                );
            }
            details.child(render_task_details(task))
        })
    }

    fn render_milestone(
        &self,
        index: usize,
        last: bool,
        milestone: &RoadmapMilestone,
        cx: &mut Context<Self>,
    ) -> Div {
        let (label, role) = status_look(milestone.status);
        let color = role.color();
        let total = milestone.tasks.len();
        let done = milestone.tasks.iter().filter(|t| is_done(t.status)).count();
        let verified = milestone.tasks.iter().filter(|t| t.verified).count();
        let pct = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        let expanded = self.expanded.contains(&milestone.id);
        let id = milestone.id.clone();
        let tasks: Vec<Div> = if expanded {
            milestone
                .tasks
                .iter()
                .filter(|t| self.filter.keeps(t))
                .map(|t| self.render_task(&milestone.id, t, cx))
                .collect()
        } else {
            Vec::new()
        };
        let hidden_by_filter = expanded && tasks.is_empty() && total > 0;

        // Left rail: numbered node + the line to the next milestone.
        let rail = div()
            .v_flex()
            .items_center()
            .w(px(28.0))
            .flex_none()
            .child(
                div()
                    .size(px(26.0))
                    .rounded_full()
                    .border_1()
                    .border_color(theme::stroke(color))
                    .bg(theme::tint(color))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .child(if is_done(milestone.status) {
                        Icon::new(IconName::Check).size(px(12.0)).into_any_element()
                    } else {
                        (index + 1).to_string().into_any_element()
                    }),
            )
            .when(!last, |el| {
                el.child(
                    div()
                        .w(px(1.0))
                        .flex_1()
                        .min_h(px(16.0))
                        .bg(theme::hairline_strong()),
                )
            });

        let header = div()
            .id(SharedString::from(format!("milestone-{id}")))
            .role(Role::Button)
            .aria_label(format!("Milestone {}", milestone.title))
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(14.0))
            .py(px(12.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _e, _w, cx| {
                if !this.expanded.remove(&id) {
                    this.expanded.insert(id.clone());
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(6.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .truncate()
                                    .child(milestone.title.clone()),
                            )
                            .child(ui::pill(label, color, theme::tint(color))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(12.0))
                            .items_center()
                            .child(div().w(px(180.0)).child(progress_bar(pct, color)))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_dim())
                                    .child(format!("{done}/{total} done · {verified} verified")),
                            ),
                    ),
            )
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(14.0))
                .text_color(theme::text_muted()),
            );

        div()
            .h_flex()
            .items_stretch()
            .gap(px(12.0))
            .child(rail)
            .child(
                div().flex_1().min_w(px(0.0)).pb(px(12.0)).child(
                    ui::panel()
                        .v_flex()
                        .overflow_hidden()
                        .child(header)
                        .when(expanded, |el| {
                            el.child(
                                div()
                                    .v_flex()
                                    .gap(px(1.0))
                                    .px(px(6.0))
                                    .pb(px(8.0))
                                    .pt(px(2.0))
                                    .border_t_1()
                                    .border_color(theme::hairline())
                                    .children(tasks)
                                    .when(hidden_by_filter, |el| {
                                        el.child(
                                            div()
                                                .px(px(10.0))
                                                .py(px(8.0))
                                                .text_size(px(11.0))
                                                .text_color(theme::text_dim())
                                                .child(format!(
                                                    "No {} tasks here.",
                                                    self.filter.label().to_lowercase()
                                                )),
                                        )
                                    }),
                            )
                        }),
                ),
            )
    }

    fn render_risks(&self, artifact: &RoadmapArtifact) -> Option<Div> {
        if artifact.risks.is_empty() {
            return None;
        }
        let rows = artifact.risks.iter().map(|risk| {
            div()
                .h_flex()
                .gap(px(10.0))
                .items_start()
                .py(px(6.0))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(12.0))
                        .text_color(theme::warning()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .v_flex()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(theme::text_primary())
                                .child(risk.description.clone()),
                        )
                        .children(risk.mitigation.clone().map(|m| {
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(format!("Mitigation: {m}"))
                        })),
                )
        });
        Some(
            ui::panel()
                .v_flex()
                .p(px(16.0))
                .gap(px(4.0))
                .child(ui::section_label("Risks"))
                .children(rows),
        )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> Div {
        ui::panel().child(
            ui::empty_state(
                "◇",
                "No plan yet",
                "Describe your app and Surge drafts milestones and tasks you approve before \
                 any code is written.",
            )
            .child(
                Button::new("roadmap-describe-app")
                    .primary()
                    .icon(IconName::Plus)
                    .label("Describe an app")
                    .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(RoadmapEvent::DescribeApp))),
            ),
        )
    }
}

/// One row of the milestone timeline.
enum Row<'a> {
    Stage(usize, &'a RoadmapStage),
    Milestone(&'a RoadmapMilestone),
}

/// The timeline in reading order: each stage banner followed by its
/// milestones, or the plain milestone list when no stages are declared.
/// Milestones a stage names but the roadmap lacks are skipped, never invented.
fn layout_rows(artifact: &RoadmapArtifact) -> Vec<Row<'_>> {
    if artifact.stages.is_empty() {
        return artifact.milestones.iter().map(Row::Milestone).collect();
    }
    let mut rows = Vec::new();
    for (index, stage) in artifact.stages.iter().enumerate() {
        rows.push(Row::Stage(index, stage));
        rows.extend(
            stage
                .milestones
                .iter()
                .filter_map(|id| artifact.milestones.iter().find(|m| &m.id == id))
                .map(Row::Milestone),
        );
    }
    rows
}

/// P0..P3 as a small pill; P0 is the only one that shouts.
fn priority_chip(priority: TaskPriority) -> Div {
    let role = match priority {
        TaskPriority::P0 => Semantic::Failure,
        TaskPriority::P1 => Semantic::You,
        TaskPriority::P2 | TaskPriority::P3 => Semantic::External,
    };
    let color = role.color();
    ui::pill(
        priority.to_string().to_uppercase(),
        color,
        theme::tint(color),
    )
}

/// Tasks sharing a group run at the same time.
fn group_chip(group: &str) -> Div {
    let color = Semantic::Loop.color();
    ui::pill(format!("∥ {group}"), color, theme::tint(color))
}

/// A stage banner: its title and goal, then the conditions that end it.
fn render_stage_header(index: usize, stage: &RoadmapStage) -> Div {
    div()
        .v_flex()
        .gap(px(4.0))
        .mt(px(if index == 0 { 0.0 } else { 14.0 }))
        .pb(px(6.0))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .h_flex()
                .gap(px(8.0))
                .items_center()
                .child(ui::pill(
                    format!("Stage {}", index + 1),
                    Semantic::Plan.color(),
                    theme::tint(Semantic::Plan.color()),
                ))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child(stage.title.clone()),
                ),
        )
        .when(!stage.goal.trim().is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(stage.goal.clone()),
            )
        })
        .when(!stage.exit_criteria.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::text_dim())
                    .child(format!("Done when: {}", stage.exit_criteria.join(" · "))),
            )
        })
}

fn progress_bar(pct: f32, color: Hsla) -> Div {
    div()
        .w_full()
        .h(px(4.0))
        .rounded_full()
        .bg(theme::hairline())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(color)
                .w(relative(pct.clamp(0.0, 1.0))),
        )
}

fn render_task_details(task: &RoadmapTask) -> Div {
    let mut body = div()
        .v_flex()
        .gap(px(10.0))
        .ml(px(27.0))
        .mr(px(10.0))
        .mb(px(8.0))
        .p(px(12.0))
        .rounded(px(ui::R_CONTROL))
        .bg(theme::panel_deep())
        .border_1()
        .border_color(theme::hairline());
    if let Some(desc) = task.description.as_ref().filter(|d| !d.trim().is_empty()) {
        body = body.child(
            div()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(theme::text_primary())
                .child(desc.clone()),
        );
    }
    if !task.acceptance_criteria.is_empty() {
        let done = is_done(task.status) && task.verified;
        body = body.child(
            div()
                .v_flex()
                .gap(px(5.0))
                .child(ui::section_label("Done when"))
                .children(task.acceptance_criteria.iter().map(|criterion| {
                    div()
                        .h_flex()
                        .gap(px(8.0))
                        .items_start()
                        .child(
                            Icon::new(if done {
                                Lucide::CircleCheck
                            } else {
                                Lucide::CircleDot
                            })
                            .size(px(12.0))
                            .text_color(if done {
                                theme::success()
                            } else {
                                theme::text_dim()
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .line_height(px(17.0))
                                .text_color(theme::text_muted())
                                .child(criterion.clone()),
                        )
                })),
        );
    }
    let mut meta = div()
        .h_flex()
        .gap(px(12.0))
        .text_size(px(10.5))
        .text_color(theme::text_dim())
        .child(task.id.to_string());
    if let Some(size) = task.size {
        meta = meta.child(format!("size {}", size_label(size)));
    }
    if !task.depends_on.is_empty() {
        meta = meta.child(format!("after {}", task.depends_on.join(", ")));
    }
    if let Some(from) = &task.discovered_from {
        meta = meta.child(format!("found while doing {from}"));
    }
    body.child(meta)
}

impl Render for RoadmapScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let roadmap = self.roadmap.clone();
        let subtitle = roadmap.as_ref().map(|r| match &r.origin {
            RoadmapOrigin::ProjectFile(path) => {
                SharedString::from(format!("From {}", ui::abbreviate_home(path)))
            },
            RoadmapOrigin::Planned { prompt, .. } => {
                SharedString::from(format!("Planned from “{}”", ui::headline(prompt, 70)))
            },
        });

        let mut content = div()
            .v_flex()
            .gap(px(16.0))
            .max_w(px(1040.0))
            .w_full()
            .child(ui::page_header(
                "Roadmap",
                subtitle,
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .when(roadmap.is_some(), |el| el.child(self.render_filter(cx)))
                    .child(
                        Button::new("roadmap-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .loading(self.loading)
                            .tooltip("Reload")
                            .on_click(cx.listener(|this, _e, _w, cx| this.reload(cx))),
                    ),
            ));

        match roadmap.as_ref() {
            Some(loaded) => {
                let artifact = &loaded.artifact;
                content = content.child(self.render_stats(&tally(artifact)));
                content = content.child(self.render_about(cx));
                let n = artifact.milestones.len();
                let mut milestones: Vec<Div> = Vec::new();
                let mut index = 0;
                for row in layout_rows(artifact) {
                    match row {
                        Row::Stage(stage_index, stage) => {
                            milestones.push(render_stage_header(stage_index, stage));
                        },
                        Row::Milestone(m) => {
                            milestones.push(self.render_milestone(index, index + 1 == n, m, cx));
                            index += 1;
                        },
                    }
                }
                content = content
                    .child(ui::section_label(format!("Milestones · {n}")))
                    .child(div().v_flex().children(milestones))
                    .children(self.render_risks(artifact));
            },
            None if self.loading => {},
            None => {
                content = content
                    .child(self.render_about(cx))
                    .child(self.render_empty(cx));
            },
        }

        div()
            .id("roadmap-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::background())
            .flex()
            .items_start()
            .justify_center()
            .px(px(28.0))
            .pt(px(22.0))
            .pb(px(32.0))
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::{RoadmapArtifact, RoadmapScreen, Row, layout_rows, summary_of, tally};
    use crate::app_state::AppState;
    use gpui_kit::{AppContext, TestAppContext};

    #[gpui_kit::test]
    fn missing_roadmap_never_invents_milestones(cx: &mut TestAppContext) {
        let screen = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            cx.new(|cx| RoadmapScreen::new(state, cx))
        });
        screen.update(cx, |screen, _cx| assert!(screen.roadmap.is_none()));
    }

    #[test]
    fn summary_takes_the_first_prose_paragraph() {
        let md = "---\ntitle: x\n---\n# Pomodoro\n\n```sh\nnpm i\n```\n\nA **small** timer app\nfor `focus` sessions.\n\nSecond paragraph.";
        assert_eq!(
            summary_of(md).as_deref(),
            Some("A small timer app for focus sessions.")
        );
        assert_eq!(summary_of("# Only a title\n"), None);
    }

    #[test]
    fn summary_skips_readme_decoration() {
        let md = "<p align=\"center\"><img src=\"x.svg\"/></p>\n[![ci](b.svg)](x)\n\nTurn ideas into apps.\n";
        assert_eq!(summary_of(md).as_deref(), Some("Turn ideas into apps."));
    }

    #[test]
    fn facts_drop_what_the_scanner_could_not_detect() {
        let md = "<!-- surge:project-context x -->\n## Project name\ndemo\n\n## Primary language\nRust\n\n## Framework\nNot detected\n\n## Build commands\n- cargo build\n- cargo test\n\n## Tests\n- Test layout not detected from scanned files.\n";
        assert_eq!(
            super::facts_of(md),
            vec![
                ("Primary language".to_string(), "Rust".to_string()),
                (
                    "Build commands".to_string(),
                    "cargo build, cargo test".to_string()
                ),
            ]
        );
    }

    #[test]
    fn tally_counts_verification_separately_from_completion() {
        let artifact: surge_core::roadmap::RoadmapArtifact = toml::from_str(
            r#"
schema_version = 2
[[milestones]]
id = "m1"
title = "M"
status = "running"
[[milestones.tasks]]
id = "a"
title = "A"
status = "completed"
verified = true
[[milestones.tasks]]
id = "b"
title = "B"
status = "completed"
[[milestones.tasks]]
id = "c"
title = "C"
status = "failed"
"#,
        )
        .unwrap();
        let t = tally(&artifact);
        assert_eq!((t.total, t.done, t.verified, t.attention), (3, 2, 1, 1));
    }

    #[test]
    fn stages_group_their_milestones_and_none_means_a_flat_list() {
        let flat: RoadmapArtifact =
            toml::from_str("schema_version = 2\n[[milestones]]\nid = \"m1\"\ntitle = \"One\"\n")
                .unwrap();
        assert!(matches!(layout_rows(&flat)[..], [Row::Milestone(_)]));

        let staged: RoadmapArtifact = toml::from_str(
            r#"
schema_version = 2
[[stages]]
id = "s1"
title = "Usable core"
milestones = ["m1", "m2"]
[[milestones]]
id = "m1"
title = "One"
[[milestones]]
id = "m2"
title = "Two"
"#,
        )
        .unwrap();
        let rows = layout_rows(&staged);
        assert!(matches!(rows[0], Row::Stage(0, _)));
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[2], Row::Milestone(m) if m.id == "m2"));
    }
}
