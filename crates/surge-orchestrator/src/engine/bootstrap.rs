//! Bootstrap-mode post-processing hooks for the engine.
//!
//! Currently this module provides the Flow Generator validation-retry hook
//! (Task 10): after a Flow Generator agent stage finishes, the engine reads
//! the produced `flow.toml` from the run's worktree, parses and validates it
//! via [`crate::engine::validate::validate_for_m6`].
//!
//! - On success the engine appends a `PipelineMaterialized` event so the
//!   bootstrap-to-pipeline driver (Task 19) can pick it up, and the agent's
//!   original outcome continues routing forward as declared.
//! - On parse / validation failure the engine appends `BootstrapEditRequested`
//!   (so [`surge_core::run_state::RunMemory::bootstrap_edit_counts`] increments
//!   via the existing fold rule). The agent stage commits the validation-failure
//!   outcome together with its other stage effects, and the bundled bootstrap
//!   graph wires that key to a `Backtrack` edge that re-enters the Flow Generator.
//! - When the per-stage edit-loop cap (`EngineRunConfig.bootstrap.edit_loop_cap`)
//!   is already exhausted the hook short-circuits with `EscalationRequested`,
//!   mirroring the human-gate cap behaviour wired in Task 9.

use std::path::{Path, PathBuf};

