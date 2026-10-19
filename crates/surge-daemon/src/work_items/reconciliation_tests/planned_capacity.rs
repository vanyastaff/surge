//! A stale launch page cannot consume a real pre-dispatch capacity suspension.
use super::*;
use surge_core::{
    ContentHash, EventPayload as E, NodeKey, SessionId, VersionedEventPayload as V,
    config::{CapacityRoute, ConfiguredSourceCompleteness},
    execution_recovery::{
        ExecutionControlState, OpenedSession, ProviderSessionDescriptor, ProviderSessionId,
        SessionOpenMode, SessionRestoreCapabilities, process::ExecutionWriterObservation,
    },
    id::ExecutionWriterId,
    run_event::SessionDisposition,
};
use surge_persistence::{
    runs::RunWriter,
    work_items::recovery_cycles::{
        AccountEvidence, CapacityEvidenceOrigin, CapacitySelection, FrozenQuotaCandidate,
        FrozenQuotaPolicy, FrozenQuotaStage, QuotaEvidence, QuotaObservation, QuotaRateLimitSource,
        QuotaRoutingMode,
    },
};

fn policy() -> FrozenQuotaPolicy {
    let candidate = FrozenQuotaCandidate::new(
        surge_persistence::work_items::recovery_cycles::RecoveryCandidate::new(
            "capacity-fixture".into(),
            AccountEvidence::Unknown,
        )
        .unwrap(),
        Some("fixture-model".into()),
        ContentHash::compute(b"capacity-fixture-launch"),
    )
    .unwrap()
    .with_configured_snapshot(
        CapacityRoute {
            provider_family: "fixture".into(),
            configured_route: "capacity-fixture".into(),
            auth_sources: Vec::new(),
            completeness: ConfiguredSourceCompleteness::CompleteConfiguredSources,
        },
        Some(ContentHash::compute(b"capacity-fixture-configured-sources")),
    );
    FrozenQuotaPolicy::new(vec![
        FrozenQuotaStage::new(
            "impl_1".parse().unwrap(),
            QuotaRoutingMode::Configured,
            vec![candidate],
            1000,
            1000,
        )
        .unwrap(),
    ])
    .unwrap()
}

// Establish the earlier exact recipe through the same public admission, journal
// binding and typed-rate-limit APIs used by the host. No SQL grants authority.
fn prior_capacity_opened(
    workspace: &WorkItemWorkspace,
    target: &FrozenQuotaCandidate,
    invocation: StageInvocationId,
    execution_writer: ExecutionWriterId,
    session: SessionId,
) -> OpenedSession {
    let descriptor = ProviderSessionDescriptor::new(
        ProviderSessionId::new("prior-capacity-fixture-session".into()).unwrap(),
        invocation,
        target.candidate().runtime().into(),
        *target.launch_hash(),
        workspace.path.clone(),
        SessionRestoreCapabilities {
            resume: true,
            load: true,
        },
        None,
    )
    .unwrap();
    let mut opened = OpenedSession::new(session, descriptor, SessionOpenMode::New).unwrap();
    opened.execution_writer =
        Some(ExecutionWriterObservation::new(execution_writer, None).unwrap());
    opened
}

