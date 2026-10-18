//! Fixed semantic/schema/report oracles, labelled post-edit coverage.
mod fixture {
    use crate as domain;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/continuation_seed.rs"
    ));
}
use super::seed_guard::{SeedHistoryError, SeedStartupGuard, preflight_unhydrated};
use super::semantic_position::{ContinuationSeedId, SeedLineage, SemanticPosition, SourceEventRef};
use crate::{EventPayload, RunId, RunMemory};

#[test]
fn unhydrated_marker_blocks_full_memory_task_and_report_semantics_but_retains_terminal() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let mut ordinary = startup.clone();
    ordinary.extend(fixture::positive_tail(run, 3));
    let control = crate::run_report::RunReport::compile(run, &ordinary);
    assert_eq!(
        control.evidence_backed,
        Some(true),
        "positive ordinary control must be meaningful"
    );
    let mut seeded = startup.clone();
    seeded.push(fixture::event(
        run,
        4,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest),
        },
    ));
    seeded.extend(fixture::positive_tail(run, 4));
    assert_eq!(
        preflight_unhydrated(&seeded),
        Err(SeedHistoryError::Unavailable)
    );
    assert!(crate::run_state::fold(&seeded).is_err());
    assert!(crate::task_execution::TaskExecutionView::compile(run, &seeded).is_err());
    let mut memory = RunMemory::default();
    let before = memory.clone();
    assert!(memory.apply_events(&seeded).is_err());
    assert_eq!(memory, before);
    let report = crate::run_report::RunReport::compile(run, &seeded);
    assert!(matches!(
        report.completion,
        crate::run_report::RunCompletion::Completed { .. }
    ));
    assert_eq!(
        report.evidence_backed, None,
        "unhydrated semantic tasks cannot be certified or declared failed"
    );
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["semantic_history"]["availability"], "unavailable");
    assert!(crate::run_report::render_markdown(&report).contains("Semantic history unavailable"));
    assert!(crate::run_report::render_html(&report).contains("Semantic history unavailable"));
    let mut old = serde_json::to_value(&control).unwrap();
    old.as_object_mut().unwrap().remove("semantic_history");
    let restored: crate::run_report::RunReport = serde_json::from_value(old).unwrap();
    assert_eq!(
        serde_json::to_value(restored).unwrap()["semantic_history"]["availability"],
        "available"
    );
}
#[test]
fn shared_incremental_marker_guard_distinguishes_duplicate_late_and_wrong_binding() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let marker = fixture::event(
        run,
        4,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest.clone()),
        },
    );
    let mut guard = SeedStartupGuard::default();
    for event in &startup {
        guard.observe(event).unwrap();
    }
    guard.observe(&marker).unwrap();
    assert_eq!(guard.finish(), Err(SeedHistoryError::Unavailable));
    assert_eq!(
        guard.observe(&fixture::event(run, 5, marker.payload.clone())),
        Err(SeedHistoryError::Duplicate)
    );
    let mut late = startup.clone();
    late.push(fixture::event(
        run,
        4,
        EventPayload::StageEntered {
            node: crate::NodeKey::try_from("verify").unwrap(),
            attempt: 1,
        },
    ));
    late.push(fixture::event(run, 5, marker.payload.clone()));
    assert_eq!(preflight_unhydrated(&late), Err(SeedHistoryError::Late));
    let mut fields = manifest.fields().clone();
    fields.binding.generation += 1;
    let wrong = crate::work_item::ContinuationSeedManifest::try_from(fields).unwrap();
    let mut wrong_prefix = startup;
    wrong_prefix.push(fixture::event(
        run,
        4,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(wrong),
        },
    ));
    assert_eq!(
        preflight_unhydrated(&wrong_prefix),
        Err(SeedHistoryError::Binding)
    );
}
#[test]
fn manifest_deserialization_cannot_bypass_private_identity_checks() {
    let run = RunId::new();
    let (_, manifest) = fixture::startup(run);
    let original = serde_json::to_value(&manifest).unwrap();
    for (field, value) in [
        ("schema_version", serde_json::json!(2)),
        ("run", original["source"]["run"].clone()),
        ("source", serde_json::json!({"run":RunId::new(),"seq":0})),
        ("extra", serde_json::json!(true)),
    ] {
        let mut bad = original.clone();
        bad[field] = value;
        assert!(serde_json::from_value::<crate::work_item::ContinuationSeedManifest>(bad).is_err());
    }
    let mut bad = original.clone();
    bad["binding"]["revision"] = serde_json::json!(0);
    assert!(serde_json::from_value::<crate::work_item::ContinuationSeedManifest>(bad).is_err());
    let mut good = original.clone();
    good.as_object_mut().unwrap().remove("schema_version");
    let rest = serde_json::to_string(&good).unwrap();
    let duplicate = format!("{{\"schema_version\":1,\"schema_version\":1,{}", &rest[1..]);
    assert!(
        serde_json::from_str::<crate::work_item::ContinuationSeedManifest>(&duplicate).is_err()
    );
    assert_eq!(
        serde_json::from_value::<crate::work_item::ContinuationSeedManifest>(original).unwrap(),
        manifest
    );
}
#[test]
fn source_ledger_origin_is_separate_from_local_update_and_local_change_supersedes_it() {
    let run = RunId::new();
    let source = RunId::new();
    let seed = ContinuationSeedId::new(crate::ContentHash::compute(b"ledger"));
    let provenance = super::semantic_position::InheritedProvenance::new(
        seed,
        SourceEventRef::new(source, 87).unwrap(),
    );
    let task = crate::roadmap::RoadmapTaskId::from("task");
    let mut memory = RunMemory {
        semantic_lineage: Some(SeedLineage::new(
            seed,
            SourceEventRef::new(source, 100).unwrap(),
        )),
        ..RunMemory::default()
    };
    memory.ledger.tasks.insert(
        task.clone(),
        crate::run_state::LedgerTask {
            updated_seq: 4,
            inherited_provenance: Some(provenance),
            ..crate::run_state::LedgerTask::default()
        },
    );
    assert_eq!(memory.ledger.tasks[&task].updated_seq, 4);
    assert_eq!(
        memory.ledger.tasks[&task]
            .inherited_provenance
            .unwrap()
            .source()
            .seq(),
        87
    );
    memory
        .apply_event(&fixture::event(
            run,
            5,
            EventPayload::RequirementRevised {
                node: crate::NodeKey::try_from("verify").unwrap(),
                task: Some(task.clone()),
                text: "Local requirement".into(),
            },
        ))
        .unwrap();
    assert_eq!(memory.ledger.tasks[&task].updated_seq, 5);
    assert_eq!(memory.ledger.tasks[&task].inherited_provenance, None);
    assert_eq!(
        memory.requirement_revisions[0].seq,
        SemanticPosition::local(5).unwrap()
    );
}
#[test]
fn all_mutation_families_refuse_zero_before_observers_and_atomic_batch_prefix() {
    let run = RunId::new();
    for payload in [
        EventPayload::RequirementRevised {
            node: crate::NodeKey::try_from("verify").unwrap(),
            task: Some("task".into()),
            text: "Invalid requirement".into(),
        },
        EventPayload::ArtifactProduced {
            node: crate::NodeKey::try_from("verify").unwrap(),
            artifact: crate::ContentHash::compute(b"bytes"),
            path: "blob".into(),
            name: "artifact".into(),
            source_path: None,
        },
        EventPayload::OutcomeReported {
            node: crate::NodeKey::try_from("verify").unwrap(),
            outcome: crate::OutcomeKey::try_from("passed").unwrap(),
            summary: "invalid outcome".into(),
        },
    ] {
        let mut memory = RunMemory::default();
        let before = memory.clone();
        assert!(matches!(
            memory.apply_event(&fixture::event(run, 0, payload.clone())),
            Err(crate::run_state::FoldError::InvalidEventPosition)
        ));
        assert_eq!(memory, before);
        assert!(
            memory
                .apply_events(&[
                    fixture::event(run, 1, payload.clone()),
                    fixture::event(run, 0, payload)
                ])
                .is_err()
        );
        assert_eq!(memory, before);
    }
}