use surge_core::content_hash::ContentHash;
use surge_core::graph::Graph;
use surge_core::keys::NodeKey;
use surge_core::run_event::{BootstrapStage, EscalationCause, EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;

use crate::engine::stage::StageError;
use crate::engine::validate::{validate_archetype_topology, validate_for_m6};

/// Synthetic outcome key the engine substitutes when the Flow Generator's
/// emitted `flow.toml` fails to parse or validate. The bundled bootstrap graph
/// wires this key to a `Backtrack` edge so the engine re-enters the Flow
/// Generator agent for another attempt.
pub const VALIDATION_FAILED_OUTCOME: &str = "validation_failed";

/// Default relative path the Flow Generator profile is expected to write its
/// produced graph to inside the run's isolated worktree.
const FLOW_ARTIFACT_FILENAME: &str = "flow.toml";

const FLOW_SERIALIZATION_REFERENCE: &str =
    include_str!("../../../surge-core/bundled/flows/multi-milestone-1.0.toml");

const ARTIFACT_ITERABLE_REFERENCE: &str = r#"type = "run_artifact"
[value]
name = "roadmap"
jsonpath = "milestones"
"#;
const LOOP_ITEM_ITERABLE_REFERENCE: &str = r#"type = "loop_item"
[value]
var = "milestone"
jsonpath = "tasks"
"#;
const BINDING_REFERENCE: &str = r#"target = "description_artifact"
optional = false
[source]
type = "run_artifact"
name = "description"
"#;

fn iterable_serialization_reference() -> String {
    format!(
        "\nFor a roadmap-driven loop, use \
         the following structure. These snippets are relative to the loop config's \
         iterates_over table; prefix [value] with that full table path. The approved roadmap is supplied as a run artifact. Do not add a planner \
         to regenerate it; there is no producer node in the child graph.\n\n\
         Artifact iterable:\n```toml\n{ARTIFACT_ITERABLE_REFERENCE}```\n\n\
         Nested task iterable:\n```toml\n{LOOP_ITEM_ITERABLE_REFERENCE}```\n\
         The value field is a table, never a string. Do not put node/name/jsonpath \
         or var/jsonpath directly beside type.\n\n\
         Agent bindings are arrays of records with target and a tagged source table. \
         This example binds the approved description to a prompt variable; select \
         the actual artifact appropriate for the profile's required input. For a \
         node-produced specification use source type node_output with node and artifact. \
         Never use source = \"file\" or a path field. Prefix [source] with the \
         binding's full table path:\n```toml\n{BINDING_REFERENCE}```\n\
         End each loop body with an explicit terminal node and incoming edges; \
         is_terminal on an agent outcome does not perform that transition."
    )
}

fn flow_parse_feedback(error: &toml::de::Error) -> String {
    format!(
        "flow.toml parse failed: {error}\n\n\
         Canonical Graph TOML serialization reference follows. Use its table and enum \
         structure, including edges and nested subgraphs. Preserve the approved roadmap, \
         requested runtime constraints, verifier authority and ledger effects. The profile choices are illustrative; adapt them to this run's inputs.\n\n\
         ```toml\n{FLOW_SERIALIZATION_REFERENCE}\n```{}",
        iterable_serialization_reference()
    )
}

/// Supply exact serialization before the first generation attempt, including
/// when a disk profile overrides the bundled prompt.
pub(crate) fn append_flow_serialization_reference(prompt: &str) -> String {
    format!(
        "{prompt}\n\n# Executable Graph serialization reference\n\
         The following bundled graph demonstrates exact TOML syntax for nodes, \
         tagged enums, edges and nested loop bodies. Preserve this run's approved \
         requirements and runtime constraints. Adapt its profiles and bindings to this run's inputs; preserve the approved roadmap.\n\n\
         ```toml\n{FLOW_SERIALIZATION_REFERENCE}\n```{}",
        iterable_serialization_reference()
    )
}

/// Whether `profile_ref` resolves to the Flow Generator family.
///
/// Profile references can be plain (`flow-generator`), pinned (`flow-generator@1.0`),
/// or version-suffixed (`flow-generator-1.0`) — the registry is responsible
/// for the actual lookup, the post-processor only needs a fast prefix match.
#[must_use]
pub fn is_flow_generator_profile(profile_ref: &str) -> bool {
    let head = profile_ref.split('@').next().unwrap_or(profile_ref);
    head == "flow-generator" || head.starts_with("flow-generator-")
}

/// Decision returned by [`run_flow_generator_post_processing`].
#[derive(Debug)]
#[non_exhaustive]
pub enum FlowValidationDecision {
    /// `flow.toml` parsed and validated; the engine has appended
    /// `PipelineMaterialized`. Routing proceeds with the agent's original
    /// outcome.
    Materialized,
    /// Parse or validation failed and the per-stage edit-loop cap still has
    /// budget left. The engine has appended `BootstrapEditRequested`; the
    /// caller must commit [`VALIDATION_FAILED_OUTCOME`] as the stage outcome so
    /// the bootstrap graph's `Backtrack` edge re-enters the Flow Generator.
    EditRequested {
        /// Operator-readable feedback text appended to `BootstrapEditRequested`.
        feedback: String,
    },
    /// The per-stage edit-loop cap is exhausted. The engine has appended an
    /// `EscalationRequested` event; the caller MUST surface this as a
    /// terminal stage failure (typically [`StageError::EditLoopCapExceeded`]).
    CapExceeded {
        /// Configured cap value, copied for convenient error construction.
        cap: u32,
    },
    /// The agent reported success but no `flow.toml` was found in the run's
    /// worktree. The caller MUST surface this as a stage failure — there is
    /// no graph to validate or to route on.
    MissingArtifact,
}

/// Locate the produced `flow.toml`. Prefer the canonical worktree-rooted file
/// because post-processing runs before the current stage's `ArtifactProduced`
/// event has been folded into [`RunMemory`]. Fall back to the most recently
/// folded artifact named `flow` for resumed or partially reconstructed runs.
fn locate_flow_artifact(memory: &RunMemory, worktree: &Path) -> Option<PathBuf> {
    let canonical = worktree.join(FLOW_ARTIFACT_FILENAME);
    if canonical.exists() {
        return Some(canonical);
    }
    if let Some(artifact) = memory.artifacts.get("flow") {
        let absolute = worktree.join(&artifact.path);
        return Some(absolute);
    }
    None
}

/// Run the Flow Generator post-processing hook.
///
/// `node` is the Flow Generator agent node key. The caller records the returned
/// validation-failure outcome in the same durable stage-effects batch as its
/// invocation commit.
///
/// `memory` is the run state observed BEFORE the post-processing pass — the
/// `bootstrap_edit_counts[Flow]` snapshot drives the cap check, mirroring the
/// human-gate semantics wired in Task 9.
///
/// `edit_loop_cap` is the value of `EngineRunConfig.bootstrap.edit_loop_cap`
/// for the current run; `0` disables the cap (used by harnesses that need
/// unbounded retry loops).
///
/// `worktree` is the run's isolated checkout root. The Flow Generator profile
/// is expected to write `flow.toml` at this path (per the bundled profile
/// prompt landing in Task 12).
///
/// `writer` is the run's append-only event writer.
///
/// # Errors
/// Returns [`StageError::Storage`] when the writer fails to append an event.
/// Parse / validation failures are NOT errors — they are represented as
/// [`FlowValidationDecision::EditRequested`] so the caller can convert them
/// into the appropriate routing outcome.
pub async fn run_flow_generator_post_processing(
    node: &NodeKey,
    memory: &RunMemory,
    edit_loop_cap: u32,
    worktree: &Path,
    writer: &RunWriter,
) -> Result<FlowValidationDecision, StageError> {
    run_flow_generator_post_processing_with_registry(
        node,
        memory,
        edit_loop_cap,
        worktree,
        writer,
        None,
    )
    .await
}

/// Validate generated flows with the same required profile inputs checked at execution.
///
/// # Errors
/// Returns a stage error if validation feedback cannot be persisted.
pub async fn run_flow_generator_post_processing_with_registry(
    node: &NodeKey,
    memory: &RunMemory,
    edit_loop_cap: u32,
    worktree: &Path,
    writer: &RunWriter,
    registry: Option<&crate::profile_loader::ProfileRegistry>,
) -> Result<FlowValidationDecision, StageError> {
    let Some(flow_path) = locate_flow_artifact(memory, worktree) else {
        tracing::warn!(
            target: "engine::bootstrap::validation",
            node = %node,
            "Flow Generator finished without producing a flow.toml artifact"
        );
        return Ok(FlowValidationDecision::MissingArtifact);
    };

    tracing::debug!(
        target: "engine::bootstrap::validation",
        node = %node,
        path = %flow_path.display(),
        "validating Flow Generator output"
    );

    let bytes = match tokio::fs::read(&flow_path).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "engine::bootstrap::validation",
                node = %node,
                path = %flow_path.display(),
                err = %e,
                "Flow Generator output unreadable — emitting BootstrapEditRequested"
            );
            return route_validation_failure(
                node,
                memory,
                edit_loop_cap,
                writer,
                format!("flow.toml unreadable at {}: {e}", flow_path.display()),
            )
            .await;
        },
    };

    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(e) => {
            return route_validation_failure(
                node,
                memory,
                edit_loop_cap,
                writer,
                format!("flow.toml is not valid UTF-8: {e}"),
            )
            .await;
        },
    };

    let graph = match toml::from_str::<Graph>(text) {
        Ok(g) => g,
        Err(e) => {
            return route_validation_failure(
                node,
                memory,
                edit_loop_cap,
                writer,
                flow_parse_feedback(&e),
            )
            .await;
        },
    };

    if let Err(feedback) = validate_generated_graph(&graph, registry) {
        return route_validation_failure(node, memory, edit_loop_cap, writer, feedback).await;
    }

    let graph_hash = ContentHash::compute(text.as_bytes());
    tracing::info!(
        target: "engine::bootstrap::validation",
        node = %node,
        graph_hash = %graph_hash,
        "Flow Generator output validated; emitting PipelineMaterialized"
    );

    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash,
            },
        ))
        .await
        .map_err(|e| StageError::Storage(format!("append PipelineMaterialized: {e}")))?;

    Ok(FlowValidationDecision::Materialized)
}

