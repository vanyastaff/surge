//! Store-boundary evidence uses an actual validated durable event journal.
//! Seeding a Reserved record here is not a production cleanup/domain producer.
use super::*;
use crate::runs::{RunWriter, Storage};
use surge_core::{
    NodeKey, VersionedEventPayload as V,
    execution_recovery::{OpenedSession, ProviderSessionId, SessionRestoreCapabilities},
    stage_tool::StageToolContext,
};

struct Fixture {
    _home: tempfile::TempDir,
    store: WorkItemStore,
    claim: WorkItemLaunchClaim,
    writer: RunWriter,
    body: HandoffBody,
    cycle: RecoveryCycle,
    reservation: CandidateReservation,
    original: OpenedSession,
    opening_seq: u64,
}

fn descriptor(invocation: StageInvocationId, cwd: std::path::PathBuf) -> ProviderSessionDescriptor {
    ProviderSessionDescriptor::new(
        ProviderSessionId::new("same-provider-id".into()).unwrap(),
        invocation,
        "a".into(),
        ContentHash::compute(b"launch-a"),
        cwd,
        SessionRestoreCapabilities {
            resume: true,
            load: true,
        },
    )
    .unwrap()
}

async fn fixture() -> Fixture {
    fixture_with_admission(false).await
}

async fn fixture_with_admission(admit: bool) -> Fixture {
    fixture_with_options(admit, false).await
}

async fn fixture_with_options(admit: bool, pinned: bool) -> Fixture {
    fixture_with_loop(admit, pinned, false).await
}