#[test]
fn actual_schema_23_marker_kind_roundtrip_and_old_reader_refusal() {
    let run = RunId::new();
    let (_, manifest) = fixture::startup(run);
    let payload = EventPayload::ContinuationSeedBound {
        manifest: Box::new(manifest),
    };
    assert_eq!(payload.discriminant_str(), "ContinuationSeedBound");
    let bytes = serde_json::to_vec(&crate::run_event::VersionedEventPayload {
        schema_version: 23,
        payload: payload.clone(),
    })
    .unwrap();
    assert_eq!(crate::migrate_payload(23, &bytes).unwrap(), payload);
    assert_eq!(crate::migrations::MAX_SUPPORTED_VERSION, 23);
    // An actual v22-max reader checks the wrapper version before attempting
    // enum decoding; the supported-range error must remain distinguishable.
    let too_new = serde_json::json!({"schema_version": 24, "payload": payload});
    assert!(matches!(
        crate::migrate_payload(24, &serde_json::to_vec(&too_new).unwrap()),
        Err(crate::SurgeError::SchemaTooNew { found: 24, max: 23 })
    ));
}

#[test]
fn run_memory_field_policy_is_exhaustive_and_ordinary_lineage_is_fresh() {
    // No `..`: adding a field requires an explicit seed/reset policy review.
    let RunMemory {
        semantic_lineage,
        semantic_startup_guard,
        unresolved_mcp_calls,
        execution_writers,
        provider_sessions,
        session_establishments,
        stage_occurrences,
        quota_plans,
        gate_recovery_error,
        gate_decisions,
        committed_stage_outcomes,
        committed_gate_stages,
        suspension,
        control_generation,
        bootstrap_edit_loop_cap,
        artifacts,
        artifacts_by_node,
        outcomes,
        costs,
        budget_warning_raised,
        budget_exceeded_noted,
        bootstrap_edit_counts,
        node_visits,
        backtrack_feedback,
        escalations,
        entered_via,
        requirement_revisions,
        runtime_rotations,
        last_edit_feedback_by_stage,
        roadmap_patches,
        latest_graph_revision,
        ledger,
        verification,
        verification_graph,
        work_item,
        recovery,
    } = RunMemory::default();
    assert_eq!(semantic_lineage, None);
    assert_eq!(semantic_startup_guard, SeedStartupGuard::default());
    assert!(
        unresolved_mcp_calls.is_empty()
            && execution_writers.is_empty()
            && provider_sessions.is_empty()
    );
    assert!(
        session_establishments.is_empty() && stage_occurrences.is_empty() && quota_plans.is_empty()
    );
    assert!(
        gate_recovery_error.is_none()
            && gate_decisions.is_empty()
            && committed_stage_outcomes.is_empty()
            && committed_gate_stages.is_empty()
    );
    assert!(suspension.is_none() && control_generation == 0 && bootstrap_edit_loop_cap.is_none());
    assert!(artifacts.is_empty() && artifacts_by_node.is_empty() && outcomes.is_empty());
    assert_eq!(costs, crate::run_state::CostSummary::default());
    assert!(!budget_warning_raised && !budget_exceeded_noted);
    assert!(
        bootstrap_edit_counts.is_empty()
            && node_visits.is_empty()
            && backtrack_feedback.is_empty()
            && escalations.is_empty()
    );
    assert!(
        entered_via.is_empty()
            && requirement_revisions.is_empty()
            && runtime_rotations.is_empty()
            && last_edit_feedback_by_stage.is_empty()
    );
    assert!(
        roadmap_patches.is_empty() && latest_graph_revision.is_none() && ledger.tasks.is_empty()
    );
    assert_eq!(
        verification,
        crate::verification_evidence::VerificationContext::default()
    );
    assert!(verification_graph.is_none() && work_item.is_none());
    assert_eq!(
        recovery,
        crate::recovery_observation::RecoveryObserver::default()
    );
}

