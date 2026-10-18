use super::*;
use crate::run_event::RunConfig;
use crate::run_state::{RunMemory, RunState};
use crate::task_execution::TaskExecutionView;
use crate::work_item::{WorkItemBinding, WorkItemContext, WorkItemRequirements};

fn run() -> crate::RunId {
    "run-00000000000000000000000001".parse().unwrap()
}
fn start() -> EventPayload {
    EventPayload::RunStarted {
        pipeline_template: None,
        project_path: "/project".into(),
        initial_prompt: "build".into(),
        config: RunConfig {
            bootstrap_edit_loop_cap: None,
            sandbox_default: crate::sandbox::SandboxMode::WorkspaceWrite,
            approval_default: crate::approvals::ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
            budget: Default::default(),
        },
    }
}
fn event(seq: u64, payload: EventPayload) -> RunEvent {
    RunEvent {
        run_id: run(),
        seq,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload,
    }
}
fn recovery(generation: u64, diagnostic: &str) -> EventPayload {
    EventPayload::RunRecoveryRequired {
        control_generation: generation,
        diagnostic: diagnostic.into(),
    }
}
fn stage() -> EventPayload {
    EventPayload::StageEntered {
        node: "agent".parse().unwrap(),
        attempt: 1,
    }
}
fn binding() -> EventPayload {
    let requirements = WorkItemRequirements::new("build".into(), vec!["works".into()]).unwrap();
    let binding = WorkItemBinding {
        item: crate::id::WorkItemId::new(),
        revision: 1,
        generation: 1,
        requirements_hash: requirements.hash().unwrap(),
    };
    EventPayload::WorkItemAttemptBound {
        context: WorkItemContext::new(binding, requirements).unwrap(),
    }
}
fn observe(payloads: Vec<EventPayload>) -> RecoveryObserver {
    let mut observer = RecoveryObserver::default();
    for (i, payload) in payloads.into_iter().enumerate() {
        observer.observe(&event(i as u64 + 1, payload));
    }
    observer
}
#[test]
fn recovery_bound_generation_zero_requires_exact_strict_continuation() {
    let mut observer = observe(vec![start(), binding(), recovery(0, "bound"), stage()]);
    assert_eq!(observer.observation().active().unwrap().diagnostic, "bound");
    observer.observe(&event(
        5,
        EventPayload::RunContinued {
            control_generation: 0,
        },
    ));
    assert!(observer.observation().active().is_some());
    observer.observe(&event(
        6,
        EventPayload::RunContinued {
            control_generation: 1,
        },
    ));
    assert!(observer.observation().active().is_none());
    assert_eq!(
        observer.observation().latest().unwrap().disposition,
        RecoveryDisposition::ProgressObserved { seq: 6 }
    );
}
#[test]
fn recovery_latest_sequence_wins_and_resets_supersession_without_generation_promotion() {
    let mut observer = observe(vec![
        start(),
        recovery(8, "older"),
        stage(),
        recovery(2, "newer"),
    ]);
    let latest = observer.observation().active().unwrap();
    assert_eq!(
        (
            latest.seq,
            latest.control_generation,
            latest.diagnostic.as_str()
        ),
        (4, 2, "newer")
    );
    observer.observe(&event(
        5,
        EventPayload::RunContinued {
            control_generation: 8,
        },
    ));
    assert!(observer.observation().active().is_some());
    observer.observe(&event(
        6,
        EventPayload::RunContinued {
            control_generation: 9,
        },
    ));
    assert!(observer.observation().active().is_none());
}
#[test]
fn recovery_higher_suspension_is_not_progress() {
    let mut observer = observe(vec![start(), binding(), recovery(1, "blocked")]);
    observer.observe(&event(
        4,
        EventPayload::RunSuspended {
            fence: crate::execution_recovery::SuspensionFence {
                control_generation: 9,
                snapshot_seq: 3,
                pending_stage: crate::execution_recovery::PendingStagePhase::BetweenStages,
                cleanup_confirmed: true,
                reason: crate::execution_recovery::SuspensionReason::Manual,
            },
        },
    ));
    observer.observe(&event(5, stage()));
    assert!(observer.observation().active().is_some());
    observer.observe(&event(6, recovery(12, "higher refusal")));
    assert_eq!(observer.observation().active().unwrap().seq, 6);
}
#[test]
fn recovery_unbound_progress_requires_complete_started_prefix_and_gaps_are_sticky() {
    let progressed = observe(vec![start(), recovery(0, "ordinary"), stage()]);
    assert!(progressed.observation().is_complete());
    assert!(progressed.observation().active().is_none());
    let missing = observe(vec![recovery(0, "missing start"), stage()]);
    assert!(!missing.observation().is_complete());
    assert_eq!(
        missing.observation().latest().unwrap().disposition,
        RecoveryDisposition::Blocked
    );
    let mut gap = observe(vec![start(), recovery(0, "gap")]);
    gap.observe(&event(4, stage()));
    gap.observe(&event(5, stage()));
    assert!(!gap.observation().is_complete());
    assert_eq!(
        gap.observation().latest().unwrap().disposition,
        RecoveryDisposition::Blocked
    );
    assert!(gap.observation().active().is_none());
}
#[test]
fn recovery_duplicate_or_descending_payload_cannot_change_diagnostic_or_progress() {
    let mut observer = observe(vec![start(), recovery(0, "original")]);
    observer.observe(&event(2, recovery(99, "divergent duplicate")));
    observer.observe(&event(
        1,
        EventPayload::RunContinued {
            control_generation: 100,
        },
    ));
    observer.observe(&event(0, stage()));
    assert!(!observer.observation().is_complete());
    let latest = observer.observation().latest().unwrap();
    assert_eq!(latest.diagnostic, "original");
    assert_eq!(latest.disposition, RecoveryDisposition::Blocked);
    assert_eq!(observer.control_generation, 0);
}
#[test]
fn recovery_terminal_is_historical_and_invalidation_preserves_problem() {
    let mut observer = observe(vec![
        start(),
        recovery(0, "problem"),
        EventPayload::RunCompleted {
            terminal_node: "end".parse().unwrap(),
        },
    ]);
    assert!(observer.observation().active().is_none());
    observer.observe(&event(4, recovery(0, "later diagnostic")));
    assert!(observer.observation().active().is_none());
    assert_eq!(
        observer.observation().latest().unwrap().diagnostic,
        "later diagnostic"
    );
    observer.invalidate();
    observer.invalidate();
    assert_eq!(observer.observation().latest().unwrap().seq, 4);
    assert!(!observer.observation().is_complete());
}
fn state_observation(state: &RunState) -> &RecoveryObservation {
    match state {
        RunState::Bootstrapping { recovery, .. } => recovery.observation(),
        RunState::Pipeline { memory, .. } => memory.recovery.observation(),
        _ => panic!("expected nonterminal observation"),
    }
}
#[test]
fn recovery_exact_once_parity_across_bootstrap_materialization_and_nested_memory_paths() {
    let graph: crate::Graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    let node = graph.start.clone();
    let hash = crate::ContentHash::compute(b"graph");
    let payloads = vec![
        start(),
        binding(),
        recovery(0, "before materialization"),
        EventPayload::BootstrapApprovalDecided {
            stage: crate::run_event::BootstrapStage::Description,
            decision: crate::run_event::BootstrapDecision::Approve,
            comment: None,
        },
        EventPayload::PipelineMaterialized {
            graph: Box::new(graph.clone()),
            graph_hash: hash,
        },
        EventPayload::StageEntered {
            node: node.clone(),
            attempt: 1,
        },
        EventPayload::GraphRevisionAccepted {
            patch_id: crate::roadmap_patch::RoadmapPatchId::new("rpatch-observation").unwrap(),
            target: crate::roadmap_patch::RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: "roadmap.md".into(),
            },
            previous_graph_hash: hash,
            graph: Box::new(graph),
            graph_hash: hash,
            active_pickup: crate::roadmap_patch::ActivePickupPolicy::Allowed,
        },
        EventPayload::BootstrapEditRequested {
            stage: crate::run_event::BootstrapStage::Flow,
            feedback: "edit".into(),
        },
        EventPayload::RunContinued {
            control_generation: 1,
        },
        EventPayload::StageEntered { node, attempt: 2 },
        recovery(0, "later lower generation"),
    ];
    let mut state = RunState::not_started();
    let mut memory = RunMemory::default();
    let mut observer = RecoveryObserver::default();
    let mut log = Vec::new();
    for (i, payload) in payloads.into_iter().enumerate() {
        let event = event(i as u64 + 1, payload);
        state = crate::run_state::apply(state, &event).unwrap();
        memory.apply_event(&event).unwrap();
        observer.observe(&event);
        log.push(event);
        let projected = TaskExecutionView::compile(run(), &log).unwrap();
        assert_eq!(
            state_observation(&state),
            observer.observation(),
            "state at {}",
            i + 1
        );
        assert_eq!(
            memory.recovery.observation(),
            observer.observation(),
            "memory at {}",
            i + 1
        );
        assert_eq!(
            &projected.recovery,
            observer.observation(),
            "compiler at {}",
            i + 1
        );
        assert_eq!(
            state_observation(&crate::run_state::fold(&log).unwrap()),
            observer.observation()
        );
        if i == 2 || i == 5 || i == 10 {
            assert_eq!(
                crate::run_display::RunDisplayState::from_state(Some(&state), None),
                crate::run_display::RunDisplayState::Waiting(
                    crate::run_display::WaitingReason::RecoveryRequired
                )
            );
        }
        if i == 8 {
            assert_eq!(
                crate::run_display::RunDisplayState::from_state(Some(&state), None),
                crate::run_display::RunDisplayState::Unknown
            );
            assert_eq!(
                crate::run_display::RunDisplayState::from_state_with_observation(
                    Some(&state),
                    None,
                    true
                ),
                crate::run_display::RunDisplayState::Working
            );
        }
        if i == 5 {
            assert!(
                observer.observation().active().is_some(),
                "pre-materialization binding retained"
            );
        }
    }
}
#[test]
fn recovery_diagnostic_is_bounded_utf8_literal_and_preserves_original() {
    let original = format!("\u{1b}[31m\0\r\t\u{202e}\n{}", "界".repeat(3000));
    let rendered = recovery_diagnostic(&original);
    assert!(rendered.len() <= RECOVERY_DIAGNOSTIC_BYTES);
    assert!(rendered.ends_with(TRUNCATED));
    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{202e}'));
    assert!(!rendered.contains('\r'));
    assert!(original.contains('\u{1b}'));
    assert_eq!(recovery_diagnostic("ordinary\nline"), "ordinary\nline");
    assert_eq!(recovery_diagnostic(&"a".repeat(4096)).len(), 4096);
}

