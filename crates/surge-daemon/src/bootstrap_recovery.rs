//! Startup classification for the durable bootstrap supervisor.
//!
//! The supervisor checks active handles and serializes starts/resumes before
//! inspecting storage. This module never starts, repairs, or deletes a run.

use std::path::PathBuf;

use surge_core::{
    ContentHash, RunId, RunStatus,
    run_event::{EventPayload, RunConfig},
};
use surge_orchestrator::engine::handle::RunOutcome;
use surge_persistence::runs::inspection::{RunDatabaseInspection, RunInspection};

/// Expected persisted projection of a prepared bootstrap launch (not a journal payload).
pub struct ExpectedBootstrapRun {
    /// Reserved identity, never replaced on recovery.
    pub run_id: RunId,
    /// Exact execution checkout, not the original project or a guessed path.
    pub worktree: PathBuf,
    /// Exact original input.
    pub initial_prompt: String,
    /// Persisted engine configuration projection.
    pub config: RunConfig,
    /// Frozen initial graph identity.
    pub graph_hash: ContentHash,
}

/// Durable evidence for the supervisor's next transition.
#[derive(Debug, PartialEq)]
pub enum StartupState {
    /// No storage artifacts or registry row; phase policy may allow initial start.
    Absent,
    /// Safe startup identity exists; resume the same ID after remaining checks.
    Started,
    /// Capacity pause; the supervisor retains ownership of wake scheduling.
    Parked {
        /// Persisted capacity wake time.
        wake_at: chrono::DateTime<chrono::Utc>,
    },
    /// Authoritative final outcome, including failures and cancellation.
    Terminal(RunOutcome),
    /// Preserve evidence and enter attention; never retry a fresh start blindly.
    Incomplete(StartupConflict),
}

/// Typed inconsistencies; diagnostics deliberately omit configuration contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupConflict {
    /// A directory exists without its event database.
    DirectoryWithoutDatabase,
    /// A registry row exists without its event database.
    RegistryWithoutDatabase,
    /// A database exists before registry commit.
    DatabaseWithoutRegistry,
    /// Initial ordered startup events are incomplete or duplicated.
    StartupEvents,
    /// Event sequence is not contiguous or its tag disagrees with its payload.
    EventSequence,
    /// Persisted identity differs from the reserved launch.
    Identity,
    /// Registry claims termination without matching durable evidence.
    RegistryOutcome,
    /// More than one definitive final outcome exists.
    ConflictingOutcomes,
    /// Capacity event and registry state disagree.
    ParkState,
}

/// Validate startup identity and fold typed durable lifecycle evidence.
///
/// Errors encoding a graph are surfaced rather than converted into absence.
/// This is not artifact validation or proof that runtime configuration still matches.
pub fn classify_startup(
    inspection: &RunInspection,
    expected: &ExpectedBootstrapRun,
) -> Result<StartupState, serde_json::Error> {
    use StartupConflict as Conflict;
    let events = match &inspection.database {
        RunDatabaseInspection::Absent => {
            return Ok(
                match (&inspection.registry, inspection.run_directory_present) {
                    (Some(_), _) => StartupState::Incomplete(Conflict::RegistryWithoutDatabase),
                    (None, true) => StartupState::Incomplete(Conflict::DirectoryWithoutDatabase),
                    (None, false) => StartupState::Absent,
                },
            );
        },
        RunDatabaseInspection::Present { events } => events,
    };
    let Some(registry) = &inspection.registry else {
        return Ok(StartupState::Incomplete(Conflict::DatabaseWithoutRegistry));
    };
    if registry.id != expected.run_id || registry.project_path != expected.worktree {
        return Ok(StartupState::Incomplete(Conflict::Identity));
    }
    if events.iter().enumerate().any(|(index, event)| {
        event.seq.0 != index as u64 + 1 || event.kind != event.payload.payload.discriminant_str()
    }) {
        return Ok(StartupState::Incomplete(Conflict::EventSequence));
    }
    let Some(start) = events.first() else {
        return Ok(StartupState::Incomplete(Conflict::StartupEvents));
    };
    let EventPayload::RunStarted {
        project_path,
        initial_prompt,
        config,
        pipeline_template,
    } = &start.payload.payload
    else {
        return Ok(StartupState::Incomplete(Conflict::StartupEvents));
    };
    if project_path != &expected.worktree
        || initial_prompt != &expected.initial_prompt
        || config != &expected.config
        || pipeline_template.is_some()
    {
        return Ok(StartupState::Incomplete(Conflict::Identity));
    }
    let Some(materialized) = events.get(1) else {
        return Ok(StartupState::Incomplete(Conflict::StartupEvents));
    };
    let EventPayload::PipelineMaterialized { graph, graph_hash } = &materialized.payload.payload
    else {
        return Ok(StartupState::Incomplete(Conflict::StartupEvents));
    };
    if graph_hash != &expected.graph_hash
        || ContentHash::compute(&serde_json::to_vec(graph)?) != *graph_hash
    {
        return Ok(StartupState::Incomplete(Conflict::Identity));
    }
    if events
        .iter()
        .skip(2)
        .any(|event| matches!(event.payload.payload, EventPayload::RunStarted { .. }))
    {
        return Ok(StartupState::Incomplete(Conflict::StartupEvents));
    }
    Ok(fold_lifecycle(events, registry, expected))
}