#[test]
fn settled_amendment_dtos_keep_qualified_history_out_of_current_runtime_maps() {
    use crate::run_state::{SettledGraphRevisionHistory, SettledRoadmapPatchHistory};
    let seed = ContinuationSeedId::new(crate::ContentHash::compute(b"amendment-seed"));
    let position =
        SemanticPosition::inherited(seed, SourceEventRef::new(RunId::new(), 87).unwrap());
    let target = crate::roadmap_patch::RoadmapPatchTarget::ProjectRoadmap {
        roadmap_path: "roadmap.toml".into(),
    };
    let patch = SettledRoadmapPatchHistory {
        patch_id: crate::roadmap_patch::RoadmapPatchId::new("rpatch-source").unwrap(),
        target: target.clone(),
        status: crate::roadmap_patch::RoadmapPatchStatus::Applied,
        patch_artifact: None,
        patch_path: None,
        roadmap_artifact: None,
        roadmap_path: None,
        flow_artifact: None,
        flow_path: None,
        updated_at: position,
    };
    let graph = SettledGraphRevisionHistory {
        patch_id: patch.patch_id.clone(),
        target,
        previous_graph_hash: crate::ContentHash::compute(b"old"),
        graph_hash: crate::ContentHash::compute(b"new"),
        active_pickup: crate::roadmap_patch::ActivePickupPolicy::Allowed,
        updated_at: position,
    };
    let patch_wire = serde_json::to_value(&patch).unwrap();
    assert_eq!(patch_wire["updated_at"]["source"]["seq"], 87);
    assert!(patch_wire.get("updated_seq").is_none());
    assert_eq!(
        serde_json::from_value::<SettledRoadmapPatchHistory>(patch_wire).unwrap(),
        patch
    );
    assert_eq!(
        serde_json::from_value::<SettledGraphRevisionHistory>(
            serde_json::to_value(&graph).unwrap()
        )
        .unwrap(),
        graph
    );
    let memory = RunMemory::default();
    assert!(memory.roadmap_patches.is_empty());
    assert_eq!(memory.latest_graph_revision, None);
    assert!(memory.stage_occurrences.is_empty());
}

