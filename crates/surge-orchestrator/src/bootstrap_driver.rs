//! Bootstrap-to-pipeline driver.
//!
//! Runs the bundled three-stage bootstrap graph, waits for it to finish, then
//! extracts the graph materialized by the Flow Generator from the event log.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::engine::handle::RunOutcome;
use crate::engine::{Engine, EngineError, EngineRunConfig};
use surge_core::BundledFlows;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::run_event::{BootstrapStage, EventPayload, VersionedEventPayload};
use surge_core::run_state::ArtifactRef;
use surge_persistence::runs::{EventSeq, OpenError, ReadEvent, RunWriter, StorageError};

const BOOTSTRAP_FLOW_NAME: &str = "bootstrap";
const BOOTSTRAP_ARTIFACTS: [&str; 3] = ["description", "roadmap", "flow"];

/// Result of a completed bootstrap run.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializedRun {
    /// Run id of the bootstrap run that produced the materialized graph.
    pub bootstrap_run_id: RunId,
    /// Follow-up graph emitted by the Flow Generator.
    pub materialized_graph: Graph,
    /// Latest `description`, `roadmap`, and `flow` artifacts in that order.
    pub artifacts: Vec<ArtifactRef>,
}

/// Errors returned by [`run_bootstrap`] and [`run_bootstrap_in_worktree`].
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// Current working directory could not be resolved for the default driver.
    #[error("current directory error: {0}")]
    CurrentDir(#[from] std::io::Error),
    /// The bundled bootstrap flow is missing from the registry.
    #[error("bundled bootstrap flow is not registered")]
    BundledFlowMissing,
    /// Engine API failed.
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    /// Opening the completed run's event log failed.
    #[error("open run event log failed: {0}")]
    Open(#[from] OpenError),
    /// Reading the completed run's event log failed.
    #[error("read run event log failed: {0}")]
    Storage(#[from] StorageError),
    /// Bootstrap run reached a failure terminal or stage error.
    #[error("bootstrap run failed: {0}")]
    RunFailed(String),
    /// Bootstrap run was aborted.
    #[error("bootstrap run aborted: {0}")]
    RunAborted(String),
    /// No post-bootstrap `PipelineMaterialized` event was found.
    #[error("bootstrap did not materialize a follow-up graph")]
    MaterializedGraphMissing,
    /// Required bootstrap artifact was not produced.
    #[error("bootstrap artifact missing: {0}")]
    ArtifactMissing(String),
}

/// Run the bundled bootstrap flow in the current directory.
///
/// # Errors
/// Returns [`BootstrapError`] if the current directory cannot be resolved, the
/// engine fails to start or complete the run, the run does not emit a follow-up
/// graph, or any required bootstrap artifact is missing.
pub async fn run_bootstrap(
    engine: &Engine,
    prompt: String,
    run_id: RunId,
) -> Result<MaterializedRun, BootstrapError> {
    let worktree_path = std::env::current_dir()?;
    run_bootstrap_in_worktree(engine, prompt, run_id, worktree_path).await
}

/// Run the bundled bootstrap flow against an explicit worktree path.
///
/// This is the testable form used by CLI / daemon code that already knows the
/// run worktree. [`run_bootstrap`] is the convenience wrapper for callers that
/// want to use the process current directory.
///
/// # Errors
/// Returns [`BootstrapError`] if the engine run fails, the event log cannot be
/// read, no follow-up graph was materialized, or any required bootstrap
/// artifact is absent.
pub async fn run_bootstrap_in_worktree(
    engine: &Engine,
    prompt: String,
    run_id: RunId,
    worktree_path: PathBuf,
) -> Result<MaterializedRun, BootstrapError> {
    let bundled = BundledFlows::by_name_latest(BOOTSTRAP_FLOW_NAME)
        .ok_or(BootstrapError::BundledFlowMissing)?;

    tracing::info!(
        target: "engine::bootstrap",
        run_id = %run_id,
        "bootstrap_started"
    );

    let run_config = EngineRunConfig {
        initial_prompt: prompt,
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(run_id, bundled.graph, worktree_path, run_config)
        .await?;

    match handle.await_completion().await? {
        RunOutcome::Completed { .. } => {},
        RunOutcome::Failed { error } => return Err(BootstrapError::RunFailed(error)),
        RunOutcome::Aborted { reason } => return Err(BootstrapError::RunAborted(reason)),
    }

    let materialized = materialized_run_from_completed(engine, run_id).await?;

    tracing::info!(
        target: "engine::bootstrap",
        run_id = %run_id,
        archetype = ?materialized.materialized_graph.metadata.archetype,
        "bootstrap_completed"
    );

    Ok(materialized)
}

/// Extract the materialized follow-up graph and bootstrap artifacts from an
/// already-completed bootstrap run.
///
/// This is used by CLI resume flows after `Engine::resume_run` reaches a
/// terminal outcome.
///
/// # Errors
/// Returns [`BootstrapError`] if the event log cannot be read, if no
/// follow-up graph was materialized, or if a required bootstrap artifact is
/// missing.
pub async fn materialized_run_from_completed(
    engine: &Engine,
    run_id: RunId,
) -> Result<MaterializedRun, BootstrapError> {
    let events = read_all_events(engine, run_id).await?;
    let materialized_graph =
        latest_followup_graph(&events).ok_or(BootstrapError::MaterializedGraphMissing)?;
    let artifacts = latest_bootstrap_artifacts(&events)?;
    append_bootstrap_telemetry(engine, run_id, &events, &materialized_graph).await?;

    Ok(MaterializedRun {
        bootstrap_run_id: run_id,
        materialized_graph,
        artifacts,
    })
}

async fn read_all_events(engine: &Engine, run_id: RunId) -> Result<Vec<ReadEvent>, BootstrapError> {
    let reader = engine.storage().open_run_reader(run_id).await?;
    let max_seq = reader.current_seq().await?;
    let end = EventSeq(max_seq.as_u64().saturating_add(1));
    Ok(reader.read_events(EventSeq(1)..end).await?)
}

fn latest_followup_graph(events: &[ReadEvent]) -> Option<Graph> {
    let mut skipped_initial_bootstrap_graph = false;
    let mut latest = None;

    for event in events {
        let EventPayload::PipelineMaterialized { graph, .. } = &event.payload.payload else {
            continue;
        };
        if graph.metadata.name == BOOTSTRAP_FLOW_NAME && !skipped_initial_bootstrap_graph {
            skipped_initial_bootstrap_graph = true;
            continue;
        }
        latest = Some((**graph).clone());
    }

    latest
}

fn latest_bootstrap_artifacts(events: &[ReadEvent]) -> Result<Vec<ArtifactRef>, BootstrapError> {
    let mut by_name: BTreeMap<String, ArtifactRef> = BTreeMap::new();

    for event in events {
        if let EventPayload::ArtifactProduced {
            node,
            artifact,
            path,
            name,
        } = &event.payload.payload
        {
            if !BOOTSTRAP_ARTIFACTS.contains(&name.as_str()) {
                continue;
            }
            by_name.insert(
                name.clone(),
                ArtifactRef {
                    hash: *artifact,
                    path: path.clone(),
                    name: name.clone(),
                    produced_by: node.clone(),
                    produced_at_seq: event.seq.as_u64(),
                },
            );
        }
    }

    BOOTSTRAP_ARTIFACTS
        .iter()
        .map(|name| {
            by_name
                .get(*name)
                .cloned()
                .ok_or_else(|| BootstrapError::ArtifactMissing((*name).to_owned()))
        })
        .collect()
}

async fn append_bootstrap_telemetry(
    engine: &Engine,
    run_id: RunId,
    events: &[ReadEvent],
    materialized_graph: &Graph,
) -> Result<(), BootstrapError> {
    if events.iter().any(|event| {
        matches!(
            &event.payload.payload,
            EventPayload::BootstrapTelemetry { .. }
        )
    }) {
        return Ok(());
    }

    let telemetry = EventPayload::BootstrapTelemetry {
        stage_durations: bootstrap_stage_durations(events),
        edit_counts: bootstrap_edit_counts(events),
        archetype: materialized_graph.metadata.archetype.clone(),
    };
    let writer = open_writer_after_completion(engine, run_id).await?;
    writer
        .append_event(VersionedEventPayload::new(telemetry))
        .await?;
    writer.flush().await?;
    Ok(())
}

async fn open_writer_after_completion(
    engine: &Engine,
    run_id: RunId,
) -> Result<RunWriter, BootstrapError> {
    for _ in 0..20 {
        match engine.storage().open_run_writer(run_id).await {
            Ok(writer) => return Ok(writer),
            Err(OpenError::WriterAlreadyHeld { .. }) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            },
            Err(e) => return Err(BootstrapError::Open(e)),
        }
    }
    Err(BootstrapError::Open(OpenError::WriterAlreadyHeld {
        run_id,
    }))
}

fn bootstrap_stage_durations(events: &[ReadEvent]) -> BTreeMap<BootstrapStage, u64> {
    let mut starts: BTreeMap<BootstrapStage, i64> = BTreeMap::new();
    let mut ends: BTreeMap<BootstrapStage, i64> = BTreeMap::new();

    for event in events {
        match &event.payload.payload {
            EventPayload::StageEntered { node, .. } => {
                if let Some(stage) = bootstrap_stage_for_node(node.as_ref()) {
                    starts.entry(stage).or_insert(event.timestamp_ms);
                }
            },
            EventPayload::BootstrapApprovalDecided { stage, .. } => {
                ends.insert(*stage, event.timestamp_ms);
            },
            _ => {},
        }
    }

    starts
        .into_iter()
        .filter_map(|(stage, started_at)| {
            let ended_at = ends.get(&stage)?;
            Some((stage, ended_at.saturating_sub(started_at).max(0) as u64))
        })
        .collect()
}

fn bootstrap_edit_counts(events: &[ReadEvent]) -> BTreeMap<BootstrapStage, u32> {
    let mut counts = BTreeMap::new();
    for event in events {
        if let EventPayload::BootstrapEditRequested { stage, .. } = &event.payload.payload {
            *counts.entry(*stage).or_insert(0) += 1;
        }
    }
    counts
}

fn bootstrap_stage_for_node(node: &str) -> Option<BootstrapStage> {
    if node.contains("description") {
        Some(BootstrapStage::Description)
    } else if node.contains("roadmap") {
        Some(BootstrapStage::Roadmap)
    } else if node.contains("flow") {
        Some(BootstrapStage::Flow)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::content_hash::ContentHash;
    use surge_core::keys::NodeKey;
    use surge_core::run_event::VersionedEventPayload;
    use surge_persistence::runs::EventSeq;

    fn read_event(seq: u64, payload: EventPayload) -> ReadEvent {
        read_event_at(seq, 0, payload)
    }

    fn read_event_at(seq: u64, timestamp_ms: i64, payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms,
            kind: payload.discriminant_str().to_owned(),
            payload: VersionedEventPayload::new(payload),
        }
    }

    #[test]
    fn latest_followup_graph_skips_initial_bootstrap_graph() {
        let bootstrap = BundledFlows::by_name_latest("bootstrap")
            .expect("bootstrap bundled")
            .graph;
        let followup: Graph = toml::from_str(include_str!(
            "../tests/fixtures/golden_multi_milestone_flow.toml"
        ))
        .expect("golden graph parses");
        let events = vec![
            read_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(bootstrap),
                    graph_hash: ContentHash::compute(b"bootstrap"),
                },
            ),
            read_event(
                20,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(followup.clone()),
                    graph_hash: ContentHash::compute(b"followup"),
                },
            ),
        ];

        let graph = latest_followup_graph(&events).expect("follow-up graph found");
        assert_eq!(graph.metadata.name, followup.metadata.name);
    }

    #[test]
    fn latest_bootstrap_artifacts_returns_ordered_latest_refs() {
        let node = NodeKey::try_from("description_author").unwrap();
        let stale = read_event(
            4,
            EventPayload::ArtifactProduced {
                node: node.clone(),
                artifact: ContentHash::compute(b"old"),
                path: PathBuf::from("description.md"),
                name: "description".into(),
            },
        );
        let latest_description = read_event(
            8,
            EventPayload::ArtifactProduced {
                node: node.clone(),
                artifact: ContentHash::compute(b"new"),
                path: PathBuf::from("description.md"),
                name: "description".into(),
            },
        );
        let roadmap = read_event(
            12,
            EventPayload::ArtifactProduced {
                node: NodeKey::try_from("roadmap_planner").unwrap(),
                artifact: ContentHash::compute(b"roadmap"),
                path: PathBuf::from("roadmap.md"),
                name: "roadmap".into(),
            },
        );
        let flow = read_event(
            16,
            EventPayload::ArtifactProduced {
                node: NodeKey::try_from("flow_generator").unwrap(),
                artifact: ContentHash::compute(b"flow"),
                path: PathBuf::from("flow.toml"),
                name: "flow".into(),
            },
        );

        let artifacts =
            latest_bootstrap_artifacts(&[stale, latest_description, roadmap, flow]).unwrap();

        assert_eq!(
            artifacts
                .iter()
                .map(|artifact| artifact.name.as_str())
                .collect::<Vec<_>>(),
            vec!["description", "roadmap", "flow"]
        );
        assert_eq!(artifacts[0].produced_at_seq, 8);
    }

    #[test]
    fn telemetry_helpers_derive_durations_and_edit_counts() {
        let events = vec![
            read_event_at(
                1,
                100,
                EventPayload::StageEntered {
                    node: NodeKey::try_from("description_author").unwrap(),
                    attempt: 1,
                },
            ),
            read_event_at(
                2,
                125,
                EventPayload::BootstrapEditRequested {
                    stage: BootstrapStage::Description,
                    feedback: "tighten".into(),
                },
            ),
            read_event_at(
                3,
                200,
                EventPayload::BootstrapApprovalDecided {
                    stage: BootstrapStage::Description,
                    decision: surge_core::run_event::BootstrapDecision::Approve,
                    comment: None,
                },
            ),
            read_event_at(
                4,
                250,
                EventPayload::StageEntered {
                    node: NodeKey::try_from("flow_generator").unwrap(),
                    attempt: 1,
                },
            ),
            read_event_at(
                5,
                350,
                EventPayload::BootstrapEditRequested {
                    stage: BootstrapStage::Flow,
                    feedback: "invalid graph".into(),
                },
            ),
            read_event_at(
                6,
                400,
                EventPayload::BootstrapEditRequested {
                    stage: BootstrapStage::Flow,
                    feedback: "still invalid".into(),
                },
            ),
            read_event_at(
                7,
                550,
                EventPayload::BootstrapApprovalDecided {
                    stage: BootstrapStage::Flow,
                    decision: surge_core::run_event::BootstrapDecision::Approve,
                    comment: None,
                },
            ),
        ];

        let durations = bootstrap_stage_durations(&events);
        assert_eq!(durations[&BootstrapStage::Description], 100);
        assert_eq!(durations[&BootstrapStage::Flow], 300);
        assert!(!durations.contains_key(&BootstrapStage::Roadmap));

        let edits = bootstrap_edit_counts(&events);
        assert_eq!(edits[&BootstrapStage::Description], 1);
        assert_eq!(edits[&BootstrapStage::Flow], 2);
        assert!(!edits.contains_key(&BootstrapStage::Roadmap));
    }
}
