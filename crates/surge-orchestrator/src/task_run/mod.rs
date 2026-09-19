//! Task-run flow selection (ADR-0020).
//!
//! A queued task does not run one project-wide graph. It runs a **flow
//! template** chosen for it: a pinned `RoadmapTask.flow` if one is set, else
//! a template selected by the task's size and source from the flow catalog.
//!
//! ## Selection is deterministic in v1
//!
//! The spec's long-term shape has a classifier stage (one cheap model turn
//! over `{{flow_catalog}}`) that can answer `use: <ref>` or `compose`, with a
//! generator writing a new template into `.surge/flows/` behind the approval
//! gate. Until that lands, selection is a pure function of the task:
//!
//! - `s` → `linear-3` (the smallest bundled template that carries a
//!   verifier; `single-task` declares none, so the sealed-verifier rule
//!   refuses it as a task flow — correctly, and by design)
//! - `m` → `linear-with-review` (implement + review + verify)
//! - `l` → `linear-3` (spec, implement, verify — sized for a fuller session)
//! - unknown size → `linear-with-review` (the conservative middle)
//!
//! A pinned reference is resolved against the catalog and **never falls
//! back**: an unresolvable pin is the error the operator needs to see.
//! `compose` is not reachable in v1 — [`TaskFlowError::ComposeUnavailable`]
//! says so by name instead of silently pretending.
//!
//! ## Every selected flow passes task validation
//!
//! [`select_task_flow`] runs [`crate::engine::validate::validate_for_task`]
//! with the run's runtime count before returning. A template whose success
//! is reachable without a verifier, or that verifies in a single vendor when
//! a second one is configured, is refused at dispatch time.

use std::path::Path;

use surge_core::roadmap::{RoadmapTask, TaskSize};
use surge_core::{FlowCatalog, FlowRef, Graph, ProjectLayer};

use crate::engine::validate::{FlowPurpose, validate_for_task};

/// Why a task could not be given a flow.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TaskFlowError {
    /// The task pins a template the catalog does not contain.
    #[error("task {task_id:?} pins flow {reference} which is not in the catalog")]
    PinnedFlowNotFound {
        /// Task whose pin failed.
        task_id: String,
        /// The reference that did not resolve.
        reference: String,
    },
    /// The task's size maps to a template that is missing from the catalog.
    #[error("no template for size {size} in the catalog (expected {expected})")]
    NoTemplateForSize {
        /// Size that was mapped.
        size: String,
        /// Template the mapping expected.
        expected: String,
    },
    /// A classifier would have been asked to compose a new template; that
    /// path is not implemented in v1.
    #[error(
        "flow composition (`compose`) is not available in this build; pin the task's flow \
         with `flow = \"name@MAJOR\"` in .surge/roadmap.toml or extend the task's size mapping"
    )]
    ComposeUnavailable,
    /// The catalog could not be scanned.
    #[error("scan flow catalog: {0}")]
    Catalog(String),
    /// The selected graph failed task validation.
    #[error("selected flow {reference} is not a valid task flow: {reason}")]
    Validation {
        /// Template that failed.
        reference: String,
        /// Rendered validation error.
        reason: String,
    },
}

/// Which template a task's size selects when nothing is pinned.
#[must_use]
pub fn template_name_for_size(size: Option<TaskSize>) -> &'static str {
    match size {
        // `single-task` has no verifier and is refused as a task flow; the
        // smallest valid task flow is `linear-3`.
        Some(TaskSize::S) => "linear-3",
        Some(TaskSize::M) => "linear-with-review",
        Some(TaskSize::L) => "linear-3",
        // Unknown size is treated as the conservative middle: a task whose
        // size nobody declared might be substantial, and the review template
        // is the cheaper way to be wrong.
        None => "linear-with-review",
    }
}

/// The flow a task will run, with the reference it resolved at.
#[derive(Debug)]
pub struct SelectedFlow {
    /// Reference of the winning template.
    pub reference: FlowRef,
    /// Layer the template came from.
    pub layer: surge_core::Layer,
    /// Parsed graph, ready to start.
    pub graph: Graph,
    /// True when the task pinned this reference explicitly.
    pub pinned: bool,
}