#[rustfmt::skip]
#[path = "baseline_v22_migrations.rs"]
mod baseline_v22_migrations;
#[test]
fn executed_baseline22_migration_boundary_refuses_23_before_decoding() {
    assert_eq!(baseline_v22_migrations::MAX_SUPPORTED_VERSION, 22);
    let (_, manifest) = fixture::startup(RunId::new());
    let payload = EventPayload::ContinuationSeedBound {
        manifest: Box::new(manifest),
    };
    let valid = serde_json::to_vec(&crate::run_event::VersionedEventPayload::new(payload)).unwrap();
    for bytes in [valid.as_slice(), b"not json".as_slice()] {
        assert!(matches!(
            baseline_v22_migrations::migrate_payload(23, bytes),
            Err(crate::SurgeError::SchemaTooNew { found: 23, max: 22 })
        ));
    }
    let historical = br#"{"schema_version":22,"payload":{"type":"run_woke_from_park"}}"#;
    assert_eq!(
        baseline_v22_migrations::migrate_payload(22, historical).unwrap(),
        EventPayload::RunWokeFromPark {}
    );
}

#[test]
fn actual_single_event_marker_after_retained_semantic_history_is_late_before_mutation() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let outcome = fixture::event(
        run,
        4,
        EventPayload::OutcomeReported {
            node: crate::NodeKey::try_from("verify").unwrap(),
            outcome: crate::OutcomeKey::try_from("passed").unwrap(),
            summary: "Recorded accepted semantic occurrence before any StageEntered".into(),
        },
    );
    let marker = fixture::event(
        run,
        5,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest),
        },
    );
    let mut memory = RunMemory::default();
    memory.apply_events(&startup).unwrap();
    memory.apply_event(&outcome).unwrap();
    let before = memory.clone();
    assert_eq!(
        memory.apply_event(&marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
    assert_eq!(memory, before);
    let state = crate::run_state::fold(&startup).unwrap();
    let state = crate::run_state::apply(state, &outcome).unwrap();
    assert_eq!(
        crate::run_state::apply(state, &marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
}

#[test]
fn actual_single_event_valid_marker_is_uniformly_unavailable_after_actual_startup() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let marker = fixture::event(
        run,
        4,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest),
        },
    );
    let mut memory = RunMemory::default();
    memory.apply_events(&startup).unwrap();
    let before = memory.clone();
    assert_eq!(
        memory.apply_event(&marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Unavailable
        ))
    );
    assert_eq!(memory, before);
    let state = crate::run_state::fold(&startup).unwrap();
    assert_eq!(
        crate::run_state::apply(state, &marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Unavailable
        ))
    );
}

