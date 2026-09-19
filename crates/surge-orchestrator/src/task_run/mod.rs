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
//!
//! ## Composing a new flow
//!
//! When no template fits, a classifier may answer `compose`. The generator
//! runs behind [`FlowComposer`] (the ACP seam), its output is validated as a
//! task flow, and the validated graph is installed into
//! `.surge/flows/<name>-1.0.toml` and pinned in the trust store **in the same
//! step** — see [`compose_task_flow`]. Nothing composed is used before it is
//! on disk, validated and trusted, which is what makes the artifact the
//! operator can read, edit and pin (and what the `ComposedArtifactInstalled`
//! event records).
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
    /// A classifier answered `compose` but no composer was wired for this
    /// caller (the CLI's `surge flow show` path, or a test).
    #[error(
        "flow composition (`compose`) is not available in this caller; pin the task's flow \
         with `flow = \"name@MAJOR\"` in .surge/roadmap.toml or extend the size mapping"
    )]
    ComposeUnavailable,
    /// The catalog could not be scanned.
    #[error("scan flow catalog: {0}")]
    Catalog(String),
    /// The composer failed to produce a graph.
    #[error("flow composition failed: {0}")]
    Compose(String),
    /// The composed graph failed task validation (nothing was written).
    #[error("composed flow is not a valid task flow: {reason}")]
    ComposeInvalid {
        /// Rendered validation error.
        reason: String,
    },
    /// The composed graph could not be installed or pinned.
    #[error("install composed flow: {0}")]
    ComposeInstall(String),
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
    /// Set when the flow was **composed** for this task rather than selected
    /// from the catalog: the file installed under `.surge/flows/`.
    pub composed: Option<crate::task_compose::ComposedArtifact>,
}

/// Generates a new flow graph for a task that no template fits.
///
/// The production adapter runs the ACP flow generator; tests script it. The
/// generator's output is untrusted until [`compose_task_flow`] validates and
/// installs it.
#[async_trait::async_trait]
pub trait FlowComposer: Send + Sync {
    /// Compose a graph for `task`, with the catalog rendered into the prompt.
    ///
    /// # Errors
    /// A human-readable reason when composition fails (no agent, refusal,
    /// unparseable output).
    async fn compose(&self, task: &RoadmapTask, catalog: String) -> Result<Graph, String>;
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
        composed: None,
    })
}