/// Select, validate and return the flow for `task`.
///
/// `home_flows_dir` is the operator's `SURGE_HOME/flows`; `runtime_count` is
/// how many distinct runtimes the run could dispatch to (drives the
/// same-runtime verification rule).
///
/// # Errors
/// [`TaskFlowError`] for an unresolvable pin, a missing size template, a
/// catalog scan failure, or a graph that fails task validation.
pub fn select_task_flow(
    task: &RoadmapTask,
    project: &ProjectLayer,
    home_flows_dir: Option<&Path>,
    runtime_count: usize,
) -> Result<SelectedFlow, TaskFlowError> {
    let catalog = FlowCatalog::scan(project, home_flows_dir)
        .map_err(|e| TaskFlowError::Catalog(e.to_string()))?;

    let (reference, pinned) = match &task.flow {
        Some(pin) => {
            let entry = catalog
                .resolve(pin)
                .map_err(|_| TaskFlowError::PinnedFlowNotFound {
                    task_id: task.id.clone(),
                    reference: pin.to_string(),
                })?;
            (entry.reference.clone(), true)
        },
        None => {
            let name = template_name_for_size(task.size);
            // A bare `name@MAJOR`; the catalog picks the highest minor.
            let reference: FlowRef =
                format!("{name}@1")
                    .parse()
                    .map_err(|_| TaskFlowError::NoTemplateForSize {
                        size: task
                            .size
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "unknown".into()),
                        expected: name.to_string(),
                    })?;
            let entry =
                catalog
                    .resolve(&reference)
                    .map_err(|_| TaskFlowError::NoTemplateForSize {
                        size: task
                            .size
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "unknown".into()),
                        expected: name.to_string(),
                    })?;
            (entry.reference.clone(), false)
        },
    };

    let entry = catalog
        .resolve(&reference)
        .map_err(|_| TaskFlowError::PinnedFlowNotFound {
            task_id: task.id.clone(),
            reference: reference.to_string(),
        })?;

    validate_for_task(
        &entry.graph,
        FlowPurpose::Task,
        None,
        runtime_count,
        entry.autonomy,
    )
    .map_err(|e| TaskFlowError::Validation {
        reference: reference.to_string(),
        reason: e.to_string(),
    })?;

    Ok(SelectedFlow {
        reference,
        layer: entry.layer,
        graph: entry.graph.clone(),
        pinned,
    })
}

/// Render the task into the run's initial prompt.
///
/// The text is both seeded as the run's `user_prompt` artifact (so a profile
/// binding `InitialPrompt` receives it) and appended to each agent node's
/// system prompt by [`apply_task_context`].
#[must_use]
pub fn render_task_prompt(task: &RoadmapTask) -> String {
    use std::fmt::Write;

    let mut s = String::new();
    s.push_str("You are executing one queued project task.\n\n");
    let _ = writeln!(s, "Task: {} — {}", task.id, task.title);
    if let Some(description) = &task.description {
        let _ = write!(s, "\nDescription:\n{description}\n");
    }
    if !task.acceptance_criteria.is_empty() {
        s.push_str("\nAcceptance criteria:\n");
        for criterion in &task.acceptance_criteria {
            let _ = writeln!(s, "- {criterion}");
        }
    }
    s.push_str(
        "\nImplement the task in this worktree, run the project's tests, and report the \
         outcome.",
    );
    s
}