fn validate_generated_graph(
    graph: &Graph,
    registry: Option<&crate::profile_loader::ProfileRegistry>,
) -> Result<(), String> {
    validate_for_m6(graph).map_err(|e| format!("validate_for_m6 failed: {e}"))?;
    if let Some(registry) = registry {
        crate::engine::validate::validate_profile_inputs(graph, registry).map_err(|e| {
            format!("required profile inputs failed: {e}. Provide the declared bindings; custom_fields do not supply prompt inputs.")
        })?;
    }
    crate::engine::validate::require_archetype(graph).map_err(|e| e.to_string())?;
    validate_archetype_topology(graph).map_err(|e| format!("archetype topology check failed: {e}"))
}

/// Persist `BootstrapEditRequested` when the cap still has budget, or `EscalationRequested`
/// when it doesn't. Centralises the cap accounting so the success path stays
/// linear above.
async fn route_validation_failure(
    node: &NodeKey,
    memory: &RunMemory,
    edit_loop_cap: u32,
    writer: &RunWriter,
    feedback: String,
) -> Result<FlowValidationDecision, StageError> {
    let prior_edits = memory
        .bootstrap_edit_counts
        .get(&BootstrapStage::Flow)
        .copied()
        .unwrap_or(0);

    if edit_loop_cap > 0 && prior_edits >= edit_loop_cap {
        let reason = format!(
            "Flow Generator validation retry cap exceeded \
             (cap = {edit_loop_cap}, prior_edits = {prior_edits}); last failure: {feedback}"
        );
        tracing::error!(
            target: "engine::bootstrap::validation",
            node = %node,
            cap = edit_loop_cap,
            prior_edits,
            feedback = %feedback,
            "EditLoopCapExceeded — Flow Generator validation retries exhausted"
        );
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::EscalationRequested {
                    stage: Some(BootstrapStage::Flow),
                    reason,
                    cause: EscalationCause::BootstrapEditLoopExhausted,
                },
            ))
            .await
            .map_err(|e| StageError::Storage(format!("append EscalationRequested: {e}")))?;
        return Ok(FlowValidationDecision::CapExceeded { cap: edit_loop_cap });
    }

    if edit_loop_cap > 0 && prior_edits + 1 == edit_loop_cap {
        tracing::warn!(
            target: "engine::bootstrap::validation",
            node = %node,
            cap = edit_loop_cap,
            prior_edits,
            "approaching Flow Generator validation retry cap"
        );
    } else {
        tracing::info!(
            target: "engine::bootstrap::validation",
            node = %node,
            attempt = prior_edits + 1,
            feedback = %feedback,
            "Flow Generator output failed validation — emitting BootstrapEditRequested"
        );
    }

    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::BootstrapEditRequested {
                stage: BootstrapStage::Flow,
                feedback: feedback.clone(),
            },
        ))
        .await
        .map_err(|e| StageError::Storage(format!("append BootstrapEditRequested: {e}")))?;

    Ok(FlowValidationDecision::EditRequested { feedback })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use surge_core::LedgerEffect;
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use surge_core::graph::{GraphMetadata, SCHEMA_VERSION};
    use surge_core::id::RunId;
    use surge_core::keys::OutcomeKey;
    use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use surge_persistence::runs::{EventSeq, Storage};
    use tempfile::TempDir;

    #[test]
    fn generation_reference_preserves_operator_prompt_and_executable_schema() {
        use surge_core::loop_config::IterableSource;
        let prompt = append_flow_serialization_reference("Use Codex only. {{literal}}");
        assert!(prompt.starts_with("Use Codex only. {{literal}}"));
        assert!(prompt.contains(FLOW_SERIALIZATION_REFERENCE));
        let reference: Graph = toml::from_str(FLOW_SERIALIZATION_REFERENCE).unwrap();
        validate_for_m6(&reference).unwrap();
        let artifact: IterableSource = toml::from_str(ARTIFACT_ITERABLE_REFERENCE).unwrap();
        let item: IterableSource = toml::from_str(LOOP_ITEM_ITERABLE_REFERENCE).unwrap();
        assert!(matches!(artifact, IterableSource::RunArtifact { .. }));
        assert!(matches!(item, IterableSource::LoopItem { .. }));
        assert!(prompt.contains(ARTIFACT_ITERABLE_REFERENCE));
        assert!(prompt.contains(LOOP_ITEM_ITERABLE_REFERENCE));
        let binding: surge_core::agent_config::Binding = toml::from_str(BINDING_REFERENCE).unwrap();
        assert_eq!(binding.target.0, "description_artifact");
    }

    #[test]
    fn flow_generator_profile_predicate_matches_canonical_and_pinned_refs() {
        assert!(is_flow_generator_profile("flow-generator"));
        assert!(is_flow_generator_profile("flow-generator@1.0"));
        assert!(is_flow_generator_profile("flow-generator-1.0"));
        assert!(is_flow_generator_profile("flow-generator-1.0@2"));
        assert!(!is_flow_generator_profile("description-author"));
        assert!(!is_flow_generator_profile("roadmap-planner"));
        assert!(!is_flow_generator_profile("flow-generators"));
        assert!(!is_flow_generator_profile("flow"));
        assert!(!is_flow_generator_profile(""));
    }

    /// Build a tiny but valid Graph in Rust, then serialize it to TOML so the
    /// fixture survives schema renames without manual maintenance.
    ///
    /// Three nodes — `agent_a` -> `agent_b` -> `end` (Terminal) — so the
    /// graph satisfies `surge_core::validate`'s full rule set (a Terminal
    /// node must be reachable, and every non-terminal declared outcome must
    /// carry an edge), not just the M6-specific checks `validate_for_m6`
    /// enforced before Ticket 18 wired the rest of `surge_core::validate`
    /// into it. A two-agent, no-terminal version of this fixture used to
    /// pass validation only because those broader rules never ran.
    fn valid_flow_toml() -> String {
        let mut nodes = std::collections::BTreeMap::new();
        let outcome_done = OutcomeKey::try_from("done").unwrap();
        let outcomes = vec![OutcomeDecl {
            id: outcome_done.clone(),
            description: "done".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: LedgerEffect::default(),
        }];
        let agent_cfg = NodeConfig::Agent(surge_core::agent_config::AgentConfig {
            profile: surge_core::keys::ProfileKey::try_from("mock").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: surge_core::agent_config::NodeLimits::default(),
            hooks: vec![],
            custom_fields: std::collections::BTreeMap::default(),
        });
        let key_a = NodeKey::try_from("agent_a").unwrap();
        let key_b = NodeKey::try_from("agent_b").unwrap();
        let key_end = NodeKey::try_from("end").unwrap();
        nodes.insert(
            key_a.clone(),
            Node {
                id: key_a.clone(),
                position: Position::default(),
                declared_outcomes: outcomes.clone(),
                config: agent_cfg.clone(),
            },
        );
        nodes.insert(
            key_b.clone(),
            Node {
                id: key_b.clone(),
                position: Position::default(),
                declared_outcomes: outcomes,
                config: agent_cfg,
            },
        );
        nodes.insert(
            key_end.clone(),
            Node {
                id: key_end.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let edges = vec![
            Edge {
                id: surge_core::keys::EdgeKey::try_from("e1").unwrap(),
                from: PortRef {
                    node: key_a.clone(),
                    outcome: outcome_done.clone(),
                },
                to: key_b.clone(),
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            },
            Edge {
                id: surge_core::keys::EdgeKey::try_from("e2").unwrap(),
                from: PortRef {
                    node: key_b,
                    outcome: outcome_done,
                },
                to: key_end,
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            },
        ];
        let mut metadata = GraphMetadata::new("test", chrono::Utc::now());
        metadata.archetype = Some(surge_core::ArchetypeMetadata {
            name: surge_core::ArchetypeName::Linear3,
            milestones: None,
            edit_loop_cap: None,
            node_capacity_estimate: None,
        });
        let graph = surge_core::graph::Graph {
            schema_version: SCHEMA_VERSION,
            metadata,
            start: key_a,
            nodes,
            edges,
            subgraphs: std::collections::BTreeMap::new(),
        };
        toml::to_string(&graph).expect("serialize valid graph")
    }

    /// Mutate the valid fixture so its `start` references a non-existent node.
    /// `validate_for_m6` rejects this.
    fn invalid_flow_toml() -> String {
        valid_flow_toml().replace("start = \"agent_a\"", "start = \"ghost_node\"")
    }

    /// Spin up a fresh per-run Storage and return the writer plus the run id
    /// so the test can later open a reader against the same root.
    async fn fresh_writer(
        home: &Path,
    ) -> (Arc<Storage>, RunId, surge_persistence::runs::RunWriter) {
        let storage = Storage::open(home).await.expect("open storage");
        let run_id = RunId::new();
        let writer = storage
            .create_run(run_id, home, None)
            .await
            .expect("create run");
        (storage, run_id, writer)
    }

    async fn payload_kinds(storage: &Arc<Storage>, run_id: RunId) -> Vec<&'static str> {
        let reader = storage.open_run_reader(run_id).await.expect("open reader");
        let events = reader
            .read_events(EventSeq(0)..EventSeq(64))
            .await
            .expect("read_events");
        events
            .iter()
            .map(|e| e.payload.payload.discriminant_str())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_flow_returns_missing_artifact_decision() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();

        let (storage, _run_id, writer) = fresh_writer(home.path()).await;
        let memory = RunMemory::default();
        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        assert!(matches!(decision, FlowValidationDecision::MissingArtifact));
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_profile_inputs_retry_before_materialization() {
        use crate::profile_loader::{DiskProfileSet, ProfileRegistry};
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(FLOW_ARTIFACT_FILENAME), {
            let mut graph: Graph = toml::from_str(FLOW_SERIALIZATION_REFERENCE).unwrap();
            let NodeConfig::Agent(config) = &mut graph
                .subgraphs
                .get_mut(&surge_core::keys::SubgraphKey::try_from("task_body").unwrap())
                .unwrap()
                .nodes
                .get_mut(&NodeKey::try_from("impl_task").unwrap())
                .unwrap()
                .config
            else {
                panic!("expected implementation agent");
            };
            config.bindings.clear();
            toml::to_string(&graph).unwrap()
        })
        .unwrap();
        let profiles = tmp.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        let registry = ProfileRegistry::new(DiskProfileSet::scan(&profiles).unwrap());
        let (storage, run_id, writer) = fresh_writer(home.path()).await;
        let decision = run_flow_generator_post_processing_with_registry(
            &NodeKey::try_from("flow_generator").unwrap(),
            &RunMemory::default(),
            3,
            &worktree,
            &writer,
            Some(&registry),
        )
        .await
        .unwrap();
        let FlowValidationDecision::EditRequested { feedback } = decision else {
            panic!("missing inputs must go back to generator");
        };
        assert!(feedback.contains("spec"));
        let kinds = payload_kinds(&storage, run_id).await;
        assert!(kinds.contains(&"BootstrapEditRequested"));
        assert!(!kinds.contains(&"PipelineMaterialized"));
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn valid_flow_emits_pipeline_materialized_and_decision_materialized() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(FLOW_ARTIFACT_FILENAME), valid_flow_toml()).unwrap();

        let (storage, run_id, writer) = fresh_writer(home.path()).await;
        let memory = RunMemory::default();
        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        assert!(matches!(decision, FlowValidationDecision::Materialized));
        let kinds = payload_kinds(&storage, run_id).await;
        assert!(
            kinds.contains(&"PipelineMaterialized"),
            "expected PipelineMaterialized in event log, got {kinds:?}"
        );
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parse_failure_emits_edit_request_before_stage_outcome_is_committed() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(FLOW_ARTIFACT_FILENAME),
            "this is not toml = = =",
        )
        .unwrap();

        let (storage, run_id, writer) = fresh_writer(home.path()).await;
        let memory = RunMemory::default();
        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        match decision {
            FlowValidationDecision::EditRequested { feedback } => {
                assert!(
                    feedback.contains("parse"),
                    "feedback should mention parse error, got: {feedback}"
                );
                assert!(feedback.contains(FLOW_SERIALIZATION_REFERENCE));
                let reference: Graph = toml::from_str(FLOW_SERIALIZATION_REFERENCE).unwrap();
                validate_for_m6(&reference).expect("reference must remain executable");
            },
            other => panic!("expected EditRequested, got {other:?}"),
        }

        let kinds = payload_kinds(&storage, run_id).await;
        let edit_index = kinds
            .iter()
            .position(|k| *k == "BootstrapEditRequested")
            .expect("BootstrapEditRequested missing");
        assert!(
            !kinds.contains(&"OutcomeReported"),
            "post-processing must leave the stage outcome to its owning invocation batch (kinds = {kinds:?})"
        );
        assert_eq!(kinds[edit_index], "BootstrapEditRequested");
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn validation_failure_emits_edit_requested_with_validator_feedback() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(FLOW_ARTIFACT_FILENAME), invalid_flow_toml()).unwrap();

        let (storage, _run_id, writer) = fresh_writer(home.path()).await;
        let memory = RunMemory::default();
        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        match decision {
            FlowValidationDecision::EditRequested { feedback } => {
                assert!(
                    feedback.contains("validate_for_m6"),
                    "feedback should mention validator failure, got: {feedback}"
                );
            },
            other => panic!("expected EditRequested, got {other:?}"),
        }
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flow_without_archetype_is_sent_back_with_the_catalog() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let mut graph: surge_core::graph::Graph = toml::from_str(&valid_flow_toml()).unwrap();
        graph.metadata.archetype = None;
        std::fs::write(
            worktree.join(FLOW_ARTIFACT_FILENAME),
            toml::to_string(&graph).unwrap(),
        )
        .unwrap();

        let (storage, _run_id, writer) = fresh_writer(home.path()).await;
        let memory = RunMemory::default();
        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        match decision {
            FlowValidationDecision::EditRequested { feedback } => {
                assert!(feedback.contains("[metadata.archetype]"), "{feedback}");
                assert!(feedback.contains("multi-milestone"), "{feedback}");
            },
            other => panic!("expected EditRequested, got {other:?}"),
        }
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cap_exceeded_emits_escalation_and_returns_cap_exceeded() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(FLOW_ARTIFACT_FILENAME), invalid_flow_toml()).unwrap();

        let (storage, run_id, writer) = fresh_writer(home.path()).await;
        let mut counts: BTreeMap<BootstrapStage, u32> = BTreeMap::new();
        counts.insert(BootstrapStage::Flow, 3);
        let memory = RunMemory {
            bootstrap_edit_counts: counts,
            ..RunMemory::default()
        };

        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        match decision {
            FlowValidationDecision::CapExceeded { cap } => assert_eq!(cap, 3),
            other => panic!("expected CapExceeded, got {other:?}"),
        }

        let kinds = payload_kinds(&storage, run_id).await;
        assert!(
            kinds.contains(&"EscalationRequested"),
            "expected EscalationRequested, got {kinds:?}"
        );
        assert!(
            !kinds.contains(&"BootstrapEditRequested"),
            "BootstrapEditRequested must NOT be emitted on cap exceedance, got {kinds:?}"
        );
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    /// Build a TOML graph that passes `validate_for_m6` but declares the
    /// `multi-milestone` archetype without any matching `roadmap.milestones`
    /// loop — exercising the Task 11 archetype topology rule.
    fn multi_milestone_without_loop_toml() -> String {
        // Re-use the valid graph (two Agent nodes, no Loop) and relabel it
        // multi-milestone, which requires a milestone Loop it does not have.
        let mut graph: surge_core::graph::Graph = toml::from_str(&valid_flow_toml()).unwrap();
        graph.metadata.archetype = Some(surge_core::ArchetypeMetadata {
            name: surge_core::ArchetypeName::MultiMilestone,
            milestones: Some(3),
            edit_loop_cap: None,
            node_capacity_estimate: None,
        });
        toml::to_string(&graph).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archetype_mismatch_emits_edit_requested_with_topology_feedback() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(FLOW_ARTIFACT_FILENAME),
            multi_milestone_without_loop_toml(),
        )
        .unwrap();

        let (storage, _run_id, writer) = fresh_writer(home.path()).await;
        let memory = RunMemory::default();
        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 3, &worktree, &writer)
            .await
            .expect("post processing");

        match decision {
            FlowValidationDecision::EditRequested { feedback } => {
                assert!(
                    feedback.contains("archetype topology"),
                    "feedback should mention archetype topology check, got: {feedback}"
                );
                assert!(
                    feedback.contains("multi-milestone"),
                    "feedback should mention declared archetype, got: {feedback}"
                );
            },
            other => panic!("expected EditRequested, got {other:?}"),
        }
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cap_zero_disables_check_and_keeps_emitting_edit_requested() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(FLOW_ARTIFACT_FILENAME), invalid_flow_toml()).unwrap();

        let (storage, _run_id, writer) = fresh_writer(home.path()).await;
        let mut counts: BTreeMap<BootstrapStage, u32> = BTreeMap::new();
        counts.insert(BootstrapStage::Flow, 99);
        let memory = RunMemory {
            bootstrap_edit_counts: counts,
            ..RunMemory::default()
        };

        let node = NodeKey::try_from("flow_generator").unwrap();

        let decision = run_flow_generator_post_processing(&node, &memory, 0, &worktree, &writer)
            .await
            .expect("post processing");

        assert!(matches!(
            decision,
            FlowValidationDecision::EditRequested { .. }
        ));
        writer.close().await.unwrap();
        drop(storage);
        home.close().unwrap();
    }
}
