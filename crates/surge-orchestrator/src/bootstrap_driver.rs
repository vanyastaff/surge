//! Bootstrap-to-pipeline driver.
//!
//! Runs the bundled three-stage bootstrap graph, waits for it to finish,
//! records its telemetry once, then hands off through
//! [`crate::bootstrap::handoff`] — the one place that selects and validates
//! what the follow-up run inherits (#98).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::bootstrap::handoff::{BootstrapHandoff, BootstrapStop, HandoffRejection};
use crate::engine::config::ProjectContextSeed;
use crate::engine::{Engine, EngineError, EngineRunConfig};
use surge_core::BundledFlows;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::run_event::{BootstrapStage, EventPayload, VersionedEventPayload};
use surge_persistence::runs::{
    CloseError, EventSeq, OpenError, ReadEvent, RunWriter, Storage, StorageError,
};

const BOOTSTRAP_FLOW_NAME: &str = "bootstrap";

/// Errors returned by [`run_bootstrap_in_worktree`].
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// The bundled bootstrap flow is missing from the registry.
    #[error("bundled bootstrap flow is not registered")]
    BundledFlowMissing,
    /// Engine API failed.
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    /// Opening the completed run's event log failed.
    #[error("open run event log failed: {0}")]
    Open(#[from] OpenError),
    /// Reading or writing the completed run's event log failed.
    #[error("read/write run event log failed: {0}")]
    Storage(#[from] StorageError),
    /// Closing the telemetry writer failed after its event was written.
    #[error("close bootstrap telemetry writer failed: {0}")]
    WriterClose(#[from] CloseError),
    /// Both writing telemetry and joining its writer failed; neither error is discarded.
    #[error("write bootstrap telemetry failed: {write}; close its writer also failed: {close}")]
    TelemetryWriteAndClose {
        /// Primary append or flush failure.
        #[source]
        write: Box<StorageError>,
        /// Additional failure while settling the same writer.
        close: Box<CloseError>,
    },
    /// The bootstrap run stopped without completing ([`BootstrapStop`] is
    /// never `Completed` here): parked, suspended, recovery-required, failed,
    /// aborted, or unsettled.
    #[error("bootstrap run did not complete: {0:?}")]
    Stopped(BootstrapStop),
    /// The completed run's handoff was refused.
    #[error(transparent)]
    Handoff(#[from] HandoffRejection),
}

/// Run the bundled bootstrap flow against an explicit worktree path.
///
/// This is the testable form used by CLI / daemon code that already knows the
/// isolated run worktree.
///
/// `memory_store_path` is forwarded verbatim into the bootstrap run's
/// `EngineRunConfig`: `None` — every production caller — resolves the real
/// `MemoryStore::default_path()`; `Some(path)` is test-only, redirecting a
/// driven-to-failure bootstrap run away from the developer's own store.
///
/// # Errors
/// [`BootstrapError::Stopped`] when the run does not complete, or
/// [`BootstrapError::Handoff`] when its plan is refused.
#[must_use = "await the bootstrap driver result and handle failures"]
pub async fn run_bootstrap_in_worktree(
    engine: &Engine,
    prompt: String,
    run_id: RunId,
    worktree_path: PathBuf,
    project_context: Option<ProjectContextSeed>,
    memory_store_path: Option<PathBuf>,
) -> Result<BootstrapHandoff, BootstrapError> {
    let bundled = BundledFlows::by_name_latest(BOOTSTRAP_FLOW_NAME)
        .ok_or(BootstrapError::BundledFlowMissing)?;

    tracing::info!(
        target: "engine::bootstrap",
        run_id = %run_id,
        "bootstrap_started"
    );

    let run_config = EngineRunConfig {
        initial_prompt: prompt,
        project_context,
        memory_store_path,
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(run_id, bundled.graph, worktree_path, run_config)
        .await?;
    let outcome = handle.await_completion().await?;
    match crate::bootstrap::handoff::classify(&outcome) {
        BootstrapStop::Completed => {},
        stopped => return Err(BootstrapError::Stopped(stopped)),
    }

    let handoff = handoff_from_completed(engine, run_id).await?;
    tracing::info!(
        target: "engine::bootstrap",
        run_id = %run_id,
        archetype = ?handoff.graph.metadata.archetype,
        "bootstrap_completed"
    );
    Ok(handoff)
}

/// The handoff of an already-completed bootstrap run, after recording its
/// telemetry once (idempotent). Used by the driver and by `surge bootstrap
/// resume`.
///
/// # Errors
/// [`BootstrapError::Handoff`] when the plan is refused, or a storage error
/// writing telemetry.
#[must_use = "await the handoff and handle failures"]
pub async fn handoff_from_completed(
    engine: &Engine,
    run_id: RunId,
) -> Result<BootstrapHandoff, BootstrapError> {
    let handoff = engine.prepare_bootstrap_handoff(run_id).await?;
    record_bootstrap_telemetry(&engine.storage(), run_id, &handoff.graph).await?;
    Ok(handoff)
}

/// Append `BootstrapTelemetry` to a completed bootstrap run once: an
/// explicit write by the completion path, never a side effect of reading the
/// handoff.
///
/// # Errors
/// A storage error reading the log or writing the event.
pub async fn record_bootstrap_telemetry(
    storage: &Arc<Storage>,
    run_id: RunId,
    follow_up: &Graph,
) -> Result<(), BootstrapError> {
    let events = read_all_events(storage, run_id).await?;
    append_bootstrap_telemetry(storage, run_id, &events, follow_up).await
}

async fn read_all_events(
    storage: &Arc<Storage>,
    run_id: RunId,
) -> Result<Vec<ReadEvent>, BootstrapError> {
    let reader = storage.open_run_reader(run_id).await?;
    let max_seq = reader.current_seq().await?;
    let end = EventSeq(max_seq.as_u64().saturating_add(1));
    Ok(reader.read_events(EventSeq(1)..end).await?)
}

async fn append_bootstrap_telemetry(
    storage: &Arc<Storage>,
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
    let writer = open_writer_after_completion(storage, run_id).await?;
    let write = async {
        writer
            .append_event(VersionedEventPayload::new(telemetry))
            .await?;
        writer.flush().await
    }
    .await;
    let close = writer.close().await;
    match (write, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(write), Ok(())) => Err(BootstrapError::Storage(write)),
        (Ok(()), Err(close)) => Err(BootstrapError::WriterClose(close)),
        (Err(write), Err(close)) => Err(BootstrapError::TelemetryWriteAndClose {
            write: Box::new(write),
            close: Box::new(close),
        }),
    }
}

async fn open_writer_after_completion(
    storage: &Arc<Storage>,
    run_id: RunId,
) -> Result<RunWriter, BootstrapError> {
    for _ in 0..20 {
        match storage.open_run_writer(run_id).await {
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
    use surge_core::keys::NodeKey;
    use surge_core::run_event::VersionedEventPayload;
    use surge_persistence::runs::EventSeq;

    fn read_event_at(seq: u64, timestamp_ms: i64, payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms,
            kind: payload.discriminant_str().to_owned(),
            payload: VersionedEventPayload::new(payload),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn telemetry_returns_with_writer_released_and_fresh_repeat_preserves_one_record() {
        tokio::spawn(async {
        use surge_persistence::runs::Storage;

        let directory = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(directory.path()).await.unwrap();
        let run = RunId::new();
        storage.create_run(run, directory.path(), None).await.unwrap().close().await.unwrap();
        let graph = BundledFlows::by_name_latest("bootstrap").unwrap().graph;
        append_bootstrap_telemetry(&storage, run, &[], &graph).await.unwrap();
        // No scheduling delay or retry: success must already have released ownership.
        let writer = storage.open_run_writer(run).await.unwrap();
        writer.close().await.unwrap();
        let reader = storage.open_run_reader(run).await.unwrap();
        let events = reader.read_events(EventSeq(0)..EventSeq(u64::MAX)).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].payload.payload, EventPayload::BootstrapTelemetry {
            stage_durations, edit_counts, archetype
        } if stage_durations.is_empty() && edit_counts.is_empty() && archetype == &graph.metadata.archetype));
        // The second call uses freshly read evidence, not a stale concurrent snapshot.
        append_bootstrap_telemetry(&storage, run, &events, &graph).await.unwrap();
        let writer = storage.open_run_writer(run).await.unwrap();
        writer.close().await.unwrap();
        let repeated = reader.read_events(EventSeq(0)..EventSeq(u64::MAX)).await.unwrap();
        assert_eq!(repeated.len(), 1);
        drop(reader);
        drop(storage);
        directory.close().unwrap();
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn telemetry_write_failure_returns_with_writer_released() {
        tokio::spawn(async {
        use surge_persistence::runs::Storage;

        let directory = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(directory.path()).await.unwrap();
        let run = RunId::new();
        storage.create_run(run, directory.path(), None).await.unwrap().close().await.unwrap();
        let connection = rusqlite::Connection::open(directory.path().join("runs").join(run.to_string()).join("events.sqlite")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_telemetry BEFORE INSERT ON events WHEN NEW.kind = 'BootstrapTelemetry' BEGIN SELECT RAISE(ABORT, 'fixture telemetry refused'); END;").unwrap();
        let graph = BundledFlows::by_name_latest("bootstrap").unwrap().graph;
        let error = append_bootstrap_telemetry(&storage, run, &[], &graph).await.unwrap_err();
        assert!(matches!(error, BootstrapError::Storage(StorageError::Sqlite(_))), "{error}");
        let writer = storage.open_run_writer(run).await.unwrap();
        writer.close().await.unwrap();
        let events = storage.open_run_reader(run).await.unwrap().read_events(EventSeq(0)..EventSeq(u64::MAX)).await.unwrap();
        assert!(events.is_empty());
        drop(connection);
        drop(storage);
        directory.close().unwrap();
        }).await.unwrap();
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