async fn exhaust_prior_recipe(
    storage: &Arc<Storage>,
    claim: &WorkItemLaunchClaim,
    writer: &RunWriter,
    workspace: &WorkItemWorkspace,
    target: &FrozenQuotaCandidate,
) {
    let store = storage.work_items();
    let node: NodeKey = "impl_1".parse().unwrap();
    let invocation = StageInvocationId::new();
    let execution_writer = ExecutionWriterId::new();
    store
        .admit_recipe_opening(
            execution_writer,
            invocation,
            target.candidate().runtime(),
            target.launch_hash(),
        )
        .unwrap();
    store
        .attach_admitted_configured_pin(execution_writer, target.configured_pin().unwrap())
        .unwrap();
    let session = SessionId::new();
    let opened = prior_capacity_opened(workspace, target, invocation, execution_writer, session);
    writer
        .append_events(vec![
            V::new(E::StageEntered {
                node: node.clone(),
                attempt: 1,
            }),
            V::new(E::SessionEstablishmentRequested {
                node: node.clone(),
                invocation,
                restore: false,
                authority: None,
            }),
        ])
        .await
        .unwrap();
    let opening = writer
        .append_event(V::new(E::SessionOpened {
            node,
            session,
            agent: target.candidate().runtime().into(),
            agent_id: None,
            opened: Some(opened),
            handoff: None,
        }))
        .await
        .unwrap();
    store
        .bind_quota_stage(claim, invocation, opening.as_u64())
        .unwrap();
    let cycle = store
        .begin_recovery_cycle(claim, &invocation.to_string(), 0)
        .unwrap();
    let reservation = store
        .reserve_recovery_candidate(claim, &cycle, target.candidate())
        .unwrap();
    let current = store
        .recovery_cycle(claim.run(), &invocation.to_string(), cycle.generation)
        .unwrap();
    store
        .record_selected_rate_limit(
            claim,
            &current,
            &reservation.receipt,
            &QuotaRateLimitSource::new(
                opening.as_u64(),
                invocation,
                session,
                Some(1000),
                "typed quota fixture response".into(),
            )
            .unwrap(),
            &QuotaObservation::new(QuotaEvidence::Observed {
                observed_at_ms: 100,
                expires_at_ms: 1100,
                available: false,
                reset_at_ms: Some(1100),
            })
            .unwrap(),
        )
        .unwrap();
    writer
        .append_event(V::new(E::SessionClosed {
            session,
            disposition: SessionDisposition::Normal,
        }))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn stale_reconciliation_page_preserves_planned_capacity_and_exact_wake() {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let policy = policy();
    let (storage, attempt, workspace) =
        reserve_with_policy(home.path(), project.path(), Some(policy.clone())).await;
    write_startup(&storage, &attempt, &workspace).await;
    let store = storage.work_items();
    store
        .settle(
            attempt.run,
            attempt.binding.generation,
            WorkItemAttemptState::Launched,
            None,
        )
        .unwrap();
    let captured = store.for_run(attempt.run).unwrap().unwrap();
    assert_eq!(captured.state, WorkItemAttemptState::Launched);
    let claim = store.claim(attempt.run).unwrap();
    let writer = storage.open_run_writer(attempt.run).await.unwrap();
    exhaust_prior_recipe(
        &storage,
        &claim,
        &writer,
        &workspace,
        &policy.stages()[0].candidates()[0],
    )
    .await;
    let node: NodeKey = "impl_1".parse().unwrap();
    let entry = writer
        .append_event(V::new(E::StageEntered {
            node: node.clone(),
            attempt: 2,
        }))
        .await
        .unwrap();
    let logical = StageInvocationId::new();
    let plan = writer
        .append_event(V::new(E::QuotaStagePlanned {
            node: node.clone(),
            attempt: 2,
            stage_entry_seq: entry.as_u64(),
            logical_invocation: logical,
            control_generation: 0,
            policy_hash: policy.content_hash().unwrap(),
        }))
        .await
        .unwrap();
    store
        .bind_planned_quota_stage(&claim, logical, plan.as_u64())
        .unwrap();
    let cycle = store
        .begin_recovery_cycle(&claim, &logical.to_string(), 0)
        .unwrap();
    let CapacitySelection::AllExhausted { cycle, skipped } =
        store.select_planned_capacity(&claim, &cycle, 100).unwrap()
    else {
        panic!("the earlier pinned typed exhaustion must exclude the exact recipe")
    };
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].candidate, policy.stages()[0].candidates()[0]);
    let association = store.suspend_planned_capacity(&claim, &cycle, 100).unwrap();
    assert!(matches!(
        association.evidence_origin().unwrap(),
        CapacityEvidenceOrigin::PlannedExhaustion(_)
    ));
    let scheduled = store
        .recovery_cycle(attempt.run, &logical.to_string(), cycle.generation)
        .unwrap();
    let wake = scheduled.wake.as_ref().unwrap();
    assert_eq!(wake.due_at_ms(), 1100);
    let pending = PendingStagePhase::PlannedCapacity {
        node,
        logical_invocation: logical,
        stage_entry_seq: entry.as_u64(),
        plan_seq: plan.as_u64(),
    };
    let prefix = writer.current_seq().await.unwrap().as_u64();
    let fence = SuspensionFence {
        control_generation: association.target_control(),
        snapshot_seq: prefix,
        pending_stage: pending.clone(),
        cleanup_confirmed: true,
        reason: SuspensionReason::Capacity {
            wake_at_ms: Some(wake.due_at_ms()),
        },
    };
    writer
        .seal_suspension(
            fence.clone(),
            serde_json::to_vec(&serde_json::json!({"at_seq": prefix, "pending_stage": pending}))
                .unwrap(),
        )
        .await
        .unwrap();
    let confirmed = store.confirm_suspension(&claim, &fence).unwrap();
    assert_eq!(confirmed.state, ExecutionControlState::Suspended);
    writer.close().await.unwrap();
    drop(claim);

    let before = store.for_run(attempt.run).unwrap().unwrap();
    assert_eq!(before.state, WorkItemAttemptState::Suspended);
    let control_before = store.execution_control(attempt.run).unwrap();
    let journal_before = journal(&storage, attempt.run).await;
    let wake_before = store
        .due_recovery_wakes_for_run(attempt.run, 1100, 10)
        .unwrap();
    assert_eq!(wake_before, vec![scheduled.clone()]);
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let claim = store.claim(attempt.run).unwrap();
    engine.validate_work_item_startup(&claim).await.unwrap();
    drop(claim);
    let tracking = TrackingContext::new(engine.clone(), storage.clone());
    reconcile_candidate(
        captured,
        &tracking,
        &Arc::new(AdmissionController::new(1, 1)),
        &Arc::new(BroadcastRegistry::new()),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(store.for_run(attempt.run).unwrap().unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(
        store.execution_control(attempt.run).unwrap(),
        control_before
    );
    assert_eq!(journal(&storage, attempt.run).await, journal_before);
    assert_eq!(
        store
            .recovery_cycle(attempt.run, &logical.to_string(), cycle.generation)
            .unwrap(),
        scheduled
    );
    assert_eq!(
        store
            .due_recovery_wakes_for_run(attempt.run, 1100, 10)
            .unwrap(),
        wake_before
    );
    assert_eq!(
        store
            .capacity_control_association(attempt.run, association.target_control())
            .unwrap(),
        Some(association)
    );
    assert!(
        bridge.recorded_calls.lock().await.is_empty(),
        "stale reconciliation contacted a provider"
    );
    drop(tracking);
    drop(engine);
    drop(store);
    drop(storage);
    home.close().unwrap();
}
