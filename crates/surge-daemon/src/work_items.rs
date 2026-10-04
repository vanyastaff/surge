//! Persistent-task host: exclusive preparation precedes source freezing and durable dispatch.
#[cfg(debug_assertions)]
mod start_preparation_barrier;
use crate::{
    admission::AdmissionController,
    broadcast::BroadcastRegistry,
    tracked_run::{TrackingContext, spawn_tracked_run},
};
use std::sync::Arc;
use surge_core::{
    RunId,
    run_event::EventPayload,
    work_item::{
        WorkItemAttempt, WorkItemAttemptState, WorkItemBinding, WorkItemCommand, WorkItemResult,
    },
};
use surge_orchestrator::engine::{
    EngineRunConfig,
    ipc::{ErrorCode, GlobalDaemonEvent},
};
use surge_persistence::work_items::{WorkItemAdmission, WorkItemError};

/// Apply one typed task operation through the real durable host.
pub(crate) async fn execute(
    command: &WorkItemCommand,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> Result<WorkItemResult, (ErrorCode, WorkItemError)> {
    let command = command.clone();
    let tracking = tracking.clone();
    let admission = admission.clone();
    let broadcasts = broadcasts.clone();
    // Connection cancellation cannot discard a launch after its startup commit.
    tokio::spawn(async move {
        let result = execute_owned(&command, &tracking, &admission, &broadcasts).await;
        match result {
            Ok(value) => Ok(value),
            Err(error) => classify_operation_failure(&command, &tracking, error),
        }
    })
    .await
    .map_err(|error| {
        (
            ErrorCode::Internal,
            WorkItemError::Invalid(error.to_string()),
        )
    })?
}
fn classify_operation_failure(
    command: &WorkItemCommand,
    tracking: &TrackingContext,
    error: WorkItemError,
) -> Result<WorkItemResult, (ErrorCode, WorkItemError)> {
    let Some((_, storage)) = tracking.task_sources() else {
        return Err((ErrorCode::EngineError, error));
    };
    match storage.work_items().operation_admission(command) {
        Ok(WorkItemAdmission::Accepted(value)) => {
            if let WorkItemResult::Control(control) = &value {
                storage
                    .work_items()
                    .record_control_diagnostic(control, &error.to_string())
                    .map_err(|failure| (ErrorCode::StorageError, failure))?;
                return match storage
                    .work_items()
                    .operation_admission(command)
                    .map_err(|failure| (ErrorCode::StorageError, failure))?
                {
                    WorkItemAdmission::Accepted(current) => Ok(current),
                    _ => Err((ErrorCode::StorageError, error)),
                };
            }
            Ok(value)
        },
        Ok(WorkItemAdmission::Rejected(reason)) => {
            Err((ErrorCode::WorkItemConflict, WorkItemError::Conflict(reason)))
        },
        Ok(WorkItemAdmission::Absent) => {
            let code = match &error {
                WorkItemError::Conflict(_) => ErrorCode::WorkItemConflict,
                WorkItemError::Busy => ErrorCode::WorkItemBusy,
                WorkItemError::NotFound => ErrorCode::WorkItemRejected,
                _ => ErrorCode::EngineError,
            };
            Err((code, error))
        },
        Err(lookup) => Err((ErrorCode::StorageError, lookup)),
    }
}
async fn execute_owned(
    command: &WorkItemCommand,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> Result<WorkItemResult, WorkItemError> {
    let (engine, storage) = tracking
        .task_sources()
        .ok_or_else(|| WorkItemError::Invalid("persistent task host unavailable".into()))?;
    let store = storage.work_items();
    if command.operation_id().is_none() {
        return store.query(command);
    }
    let result = mutate_owned(command, &engine, &storage, &store).await?;
    if let WorkItemResult::Control(control) = &result {
        return execute_control(command, control, tracking, admission, broadcasts, &store).await;
    }
    let WorkItemResult::Attempt(attempt) = &result else {
        return Ok(result);
    };
    if !attempt.state.is_active() {
        return Ok(result);
    }
    let launched = launch_attempt(attempt, tracking, admission, broadcasts).await;
    if let Err(error) = &launched {
        if let Some(current) = store.for_run(attempt.run)?
            && (!current.state.is_active() || matches!(error, WorkItemError::Busy))
        {
            return Ok(WorkItemResult::Attempt(Box::new(current)));
        }
        if !matches!(error, WorkItemError::Busy) {
            store.settle(
                attempt.run,
                attempt.binding.generation,
                WorkItemAttemptState::Attention,
                Some(error.to_string()),
            )?;
        }
    }
    launched
}

async fn mutate_owned(
    command: &WorkItemCommand,
    engine: &Arc<surge_orchestrator::engine::Engine>,
    storage: &Arc<surge_persistence::runs::Storage>,
    store: &surge_persistence::work_items::WorkItemStore,
) -> Result<WorkItemResult, WorkItemError> {
    let result = if let Some(original) = store.replay(command)? {
        original
    } else {
        let intent = if let WorkItemCommand::Create { project, .. } = command {
            Some(
                surge_git::task_workspace::plan(
                    project,
                    &storage.home().join("work-items/workspaces"),
                    RunId::new(),
                )
                .map_err(|e| WorkItemError::Invalid(e.to_string()))?,
            )
        } else {
            None
        };
        let preparation = match prepare_configured_start(command, engine, store).await? {
            StartPreparationResult::None => None,
            StartPreparationResult::Replay(original) => return Ok(original),
            StartPreparationResult::Exclusive(guard) => Some(*guard),
        };
        let (preparation, frozen) =
            freeze_start_config(command, engine, store, preparation).await?;
        if let WorkItemCommand::AttachPr { item, pr, .. } = command {
            let workspace = store.show(*item)?.item.workspace;
            surge_git::task_workspace::validate_pr_repository(&workspace, pr)
                .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        }
        let config = serde_json::to_string(&frozen)?;
        if let Some(guard) = preparation {
            guard.finalize(command, &config)?
        } else {
            store.mutate(
                command,
                intent.as_ref(),
                Some(&config),
                "operator",
                chrono::Utc::now().timestamp_millis(),
            )?
        }
    };
    Ok(result)
}

enum StartPreparationResult {
    None,
    Replay(WorkItemResult),
    Exclusive(Box<surge_persistence::work_items::StartPreparation>),
}
async fn prepare_configured_start(
    command: &WorkItemCommand,
    engine: &Arc<surge_orchestrator::engine::Engine>,
    store: &surge_persistence::work_items::WorkItemStore,
) -> Result<StartPreparationResult, WorkItemError> {
    let WorkItemCommand::Start { graph, .. } = command else {
        return Ok(StartPreparationResult::None);
    };
    if !engine.requires_start_preparation(graph) {
        return Ok(StartPreparationResult::None);
    }
    let preparing_store = store.clone();
    let preparing_command = command.clone();
    let begin = tokio::task::spawn_blocking(move || {
        preparing_store.begin_start_preparation(&preparing_command)
    })
    .await
    .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
    let guard = match begin {
        Ok(guard) => guard,
        Err(error) => {
            // A concurrent exact Start may have committed since the initial replay read.
            if let Some(original) = store.replay(command)? {
                return Ok(StartPreparationResult::Replay(original));
            }
            return Err(error);
        },
    };
    Ok(StartPreparationResult::Exclusive(Box::new(guard)))
}
async fn freeze_start_config(
    command: &WorkItemCommand,
    engine: &Arc<surge_orchestrator::engine::Engine>,
    store: &surge_persistence::work_items::WorkItemStore,
    preparation: Option<surge_persistence::work_items::StartPreparation>,
) -> Result<
    (
        Option<surge_persistence::work_items::StartPreparation>,
        EngineRunConfig,
    ),
    WorkItemError,
> {
    let WorkItemCommand::Start { item, graph, .. } = command else {
        return Ok((None, EngineRunConfig::default()));
    };
    if let Some(guard) = preparation {
        let engine = engine.clone();
        let command = command.clone();
        let runtime = tokio::runtime::Handle::current();
        // The blocking worker OWNS the guard through Git, configuration and source reads.
        // Cancelling its awaiting future cannot release the lease while that worker runs.
        let (guard, config) = tokio::task::spawn_blocking(move || {
            let workspace = guard.workspace();
            let phase = if guard.workspace_prepared() {
                surge_git::run_worktree::ReconcilePhase::AfterExecution
            } else {
                surge_git::run_worktree::ReconcilePhase::BeforeExecution
            };
            surge_git::task_workspace::prepare(workspace, phase)
                .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
            let config = start_run_config(&command, &workspace.checkout)?;
            let WorkItemCommand::Start { graph, .. } = &command else {
                return Err(WorkItemError::Invalid("preparation requires Start".into()));
            };
            #[cfg(debug_assertions)]
            if let WorkItemCommand::Start { item, .. } = &command {
                start_preparation_barrier::wait(*item)?;
            }
            let frozen = runtime
                .block_on(engine.freeze_work_item_config(graph, config, &workspace.path))
                .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
            Ok::<_, WorkItemError>((guard, frozen))
        })
        .await
        .map_err(|error| WorkItemError::Invalid(error.to_string()))??;
        Ok((Some(guard), config))
    } else {
        let path = store.show(*item)?.item.workspace.checkout;
        let config = start_run_config(command, &path)?;
        let config = engine
            .freeze_work_item_config(graph, config, &path)
            .await
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        Ok((None, config))
    }
}
fn start_run_config(
    command: &WorkItemCommand,
    path: &std::path::Path,
) -> Result<EngineRunConfig, WorkItemError> {
    let app = surge_core::SurgeConfig::discover_from(path)
        .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
    let mut config = surge_orchestrator::project_context::with_project_context_seed(
        EngineRunConfig::default(),
        path,
        &app,
    );
    config.budget = app.analytics.budget_guard();
    if let WorkItemCommand::Start {
        quota_recovery: Some(policy),
        ..
    } = command
    {
        let supplied: surge_persistence::work_items::recovery_cycles::FrozenQuotaPolicy =
            serde_json::from_value(policy.clone()).map_err(|error| {
                WorkItemError::Invalid(format!("quota recovery policy: {error}"))
            })?;
        if supplied
            .stages()
            .iter()
            .flat_map(surge_persistence::work_items::recovery_cycles::FrozenQuotaStage::candidates)
            .any(|target| {
                target.configured_pin().is_some()
                    || target.configured_route().is_some()
                    || !matches!(
                        target.candidate().account(),
                        surge_persistence::work_items::recovery_cycles::AccountEvidence::Unknown
                    )
            })
        {
            return Err(WorkItemError::Invalid(
                "quota sources and account observations must be frozen by the host".into(),
            ));
        }
        config.quota_recovery = supplied;
    }
    Ok(config)
}

async fn execute_control(
    command: &WorkItemCommand,
    control: &surge_core::execution_recovery::WorkItemExecutionControl,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
    store: &surge_persistence::work_items::WorkItemStore,
) -> Result<WorkItemResult, WorkItemError> {
    let (engine, _) = tracking
        .task_sources()
        .ok_or_else(|| WorkItemError::Invalid("persistent task host unavailable".into()))?;
    let latest = store
        .execution_control(control.run)?
        .ok_or(WorkItemError::NotFound)?;
    if latest.generation != control.generation {
        // Replay acknowledges the historical operation; it never grants an
        // old generation permission to affect the current actor.
        return Ok(WorkItemResult::Control(Box::new(latest)));
    }
    if control.state == surge_core::execution_recovery::ExecutionControlState::SuspendRequested {
        engine
            .suspend_work_item(control.run, control.generation)
            .await
            .map_err(|error| WorkItemError::Conflict(error.to_string()))?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let current = store
                .execution_control(control.run)?
                .ok_or(WorkItemError::NotFound)?;
            if current.generation != control.generation {
                return store.replay(command)?.ok_or(WorkItemError::NotFound);
            }
            if current.state
                != surge_core::execution_recovery::ExecutionControlState::SuspendRequested
            {
                return Ok(WorkItemResult::Control(Box::new(current)));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkItemError::Conflict(
                    "suspension remains pending; cleanup has not been durably confirmed".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    if control.state == surge_core::execution_recovery::ExecutionControlState::ContinueReserved {
        let attempt = store.for_run(control.run)?.ok_or(WorkItemError::NotFound)?;
        launch_attempt(&attempt, tracking, admission, broadcasts).await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let current = store
                .execution_control(control.run)?
                .ok_or(WorkItemError::NotFound)?;
            if current.generation != control.generation {
                return store.replay(command)?.ok_or(WorkItemError::NotFound);
            }
            if current.state
                != surge_core::execution_recovery::ExecutionControlState::ContinueReserved
            {
                return Ok(WorkItemResult::Control(Box::new(current)));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WorkItemError::Conflict(
                    "Continue is pending provider restoration".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    Ok(WorkItemResult::Control(Box::new(latest)))
}
async fn launch_attempt(
    attempt: &WorkItemAttempt,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> Result<WorkItemResult, WorkItemError> {
    let (engine, storage) = tracking
        .task_sources()
        .ok_or_else(|| WorkItemError::Invalid("task host unavailable".into()))?;
    let store = storage.work_items();
    let claim = store.claim(attempt.run)?;
    let item = store.show(attempt.item)?.item;
    let expected_control = store.execution_control(attempt.run)?;
    let continue_reserved = expected_control.as_ref().is_some_and(|control| {
        control.state == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
    });
    let inspected = match storage.inspect_folded_run(attempt.run).await {
        Ok(history) => history,
        Err(error) => {
            if let Some(control) = expected_control.filter(|control| {
                control.state
                    == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
            }) {
                store.mark_control_recovery_required(&claim, &control, &error.to_string())?;
            }
            return Err(WorkItemError::Invalid(error.to_string()));
        },
    };
    let (resume, terminal_state) = match inspected.database {
        None => (false, None),
        Some(history) if history.event_count == 0 => (false, None),
        Some(history) => {
            if let Err(error) = engine.validate_work_item_startup(&claim).await {
                store.settle(
                    attempt.run,
                    attempt.binding.generation,
                    WorkItemAttemptState::Attention,
                    Some(error.to_string()),
                )?;
                return Err(WorkItemError::Conflict(error.to_string()));
            }
            let folded = &history.state;
            if !continue_reserved
                && matches!(folded.attention(),surge_core::run_state::Attention::Waiting{until,..} if until>chrono::Utc::now())
            {
                store.settle(
                    attempt.run,
                    attempt.binding.generation,
                    WorkItemAttemptState::Launched,
                    None,
                )?;
                return Ok(WorkItemResult::Attempt(Box::new(
                    store.for_run(attempt.run)?.ok_or(WorkItemError::NotFound)?,
                )));
            }
            (true, terminal(&history, &attempt.binding)?)
        },
    };
    let prepared = resume || store.workspace_prepared(item.id)?;
    let phase = if prepared {
        surge_git::run_worktree::ReconcilePhase::AfterExecution
    } else {
        surge_git::run_worktree::ReconcilePhase::BeforeExecution
    };
    let workspace = item.workspace.clone();
    tokio::task::spawn_blocking(move || surge_git::task_workspace::prepare(&workspace, phase))
        .await
        .map_err(|e| WorkItemError::Invalid(e.to_string()))?
        .map_err(|e| WorkItemError::Invalid(e.to_string()))?;
    store.mark_workspace_prepared(&claim)?;
    if let Some(state) = terminal_state {
        store.settle(attempt.run, attempt.binding.generation, state, None)?;
        store.sync_usage(attempt.run)?;
        return Ok(WorkItemResult::Attempt(Box::new(
            store.for_run(attempt.run)?.ok_or(WorkItemError::NotFound)?,
        )));
    }
    if engine
        .snapshot_active_runs()
        .await
        .iter()
        .any(|run| run.run_id == attempt.run)
    {
        store.settle(
            attempt.run,
            attempt.binding.generation,
            WorkItemAttemptState::Launched,
            None,
        )?;
        return Ok(WorkItemResult::Attempt(Box::new(
            store.for_run(attempt.run)?.ok_or(WorkItemError::NotFound)?,
        )));
    }
    dispatch_attempt(attempt, claim, resume, tracking, admission, broadcasts).await
}

/// Resume a due parked run through its durable task claim when one owns it.
/// Returns `true` for every task-owned run (including inactive/stale attempts),
/// so callers never fall through to generic run resume without task fencing.
pub(crate) async fn resume_parked_work_item(
    run_id: RunId,
    now_ms: i64,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> Result<bool, WorkItemError> {
    let Some((engine, storage)) = tracking.task_sources() else {
        return Ok(false);
    };
    if engine
        .snapshot_active_runs()
        .await
        .iter()
        .any(|run| run.run_id == run_id)
    {
        // Suspension confirmation precedes the run task's final parked
        // projection. Keep the due reservation untouched until the current
        // engine owner has actually exited.
        return Ok(true);
    }
    let store = storage.work_items();
    let Some(attempt) = store.for_run(run_id)? else {
        return Ok(false);
    };
    if attempt.state == WorkItemAttemptState::Suspended {
        let should_resume = {
            let claim = store.claim(run_id)?;
            let current = store.execution_control(run_id)?;
            if let Some(control) = current.as_ref()
                && control.state == surge_core::execution_recovery::ExecutionControlState::Suspended
            {
                reserve_due_quota_wake(&store, &claim, now_ms)?;
            }
            store.execution_control(run_id)?.is_some_and(|control| {
                control.state
                    == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
            })
        };
        if should_resume {
            launch_attempt(&attempt, tracking, admission, broadcasts).await?;
        }
        return Ok(true);
    }
    if attempt.state != WorkItemAttemptState::Launched {
        // Attention and Suspended are explicit recovery/control decisions;
        // Reserved has no confirmed startup and must never become an implicit
        // retry merely because its run row happens to be parked.
        return Ok(true);
    }
    launch_attempt(&attempt, tracking, admission, broadcasts).await?;
    Ok(true)
}

fn reserve_due_quota_wake(
    store: &surge_persistence::work_items::WorkItemStore,
    claim: &surge_persistence::work_items::WorkItemLaunchClaim,
    now_ms: i64,
) -> Result<bool, WorkItemError> {
    let cycles = store.due_recovery_wakes_for_run(claim.run(), now_ms, 100)?;
    let Some(mut cycle) = cycles.into_iter().next() else {
        return Ok(false);
    };
    let control = store
        .execution_control(claim.run())?
        .ok_or(WorkItemError::NotFound)?;
    if control.state != surge_core::execution_recovery::ExecutionControlState::Suspended {
        return Ok(control.state
            == surge_core::execution_recovery::ExecutionControlState::ContinueReserved);
    }
    if cycle.control_generation != control.generation {
        cycle = store.rebind_recovery_control(claim, &cycle, control.generation)?;
    }
    let stage = store.bound_quota_stage(claim.run(), &cycle.invocation)?;
    let runtime = cycle
        .selected_runtime
        .as_deref()
        .or_else(|| {
            stage
                .candidates()
                .first()
                .map(|candidate| candidate.candidate().runtime())
        })
        .ok_or_else(|| {
            WorkItemError::Invalid("due capacity cycle has no frozen candidate".into())
        })?;
    let candidate = stage
        .candidates()
        .iter()
        .find(|candidate| candidate.candidate().runtime() == runtime)
        .cloned()
        .ok_or_else(|| {
            WorkItemError::Conflict("selected quota candidate is outside frozen policy".into())
        })?;
    let wake = cycle
        .wake
        .as_ref()
        .ok_or_else(|| WorkItemError::Conflict("due quota cycle lost its wake".into()))?;
    let launch = surge_persistence::work_items::recovery_cycles::QuotaLaunchContract::new(
        candidate,
        surge_core::id::StageInvocationId::new(),
        surge_core::execution_recovery::SessionOpenMode::New,
        None,
    )?;
    let reserved = store.reserve_automatic_wake(claim, &cycle, wake.identity(), now_ms, launch)?;
    Ok(reserved.reservation().disposition
        == surge_persistence::work_items::recovery_cycles::ReservationDisposition::Reserved)
}

async fn dispatch_attempt(
    attempt: &WorkItemAttempt,
    claim: surge_persistence::work_items::WorkItemLaunchClaim,
    resume: bool,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> Result<WorkItemResult, WorkItemError> {
    let (engine, storage) = tracking
        .task_sources()
        .ok_or_else(|| WorkItemError::Invalid("task host unavailable".into()))?;
    let store = storage.work_items();
    if !admission.try_admit_no_queue(attempt.run).await {
        return Err(WorkItemError::Busy);
    }
    let publisher = broadcasts.register(attempt.run).await;
    let control_events = engine.subscribe_tap();
    match tracking.task_run(&claim, resume).await {
        Ok(run) => {
            broadcasts.publish_global(GlobalDaemonEvent::RunAccepted {
                run_id: attempt.run,
            });
            let completion = spawn_tracked_run(
                attempt.run,
                run,
                publisher,
                admission.clone(),
                broadcasts.clone(),
            );
            if let Err(error) = store.settle(
                attempt.run,
                attempt.binding.generation,
                WorkItemAttemptState::Launched,
                None,
            ) {
                tracing::error!(run_id=%attempt.run,%error,"started task requires registry reconciliation");
            }
            let run_id = attempt.run;
            let binding = attempt.binding.clone();
            tokio::spawn(supervise_completion(
                completion,
                control_events,
                engine,
                storage,
                run_id,
                binding,
                claim,
            ));
        },
        Err(error) => {
            broadcasts.deregister(attempt.run).await;
            admission.notify_completed(attempt.run).await;
            // Engine errors can follow a committed startup. Inspect before releasing ownership.
            let state = match storage.inspect_folded_run(attempt.run).await {
                Ok(inspected) => match inspected.database {
                    None => WorkItemAttemptState::Rejected,
                    Some(history) if history.event_count == 0 => WorkItemAttemptState::Rejected,
                    _ => WorkItemAttemptState::Attention,
                },
                Err(_) => WorkItemAttemptState::Attention,
            };
            store.settle(
                attempt.run,
                attempt.binding.generation,
                state,
                Some(error.to_string()),
            )?;
            return Err(WorkItemError::Conflict(error.to_string()));
        },
    }
    Ok(WorkItemResult::Attempt(Box::new(
        store.for_run(attempt.run)?.ok_or(WorkItemError::NotFound)?,
    )))
}

async fn supervise_completion(
    mut completion: tokio::task::JoinHandle<
        Result<surge_orchestrator::engine::RunOutcome, crate::tracked_run::TrackingError>,
    >,
    mut control_events: tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
    engine: Arc<surge_orchestrator::engine::Engine>,
    storage: Arc<surge_persistence::runs::Storage>,
    run_id: RunId,
    binding: WorkItemBinding,
    claim: surge_persistence::work_items::WorkItemLaunchClaim,
) {
    let completed = loop {
        tokio::select! {
            completed = &mut completion => break completed,
            event = control_events.recv() => {
                if let Ok(event) = event
                    && event.run_id == run_id
                    && let EventPayload::RunContinued { control_generation } = event.event.payload.payload
                    && let Err(error) = storage.work_items().confirm_continued(&claim, control_generation)
                {
                    tracing::warn!(%run_id,%error,"Continue confirmation remains pending");
                }
            },
        }
    };
    match completed {
        Ok(Ok(surge_orchestrator::engine::RunOutcome::RecoveryRequired { diagnostic, .. })) => {
            if let Err(error) = storage.work_items().settle(
                run_id,
                binding.generation,
                WorkItemAttemptState::Attention,
                Some(diagnostic),
            ) {
                tracing::warn!(%run_id,%error,"provider restoration requires recovery attention");
            }
        },
        Ok(Ok(_)) => {
            if let Err(error) =
                reconcile_terminal(&engine, &storage, run_id, &binding, &claim).await
            {
                tracing::warn!(%run_id,%error,"task completion remains unconfirmed");
            }
        },
        unconfirmed => {
            if let Err(error) = storage.work_items().settle(
                run_id,
                binding.generation,
                WorkItemAttemptState::Attention,
                Some(format!(
                    "task tracking confirmation failed: {unconfirmed:?}"
                )),
            ) {
                tracing::warn!(%run_id,%error,"task tracking error remains unreconciled");
            }
        },
    }
    drop(claim);
}

fn terminal(
    history: &surge_persistence::runs::inspection::FoldedRunEvidence,
    binding: &WorkItemBinding,
) -> Result<Option<WorkItemAttemptState>, WorkItemError> {
    if !history.startup.iter().any(|row|matches!(&row.payload.payload,EventPayload::WorkItemAttemptBound{context} if context.binding()==binding)) {return Err(WorkItemError::Conflict("task journal binding mismatch".into()));}
    Ok(match &history.state {
        surge_core::RunState::Terminal { kind, .. } => Some(match kind {
            surge_core::run_state::TerminalReason::Completed => WorkItemAttemptState::Completed,
            surge_core::run_state::TerminalReason::Failed => WorkItemAttemptState::Failed,
            surge_core::run_state::TerminalReason::Aborted => WorkItemAttemptState::Aborted,
        }),
        _ => None,
    })
}

async fn reconcile_terminal(
    engine: &Arc<surge_orchestrator::engine::Engine>,
    storage: &Arc<surge_persistence::runs::Storage>,
    run: RunId,
    binding: &WorkItemBinding,
    claim: &surge_persistence::work_items::WorkItemLaunchClaim,
) -> Result<(), WorkItemError> {
    let store = storage.work_items();
    if !store
        .for_run(run)?
        .is_some_and(|attempt| attempt.state.is_active())
    {
        return Ok(());
    }
    if let Err(error) = engine.validate_work_item_startup(claim).await {
        store.settle(
            run,
            binding.generation,
            WorkItemAttemptState::Attention,
            Some(error.to_string()),
        )?;
        return Err(WorkItemError::Conflict(error.to_string()));
    }
    let inspected = storage
        .inspect_folded_run(run)
        .await
        .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
    if let Some(history) = inspected.database {
        if let Some(state) = terminal(&history, binding)? {
            store.settle(run, binding.generation, state, None)?;
        } else if let surge_core::RunState::Pipeline { memory, .. } = &history.state {
            if let Some(fence) = &memory.suspension {
                store.confirm_suspension(claim, fence)?;
            } else if store.execution_control(run)?.is_some_and(|control| {
                control.state
                    == surge_core::execution_recovery::ExecutionControlState::SuspendRequested
            }) {
                store.settle(
                    run,
                    binding.generation,
                    WorkItemAttemptState::Attention,
                    Some("suspension cleanup/fence was not confirmed".into()),
                )?;
            }
        }
    }
    storage.work_items().sync_usage(run)?;
    Ok(())
}

/// Reconcile one bounded attempt page. Unlaunched reservations require explicit retry.
pub(crate) async fn reconcile_page(
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
    after: Option<&str>,
) -> Result<Option<String>, WorkItemError> {
    let Some((_, storage)) = tracking.task_sources() else {
        return Ok(None);
    };
    let store = storage.work_items();
    let cursor = after
        .map(|value| {
            value
                .parse::<RunId>()
                .map_err(|error| WorkItemError::Invalid(error.to_string()))
        })
        .transpose()?;
    let page = store.scan_attempts(cursor, 20)?;
    for attempt in page.entries {
        if let Err(error) = store.sync_usage(attempt.run) {
            tracing::warn!(run_id=%attempt.run,%error,"task usage pending");
        }
        if !matches!(
            attempt.state,
            WorkItemAttemptState::Reserved | WorkItemAttemptState::Launched
        ) {
            continue;
        }
        if storage
            .get_run(&attempt.run)
            .await
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?
            .is_some_and(|run| run.status == surge_core::RunStatus::Parked)
        {
            // The run wake scheduler owns parked attempts. Replaying them from
            // this broad reconciliation page would bypass wake_at and race a
            // generic resume without the persistent task claim.
            continue;
        }
        // Never turn a reservation without a startup commit into an implicit new dispatch.
        let inspected = storage.inspect_folded_run(attempt.run).await;
        let present = matches!(&inspected,Ok(inspected) if matches!(&inspected.database,Some(history) if history.event_count>0));
        if !present {
            if let Err(error) = inspected {
                store.settle(
                    attempt.run,
                    attempt.binding.generation,
                    WorkItemAttemptState::Attention,
                    Some(error.to_string()),
                )?;
            }
            continue;
        }
        if let Err(error) = launch_attempt(&attempt, tracking, admission, broadcasts).await
            && !matches!(error, WorkItemError::Busy)
        {
            store.settle(
                attempt.run,
                attempt.binding.generation,
                WorkItemAttemptState::Attention,
                Some(error.to_string()),
            )?;
            tracing::warn!(run_id=%attempt.run,%error,"task recovery needs attention");
        }
    }
    Ok(page.next_cursor)
}

#[cfg(all(test, unix, debug_assertions))]
mod preparation_tests {
    use super::*;
    use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, time::Duration};
    use surge_core::{id::WorkItemOperationId, work_item::WorkItemRequirements};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_freeze_child() {
        let Some(home) = std::env::var_os("SURGE_CANCELLED_FREEZE_CHILD_HOME") else {
            return;
        };
        let home = std::path::PathBuf::from(home);
        let command: WorkItemCommand =
            serde_json::from_slice(&std::fs::read(home.join("command.json")).unwrap()).unwrap();
        let WorkItemCommand::Start { item, .. } = &command else {
            panic!("Start");
        };
        let item = *item;
        let storage = surge_persistence::runs::Storage::open(&home).await.unwrap();
        let store = storage.work_items();
        let workspace = store.show(item).unwrap().item.workspace;
        let engine = Arc::new(surge_orchestrator::engine::Engine::new(
            Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap()),
            storage.clone(),
            Arc::new(
                surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                    workspace.path,
                ),
            ),
            surge_orchestrator::engine::EngineConfig::default(),
        ));
        let guard = store.begin_start_preparation(&command).unwrap();
        let worker_store = store.clone();
        let worker_command = command.clone();
        let awaiting = tokio::spawn(async move {
            freeze_start_config(&worker_command, &engine, &worker_store, Some(guard)).await
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while !home.join("worker.ready").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        awaiting.abort();
        assert!(matches!(awaiting.await,Err(error) if error.is_cancelled()));
        assert!(matches!(
            store.begin_start_preparation(&command),
            Err(WorkItemError::Busy)
        ));
        let archive = WorkItemCommand::Archive {
            operation_id: WorkItemOperationId::new(),
            item,
            expected_version: 1,
        };
        assert!(matches!(
            store.mutate(&archive, None, None, "host", 1),
            Err(WorkItemError::Busy)
        ));
        let temporary = home.join("release.publishing");
        let mut release = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .unwrap();
        release.write_all(item.to_string().as_bytes()).unwrap();
        release.sync_all().unwrap();
        std::fs::rename(temporary, home.join("worker.release")).unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_lease_release(&store, &command),
        )
        .await
        .unwrap();
        assert!(store.replay(&command).unwrap().is_none());
        assert!(store.show(item).unwrap().item.active_run.is_none());
        assert!(!store.workspace_prepared(item).unwrap());
        assert!(store.scan_attempts(None, 10).unwrap().entries.is_empty());
    }
    async fn wait_for_lease_release(
        store: &surge_persistence::work_items::WorkItemStore,
        command: &WorkItemCommand,
    ) {
        loop {
            match store.begin_start_preparation(command) {
                Ok(guard) => {
                    drop(guard);
                    return;
                },
                Err(WorkItemError::Busy) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(error) => panic!("{error}"),
            }
        }
    }
    async fn wait_for_owned_child(child: &mut std::process::Child) {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "cancelled child failed");
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    fn init_project(path: &Path) {
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Cancellation oracle"],
            vec!["config", "user.email", "fixture@example.com"],
            vec!["commit", "--allow-empty", "-m", "base"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborted_awaiting_future_retains_worker_lease_and_cannot_finalize_in_background() {
        struct OwnedChild(std::process::Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        init_project(project.path());
        let storage = surge_persistence::runs::Storage::open(home.path())
            .await
            .unwrap();
        let workspace = surge_git::task_workspace::plan(
            project.path(),
            &home.path().join("work-items/workspaces"),
            RunId::new(),
        )
        .unwrap();
        let create = WorkItemCommand::Create {
            operation_id: WorkItemOperationId::new(),
            project: project.path().into(),
            title: "Cancel awaiting freeze".into(),
            requirements: WorkItemRequirements::new(
                "Original".into(),
                vec!["No background admission".into()],
            )
            .unwrap(),
        };
        let WorkItemResult::Detail(detail) = storage
            .work_items()
            .mutate(&create, Some(&workspace), None, "host", 0)
            .unwrap()
        else {
            panic!("detail");
        };
        let start = WorkItemCommand::Start {
            operation_id: WorkItemOperationId::new(),
            item: detail.item.id,
            expected_version: 1,
            graph: Box::new(
                toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap(),
            ),
            quota_recovery: None,
        };
        std::fs::write(
            home.path().join("command.json"),
            serde_json::to_vec(&start).unwrap(),
        )
        .unwrap();
        let mut child = OwnedChild(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "work_items::preparation_tests::cancelled_freeze_child",
                    "--nocapture",
                ])
                .env("SURGE_CANCELLED_FREEZE_CHILD_HOME", home.path())
                .env("SURGE_START_PREPARATION_ITEM", detail.item.id.to_string())
                .env(
                    "SURGE_START_PREPARATION_READY",
                    home.path().join("worker.ready"),
                )
                .env(
                    "SURGE_START_PREPARATION_RELEASE",
                    home.path().join("worker.release"),
                )
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(30), wait_for_owned_child(&mut child.0))
            .await
            .unwrap();
        assert!(storage.work_items().replay(&start).unwrap().is_none());
        assert!(
            storage
                .work_items()
                .scan_attempts(None, 10)
                .unwrap()
                .entries
                .is_empty()
        );
    }
}