fn fold_lifecycle(
    events: &[surge_persistence::runs::ReadEvent],
    registry: &surge_persistence::runs::RunSummary,
    expected: &ExpectedBootstrapRun,
) -> StartupState {
    let registry_status = registry.status;
    let mut terminal = None;
    let mut parked = None;
    for event in events {
        let outcome = match &event.payload.payload {
            EventPayload::RunCompleted { terminal_node } => Some(RunOutcome::Completed {
                terminal: terminal_node.clone(),
            }),
            EventPayload::RunFailed { error } => Some(RunOutcome::Failed {
                error: error.clone(),
            }),
            EventPayload::RunAborted { reason } => Some(RunOutcome::Aborted {
                reason: reason.clone(),
            }),
            EventPayload::RunParked {
                wake_at, worktree, ..
            } => {
                if worktree != &expected.worktree {
                    return StartupState::Incomplete(StartupConflict::Identity);
                }
                parked = Some(*wake_at);
                None
            },
            EventPayload::RunWokeFromPark {} => {
                parked = None;
                None
            },
            _ => None,
        };
        if let Some(outcome) = outcome {
            if terminal.is_some() {
                return StartupState::Incomplete(StartupConflict::ConflictingOutcomes);
            }
            terminal = Some(outcome);
        }
    }
    if let Some(outcome) = terminal {
        let agrees = match registry_status {
            RunStatus::Completed => matches!(outcome, RunOutcome::Completed { .. }),
            RunStatus::Failed => matches!(outcome, RunOutcome::Failed { .. }),
            RunStatus::Aborted => matches!(outcome, RunOutcome::Aborted { .. }),
            _ => true,
        };
        return if agrees {
            StartupState::Terminal(outcome)
        } else {
            StartupState::Incomplete(StartupConflict::RegistryOutcome)
        };
    }
    if matches!(
        registry_status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
    ) {
        return StartupState::Incomplete(StartupConflict::RegistryOutcome);
    }
    match (parked, registry_status == RunStatus::Parked) {
        (Some(wake_at), true) if registry.wake_at_ms == Some(wake_at.timestamp_millis()) => {
            StartupState::Parked { wake_at }
        },
        (None, false) => StartupState::Started,
        _ => StartupState::Incomplete(StartupConflict::ParkState),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{VersionedEventPayload, keys::NodeKey};
    use surge_persistence::runs::{ReadEvent, RunSummary, seq::EventSeq};

    fn fixture() -> (RunInspection, ExpectedBootstrapRun) {
        let graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
        let expected = ExpectedBootstrapRun {
            run_id: RunId::new(),
            worktree: PathBuf::from("/exact/planning"),
            initial_prompt: "build\nthis exactly".into(),
            config: RunConfig {
                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: vec![],
                budget: surge_core::budget::BudgetGuard::default(),
            },
            graph_hash: ContentHash::compute(&serde_json::to_vec(&graph).unwrap()),
        };
        let summary = RunSummary {
            id: expected.run_id,
            project_path: expected.worktree.clone(),
            pipeline_template: None,
            status: RunStatus::Running,
            started_at_ms: 0,
            ended_at_ms: None,
            daemon_pid: None,
            wake_at_ms: None,
        };
        let events = vec![
            event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: expected.worktree.clone(),
                    initial_prompt: expected.initial_prompt.clone(),
                    config: expected.config.clone(),
                },
            ),
            event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: expected.graph_hash,
                },
            ),
        ];
        (
            RunInspection {
                registry: Some(summary),
                run_directory_present: true,
                database: RunDatabaseInspection::Present { events },
            },
            expected,
        )
    }

    fn event(seq: u64, payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms: 0,
            kind: payload.discriminant_str().into(),
            payload: VersionedEventPayload::new(payload),
        }
    }

    fn push(inspection: &mut RunInspection, payload: EventPayload) {
        let RunDatabaseInspection::Present { events } = &mut inspection.database else {
            panic!("expected database")
        };
        events.push(event(events.len() as u64 + 1, payload));
    }

    #[test]
    fn valid_start_and_partial_storage_are_distinct() {
        let (mut inspection, expected) = fixture();
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Started
        );
        inspection.registry = None;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::DatabaseWithoutRegistry)
        );
        inspection.database = RunDatabaseInspection::Absent;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::DirectoryWithoutDatabase)
        );
        inspection.run_directory_present = false;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Absent
        );
        inspection.registry = fixture().0.registry;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::RegistryWithoutDatabase)
        );
    }

    #[test]
    fn identity_checks_path_prompt_config_and_graph_content() {
        for mutation in 0..5 {
            let (inspection, mut expected) = fixture();
            match mutation {
                0 => expected.worktree = "/guessed/path".into(),
                1 => expected.initial_prompt.push('!'),
                2 => expected.config.auto_pr = true,
                3 => expected.graph_hash = ContentHash::compute(b"foreign"),
                _ => expected.run_id = RunId::new(),
            }
            assert_eq!(
                classify_startup(&inspection, &expected).unwrap(),
                StartupState::Incomplete(StartupConflict::Identity)
            );
        }
        let (mut inspection, expected) = fixture();
        let RunDatabaseInspection::Present { events } = &mut inspection.database else {
            unreachable!()
        };
        let EventPayload::PipelineMaterialized { graph, .. } = &mut events[1].payload.payload
        else {
            unreachable!()
        };
        graph.metadata.name = "tampered".into();
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::Identity)
        );
    }

    #[test]
    fn initial_batch_and_sequence_must_be_complete() {
        for remaining in [0, 1] {
            let (mut inspection, expected) = fixture();
            let RunDatabaseInspection::Present { events } = &mut inspection.database else {
                unreachable!()
            };
            events.truncate(remaining);
            assert_eq!(
                classify_startup(&inspection, &expected).unwrap(),
                StartupState::Incomplete(StartupConflict::StartupEvents)
            );
        }
        let (mut inspection, expected) = fixture();
        let RunDatabaseInspection::Present { events } = &mut inspection.database else {
            unreachable!()
        };
        events[1].seq = EventSeq(3);
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::EventSequence)
        );
    }

    #[test]
    fn terminal_types_survive_wake_and_registry_lag() {
        for (payload, outcome) in [
            (
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_new("end").unwrap(),
                },
                RunOutcome::Completed {
                    terminal: NodeKey::try_new("end").unwrap(),
                },
            ),
            (
                EventPayload::RunFailed {
                    error: "failure".into(),
                },
                RunOutcome::Failed {
                    error: "failure".into(),
                },
            ),
            (
                EventPayload::RunAborted {
                    reason: "cancel".into(),
                },
                RunOutcome::Aborted {
                    reason: "cancel".into(),
                },
            ),
        ] {
            let (mut inspection, expected) = fixture();
            push(&mut inspection, payload.clone());
            push(&mut inspection, EventPayload::RunWokeFromPark {});
            assert_eq!(
                classify_startup(&inspection, &expected).unwrap(),
                StartupState::Terminal(outcome)
            );
            push(&mut inspection, payload);
            assert_eq!(
                classify_startup(&inspection, &expected).unwrap(),
                StartupState::Incomplete(StartupConflict::ConflictingOutcomes)
            );
            push(
                &mut inspection,
                EventPayload::RunFailed {
                    error: "conflicting".into(),
                },
            );
            assert_eq!(
                classify_startup(&inspection, &expected).unwrap(),
                StartupState::Incomplete(StartupConflict::ConflictingOutcomes)
            );
        }
    }

    fn park(expected: &ExpectedBootstrapRun) -> EventPayload {
        EventPayload::RunParked {
            wake_at: chrono::DateTime::from_timestamp(100, 0).unwrap(),
            runtime: None,
            worktree: expected.worktree.clone(),
            basis: surge_core::capacity::WakeBasis::PolicyBackoff,
            reason: "capacity".into(),
        }
    }

    #[test]
    fn park_wake_repark_and_registry_only_terminal() {
        let (mut inspection, expected) = fixture();
        push(&mut inspection, park(&expected));
        inspection.registry.as_mut().unwrap().status = RunStatus::Parked;
        inspection.registry.as_mut().unwrap().wake_at_ms = Some(100_000);
        assert!(matches!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Parked { .. }
        ));
        push(&mut inspection, EventPayload::RunWokeFromPark {});
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::ParkState)
        );
        inspection.registry.as_mut().unwrap().status = RunStatus::Running;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Started
        );
        push(&mut inspection, park(&expected));
        inspection.registry.as_mut().unwrap().status = RunStatus::Parked;
        inspection.registry.as_mut().unwrap().wake_at_ms = Some(100_000);
        assert!(matches!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Parked { .. }
        ));
        inspection.registry.as_mut().unwrap().status = RunStatus::Completed;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::RegistryOutcome)
        );
    }
    #[test]
    fn duplicate_start_and_conflicting_registry_terminal_need_attention() {
        let (mut inspection, expected) = fixture();
        let RunDatabaseInspection::Present { events } = &inspection.database else {
            unreachable!()
        };
        let duplicate = events[0].payload.payload.clone();
        push(&mut inspection, duplicate);
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::StartupEvents)
        );
        let (mut inspection, expected) = fixture();
        push(
            &mut inspection,
            EventPayload::RunCompleted {
                terminal_node: NodeKey::try_new("end").unwrap(),
            },
        );
        inspection.registry.as_mut().unwrap().status = RunStatus::Failed;
        assert_eq!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Incomplete(StartupConflict::RegistryOutcome)
        );
    }
    #[test]
    fn generated_flow_materialization_does_not_replace_initial_identity() {
        let (mut inspection, expected) = fixture();
        let mut generated: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
        generated.metadata.name = "generated implementation".into();
        // Bootstrap validation hashes original flow TOML, not graph JSON.
        push(
            &mut inspection,
            EventPayload::PipelineMaterialized {
                graph: Box::new(generated),
                graph_hash: ContentHash::compute(b"generated TOML bytes"),
            },
        );
        push(
            &mut inspection,
            EventPayload::RunCompleted {
                terminal_node: NodeKey::try_new("end").unwrap(),
            },
        );
        assert!(matches!(
            classify_startup(&inspection, &expected).unwrap(),
            StartupState::Terminal(RunOutcome::Completed { .. })
        ));
    }
    #[test]
    fn parked_registry_requires_matching_wake_time() {
        for wake_at_ms in [None, Some(999_000)] {
            let (mut inspection, expected) = fixture();
            push(&mut inspection, park(&expected));
            let registry = inspection.registry.as_mut().unwrap();
            registry.status = RunStatus::Parked;
            registry.wake_at_ms = wake_at_ms;
            assert_eq!(
                classify_startup(&inspection, &expected).unwrap(),
                StartupState::Incomplete(StartupConflict::ParkState)
            );
        }
    }
}