#[test]
fn recovery_serialized_coverage_keeps_unknown_and_complete_absence_distinct() {
    let empty = TaskExecutionView::compile(run(), &[]).unwrap();
    assert!(matches!(
        empty.recovery,
        RecoveryObservation::NotLoaded
            | RecoveryObservation::PartialPrefix {
                through: None,
                latest: None
            }
    ));
    // Compilation is a load, even when the supplied slice contains no envelopes.
    assert!(matches!(
        empty.recovery,
        RecoveryObservation::PartialPrefix {
            through: None,
            latest: None
        }
    ));
    let no_problem = observe(vec![start()]);
    assert!(matches!(
        no_problem.observation(),
        RecoveryObservation::CompletePrefix {
            through: 1,
            latest: None
        }
    ));
    let with_problem = observe(vec![start(), recovery(0, "known")]);
    for observation in [
        RecoveryObservation::NotLoaded,
        empty.recovery,
        no_problem.observation().clone(),
        with_problem.observation().clone(),
    ] {
        let value = serde_json::to_value(&observation).unwrap();
        assert_eq!(
            serde_json::from_value::<RecoveryObservation>(value).unwrap(),
            observation
        );
    }
    let mut missing_start = vec![event(1, recovery(0, "no start")), event(2, stage())];
    assert!(
        !TaskExecutionView::compile(run(), &missing_start)
            .unwrap()
            .recovery
            .is_complete()
    );
    missing_start[0].seq = 2;
    missing_start[1].seq = 3;
    assert!(
        !TaskExecutionView::compile(run(), &missing_start)
            .unwrap()
            .recovery
            .is_complete()
    );
    for invalid in [
        serde_json::Value::Null,
        serde_json::json!({"observation":"future"}),
    ] {
        assert!(serde_json::from_value::<RecoveryObservation>(invalid).is_err());
    }
}