async fn fixture_with_loop(admit: bool, pinned: bool, loop_graph: bool) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let workspace = WorkItemWorkspace {
        repository: home.path().join("repo/.git"),
        checkout: home.path().join("repo"),
        path: home.path().join("retained"),
        ownership: "owned".into(),
        branch: "retained".into(),
        base_commit: "a".repeat(40),
    };
    let create = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: workspace.checkout.clone(),
        title: "Opening epochs".into(),
        requirements: WorkItemRequirements::new("Original".into(), vec!["Retain identity".into()])
            .unwrap(),
    };
    let WorkItemResult::Detail(detail) = store
        .mutate(&create, Some(&workspace), None, "host", 1)
        .unwrap()
    else {
        panic!("detail")
    };
    let mut graph: surge_core::Graph = toml::from_str(include_str!(
        "../../../../../../../examples/flow_minimal_agent.toml"
    ))
    .unwrap();
    let node: NodeKey = "impl_1".try_into().unwrap();
    let other: NodeKey = "other".try_into().unwrap();
    let mut other_node = graph.nodes[&node].clone();
    other_node.id = other.clone();
    graph.nodes.insert(other, other_node);
    if loop_graph {
        graph.edges[0].to = node.clone();
        graph.edges[0].kind = surge_core::edge::EdgeKind::Backtrack;
        graph.edges[0].policy.max_traversals = Some(2);
    }
    let mut candidate = FrozenQuotaCandidate::new(
        RecoveryCandidate::new("a".into(), AccountEvidence::Unknown).unwrap(),
        pinned.then(|| "sonnet".into()),
        ContentHash::compute(b"launch-a"),
    )
    .unwrap();
    let mut candidate_b = FrozenQuotaCandidate::new(
        RecoveryCandidate::new("b".into(), AccountEvidence::Unknown).unwrap(),
        pinned.then(|| "sonnet".into()),
        ContentHash::compute(b"launch-b"),
    )
    .unwrap();
    if pinned {
        for (runtime, target) in [("a", &mut candidate), ("b", &mut candidate_b)] {
            *target = target.clone().with_configured_snapshot(
                surge_core::config::CapacityRoute {
                    provider_family: "fixture".into(),
                    configured_route: runtime.into(),
                    auth_sources: Vec::new(),
                    completeness:
                        surge_core::config::ConfiguredSourceCompleteness::CompleteConfiguredSources,
                },
                Some(ContentHash::compute(runtime.as_bytes())),
            );
        }
    }
    let stage = FrozenQuotaStage::new(
        node.clone(),
        QuotaRoutingMode::Configured,
        vec![candidate.clone(), candidate_b],
        1000,
        1000,
    )
    .unwrap();
    let policy = FrozenQuotaPolicy::new(vec![stage]).unwrap();
    let config = serde_json::to_string(&serde_json::json!({"quota_recovery":policy})).unwrap();
    let start = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item: detail.item.id,
        expected_version: detail.item.version,
        graph: Box::new(graph.clone()),
        quota_recovery: Some(serde_json::to_value(policy).unwrap()),
    };
    let WorkItemResult::Attempt(attempt) = store
        .mutate(&start, None, Some(&config), "host", 2)
        .unwrap()
    else {
        panic!("attempt")
    };
    let claim = store.claim(attempt.run).unwrap();
    let writer = storage
        .create_run(attempt.run, &workspace.path, None)
        .await
        .unwrap();
    let invocation = StageInvocationId::new();
    let mut original = OpenedSession::new(
        SessionId::new(),
        descriptor(invocation, workspace.path),
        SessionOpenMode::New,
    )
    .unwrap();
    if admit {
        let writer_id = surge_core::id::ExecutionWriterId::new();
        original.execution_writer = Some(
            surge_core::execution_recovery::process::ExecutionWriterObservation::new(
                writer_id, None,
            )
            .unwrap(),
        );
        store
            .admit_recipe_opening(
                writer_id,
                invocation,
                "a",
                original.descriptor.launch_hash(),
            )
            .unwrap();
        if let Some(pin) = candidate.configured_pin() {
            store
                .attach_admitted_configured_pin(writer_id, pin)
                .unwrap();
        }
    }
    let serialized = toml::to_string(&graph).unwrap();
    let events = vec![
        V::new(EventPayload::RunStarted {
            pipeline_template: None,
            project_path: workspace.checkout,
            initial_prompt: "fixture".into(),
            config: surge_core::run_event::RunConfig {
                bootstrap_edit_loop_cap: None,
                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: vec![],
                budget: Default::default(),
            },
        }),
        V::new(EventPayload::PipelineMaterialized {
            graph: Box::new(graph),
            graph_hash: ContentHash::compute(serialized.as_bytes()),
        }),
        V::new(EventPayload::StageEntered {
            node: node.clone(),
            attempt: 1,
        }),
        V::new(EventPayload::SessionEstablishmentRequested {
            node: node.clone(),
            invocation,
            restore: false,
            authority: Some(StageToolContext {
                run: claim.run,
                node: node.clone(),
                session: SessionId::new(),
                generation: surge_core::id::StageGenerationId::new(),
            }),
        }),
        V::new(EventPayload::SessionOpened {
            node,
            session: original.session,
            agent: "a".into(),
            agent_id: None,
            opened: Some(original.clone()),
            handoff: None,
        }),
    ];
    let sequences = writer.append_events(events).await.unwrap();
    store
        .bind_quota_stage(&claim, invocation, sequences[4].0)
        .unwrap();
    let cycle = store
        .begin_recovery_cycle(&claim, &invocation.to_string(), 0)
        .unwrap();
    let reservation = store
        .reserve_recovery_candidate(&claim, &cycle, candidate.candidate())
        .unwrap();
    let cycle = store
        .recovery_cycle(claim.run, &invocation.to_string(), 1)
        .unwrap();
    let body = HandoffBody {
        operation: WorkItemOperationId::new(),
        run: claim.run,
        item: claim.binding.item,
        attempt_generation: claim.binding.generation,
        logical_invocation: invocation.to_string(),
        source_cycle: 1,
        source_revision: cycle.revision - 1,
        source_control: 0,
        wake_identity: None,
        target_cycle: 1,
        target_revision: cycle.revision,
        target_control: 0,
        reservation: reservation.receipt.clone(),
        launch: QuotaLaunchContract::new(
            candidate,
            invocation,
            SessionOpenMode::Resume,
            Some(original.descriptor.clone()),
        )
        .unwrap(),
    };
    seed_reserved(&store, &body);
    Fixture {
        _home: home,
        store,
        claim,
        writer,
        body,
        cycle,
        reservation,
        original,
        opening_seq: sequences[4].0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_reservation_creates_one_opening_epoch_and_replay_cannot_spend_twice() {
    let f = fixture().await;
    let conn = f.store.pool.get().unwrap();
    conn.execute(
        "DELETE FROM work_item_quota_handoffs WHERE operation=?",
        [f.body.operation.to_string()],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM work_item_operations WHERE operation=?",
        [f.body.operation.to_string()],
    )
    .unwrap();
    drop(conn);

    let created = f
        .store
        .reserve_quota_open(&f.claim, &f.cycle, &f.reservation, f.body.launch.clone())
        .unwrap();
    assert_eq!(created.state(), QuotaHandoffState::Reserved);
    assert_ne!(created.operation(), f.body.operation);
    assert_eq!(created.launch(), &f.body.launch);

    let replay = f
        .store
        .reserve_quota_open(&f.claim, &f.cycle, &f.reservation, f.body.launch.clone())
        .unwrap();
    assert_eq!(replay.operation(), created.operation());
    assert_eq!(replay.state(), QuotaHandoffState::Reserved);

    let permit = f
        .store
        .admit_provider_open(&f.claim, created.operation())
        .unwrap();
    assert_eq!(permit.operation(), created.operation());
    assert!(
        f.store
            .admit_provider_open(&f.claim, created.operation())
            .is_err()
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_frozen_candidate_requires_typed_origin_and_is_reserved_once_in_policy_order() {
    let f = fixture().await;
    let source = QuotaRateLimitSource::new(
        f.opening_seq,
        f.original.descriptor.invocation(),
        f.original.session,
        Some(1000),
        "provider typed 429".into(),
    )
    .unwrap();
    let exhausted = QuotaObservation::new(QuotaEvidence::Observed {
        observed_at_ms: 100,
        expires_at_ms: 1100,
        available: false,
        reset_at_ms: Some(1100),
    })
    .unwrap();
    let current = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    let current = f
        .store
        .record_selected_rate_limit(
            &f.claim,
            &current,
            &f.reservation.receipt,
            &source,
            &exhausted,
        )
        .unwrap();
    assert!(
        f.store
            .typed_rate_limit(&f.reservation.receipt)
            .unwrap()
            .is_some()
    );

    let policy = f
        .store
        .bind_quota_stage(&f.claim, f.original.descriptor.invocation(), f.opening_seq)
        .unwrap();
    let next = f
        .store
        .reserve_next_candidate_after_exhaustion(&f.claim, &current, &policy)
        .unwrap()
        .unwrap();
    assert_eq!(next.candidate.runtime(), "b");
    assert_eq!(next.disposition, ReservationDisposition::Reserved);
    let selected = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    assert!(
        f.store
            .reserve_next_candidate_after_exhaustion(&f.claim, &selected, &policy)
            .is_err(),
        "an unconfirmed B opening must block another candidate grant"
    );
    f.writer.close().await.unwrap();
}

fn seed_reserved(store: &WorkItemStore, body: &HandoffBody) {
    body.validate().unwrap();
    let text = serde_json::to_string(body).unwrap();
    let conn = store.pool.get().unwrap();
    conn.execute(
        "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
        params![
            body.operation.to_string(),
            body.operation_hash().unwrap().to_string(),
            serde_json::to_string(&OperationResult::Control {
                run: body.run,
                generation: body.target_control
            })
            .unwrap()
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO work_item_quota_handoffs(operation,run,item,attempt_generation,logical_invocation,source_cycle,source_control,wake_identity,target_cycle,target_control,reservation,provider_invocation,launch_hash,state,body_hash,body) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'reserved',?,?)",
        params![body.operation.to_string(),body.run.to_string(),body.item.to_string(),body.attempt_generation,body.logical_invocation,body.source_cycle,body.source_control,body.wake_identity,body.target_cycle,body.target_control,body.reservation,body.launch.provider_invocation.to_string(),body.launch.candidate.launch_hash().to_string(),ContentHash::compute(text.as_bytes()).to_string(),text]).unwrap();
}

async fn append_opening(
    f: &Fixture,
    epoch: WorkItemOperationId,
    session: SessionId,
    node: &str,
) -> u64 {
    let node: NodeKey = node.try_into().unwrap();
    if node.as_str() != "impl_1" {
        f.writer
            .append_event(V::new(EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            }))
            .await
            .unwrap();
    }
    f.writer
        .append_event(V::new(EventPayload::SessionEstablishmentRequested {
            node: node.clone(),
            invocation: f.body.launch.provider_invocation,
            restore: true,
            authority: Some(StageToolContext {
                run: f.claim.run,
                node: node.clone(),
                session: SessionId::new(),
                generation: surge_core::id::StageGenerationId::new(),
            }),
        }))
        .await
        .unwrap();
    let opened = OpenedSession::new(
        session,
        f.original.descriptor.clone(),
        SessionOpenMode::Resume,
    )
    .unwrap();
    f.writer
        .append_event(V::new(EventPayload::SessionOpened {
            node,
            session,
            agent: "a".into(),
            agent_id: None,
            opened: Some(opened),
            handoff: Some(epoch),
        }))
        .await
        .unwrap()
        .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_store_admission_is_spent_once_without_an_rpc() {
    let f = fixture().await;
    let permit = f
        .store
        .admit_provider_open(&f.claim, f.body.operation)
        .unwrap();
    // This exercises admission only; no provider RPC is called by this fixture.
    let (epoch, run, generation, launch) = permit.into_opening();
    assert_eq!((epoch, run, generation), (f.body.operation, f.claim.run, 0));
    assert_eq!(launch.mode(), SessionOpenMode::Resume);
    assert_eq!(
        f.store.quota_handoff(epoch).unwrap().state(),
        QuotaHandoffState::OpeningUnknown
    );
    assert!(f.store.admit_provider_open(&f.claim, epoch).is_err());
    assert_eq!(
        f.store.quota_handoff(epoch).unwrap().state(),
        QuotaHandoffState::OpeningUnknown
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_resume_epoch_preserves_provider_and_confirms_fresh_internal_session_idempotently() {
    let f = fixture().await;
    let _permit = f
        .store
        .admit_provider_open(&f.claim, f.body.operation)
        .unwrap();
    let session = SessionId::new();
    let sequence = append_opening(&f, f.body.operation, session, "impl_1").await;
    let confirmed = f
        .store
        .confirm_provider_open(&f.claim, f.body.operation, sequence)
        .unwrap();
    assert_eq!(confirmed.state(), QuotaHandoffState::Established);
    assert_eq!(confirmed.internal_session(), Some(session));
    assert_ne!(session, f.original.session);
    assert_eq!(
        confirmed.launch().saved().unwrap().provider_session_id(),
        f.original.descriptor.provider_session_id()
    );
    assert_eq!(
        f.store
            .confirm_provider_open(&f.claim, f.body.operation, sequence)
            .unwrap(),
        confirmed
    );
    assert!(
        f.store
            .admit_provider_open(&f.claim, f.body.operation)
            .is_err()
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_epoch_reused_internal_session_wrong_stage_and_stale_control_never_establish() {
    for scenario in ["epoch", "session", "stage", "control", "cycle"] {
        let f = fixture().await;
        let _permit = f
            .store
            .admit_provider_open(&f.claim, f.body.operation)
            .unwrap();
        let epoch = if scenario == "epoch" {
            WorkItemOperationId::new()
        } else {
            f.body.operation
        };
        let session = if scenario == "session" {
            f.original.session
        } else {
            SessionId::new()
        };
        let node = if scenario == "stage" {
            "other"
        } else {
            "impl_1"
        };
        let sequence = append_opening(&f, epoch, session, node).await;
        if scenario == "control" {
            // An independent newer host control wins; leave admission state intact
            // to isolate confirmation's generation fence from invalidation glue.
            let operation = WorkItemOperationId::new();
            let conn = f.store.pool.get().unwrap();
            conn.execute(
                "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
                params![
                    operation.to_string(),
                    ContentHash::compute(b"new-control").to_string(),
                    "{}"
                ],
            )
            .unwrap();
            let control = surge_core::execution_recovery::WorkItemExecutionControl {
                item: f.claim.binding.item,
                run: f.claim.run,
                attempt_generation: f.claim.binding.generation,
                generation: 1,
                operation,
                state: ExecutionControlState::Executing,
                fence: None,
                allow_new_session: false,
                diagnostic: None,
            };
            conn.execute("INSERT INTO work_item_execution_controls(run,generation,item,attempt_generation,operation,state,payload) VALUES(?,?,?,?,?,'executing',?)",params![f.claim.run.to_string(),1,f.claim.binding.item.to_string(),f.claim.binding.generation,operation.to_string(),serde_json::to_string(&control).unwrap()]).unwrap();
        }
        if scenario == "cycle" {
            // A newer historical generation cannot let an older still-open row
            // regain authority, even with identical runtime/control metadata.
            let conn = f.store.pool.get().unwrap();
            conn.execute(
                "INSERT INTO work_item_quota_cycles(run,invocation,cycle_generation,item,attempt_generation,control_generation,revision,closed,selected_runtime) VALUES(?,?,2,?,?,0,1,1,'a')",
                params![f.claim.run.to_string(),f.body.logical_invocation,f.claim.binding.item.to_string(),f.claim.binding.generation],
            ).unwrap();
        }
        let error = f
            .store
            .confirm_provider_open(&f.claim, f.body.operation, sequence)
            .expect_err(scenario);
        let expected = match scenario {
            "epoch" => "another epoch",
            "session" => "provider connection metadata mismatch",
            "stage" => "stale stage",
            "control" => "control was superseded",
            "cycle" => "cycle is not current",
            _ => unreachable!(),
        };
        assert!(error.to_string().contains(expected), "{scenario}: {error}");
        let stored = f.store.quota_handoff(f.body.operation).unwrap();
        assert_eq!(
            stored.state(),
            QuotaHandoffState::OpeningUnknown,
            "{scenario}"
        );
        assert_eq!(stored.internal_session(), None, "{scenario}");
        assert!(
            f.store
                .admit_provider_open(&f.claim, f.body.operation)
                .is_err(),
            "{scenario}"
        );
        f.writer.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_journal_cannot_spend_reserved_opening_admission() {
    let f = fixture().await;
    f.writer
        .append_event(V::new(EventPayload::RunCompleted {
            terminal_node: "end".try_into().unwrap(),
        }))
        .await
        .unwrap();
    assert!(
        f.store
            .admit_provider_open(&f.claim, f.body.operation)
            .is_err()
    );
    assert_eq!(
        f.store.quota_handoff(f.body.operation).unwrap().state(),
        QuotaHandoffState::Reserved
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_store_restore_epochs_retain_provider_invocation_and_distinct_internal_sessions() {
    let f = fixture().await;
    let _first = f
        .store
        .admit_provider_open(&f.claim, f.body.operation)
        .unwrap();
    let first_session = SessionId::new();
    let first_seq = append_opening(&f, f.body.operation, first_session, "impl_1").await;
    f.store
        .confirm_provider_open(&f.claim, f.body.operation, first_seq)
        .unwrap();
    // This remains a storage protocol fixture, not automatic daemon wake/RPC
    // acceptance: the foundation's owned rollover creates a new bounded cycle.
    let cycle = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    let cycle = f
        .store
        .record_recovery_observation(
            &f.claim,
            &cycle,
            &f.body.reservation,
            &QuotaObservation::unknown(),
        )
        .unwrap();
    let wake = RecoveryWake::new(
        "second-store-epoch".into(),
        WakeOrigin::PolicyBackoff,
        100,
        1100,
    )
    .unwrap();
    let cycle = f
        .store
        .arm_recovery_wake(&f.claim, &cycle, &f.body.reservation, &wake)
        .unwrap();
    let cycle = f
        .store
        .consume_recovery_wake(&f.claim, &cycle, "second-store-epoch", 1100)
        .unwrap();
    let reserved = f
        .store
        .reserve_recovery_candidate(&f.claim, &cycle, f.body.launch.candidate.candidate())
        .unwrap();
    let mut second = f.body.clone();
    second.operation = WorkItemOperationId::new();
    second.source_cycle = cycle.generation;
    second.source_revision = cycle.revision;
    second.target_cycle = cycle.generation;
    second.target_revision = cycle.revision + 1;
    second.reservation = reserved.receipt;
    seed_reserved(&f.store, &second);
    let _second = f
        .store
        .admit_provider_open(&f.claim, second.operation)
        .unwrap();
    let second_session = SessionId::new();
    let second_seq = append_opening(&f, second.operation, second_session, "impl_1").await;
    let confirmed = f
        .store
        .confirm_provider_open(&f.claim, second.operation, second_seq)
        .unwrap();
    assert_ne!(second.operation, f.body.operation);
    assert_ne!(second_session, first_session);
    assert_eq!(
        confirmed.launch().provider_invocation(),
        f.body.launch.provider_invocation
    );
    assert_eq!(
        confirmed.launch().saved().unwrap().provider_session_id(),
        f.original.descriptor.provider_session_id()
    );
    assert_eq!(confirmed.internal_session(), Some(second_session));
    assert!(
        f.store
            .admit_provider_open(&f.claim, second.operation)
            .is_err()
    );
    assert!(
        f.store
            .confirm_provider_open(&f.claim, second.operation, first_seq)
            .is_err()
    );
    f.writer.close().await.unwrap();
}

fn quota_source(f: &Fixture) -> QuotaRateLimitSource {
    QuotaRateLimitSource::new(
        5,
        f.original.descriptor.invocation(),
        f.original.session,
        Some(1000),
        "store-boundary classified quota fixture".into(),
    )
    .unwrap()
}
fn quota_error_observation() -> QuotaObservation {
    QuotaObservation::new(QuotaEvidence::Observed {
        observed_at_ms: 100,
        expires_at_ms: 1100,
        available: false,
        reset_at_ms: Some(1100),
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_false_probe_has_no_typed_origin_and_cannot_authorize_cleanup_stop() {
    let f = fixture().await;
    let current = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    let current = f
        .store
        .record_recovery_observation(
            &f.claim,
            &current,
            &f.body.reservation,
            &quota_error_observation(),
        )
        .unwrap();
    assert_eq!(f.store.typed_rate_limit(&f.body.reservation).unwrap(), None);
    assert!(
        f.store
            .request_quota_cleanup_stop(&f.claim, &current, &f.body.reservation)
            .is_err()
    );
    assert_eq!(f.store.execution_control(f.claim.run).unwrap(), None);
    assert_eq!(
        f.store
            .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
            .unwrap(),
        current
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alternate_origin_replay_preserves_newer_available_without_open_authority() {
    let f = fixture().await;
    let _permit = f
        .store
        .admit_provider_open(&f.claim, f.body.operation)
        .unwrap();
    let session = SessionId::new();
    let sequence = append_opening(&f, f.body.operation, session, "impl_1").await;
    f.store
        .confirm_provider_open(&f.claim, f.body.operation, sequence)
        .unwrap();
    // Store-contract fixture only: seed a complete execution row so the
    // replay oracle can focus on quota-observation ordering; no provider RPC
    // or prompt-authority claim is made here.
    f.store
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE work_item_quota_handoffs SET state='executing',prompt_authorization_seq=? WHERE operation=?",
            params![sequence, f.body.operation.to_string()],
        )
        .unwrap();
    let source = QuotaRateLimitSource::new(
        sequence,
        f.body.launch.provider_invocation,
        session,
        Some(1000),
        "429".into(),
    )
    .unwrap();
    let current = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    let current = f
        .store
        .record_selected_rate_limit(
            &f.claim,
            &current,
            &f.body.reservation,
            &source,
            &quota_error_observation(),
        )
        .unwrap();
    let marker = f.store.typed_rate_limit(&f.body.reservation).unwrap();
    let available = QuotaObservation::new(QuotaEvidence::Observed {
        observed_at_ms: 200,
        expires_at_ms: 1200,
        available: true,
        reset_at_ms: None,
    })
    .unwrap();
    let current = f
        .store
        .record_recovery_observation(&f.claim, &current, &f.body.reservation, &available)
        .unwrap();
    let reopened = Storage::open(f._home.path()).await.unwrap().work_items();
    assert_eq!(
        reopened
            .record_selected_rate_limit(
                &f.claim,
                &current,
                &f.body.reservation,
                &source,
                &quota_error_observation(),
            )
            .unwrap(),
        current
    );
    assert_eq!(
        reopened.typed_rate_limit(&f.body.reservation).unwrap(),
        marker
    );
    assert_eq!(
        reopened.recovery_observation(&f.body.reservation).unwrap(),
        (Some(available), None)
    );
    assert!(
        reopened
            .admit_provider_open(&f.claim, f.body.operation)
            .is_err()
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_origin_reopens_and_replays_without_replacing_later_available_probe() {
    let f = fixture().await;
    let current = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    let current = f
        .store
        .record_selected_rate_limit(
            &f.claim,
            &current,
            &f.body.reservation,
            &quota_source(&f),
            &quota_error_observation(),
        )
        .unwrap();
    let marker = f
        .store
        .typed_rate_limit(&f.body.reservation)
        .unwrap()
        .unwrap();
    let reopened = Storage::open(f._home.path()).await.unwrap().work_items();
    assert_eq!(
        reopened.typed_rate_limit(&f.body.reservation).unwrap(),
        Some(marker.clone())
    );
    assert_eq!(
        reopened
            .record_selected_rate_limit(
                &f.claim,
                &current,
                &f.body.reservation,
                &quota_source(&f),
                &quota_error_observation()
            )
            .unwrap(),
        current
    );
    let available = QuotaObservation::new(QuotaEvidence::Observed {
        observed_at_ms: 200,
        expires_at_ms: 1200,
        available: true,
        reset_at_ms: None,
    })
    .unwrap();
    let current = reopened
        .record_recovery_observation(&f.claim, &current, &f.body.reservation, &available)
        .unwrap();
    assert_eq!(
        reopened
            .record_selected_rate_limit(
                &f.claim,
                &current,
                &f.body.reservation,
                &quota_source(&f),
                &quota_error_observation()
            )
            .unwrap(),
        current
    );
    assert_eq!(
        reopened.recovery_observation(&f.body.reservation).unwrap(),
        (Some(available), None)
    );
    assert_eq!(
        reopened.typed_rate_limit(&f.body.reservation).unwrap(),
        Some(marker)
    );
    assert!(
        reopened
            .request_quota_cleanup_stop(&f.claim, &current, &f.body.reservation)
            .is_err()
    );
    assert_eq!(reopened.execution_control(f.claim.run).unwrap(), None);
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_source_and_later_closed_cycle_cannot_create_a_typed_origin() {
    for scenario in ["session_closed", "newer_closed_cycle"] {
        let f = fixture().await;
        let current = f
            .store
            .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
            .unwrap();
        if scenario == "session_closed" {
            f.writer
                .append_event(V::new(EventPayload::SessionClosed {
                    session: f.original.session,
                    disposition: surge_core::run_event::SessionDisposition::Normal,
                }))
                .await
                .unwrap();
        } else {
            f.store.pool.get().unwrap().execute(
                "INSERT INTO work_item_quota_cycles(run,invocation,cycle_generation,item,attempt_generation,control_generation,revision,closed,selected_runtime) VALUES(?,?,2,?,?,0,1,1,'a')",
                params![f.claim.run.to_string(),f.body.logical_invocation,f.claim.binding.item.to_string(),f.claim.binding.generation],
            ).unwrap();
        }
        let error = f
            .store
            .record_selected_rate_limit(
                &f.claim,
                &current,
                &f.body.reservation,
                &quota_source(&f),
                &quota_error_observation(),
            )
            .unwrap_err();
        let expected = if scenario == "session_closed" {
            "source session was stopped"
        } else {
            "cycle is not current"
        };
        assert!(error.to_string().contains(expected), "{scenario}: {error}");
        assert_eq!(f.store.typed_rate_limit(&f.body.reservation).unwrap(), None);
        assert_eq!(
            f.store
                .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
                .unwrap(),
            current
        );
        f.writer.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_origin_survives_unknown_probe_and_closed_source_for_nonwake_cleanup_stop() {
    let f = fixture().await;
    let current = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    let current = f
        .store
        .record_selected_rate_limit(
            &f.claim,
            &current,
            &f.body.reservation,
            &quota_source(&f),
            &quota_error_observation(),
        )
        .unwrap();
    let marker = f
        .store
        .typed_rate_limit(&f.body.reservation)
        .unwrap()
        .unwrap();
    let current = f
        .store
        .record_recovery_observation(
            &f.claim,
            &current,
            &f.body.reservation,
            &QuotaObservation::unknown(),
        )
        .unwrap();
    f.writer
        .append_event(V::new(EventPayload::SessionClosed {
            session: f.original.session,
            disposition: surge_core::run_event::SessionDisposition::Normal,
        }))
        .await
        .unwrap();
    let stop = f
        .store
        .request_quota_cleanup_stop(&f.claim, &current, &f.body.reservation)
        .unwrap();
    assert_eq!(
        f.store.capacity_stop_kind(&stop).unwrap(),
        CapacityStopKind::CleanupUnknown
    );
    assert!(f.store.due_recovery_wakes(9999, 10).unwrap().is_empty());
    assert_eq!(
        f.store.typed_rate_limit(&f.body.reservation).unwrap(),
        Some(marker)
    );
    assert_eq!(
        f.store
            .recovery_reservations(f.claim.run, &f.body.logical_invocation, 1)
            .unwrap()
            .len(),
        1
    );
    let control = f.store.execution_control(f.claim.run).unwrap().unwrap();
    assert_eq!(control.state, ExecutionControlState::SuspendRequested);
    assert!(control.fence.is_none());
    assert_eq!(
        f.store
            .request_quota_cleanup_stop(&f.claim, &current, &f.body.reservation)
            .unwrap(),
        stop
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_wake_requires_current_confirmed_capacity_control_without_partial_writes() {
    let f = fixture().await;
    let mut cycle = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    cycle = f
        .store
        .record_recovery_observation(
            &f.claim,
            &cycle,
            &f.reservation.receipt,
            &QuotaObservation::unknown(),
        )
        .unwrap();
    let wake = RecoveryWake::new(
        "capacity-wake-without-control".into(),
        WakeOrigin::PolicyBackoff,
        100,
        1100,
    )
    .unwrap();
    cycle = f
        .store
        .arm_recovery_wake(&f.claim, &cycle, &f.reservation.receipt, &wake)
        .unwrap();
    let launch = QuotaLaunchContract::new(
        f.body.launch.candidate().clone(),
        StageInvocationId::new(),
        SessionOpenMode::New,
        None,
    )
    .unwrap();

    let result = f
        .store
        .reserve_automatic_wake(&f.claim, &cycle, wake.identity(), 1100, launch);

    assert!(
        matches!(result, Err(WorkItemError::Conflict(_))),
        "a due timer alone must not reserve task continuation or provider dispatch: {result:?}"
    );
    assert_eq!(
        f.store
            .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
            .unwrap(),
        cycle,
        "rejected wake must preserve its exact schedule and cycle revision"
    );
    assert!(
        f.store
            .recovery_cycle(f.claim.run, &f.body.logical_invocation, 2)
            .is_err(),
        "rejected wake must not allocate the next cycle"
    );
    let controls = f
        .store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM work_item_execution_controls WHERE run=?",
            [f.claim.run.to_string()],
            |row| row.get::<_, u64>(0),
        )
        .unwrap();
    assert_eq!(controls, 0, "rejection must not reserve Continue");
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_wake_atomically_reserves_continue_candidate_and_handoff() {
    automatic_wake_authorization_order(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_wake_subscriber_ack_before_prompt_keeps_exact_authority() {
    automatic_wake_authorization_order(true).await;
}

async fn automatic_wake_authorization_order(acknowledge: bool) {
    use surge_core::execution_recovery::{
        ExecutionControlState, PendingStagePhase, SuspensionFence, SuspensionReason,
    };

    let f = fixture().await;
    let invocation: StageInvocationId = f.body.logical_invocation.parse().unwrap();
    let mut cycle = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    cycle = f
        .store
        .record_recovery_observation(
            &f.claim,
            &cycle,
            &f.reservation.receipt,
            &QuotaObservation::unknown(),
        )
        .unwrap();

    // The alternate was also observed unavailable in this cycle. Seed its
    // already validated frozen identity so the stop producer can prove all
    // configured candidates were considered without changing the selected A.
    let candidate_b = RecoveryCandidate::new("b".into(), AccountEvidence::Unknown).unwrap();
    let conn = f.store.pool.get().unwrap();
    conn.execute(
        "INSERT INTO work_item_quota_candidates(run,invocation,cycle_generation,candidate_key,runtime,receipt,body,observation) VALUES(?,?,?,?,?,?,?,?)",
        params![
            f.claim.run.to_string(),
            f.body.logical_invocation,
            cycle.generation,
            candidate_b.key().unwrap(),
            candidate_b.runtime(),
            RunId::new().to_string(),
            serde_json::to_string(&candidate_b).unwrap(),
            serde_json::to_string(&QuotaObservation::unknown()).unwrap(),
        ],
    )
    .unwrap();
    drop(conn);

    let wake = RecoveryWake::new(
        "capacity-wake-positive".into(),
        WakeOrigin::PolicyBackoff,
        100,
        1100,
    )
    .unwrap();
    cycle = f
        .store
        .arm_recovery_wake(&f.claim, &cycle, &f.reservation.receipt, &wake)
        .unwrap();
    let capacity = f
        .store
        .request_capacity_suspend(&f.claim, &cycle, &f.reservation.receipt, 101)
        .unwrap();
    assert_eq!(capacity.target_control(), 1);

    let node: NodeKey = "impl_1".try_into().unwrap();
    let pending = PendingStagePhase::Interrupted { node, invocation };
    let prefix = f.writer.current_seq().await.unwrap().as_u64();
    let fence = SuspensionFence {
        control_generation: capacity.target_control(),
        snapshot_seq: prefix,
        pending_stage: pending.clone(),
        cleanup_confirmed: true,
        reason: SuspensionReason::Capacity {
            wake_at_ms: Some(wake.due_at_ms()),
        },
    };
    let blob = serde_json::to_vec(&serde_json::json!({
        "at_seq": prefix,
        "pending_stage": pending,
    }))
    .unwrap();
    f.writer.seal_suspension(fence.clone(), blob).await.unwrap();
    let suspended = f.store.confirm_suspension(&f.claim, &fence).unwrap();
    assert_eq!(suspended.state, ExecutionControlState::Suspended);
    assert_eq!(
        f.store
            .due_recovery_wakes_for_run(f.claim.run, 1100, 10)
            .unwrap(),
        vec![cycle.clone()],
        "cold scheduling must discover the confirmed Capacity fence before cycle rebinding"
    );
    let rebound = f
        .store
        .rebind_recovery_control(&f.claim, &cycle, capacity.target_control())
        .unwrap();

    let launch = QuotaLaunchContract::new(
        f.body.launch.candidate().clone(),
        StageInvocationId::new(),
        SessionOpenMode::New,
        None,
    )
    .unwrap();
    let first = f
        .store
        .reserve_automatic_wake(&f.claim, &rebound, wake.identity(), 1100, launch.clone())
        .unwrap();
    assert_eq!(first.cycle().generation, 2);
    assert_eq!(first.cycle().control_generation, 2);
    assert_eq!(first.cycle().selected_runtime.as_deref(), Some("a"));
    assert_eq!(
        first.reservation().disposition,
        ReservationDisposition::Reserved
    );
    assert_eq!(first.handoff().state(), QuotaHandoffState::Reserved);
    assert_eq!(first.handoff().control_generation(), 2);
    assert_eq!(
        f.store
            .execution_control(f.claim.run)
            .unwrap()
            .unwrap()
            .state,
        ExecutionControlState::ContinueReserved
    );

    let replay = f
        .store
        .reserve_automatic_wake(&f.claim, &rebound, wake.identity(), 1100, launch.clone())
        .unwrap();
    assert_eq!(replay.handoff().operation(), first.handoff().operation());
    assert_eq!(replay.reservation().receipt, first.reservation().receipt);
    assert_eq!(
        replay.reservation().disposition,
        ReservationDisposition::Replayed
    );
    assert_eq!(
        f.store
            .recovery_reservations(f.claim.run, &f.body.logical_invocation, 2)
            .unwrap()
            .len(),
        1,
        "replay must not allocate another candidate"
    );
    let permit = f
        .store
        .admit_provider_open(&f.claim, first.handoff().operation())
        .unwrap();
    assert_eq!(permit.control_generation(), 2);
    assert_eq!(permit.launch(), &launch);
    let session = SessionId::new();
    let opened = OpenedSession::new(
        session,
        descriptor(
            launch.provider_invocation(),
            f.store
                .show(f.claim.binding.item)
                .unwrap()
                .item
                .workspace
                .path,
        ),
        SessionOpenMode::New,
    )
    .unwrap();
    assert_eq!(
        opened.descriptor.cwd(),
        f.store
            .show(f.claim.binding.item)
            .unwrap()
            .item
            .workspace
            .path
    );
    assert_eq!(
        opened.descriptor.runtime(),
        launch.candidate().candidate().runtime()
    );
    assert_eq!(
        opened.descriptor.launch_hash(),
        launch.candidate().launch_hash()
    );
    f.writer
        .append_event(V::new(EventPayload::SessionEstablishmentRequested {
            node: "impl_1".try_into().unwrap(),
            invocation: launch.provider_invocation(),
            restore: false,
            authority: Some(StageToolContext {
                run: f.claim.run,
                node: "impl_1".try_into().unwrap(),
                session: SessionId::new(),
                generation: surge_core::id::StageGenerationId::new(),
            }),
        }))
        .await
        .unwrap();
    let opened_seq = f
        .writer
        .append_event(V::new(EventPayload::SessionOpened {
            node: "impl_1".try_into().unwrap(),
            session,
            agent: "a".into(),
            agent_id: None,
            opened: Some(opened),
            handoff: Some(first.handoff().operation()),
        }))
        .await
        .unwrap()
        .0;
    f.store
        .confirm_provider_open(&f.claim, first.handoff().operation(), opened_seq)
        .unwrap();
    let continued_seq = f
        .writer
        .append_event(V::new(EventPayload::RunContinued {
            control_generation: 2,
        }))
        .await
        .unwrap()
        .as_u64();
    // Deterministic scheduler ordering: the journal subscriber acknowledges
    // Continue before the engine seals its prompt authorization.
    if acknowledge {
        let acknowledged = f.store.confirm_continued(&f.claim, 2).unwrap();
        assert_eq!(acknowledged.state, ExecutionControlState::Executing);
    }
    let exact_control = f.store.execution_control(f.claim.run).unwrap().unwrap();
    let conn = f.store.pool.get().unwrap();
    for wrong_generation in [false, true] {
        let mut changed = exact_control.clone();
        if wrong_generation {
            changed.generation += 1;
        } else {
            changed.operation = WorkItemOperationId::new();
        }
        conn.execute(
            "UPDATE work_item_execution_controls SET payload=? WHERE run=? AND generation=2",
            params![
                serde_json::to_string(&changed).unwrap(),
                f.claim.run.to_string()
            ],
        )
        .unwrap();
        assert!(
            f.store
                .authorize_automatic_wake_prompt(
                    &f.claim,
                    first.handoff().operation(),
                    continued_seq
                )
                .is_err(),
            "changed operation or generation must never authorize prompt"
        );
    }
    conn.execute(
        "UPDATE work_item_execution_controls SET payload=? WHERE run=? AND generation=2",
        params![
            serde_json::to_string(&exact_control).unwrap(),
            f.claim.run.to_string()
        ],
    )
    .unwrap();
    drop(conn);
    let executing = f
        .store
        .authorize_automatic_wake_prompt(&f.claim, first.handoff().operation(), continued_seq)
        .unwrap();
    assert_eq!(executing.state(), QuotaHandoffState::Executing);
    assert_eq!(executing.prompt_authorization_seq(), Some(continued_seq));
    assert_eq!(
        f.store
            .execution_control(f.claim.run)
            .unwrap()
            .unwrap()
            .state,
        ExecutionControlState::Executing
    );
    let replay = f
        .store
        .authorize_automatic_wake_prompt(&f.claim, first.handoff().operation(), continued_seq)
        .unwrap();
    assert_eq!(replay, executing, "exact continuation replay is idempotent");
    assert!(
        f.store
            .admit_provider_open(&f.claim, first.handoff().operation())
            .is_err(),
        "a consumed automatic wake cannot issue a second provider-open permit"
    );
    f.writer.close().await.unwrap();
}

async fn seal_fixture_exhaustion(f: &Fixture) -> RecoveryCycle {
    let current = f
        .store
        .recovery_cycle(f.claim.run, &f.body.logical_invocation, 1)
        .unwrap();
    f.store
        .record_selected_rate_limit(
            &f.claim,
            &current,
            &f.body.reservation,
            &quota_source(f),
            &quota_error_observation(),
        )
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_typed_exhaustion_is_reservation_and_recipe_scoped_after_reopen() {
    let f = fixture().await;
    seal_fixture_exhaustion(&f).await;
    let reopened = Storage::open(f._home.path()).await.unwrap().work_items();
    let expected = &f.body.launch.candidate;
    let evidence = reopened
        .inspect_fresh_typed_exhaustion(&f.body.reservation, expected, 100)
        .unwrap()
        .unwrap();
    assert_eq!(evidence.receipt(), f.body.reservation);
    assert_eq!(evidence.candidate(), expected);
    assert!(evidence.source_revision() > 0);
    assert_eq!(evidence.observed_at_ms(), 100);
    assert_eq!(evidence.valid_until_ms(), 1100);
    assert!(
        reopened
            .inspect_fresh_typed_exhaustion(&f.body.reservation, expected, 100)
            .unwrap()
            .is_some()
    );
    for now in [99, 1100, 1101] {
        assert!(
            reopened
                .inspect_fresh_typed_exhaustion(&f.body.reservation, expected, now)
                .unwrap()
                .is_none()
        );
    }
    let alternatives = [
        FrozenQuotaCandidate::new(
            expected.candidate().clone(),
            None,
            ContentHash::compute(b"different-recipe"),
        )
        .unwrap(),
        FrozenQuotaCandidate::new(
            expected.candidate().clone(),
            Some("different-model".into()),
            *expected.launch_hash(),
        )
        .unwrap(),
        FrozenQuotaCandidate::new(
            RecoveryCandidate::new(
                "a".into(),
                AccountEvidence::Known {
                    provider: "provider".into(),
                    account_key: "different-account".into(),
                },
            )
            .unwrap(),
            None,
            *expected.launch_hash(),
        )
        .unwrap(),
        FrozenQuotaCandidate::new(
            RecoveryCandidate::new("b".into(), AccountEvidence::Unknown).unwrap(),
            None,
            *expected.launch_hash(),
        )
        .unwrap(),
    ];
    for candidate in alternatives {
        assert!(
            reopened
                .inspect_fresh_typed_exhaustion(&f.body.reservation, &candidate, 100)
                .unwrap()
                .is_none()
        );
    }
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_typed_exhaustion_requires_original_and_latest_false_windows() {
    for (available, observed, expiry, reset, now, fresh) in [
        (false, 200, 300, None, 299, true),
        (false, 200, 300, None, 300, false),
        (false, 200, 1200, Some(500), 499, true),
        (false, 200, 1200, Some(500), 500, false),
        (false, 200, 1200, None, 199, false),
        (false, 200, 1200, None, 1100, false),
        (true, 200, 300, None, 250, false),
        (true, 200, 300, None, 400, false),
    ] {
        let f = fixture().await;
        let current = seal_fixture_exhaustion(&f).await;
        let observation = QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: observed,
            expires_at_ms: expiry,
            available,
            reset_at_ms: reset,
        })
        .unwrap();
        f.store
            .record_recovery_observation(&f.claim, &current, &f.body.reservation, &observation)
            .unwrap();
        assert_eq!(
            f.store
                .inspect_fresh_typed_exhaustion(&f.body.reservation, &f.body.launch.candidate, now)
                .unwrap()
                .is_some(),
            fresh
        );
        f.writer.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_typed_exhaustion_unknown_probes_never_extend_typed_ttl_or_create_origin() {
    for probe in [QuotaObservation::unknown(), QuotaObservation::unsupported()] {
        let f = fixture().await;
        let current = seal_fixture_exhaustion(&f).await;
        let marker = f.store.typed_rate_limit(&f.body.reservation).unwrap();
        let current = f
            .store
            .record_recovery_observation(
                &f.claim,
                &current,
                &f.body.reservation,
                &QuotaObservation::new(QuotaEvidence::Observed {
                    observed_at_ms: 200,
                    expires_at_ms: 2200,
                    available: false,
                    reset_at_ms: None,
                })
                .unwrap(),
            )
            .unwrap();
        f.store
            .record_recovery_observation(&f.claim, &current, &f.body.reservation, &probe)
            .unwrap();
        assert!(
            f.store
                .inspect_fresh_typed_exhaustion(&f.body.reservation, &f.body.launch.candidate, 1099)
                .unwrap()
                .is_some()
        );
        assert!(
            f.store
                .inspect_fresh_typed_exhaustion(&f.body.reservation, &f.body.launch.candidate, 1100)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.store.typed_rate_limit(&f.body.reservation).unwrap(),
            marker
        );
        f.writer.close().await.unwrap();
    }
    let f = fixture().await;
    f.store
        .record_recovery_observation(
            &f.claim,
            &f.cycle,
            &f.body.reservation,
            &quota_error_observation(),
        )
        .unwrap();
    assert!(
        f.store
            .inspect_fresh_typed_exhaustion(&f.body.reservation, &f.body.launch.candidate, 100)
            .unwrap()
            .is_none()
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_typed_exhaustion_original_ttl_and_reset_each_bound_latest_probe() {
    for retry in [None, Some(400)] {
        let f = fixture().await;
        let source = QuotaRateLimitSource::new(
            f.opening_seq,
            f.original.descriptor.invocation(),
            f.original.session,
            retry,
            "429".into(),
        )
        .unwrap();
        let original = QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: 100,
            expires_at_ms: 1100,
            available: false,
            reset_at_ms: retry.map(|delay| 100 + i64::try_from(delay).unwrap()),
        })
        .unwrap();
        let current = f
            .store
            .record_selected_rate_limit(&f.claim, &f.cycle, &f.body.reservation, &source, &original)
            .unwrap();
        let later = QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: 200,
            expires_at_ms: 2200,
            available: false,
            reset_at_ms: None,
        })
        .unwrap();
        f.store
            .record_recovery_observation(&f.claim, &current, &f.body.reservation, &later)
            .unwrap();
        let until = if retry.is_some() { 500 } else { 1100 };
        assert!(
            f.store
                .inspect_fresh_typed_exhaustion(
                    &f.body.reservation,
                    &f.body.launch.candidate,
                    until - 1
                )
                .unwrap()
                .is_some()
        );
        assert!(
            f.store
                .inspect_fresh_typed_exhaustion(
                    &f.body.reservation,
                    &f.body.launch.candidate,
                    until
                )
                .unwrap()
                .is_none()
        );
        f.writer.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_typed_exhaustion_rejects_corrupted_original_or_latest_bounds() {
    for corruption in [
        "removed_reset",
        "extended_reset",
        "extended_expiry",
        "older_observed",
        "missing_exhaustion",
    ] {
        let f = fixture().await;
        let source = QuotaRateLimitSource::new(
            f.opening_seq,
            f.original.descriptor.invocation(),
            f.original.session,
            Some(400),
            "429".into(),
        )
        .unwrap();
        let original = QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: 100,
            expires_at_ms: 1100,
            available: false,
            reset_at_ms: Some(500),
        })
        .unwrap();
        let current = f
            .store
            .record_selected_rate_limit(&f.claim, &f.cycle, &f.body.reservation, &source, &original)
            .unwrap();
        let later = QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: 200,
            expires_at_ms: 2200,
            available: false,
            reset_at_ms: None,
        })
        .unwrap();
        f.store
            .record_recovery_observation(&f.claim, &current, &f.body.reservation, &later)
            .unwrap();
        let conn = f.store.pool.get().unwrap();
        if corruption == "older_observed" {
            let older = serde_json::to_string(
                &QuotaObservation::new(QuotaEvidence::Observed {
                    observed_at_ms: 50,
                    expires_at_ms: 1200,
                    available: false,
                    reset_at_ms: None,
                })
                .unwrap(),
            )
            .unwrap();
            conn.execute(
                "UPDATE work_item_quota_candidates SET observed=?,exhaustion=? WHERE receipt=?",
                params![older, older, f.body.reservation],
            )
            .unwrap();
        } else if corruption == "missing_exhaustion" {
            conn.execute(
                "UPDATE work_item_quota_candidates SET exhaustion=NULL WHERE receipt=?",
                [&f.body.reservation],
            )
            .unwrap();
        } else {
            let body: String = conn
                .query_row(
                    "SELECT typed_exhaustion FROM work_item_quota_candidates WHERE receipt=?",
                    [&f.body.reservation],
                    |row| row.get(0),
                )
                .unwrap();
            let mut marker: serde_json::Value = serde_json::from_str(&body).unwrap();
            let observed = &mut marker["observation"]["evidence"]["Observed"];
            match corruption {
                "removed_reset" => observed["reset_at_ms"] = serde_json::Value::Null,
                "extended_reset" => observed["reset_at_ms"] = serde_json::json!(900),
                "extended_expiry" => observed["expires_at_ms"] = serde_json::json!(2100),
                _ => unreachable!(),
            }
            conn.execute(
                "UPDATE work_item_quota_candidates SET typed_exhaustion=? WHERE receipt=?",
                params![serde_json::to_string(&marker).unwrap(), f.body.reservation],
            )
            .unwrap();
        }
        drop(conn);
        assert!(
            f.store
                .inspect_fresh_typed_exhaustion(&f.body.reservation, &f.body.launch.candidate, 700)
                .is_err(),
            "{corruption}"
        );
        f.writer.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newer_opening_without_quota_supersedes_exact_exhaustion() {
    let f = fixture_with_admission(true).await;
    seal_fixture_exhaustion(&f).await;
    let project = f.store.show(f.claim.binding.item).unwrap().item.project;
    let expected = &f.body.launch.candidate;
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, expected, 100)
            .unwrap()
            .is_some()
    );
    f.store
        .admit_recipe_opening(
            surge_core::id::ExecutionWriterId::new(),
            StageInvocationId::new(),
            "a",
            expected.launch_hash(),
        )
        .unwrap();
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, expected, 100)
            .unwrap()
            .is_none(),
        "newer attempted opening is Unknown, even without quota policy or successful RPC"
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_exhaustion_without_admission_is_not_actionable() {
    let f = fixture().await;
    seal_fixture_exhaustion(&f).await;
    let project = f.store.show(f.claim.binding.item).unwrap().item.project;
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, &f.body.launch.candidate, 100)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .typed_rate_limit(&f.body.reservation)
            .unwrap()
            .is_some()
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_old_origin_is_audit_only_and_unrelated_recipes_do_not_supersede() {
    let f = fixture_with_admission(true).await;
    let expected = &f.body.launch.candidate;
    let project = f.store.show(f.claim.binding.item).unwrap().item.project;
    for (runtime, hash) in [
        ("b", *expected.launch_hash()),
        ("a", ContentHash::compute(b"different")),
    ] {
        f.store
            .admit_recipe_opening(
                surge_core::id::ExecutionWriterId::new(),
                StageInvocationId::new(),
                runtime,
                &hash,
            )
            .unwrap();
    }
    seal_fixture_exhaustion(&f).await;
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, expected, 100)
            .unwrap()
            .is_some()
    );
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&WorkItemProjectId::new(), expected, 100)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, expected, 1100)
            .unwrap()
            .is_none()
    );
    let other_model = FrozenQuotaCandidate::new(
        expected.candidate().clone(),
        Some("different-model".into()),
        *expected.launch_hash(),
    )
    .unwrap();
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, &other_model, 100)
            .unwrap()
            .is_none()
    );
    f.writer.close().await.unwrap();

    let f = fixture_with_admission(true).await;
    let expected = &f.body.launch.candidate;
    let project = f.store.show(f.claim.binding.item).unwrap().item.project;
    f.store
        .admit_recipe_opening(
            surge_core::id::ExecutionWriterId::new(),
            StageInvocationId::new(),
            "a",
            expected.launch_hash(),
        )
        .unwrap();
    seal_fixture_exhaustion(&f).await;
    assert!(
        f.store
            .typed_rate_limit(&f.body.reservation)
            .unwrap()
            .is_some(),
        "retain original typed cause for audit"
    );
    assert!(
        f.store
            .inspect_current_recipe_exhaustion(&project, expected, 100)
            .unwrap()
            .is_none(),
        "late A error cannot resurrect exhaustion past a newer opening"
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_opening_cannot_attach_a_conflicting_exhaustion_origin() {
    let f = fixture_with_admission(true).await;
    let conn = f.store.pool.get().unwrap();
    conn.execute(
        "UPDATE recipe_opening_admissions SET exhaustion_receipt='different-origin'",
        [],
    )
    .unwrap();
    drop(conn);
    let error = f
        .store
        .record_selected_rate_limit(
            &f.claim,
            &f.cycle,
            &f.body.reservation,
            &quota_source(&f),
            &quota_error_observation(),
        )
        .unwrap_err();
    assert!(matches!(error, WorkItemError::Conflict(_)), "{error:?}");
    assert!(
        f.store
            .typed_rate_limit(&f.body.reservation)
            .unwrap()
            .is_none(),
        "conflicting association rolls back new origin atomically"
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn planned_binding_retains_real_occurrence_and_first_open_is_one_shot() {
    let f = fixture().await;
    f.writer
        .append_event(V::new(EventPayload::SessionClosed {
            session: f.original.session,
            disposition: surge_core::run_event::SessionDisposition::Normal,
        }))
        .await
        .unwrap();
    let entry = f
        .writer
        .append_event(V::new(EventPayload::StageEntered {
            node: "impl_1".try_into().unwrap(),
            attempt: 2,
        }))
        .await
        .unwrap();
    let invocation = StageInvocationId::new();
    let policy: FrozenQuotaPolicy = serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(
            &attempt(&f.store.pool.get().unwrap(), f.claim.run)
                .unwrap()
                .config,
        )
        .unwrap()["quota_recovery"]
            .clone(),
    )
    .unwrap();
    let plan = f
        .writer
        .append_event(V::new(EventPayload::QuotaStagePlanned {
            node: "impl_1".try_into().unwrap(),
            attempt: 2,
            stage_entry_seq: entry.0,
            logical_invocation: invocation,
            control_generation: 0,
            policy_hash: policy.content_hash().unwrap(),
        }))
        .await
        .unwrap();
    f.store
        .bind_planned_quota_stage(&f.claim, invocation, plan.0)
        .unwrap();
    let cycle = f
        .store
        .begin_recovery_cycle(&f.claim, &invocation.to_string(), 0)
        .unwrap();
    let super::super::super::CapacitySelection::Selected {
        cycle,
        reservation,
        skipped,
    } = f
        .store
        .select_planned_capacity(&f.claim, &cycle, 10)
        .unwrap()
    else {
        panic!("opaque history must attempt real provider")
    };
    assert!(skipped.is_empty());
    assert_eq!(reservation.candidate.runtime(), "a");
    let provider = StageInvocationId::new();
    let launch = QuotaLaunchContract::new(
        policy.stages()[0].candidates()[0].clone(),
        provider,
        SessionOpenMode::New,
        None,
    )
    .unwrap();
    let handoff = f
        .store
        .reserve_quota_open(&f.claim, &cycle, &reservation, launch)
        .unwrap();
    assert_ne!(provider, invocation);
    let permit = f
        .store
        .admit_provider_open(&f.claim, handoff.operation())
        .unwrap();
    assert_eq!(permit.launch().provider_invocation(), provider);
    assert!(
        f.store
            .admit_provider_open(&f.claim, handoff.operation())
            .is_err()
    );
    let current = f
        .store
        .recovery_cycle(f.claim.run, &invocation.to_string(), cycle.generation)
        .unwrap();
    let replay = f
        .store
        .select_planned_capacity(&f.claim, &current, 10)
        .unwrap();
    assert!(matches!(
        replay,
        super::super::super::CapacitySelection::Selected {
            reservation: CandidateReservation {
                disposition: ReservationDisposition::Replayed,
                ..
            },
            ..
        }
    ));
    let connection = f.store.pool.get().unwrap();
    let (original, planned): (Option<u64>, Option<u64>) = connection
        .query_row(
            "SELECT opening_seq,plan_seq FROM work_item_quota_stages WHERE run=? AND invocation=?",
            params![f.claim.run.to_string(), invocation.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(original, None);
    assert_eq!(planned, Some(plan.0));
    f.store
        .invalidate_unexecuted_quota_open(&f.claim, permit)
        .unwrap();
    assert_eq!(
        f.store.quota_handoff(handoff.operation()).unwrap().state(),
        QuotaHandoffState::Invalidated
    );
    assert!(
        f.store
            .admit_provider_open(&f.claim, handoff.operation())
            .is_err(),
        "invalidated opening cannot be retried"
    );
    f.writer
        .append_event(V::new(EventPayload::StageEntered {
            node: "impl_1".try_into().unwrap(),
            attempt: 3,
        }))
        .await
        .unwrap();
    assert!(
        f.store
            .bind_planned_quota_stage(&f.claim, invocation, plan.0)
            .is_err(),
        "old same-node stage occurrence cannot become new authority"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unexecuted_cancellation_never_erases_historical_provider_uncertainty() {
    let f = fixture().await;
    let permit = f
        .store
        .admit_provider_open(&f.claim, f.body.operation)
        .unwrap();
    assert!(
        f.store
            .invalidate_unexecuted_quota_open(&f.claim, permit)
            .is_err()
    );
    assert_eq!(
        f.store.quota_handoff(f.body.operation).unwrap().state(),
        QuotaHandoffState::OpeningUnknown
    );
    assert!(
        f.store
            .admit_provider_open(&f.claim, f.body.operation)
            .is_err()
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_skip_and_actual_exhaustion_park_with_the_real_provider_origin() {
    let (f, current, reservation, _session) = fixture_with_two_fresh_pinned_sources().await;
    let super::super::super::CapacitySelection::AllExhausted { cycle, skipped } = f
        .store
        .select_planned_capacity(&f.claim, &current, 100)
        .unwrap()
    else {
        panic!("fresh A skip plus typed B cannot reopen A")
    };
    assert_eq!(skipped.len(), 1);
    let wake = RecoveryWake::new(
        RunId::new().to_string(),
        WakeOrigin::PolicyBackoff,
        100,
        1100,
    )
    .unwrap();
    let scheduled = f
        .store
        .arm_recovery_wake(&f.claim, &cycle, &reservation.receipt, &wake)
        .unwrap();
    assert!(
        f.store
            .request_capacity_suspend(&f.claim, &scheduled, &reservation.receipt, 100)
            .is_err(),
        "strict reactive API must not accept unattempted A"
    );
    let association = f
        .store
        .request_capacity_suspend_with_skips(&f.claim, &scheduled, &reservation.receipt, 100)
        .unwrap();
    assert!(
        matches!(association.evidence_origin().unwrap(),CapacityEvidenceOrigin::ProviderReservation(receipt) if receipt==reservation.receipt)
    );
    f.writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_or_advanced_plans_never_create_a_binding() {
    for case in [
        "node",
        "attempt",
        "entry",
        "policy",
        "control",
        "invocation",
        "request-before",
        "request-after",
        "terminal",
    ] {
        let f = fixture().await;
        f.writer
            .append_event(V::new(EventPayload::SessionClosed {
                session: f.original.session,
                disposition: surge_core::run_event::SessionDisposition::Normal,
            }))
            .await
            .unwrap();
        let entry = f
            .writer
            .append_event(V::new(EventPayload::StageEntered {
                node: "impl_1".try_into().unwrap(),
                attempt: 2,
            }))
            .await
            .unwrap();
        let policy: FrozenQuotaPolicy = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(
                &attempt(&f.store.pool.get().unwrap(), f.claim.run)
                    .unwrap()
                    .config,
            )
            .unwrap()["quota_recovery"]
                .clone(),
        )
        .unwrap();
        let invocation = StageInvocationId::new();
        if case == "request-before" {
            f.writer
                .append_event(V::new(EventPayload::SessionEstablishmentRequested {
                    node: "impl_1".try_into().unwrap(),
                    invocation,
                    restore: false,
                    authority: None,
                }))
                .await
                .unwrap();
        }
        let plan = f
            .writer
            .append_event(V::new(EventPayload::QuotaStagePlanned {
                node: if case == "node" { "other" } else { "impl_1" }
                    .try_into()
                    .unwrap(),
                attempt: if case == "attempt" { 3 } else { 2 },
                stage_entry_seq: if case == "entry" {
                    entry.0 - 1
                } else {
                    entry.0
                },
                logical_invocation: invocation,
                control_generation: u64::from(case == "control"),
                policy_hash: if case == "policy" {
                    ContentHash::compute(b"different frozen policy")
                } else {
                    policy.content_hash().unwrap()
                },
            }))
            .await
            .unwrap();
        if case == "request-after" {
            f.writer
                .append_event(V::new(EventPayload::SessionEstablishmentRequested {
                    node: "impl_1".try_into().unwrap(),
                    invocation,
                    restore: false,
                    authority: None,
                }))
                .await
                .unwrap();
        }
        if case == "terminal" {
            f.writer
                .append_event(V::new(EventPayload::RunCompleted {
                    terminal_node: "end".try_into().unwrap(),
                }))
                .await
                .unwrap();
        }
        let bound = if case == "invocation" {
            StageInvocationId::new()
        } else {
            invocation
        };
        let count = || {
            f.store
                .pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM work_item_quota_stages WHERE run=?",
                    [f.claim.run.to_string()],
                    |r| r.get::<_, u64>(0),
                )
                .unwrap()
        };
        let before = count();
        assert!(
            f.store
                .bind_planned_quota_stage(&f.claim, bound, plan.0)
                .is_err(),
            "{case} must not create provider effect authority"
        );
        assert_eq!(
            count(),
            before,
            "rejected {case} plan must make no registry writes"
        );
        f.writer.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_same_node_reentry_retains_a_newer_unadmitted_plan() {
    use super::super::super::PlannedResumeInspection;
    let f = fixture_with_loop(true, true, true).await;
    let authority = f
        .writer
        .read_events(crate::runs::EventSeq(4)..crate::runs::EventSeq(5))
        .await
        .unwrap();
    let EventPayload::SessionEstablishmentRequested {
        authority: Some(authority),
        ..
    } = &authority[0].payload.payload
    else {
        panic!("actual authenticated establishment")
    };
    let node: NodeKey = "impl_1".try_into().unwrap();
    let outcome: surge_core::OutcomeKey = "done".try_into().unwrap();
    let effects = vec![V::new(EventPayload::OutcomeReported {
        node: node.clone(),
        outcome: outcome.clone(),
        summary: "host-authenticated loop outcome".into(),
    })];
    let commit = surge_core::execution_recovery::commit::StageOutcomeCommit::new(
        authority.clone(),
        f.original.session,
        f.original.descriptor.invocation(),
        outcome.clone(),
        1,
        ContentHash::compute(&serde_json::to_vec(&effects).unwrap()),
    )
    .unwrap();
    let mut batch = effects;
    batch.push(V::new(EventPayload::StageOutcomeCommitted { commit }));
    let committed = f
        .writer
        .append_events(batch)
        .await
        .unwrap()
        .last()
        .unwrap()
        .0;
    f.writer
        .append_events(vec![
            V::new(EventPayload::EdgeTraversed {
                edge: "e_impl_to_end".try_into().unwrap(),
                from: node.clone(),
                to: node.clone(),
                kind: surge_core::edge::EdgeKind::Backtrack,
            }),
            V::new(EventPayload::StageCompleted {
                node: node.clone(),
                outcome,
            }),
            V::new(EventPayload::StageRouteCommitted {
                invocation: f.original.descriptor.invocation(),
                outcome_commit_seq: committed,
            }),
        ])
        .await
        .unwrap();
    let policy: FrozenQuotaPolicy = serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(
            &attempt(&f.store.pool.get().unwrap(), f.claim.run)
                .unwrap()
                .config,
        )
        .unwrap()["quota_recovery"]
            .clone(),
    )
    .unwrap();
    let cursor = surge_core::run_state::Cursor {
        node: node.clone(),
        attempt: 1,
    };
    assert_eq!(
        f.store
            .inspect_planned_stage_resume(&f.claim, &cursor, &policy.content_hash().unwrap(), 0)
            .unwrap(),
        PlannedResumeInspection::CompletedOccurrence,
        "authenticated completed same-node routing may enter the next occurrence"
    );
    let entry = f
        .writer
        .append_event(V::new(EventPayload::StageEntered {
            node: node.clone(),
            attempt: 2,
        }))
        .await
        .unwrap();
    let logical = StageInvocationId::new();
    let plan = f
        .writer
        .append_event(V::new(EventPayload::QuotaStagePlanned {
            node: node.clone(),
            attempt: 2,
            stage_entry_seq: entry.0,
            logical_invocation: logical,
            control_generation: 0,
            policy_hash: policy.content_hash().unwrap(),
        }))
        .await
        .unwrap();
    let cursor = surge_core::run_state::Cursor {
        node: node.clone(),
        attempt: 2,
    };
    let count = || {
        f.store
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM work_item_quota_stages WHERE run=?",
                [f.claim.run.to_string()],
                |row| row.get::<_, u64>(0),
            )
            .unwrap()
    };
    let before = count();
    assert_eq!(
        f.store
            .inspect_planned_stage_resume(&f.claim, &cursor, &policy.content_hash().unwrap(), 0)
            .unwrap(),
        PlannedResumeInspection::UnadmittedOccurrence {
            stage_entry_seq: entry.0
        },
        "older authenticated completion cannot manufacture another entry or logical plan"
    );
    assert_eq!(count(), before, "inspection cannot bind or admit an effect");
    f.store
        .bind_planned_quota_stage(&f.claim, logical, plan.0)
        .unwrap();
    f.writer
        .append_event(V::new(EventPayload::StageEntered {
            node: node.clone(),
            attempt: 3,
        }))
        .await
        .unwrap();
    let cursor = surge_core::run_state::Cursor { node, attempt: 3 };
    assert!(
        f.store
            .inspect_planned_stage_resume(&f.claim, &cursor, &policy.content_hash().unwrap(), 0)
            .is_err(),
        "unfinished bound occurrence cannot be superseded even without RPC"
    );
    f.store
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM work_item_quota_stages WHERE run=?",
            [f.claim.run.to_string()],
        )
        .unwrap();
    assert!(
        f.store
            .inspect_planned_stage_resume(&f.claim, &cursor, &policy.content_hash().unwrap(), 0)
            .is_err(),
        "missing original registry binding cannot grant a new occurrence"
    );
    f.writer.close().await.unwrap();
}

async fn fixture_with_two_fresh_pinned_sources()
-> (Fixture, RecoveryCycle, CandidateReservation, SessionId) {
    let f = fixture_with_options(true, true).await;
    seal_fixture_exhaustion(&f).await;
    f.writer
        .append_event(V::new(EventPayload::SessionClosed {
            session: f.original.session,
            disposition: surge_core::run_event::SessionDisposition::Normal,
        }))
        .await
        .unwrap();
    let entry = f
        .writer
        .append_event(V::new(EventPayload::StageEntered {
            node: "impl_1".try_into().unwrap(),
            attempt: 2,
        }))
        .await
        .unwrap();
    let policy: FrozenQuotaPolicy = serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(
            &attempt(&f.store.pool.get().unwrap(), f.claim.run)
                .unwrap()
                .config,
        )
        .unwrap()["quota_recovery"]
            .clone(),
    )
    .unwrap();
    let logical = StageInvocationId::new();
    let plan = f
        .writer
        .append_event(V::new(EventPayload::QuotaStagePlanned {
            node: "impl_1".try_into().unwrap(),
            attempt: 2,
            stage_entry_seq: entry.0,
            logical_invocation: logical,
            control_generation: 0,
            policy_hash: policy.content_hash().unwrap(),
        }))
        .await
        .unwrap();
    f.store
        .bind_planned_quota_stage(&f.claim, logical, plan.0)
        .unwrap();
    let cycle = f
        .store
        .begin_recovery_cycle(&f.claim, &logical.to_string(), 0)
        .unwrap();
    let super::super::super::CapacitySelection::Selected {
        cycle,
        reservation,
        skipped,
    } = f
        .store
        .select_planned_capacity(&f.claim, &cycle, 100)
        .unwrap()
    else {
        panic!("B unknown must really attempt")
    };
    assert_eq!(skipped.len(), 1);
    assert_eq!(reservation.candidate.runtime(), "b");
    let provider = StageInvocationId::new();
    let target = policy.stages()[0].candidates()[1].clone();
    let launch =
        QuotaLaunchContract::new(target.clone(), provider, SessionOpenMode::New, None).unwrap();
    let handoff = f
        .store
        .reserve_quota_open(&f.claim, &cycle, &reservation, launch)
        .unwrap();
    let _permit = f
        .store
        .admit_provider_open(&f.claim, handoff.operation())
        .unwrap();
    let session = SessionId::new();
    let writer = surge_core::id::ExecutionWriterId::new();
    f.store
        .admit_recipe_opening(writer, provider, "b", target.launch_hash())
        .unwrap();
    f.store
        .attach_admitted_configured_pin(writer, target.configured_pin().unwrap())
        .unwrap();
    let descriptor = ProviderSessionDescriptor::new(
        ProviderSessionId::new("actual-b".into()).unwrap(),
        provider,
        "b".into(),
        *target.launch_hash(),
        f.original.descriptor.cwd().to_path_buf(),
        SessionRestoreCapabilities {
            resume: true,
            load: true,
        },
    )
    .unwrap();
    let mut opened = OpenedSession::new(session, descriptor, SessionOpenMode::New).unwrap();
    opened.execution_writer = Some(
        surge_core::execution_recovery::process::ExecutionWriterObservation::new(writer, None)
            .unwrap(),
    );
    f.writer
        .append_event(V::new(EventPayload::SessionEstablishmentRequested {
            node: "impl_1".try_into().unwrap(),
            invocation: provider,
            restore: false,
            authority: None,
        }))
        .await
        .unwrap();
    let seq = f
        .writer
        .append_event(V::new(EventPayload::SessionOpened {
            node: "impl_1".try_into().unwrap(),
            session,
            agent: "b".into(),
            agent_id: None,
            opened: Some(opened),
            handoff: Some(handoff.operation()),
        }))
        .await
        .unwrap();
    f.store
        .confirm_provider_open(&f.claim, handoff.operation(), seq.0)
        .unwrap();
    f.store
        .authorize_fallback_prompt(&f.claim, handoff.operation(), seq.0)
        .unwrap();
    let cycle = f
        .store
        .recovery_cycle(f.claim.run, &logical.to_string(), 1)
        .unwrap();
    let source =
        QuotaRateLimitSource::new(seq.0, provider, session, Some(1000), "actual B 429".into())
            .unwrap();
    let current = f
        .store
        .record_selected_rate_limit(
            &f.claim,
            &cycle,
            &reservation.receipt,
            &source,
            &quota_error_observation(),
        )
        .unwrap();
    (f, current, reservation, session)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn planned_park_rejects_advanced_journal_without_mutating_capacity_authority() {
    for case in [
        "request",
        "entry",
        "completed",
        "aborted",
        "plan",
        "same-plan",
        "unchanged",
    ] {
        let (f, _, _, session) = fixture_with_two_fresh_pinned_sources().await;
        f.writer
            .append_event(V::new(EventPayload::SessionClosed {
                session,
                disposition: surge_core::run_event::SessionDisposition::Normal,
            }))
            .await
            .unwrap();
        let node: NodeKey = "impl_1".try_into().unwrap();
        let entry = f
            .writer
            .append_event(V::new(EventPayload::StageEntered {
                node: node.clone(),
                attempt: 3,
            }))
            .await
            .unwrap();
        let policy: FrozenQuotaPolicy = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(
                &attempt(&f.store.pool.get().unwrap(), f.claim.run)
                    .unwrap()
                    .config,
            )
            .unwrap()["quota_recovery"]
                .clone(),
        )
        .unwrap();
        let logical = StageInvocationId::new();
        let plan = f
            .writer
            .append_event(V::new(EventPayload::QuotaStagePlanned {
                node: node.clone(),
                attempt: 3,
                stage_entry_seq: entry.0,
                logical_invocation: logical,
                control_generation: 0,
                policy_hash: policy.content_hash().unwrap(),
            }))
            .await
            .unwrap();
        f.store
            .bind_planned_quota_stage(&f.claim, logical, plan.0)
            .unwrap();
        let cycle = f
            .store
            .begin_recovery_cycle(&f.claim, &logical.to_string(), 0)
            .unwrap();
        let super::super::super::CapacitySelection::AllExhausted { cycle, .. } = f
            .store
            .select_planned_capacity(&f.claim, &cycle, 100)
            .unwrap()
        else {
            panic!("both actual pinned typed sources must be fresh")
        };
        let suffix = match case {
            "request" => Some(EventPayload::SessionEstablishmentRequested {
                node: node.clone(),
                invocation: StageInvocationId::new(),
                restore: false,
                authority: None,
            }),
            "entry" => Some(EventPayload::StageEntered {
                node: node.clone(),
                attempt: 4,
            }),
            "completed" => Some(EventPayload::StageCompleted {
                node: node.clone(),
                outcome: "done".try_into().unwrap(),
            }),
            "same-plan" => Some(EventPayload::QuotaStagePlanned {
                node: node.clone(),
                attempt: 3,
                stage_entry_seq: entry.0,
                logical_invocation: logical,
                control_generation: 0,
                policy_hash: policy.content_hash().unwrap(),
            }),
            "plan" => Some(EventPayload::QuotaStagePlanned {
                node: node.clone(),
                attempt: 3,
                stage_entry_seq: entry.0,
                logical_invocation: StageInvocationId::new(),
                control_generation: 0,
                policy_hash: policy.content_hash().unwrap(),
            }),
            "aborted" => Some(EventPayload::RunAborted {
                reason: "terminal after bound plan".into(),
            }),
            _ => None,
        };
        if let Some(suffix) = suffix {
            f.writer.append_event(V::new(suffix)).await.unwrap();
        }
        let before_control = f.store.execution_control(f.claim.run).unwrap();
        let count = |table: &str| {
            f.store
                .pool
                .get()
                .unwrap()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap()
        };
        let before = (
            count("work_item_capacity_plan_proofs"),
            count("work_item_capacity_controls"),
        );
        let result = f.store.suspend_planned_capacity(&f.claim, &cycle, 100);
        if case == "unchanged" {
            assert!(
                result.is_ok(),
                "current all-skipped occurrence still parks: {result:?}"
            );
        } else {
            assert!(
                result.is_err(),
                "stale suffix {case} cannot seal planned authority"
            );
            assert_eq!(
                (
                    count("work_item_capacity_plan_proofs"),
                    count("work_item_capacity_controls")
                ),
                before
            );
            assert_eq!(
                f.store.execution_control(f.claim.run).unwrap(),
                before_control
            );
            assert_eq!(
                f.store
                    .recovery_cycle(f.claim.run, &logical.to_string(), cycle.generation)
                    .unwrap(),
                cycle
            );
            assert!(
                f.store
                    .due_recovery_wakes_for_run(f.claim.run, i64::MAX, 10)
                    .unwrap()
                    .is_empty()
            );
        }
        f.writer.close().await.unwrap();
    }
}