#[test]
fn retained_prefix_noops_and_partitions_preserve_exact_late_classification() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let payloads = [
        EventPayload::RequirementRevised {
            node: crate::NodeKey::try_from("verify").unwrap(),
            task: None,
            text: "Retain requirement history".into(),
        },
        EventPayload::HookExecuted {
            hook_id: "observed-hook".into(),
            exit_status: 0,
            on_failure: crate::hooks::HookFailureMode::Reject,
        },
        EventPayload::OutcomeRejectedByHook {
            node: crate::NodeKey::try_from("verify").unwrap(),
            outcome: crate::OutcomeKey::try_from("passed").unwrap(),
            hook_id: "observed-hook".into(),
        },
    ];
    for payload in payloads {
        let mut prefix = startup.clone();
        prefix.push(fixture::event(run, 4, payload));
        let marker = fixture::event(
            run,
            5,
            EventPayload::ContinuationSeedBound {
                manifest: Box::new(manifest.clone()),
            },
        );
        for split in 0..=prefix.len() {
            let mut memory = RunMemory::default();
            memory.apply_events(&prefix[..split]).unwrap();
            memory.apply_events(&prefix[split..]).unwrap();
            let before = memory.clone();
            assert_eq!(
                memory.apply_event(&marker),
                Err(crate::run_state::FoldError::SeedHistory(
                    SeedHistoryError::Late
                )),
                "partition {split}"
            );
            assert_eq!(memory, before);
        }
        let state = crate::run_state::fold(&prefix).unwrap();
        assert_eq!(
            crate::run_state::apply(state, &marker),
            Err(crate::run_state::FoldError::SeedHistory(
                SeedHistoryError::Late
            ))
        );
    }
}

#[test]
fn retained_noop_before_start_and_after_terminal_never_restarts_guard() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let noop = fixture::event(
        run,
        1,
        EventPayload::HookExecuted {
            hook_id: "audit".into(),
            exit_status: 0,
            on_failure: crate::hooks::HookFailureMode::Reject,
        },
    );
    let mut state = crate::run_state::fold(std::slice::from_ref(&noop)).unwrap();
    for event in startup
        .iter()
        .enumerate()
        .map(|(index, event)| fixture::event(run, index as u64 + 2, event.payload.clone()))
    {
        state = crate::run_state::apply(state, &event).unwrap();
    }
    let marker = fixture::event(
        run,
        5,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest.clone()),
        },
    );
    assert_eq!(
        crate::run_state::apply(state, &marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
    let mut terminal = crate::run_state::fold(&startup).unwrap();
    terminal = crate::run_state::apply(
        terminal,
        &fixture::event(
            run,
            4,
            EventPayload::RunFailed {
                error: "Original terminal truth".into(),
            },
        ),
    )
    .unwrap();
    terminal = crate::run_state::apply(terminal, &fixture::event(run, 5, noop.payload)).unwrap();
    assert!(
        matches!(&terminal,crate::RunState::Terminal { kind:crate::run_state::TerminalReason::Failed, reason,.. } if reason == "Original terminal truth")
    );
    assert_eq!(
        crate::run_state::apply(
            terminal,
            &fixture::event(
                run,
                6,
                EventPayload::ContinuationSeedBound {
                    manifest: Box::new(manifest)
                }
            )
        ),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
}

#[test]
fn first_unhydrated_marker_error_is_identical_before_later_duplicate_across_partitions() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let marker = fixture::event(
        run,
        4,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest),
        },
    );
    let duplicate = fixture::event(run, 5, marker.payload.clone());
    let mut all = startup.clone();
    all.extend([marker.clone(), duplicate.clone()]);
    assert_eq!(
        preflight_unhydrated(&all),
        Err(SeedHistoryError::Unavailable)
    );
    assert_eq!(
        crate::run_state::fold(&all),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Unavailable
        ))
    );
    for split in 0..=startup.len() {
        let mut memory = RunMemory::default();
        memory.apply_events(&startup[..split]).unwrap();
        let before = memory.clone();
        let mut remainder = startup[split..].to_vec();
        remainder.extend([marker.clone(), duplicate.clone()]);
        assert_eq!(
            memory.apply_events(&remainder),
            Err(crate::run_state::FoldError::SeedHistory(
                SeedHistoryError::Unavailable
            ))
        );
        assert_eq!(
            memory, before,
            "failed prefix including guard must be atomic"
        );
        memory.apply_events(&startup[split..]).unwrap();
        let before = memory.clone();
        assert_eq!(
            memory.apply_event(&marker),
            Err(crate::run_state::FoldError::SeedHistory(
                SeedHistoryError::Unavailable
            ))
        );
        assert_eq!(memory, before);
    }
}