#[test]
fn recovery_disposition_roundtrips_and_rejects_unknown_or_null_leaf() {
    let observer = observe(vec![start(), recovery(0, "record")]);
    let base = serde_json::to_value(observer.observation()).unwrap();
    for disposition in [
        RecoveryDisposition::Blocked,
        RecoveryDisposition::ProgressObserved { seq: 3 },
        RecoveryDisposition::TerminalObserved { seq: 1 },
    ] {
        let value = serde_json::to_value(&disposition).unwrap();
        assert_eq!(
            serde_json::from_value::<RecoveryDisposition>(value.clone()).unwrap(),
            disposition
        );
        let mut snapshot = base.clone();
        snapshot["latest"]["disposition"] = value;
        assert_eq!(
            serde_json::from_value::<RecoveryObservation>(snapshot)
                .unwrap()
                .latest()
                .unwrap()
                .disposition,
            disposition
        );
    }
    for invalid in [
        serde_json::Value::Null,
        serde_json::json!({"state":"future"}),
    ] {
        let mut snapshot = base.clone();
        snapshot["latest"]["disposition"] = invalid;
        assert!(serde_json::from_value::<RecoveryObservation>(snapshot).is_err());
    }
    let mut terminal = observe(vec![
        start(),
        recovery(0, "before"),
        EventPayload::RunAborted {
            reason: "stopped".into(),
        },
    ]);
    assert_eq!(
        terminal.observation().latest().unwrap().disposition,
        RecoveryDisposition::TerminalObserved { seq: 3 }
    );
    terminal.observe(&event(4, recovery(0, "after")));
    assert_eq!(
        terminal.observation().latest().unwrap().disposition,
        RecoveryDisposition::TerminalObserved { seq: 3 }
    );
    terminal.invalidate();
    assert_eq!(
        terminal.observation().latest().unwrap().disposition,
        RecoveryDisposition::TerminalObserved { seq: 3 }
    );
    assert!(terminal.observation().active().is_none());
}