/// Append the task context to every agent node in `graph`.
///
/// The bundled archetypes bind no task artifact (`bindings = []`), so without
/// this the agent sees only its profile's generic prompt and has no idea which
/// of the roadmap's tasks it is running. The append is non-destructive: a node
/// that already carries an authored `prompt_overrides` keeps it, with the task
/// text appended after — an author's system prompt wins, the task is context.
#[must_use]
pub fn apply_task_context(graph: &Graph, task: &RoadmapTask) -> Graph {
    use surge_core::agent_config::PromptOverride;
    use surge_core::node::NodeConfig;

    let task_text = render_task_prompt(task);
    let mut graph = graph.clone();
    for node in graph.nodes.values_mut() {
        if let NodeConfig::Agent(cfg) = &mut node.config {
            let existing = cfg.prompt_overrides.clone().unwrap_or(PromptOverride {
                system: None,
                append_system: None,
            });
            let append = match existing.append_system {
                Some(prior) => format!("{prior}\n\n{task_text}"),
                None => task_text.clone(),
            };
            cfg.prompt_overrides = Some(PromptOverride {
                system: existing.system,
                append_system: Some(append),
            });
        }
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(size: Option<TaskSize>, flow: Option<&str>) -> RoadmapTask {
        let mut task = RoadmapTask::new("t1", "Do the thing");
        task.size = size;
        task.flow = flow.map(|f| f.parse().unwrap());
        task
    }

    fn project() -> (tempfile::TempDir, ProjectLayer) {
        let dir = tempfile::tempdir().unwrap();
        let layer = ProjectLayer::for_project(dir.path());
        (dir, layer)
    }

    #[test]
    fn render_task_prompt_carries_id_title_and_criteria() {
        let mut t = task(Some(TaskSize::S), None);
        t.acceptance_criteria.push("tests pass".into());
        let rendered = render_task_prompt(&t);
        assert!(rendered.contains("t1"));
        assert!(rendered.contains("Do the thing"));
        assert!(rendered.contains("tests pass"));
    }

    #[test]
    fn apply_task_context_appends_to_every_agent_node() {
        let (_dir, layer) = project();
        let selected = select_task_flow(&task(Some(TaskSize::M), None), &layer, None, 1).unwrap();
        let with_context = apply_task_context(&selected.graph, &task(Some(TaskSize::M), None));
        for node in with_context.nodes.values() {
            if let surge_core::node::NodeConfig::Agent(cfg) = &node.config {
                let overrides = cfg.prompt_overrides.as_ref().unwrap();
                assert!(
                    overrides
                        .append_system
                        .as_deref()
                        .unwrap_or_default()
                        .contains("Do the thing"),
                    "agent node {} must carry the task text",
                    node.id
                );
            }
        }
    }

    #[test]
    fn size_selects_the_documented_template() {
        assert_eq!(template_name_for_size(Some(TaskSize::S)), "linear-3");
        assert_eq!(
            template_name_for_size(Some(TaskSize::M)),
            "linear-with-review"
        );
        assert_eq!(template_name_for_size(Some(TaskSize::L)), "linear-3");
        assert_eq!(template_name_for_size(None), "linear-with-review");
    }

    #[test]
    fn unknown_pin_is_a_named_error_not_a_fallback() {
        let (_dir, layer) = project();
        let err = select_task_flow(&task(Some(TaskSize::S), Some("nope@1")), &layer, None, 1)
            .unwrap_err();
        assert!(matches!(err, TaskFlowError::PinnedFlowNotFound { .. }));
    }

    #[test]
    fn pinned_flow_wins_over_size() {
        let (_dir, layer) = project();
        // `spike@1` is bundled and has no verifier; as a task flow it must
        // be refused (this is the sealed-verifier rule, not a fallback).
        let err = select_task_flow(&task(Some(TaskSize::L), Some("spike@1")), &layer, None, 1)
            .unwrap_err();
        assert!(
            matches!(err, TaskFlowError::Validation { .. }),
            "expected validation refusal, got {err:?}"
        );
    }

    #[test]
    fn small_task_selects_a_template_with_a_verifier() {
        let (_dir, layer) = project();
        let selected = select_task_flow(&task(Some(TaskSize::S), None), &layer, None, 1).unwrap();
        assert_eq!(selected.reference.name(), "linear-3");
    }

    #[test]
    fn unverified_template_is_refused_even_when_pinned() {
        // `single-task` declares no verifier; a task that pins it must be
        // refused, not silently upgraded to another template.
        let (_dir, layer) = project();
        let err = select_task_flow(
            &task(Some(TaskSize::S), Some("single-task@1")),
            &layer,
            None,
            1,
        )
        .unwrap_err();
        assert!(
            matches!(err, TaskFlowError::Validation { .. }),
            "a success path without a verifier must be refused for a task, got {err:?}"
        );
    }

    #[test]
    fn medium_task_selects_a_template_with_a_verifier() {
        let (_dir, layer) = project();
        let selected = select_task_flow(&task(Some(TaskSize::M), None), &layer, None, 1).unwrap();
        assert_eq!(selected.reference.name(), "linear-with-review");
        assert!(!selected.pinned);
    }

    #[test]
    fn compose_is_named_unavailable() {
        let error = TaskFlowError::ComposeUnavailable;
        assert!(error.to_string().contains("not available in this build"));
    }
}