/// Compose, validate and install a new flow for `task`.
///
/// The generator's graph is validated as a task flow; on success it is
/// installed under `.surge/flows/<name>-1.0.toml` and pinned in the trust
/// store in the same call, and the returned [`SelectedFlow`] carries the
/// installed artifact so the caller can record
/// `ComposedArtifactInstalled` after the approval gate resolves. A graph
/// that fails validation is **not** written.
///
/// `flow_name` is the catalog name the composed template should carry (the
/// generator proposes it; the caller may pass the task id when it has no
/// better name).
///
/// # Errors
/// [`TaskFlowError::Compose`] when the generator fails;
/// [`TaskFlowError::ComposeInvalid`] when the composed graph fails task
/// validation; [`TaskFlowError::ComposeInstall`] when it cannot be written
/// or pinned.
pub async fn compose_task_flow(
    composer: &dyn FlowComposer,
    task: &RoadmapTask,
    project: &ProjectLayer,
    home_flows_dir: Option<&Path>,
    trust: &mut surge_persistence::trust_store::TrustStore,
    flow_name: &str,
    now_ms: i64,
) -> Result<SelectedFlow, TaskFlowError> {
    let catalog = FlowCatalog::scan(project, home_flows_dir)
        .map_err(|e| TaskFlowError::Catalog(e.to_string()))?;
    let graph = composer
        .compose(task, catalog.render_catalog())
        .await
        .map_err(TaskFlowError::Compose)?;
    let artifact = crate::task_compose::install_composed_flow(
        project.root(),
        trust,
        flow_name,
        &graph,
        now_ms,
    )
    .map_err(|e| match e {
        crate::task_compose::ComposeError::FlowInvalid { reason, .. } => {
            TaskFlowError::ComposeInvalid { reason }
        },
        other => TaskFlowError::ComposeInstall(other.to_string()),
    })?;
    let reference = artifact
        .path
        .as_str()
        .rsplit('/')
        .next()
        .and_then(|file| file.strip_suffix(".toml"))
        .and_then(|stem| {
            let (name, version) = stem.rsplit_once('-')?;
            let (major, minor) = version.split_once('.')?;
            FlowRef::new(name, major.parse().ok()?, Some(minor.parse().ok()?)).ok()
        })
        .ok_or_else(|| TaskFlowError::ComposeInstall("installed name is not a FlowRef".into()))?;
    Ok(SelectedFlow {
        reference,
        layer: surge_core::Layer::Project,
        graph,
        pinned: true,
        composed: Some(artifact),
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
    fn compose_unwired_is_named() {
        let error = TaskFlowError::ComposeUnavailable;
        assert!(error.to_string().contains("not available in this caller"));
    }

    struct StubComposer {
        graph: std::sync::Mutex<Option<Graph>>,
        seen_catalog: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl FlowComposer for StubComposer {
        async fn compose(&self, _task: &RoadmapTask, catalog: String) -> Result<Graph, String> {
            *self.seen_catalog.lock().unwrap() = Some(catalog);
            self.graph
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| "no scripted graph".to_string())
        }
    }

    #[tokio::test]
    async fn compose_installs_validated_flow_and_pins_it() {
        let (_dir, layer) = project();
        let composer = StubComposer {
            graph: std::sync::Mutex::new(Some(
                surge_core::BundledFlows::by_name_latest("linear-3")
                    .unwrap()
                    .graph,
            )),
            seen_catalog: std::sync::Mutex::new(None),
        };
        let mut trust = surge_persistence::trust_store::TrustStore::in_memory();
        let selected = compose_task_flow(
            &composer,
            &task(Some(TaskSize::M), None),
            &layer,
            None,
            &mut trust,
            "composed-review",
            1,
        )
        .await
        .unwrap();

        // The installed artifact is the one the event must record.
        let artifact = selected.composed.expect("composed artifact");
        assert_eq!(
            artifact.path.as_str(),
            ".surge/flows/composed-review-1.0.toml"
        );
        assert_eq!(selected.reference.to_string(), "composed-review@1.0");
        assert!(layer.root().join(artifact.path.as_str()).exists());
        // Pinned in the same step: the trust gate will not re-prompt.
        let content = std::fs::read(layer.root().join(artifact.path.as_str())).unwrap();
        assert!(trust.check(&artifact.path, &content).unwrap().is_none());
        // The classifier's prompt carried the catalog's fit guidance.
        assert!(
            composer
                .seen_catalog
                .lock()
                .unwrap()
                .as_deref()
                .unwrap_or_default()
                .contains("bug-fix@1.0")
        );
    }

    #[tokio::test]
    async fn compose_that_fails_validation_is_not_written() {
        let (_dir, layer) = project();
        let composer = StubComposer {
            // `single-task` has no verifier: invalid as a task flow.
            graph: std::sync::Mutex::new(Some(
                surge_core::BundledFlows::by_name_latest("single-task")
                    .unwrap()
                    .graph,
            )),
            seen_catalog: std::sync::Mutex::new(None),
        };
        let mut trust = surge_persistence::trust_store::TrustStore::in_memory();
        let error = compose_task_flow(
            &composer,
            &task(Some(TaskSize::S), None),
            &layer,
            None,
            &mut trust,
            "no-verifier",
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TaskFlowError::ComposeInvalid { .. }));
        assert!(
            !layer
                .root()
                .join(".surge/flows/no-verifier-1.0.toml")
                .exists()
        );
    }
}