#[test]
fn recovery_display_unknown_prefix_and_terminal_precedence_are_independent() {
    let partial = crate::run_state::apply(RunState::not_started(), &event(2, start())).unwrap();
    assert_eq!(
        crate::run_display::RunDisplayState::from_state(Some(&partial), None),
        crate::run_display::RunDisplayState::Unknown
    );
    let terminal = RunState::Terminal {
        semantic_startup_guard: Default::default(),
        kind: crate::run_state::TerminalReason::Completed,
        reason: String::new(),
    };
    assert!(matches!(
        crate::run_display::RunDisplayState::from_state(
            Some(&terminal),
            Some(crate::RunStatus::Crashed)
        ),
        crate::run_display::RunDisplayState::Done(crate::run_state::TerminalReason::Completed)
    ));
}

#[test]
fn recovery_recorded_progress_without_current_observation_is_unknown() {
    for bound in [false, true] {
        let graph: crate::Graph =
            toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
        let node = graph.start.clone();
        let mut payloads = vec![start()];
        if bound {
            payloads.push(binding());
        }
        payloads.push(EventPayload::PipelineMaterialized {
            graph: Box::new(graph),
            graph_hash: crate::ContentHash::compute(b"history"),
        });
        payloads.push(recovery(0, "recorded problem"));
        if bound {
            payloads.push(EventPayload::RunContinued {
                control_generation: 1,
            });
        } else {
            payloads.push(EventPayload::StageEntered { node, attempt: 1 });
        }
        let log: Vec<_> = payloads
            .into_iter()
            .enumerate()
            .map(|(i, payload)| event(i as u64 + 1, payload))
            .collect();
        let state = crate::run_state::fold(&log).unwrap();
        assert!(state_observation(&state).active().is_none());
        assert_eq!(
            state_observation(&state).latest().unwrap().diagnostic,
            "recorded problem"
        );
        assert_eq!(
            crate::run_display::RunDisplayState::from_state(Some(&state), None),
            crate::run_display::RunDisplayState::Unknown,
            "historical progress (bound={bound}) does not establish a current observation"
        );
        assert_eq!(
            crate::run_display::RunDisplayState::from_state_with_observation(
                Some(&state),
                None,
                true
            ),
            crate::run_display::RunDisplayState::Working
        );
        assert_eq!(
            crate::run_display::RunDisplayState::from_state_with_observation(
                Some(&state),
                None,
                false
            ),
            crate::run_display::RunDisplayState::Unknown
        );
    }
}

#[test]
fn recovery_current_observation_cannot_override_missing_coverage_refusal_or_terminal() {
    use crate::run_display::{RunDisplayState as Display, WaitingReason};
    let mut state = crate::run_state::apply(RunState::not_started(), &event(1, start())).unwrap();
    assert_eq!(Display::from_state(Some(&state), None), Display::Unknown);
    assert_eq!(
        Display::from_state_with_observation(Some(&state), None, true),
        Display::Working
    );
    state = crate::run_state::apply(state, &event(2, recovery(0, "refusal"))).unwrap();
    for current in [false, true] {
        assert_eq!(
            Display::from_state_with_observation(Some(&state), None, current),
            Display::Waiting(WaitingReason::RecoveryRequired)
        );
    }
    let partial = crate::run_state::apply(RunState::not_started(), &event(2, start())).unwrap();
    assert_eq!(
        Display::from_state_with_observation(Some(&partial), None, true),
        Display::Unknown
    );
    assert_eq!(
        Display::from_state_with_observation(None, Some(crate::RunStatus::Running), true),
        Display::Unknown
    );
    state = crate::run_state::apply(
        state,
        &event(
            3,
            EventPayload::RunFailed {
                error: "failed".into(),
            },
        ),
    )
    .unwrap();
    for current in [false, true] {
        assert_eq!(
            Display::from_state_with_observation(
                Some(&state),
                Some(crate::RunStatus::Crashed),
                current
            ),
            Display::Done(crate::run_state::TerminalReason::Failed)
        );
    }
}