#[test]
fn failed_late_marker_batch_retains_prior_guard_and_all_semantic_observers() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let requirement = fixture::event(
        run,
        4,
        EventPayload::RequirementRevised {
            node: crate::NodeKey::try_from("verify").unwrap(),
            task: Some("task".into()),
            text: "New local requirement".into(),
        },
    );
    let marker = fixture::event(
        run,
        5,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest.clone()),
        },
    );
    let mut memory = RunMemory::default();
    memory.apply_events(&startup).unwrap();
    let before = memory.clone();
    assert_eq!(
        memory.apply_events(&[requirement, marker]),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
    assert_eq!(memory, before);
    assert_eq!(
        memory.apply_event(&fixture::event(
            run,
            4,
            EventPayload::ContinuationSeedBound {
                manifest: Box::new(manifest)
            }
        )),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Unavailable
        )),
        "rejected candidate must not leak executable observation"
    );
    assert_eq!(memory, before);
}

#[test]
fn bootstrap_approval_keeps_raw_startup_guard_through_pipeline_and_terminal() {
    let run = RunId::new();
    let (startup, manifest) = fixture::startup(run);
    let approval = fixture::event(
        run,
        2,
        EventPayload::BootstrapApprovalDecided {
            stage: crate::run_event::BootstrapStage::Description,
            decision: crate::run_event::BootstrapDecision::Approve,
            comment: None,
        },
    );
    let mut prefix = vec![startup[0].clone(), approval];
    prefix.push(fixture::event(run, 3, startup[1].payload.clone()));
    prefix.push(fixture::event(run, 4, startup[2].payload.clone()));
    let state = crate::run_state::fold(&prefix).unwrap();
    let marker = fixture::event(
        run,
        5,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest),
        },
    );
    assert_eq!(
        crate::run_state::apply(state, &marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
    let mut memory = RunMemory::default();
    memory.apply_events(&prefix).unwrap();
    assert_eq!(
        memory.apply_event(&marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Late
        ))
    );
}

#[test]
fn retained_guard_uses_raw_registered_hash_when_execution_graph_has_added_gates() {
    let run = RunId::new();
    let (mut startup, manifest) = fixture::startup(run);
    let mut raw = fixture::graph();
    raw.edges[0].policy.max_traversals = Some(2);
    let raw_hash = raw.identity().unwrap();
    let effective = crate::escalation::with_default_escalation_gates(&raw);
    assert_ne!(
        raw_hash,
        effective.identity().unwrap(),
        "control must actually normalize the graph"
    );
    let mut fields = manifest.fields().clone();
    fields.graph_hash = raw_hash;
    let manifest = crate::work_item::ContinuationSeedManifest::try_from(fields).unwrap();
    startup[1] = fixture::event(
        run,
        2,
        EventPayload::PipelineMaterialized {
            graph_hash: raw_hash,
            graph: Box::new(raw),
        },
    );
    let marker = fixture::event(
        run,
        4,
        EventPayload::ContinuationSeedBound {
            manifest: Box::new(manifest),
        },
    );
    let mut memory = RunMemory::default();
    memory.apply_events(&startup).unwrap();
    assert_eq!(
        memory
            .verification_graph
            .as_ref()
            .unwrap()
            .identity()
            .unwrap(),
        effective.identity().unwrap()
    );
    let before = memory.clone();
    assert_eq!(
        memory.apply_event(&marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Unavailable
        ))
    );
    assert_eq!(memory, before);
    let state = crate::run_state::fold(&startup).unwrap();
    assert_eq!(
        crate::run_state::apply(state, &marker),
        Err(crate::run_state::FoldError::SeedHistory(
            SeedHistoryError::Unavailable
        ))
    );
}
