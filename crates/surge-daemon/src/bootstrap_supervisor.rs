//! Durable bootstrap ownership: isolate, execute, validate, continue, and settle.
use crate::{
    admission::AdmissionController,
    bootstrap_recovery::{ExpectedBootstrapRun, StartupState, classify_startup},
    bootstrap_runtime::BootstrapRuntime,
    broadcast::BroadcastRegistry,
    tracked_run::{TrackingContext, spawn_tracked_run},
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use surge_core::{
    RunId,
    bootstrap_continuation::{BootstrapFailure, BootstrapTerminal},
    bootstrap_operation::{
        BootstrapAttentionReason as Attention, BootstrapCapture, BootstrapIntent,
        BootstrapOperationStatus, BootstrapPhase as Phase, BootstrapState as State,
    },
};
use surge_orchestrator::engine::{Engine, EngineRunConfig, RunOutcome, facade::EngineFacade};
use surge_persistence::runs::{
    Storage,
    bootstrap_operations::{
        BootstrapOperationRecord as Record, BootstrapOperationStore, BootstrapStoreError,
        BootstrapStoredPayload,
    },
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

/// Runtime dependencies shared with the production daemon engine and scheduler.
pub struct BootstrapServices {
    /// Concrete engine for authoritative activity and materialization.
    pub engine: Arc<Engine>,
    /// Same facade used by ordinary run dispatch.
    pub facade: Arc<dyn EngineFacade>,
    /// Same durable storage used by the engine.
    pub storage: Arc<Storage>,
    /// Absent if daemon configuration failed; status/cancel remain available.
    pub runtime: Option<BootstrapRuntime>,
    /// Shared global admission accounting.
    pub admission: Arc<AdmissionController>,
    /// Shared publication registry.
    pub broadcast: Arc<BroadcastRegistry>,
    /// Daemon shutdown token prevents new starts.
    pub shutdown: CancellationToken,
}

/// Supervisor request refusal; messages never include captured secrets.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// Durable journal failure.
    #[error(transparent)]
    Store(#[from] BootstrapStoreError),
    /// Original environment could not be verified.
    #[error(transparent)]
    Runtime(#[from] crate::bootstrap_runtime::BootstrapRuntimeError),
    /// Daemon has no valid configured runtime for new bootstrap work.
    #[error(
        "bootstrap needs a configured daemon runtime; configure surge.toml and restart the daemon"
    )]
    NotReady,
    /// New requests are refused while shutdown drains existing work.
    #[error("daemon is shutting down")]
    Shutdown,
}

/// One owner task per operation; journal commits linearize cancellation and admission.
pub struct BootstrapSupervisor {
    services: BootstrapServices,
    store: BootstrapOperationStore,
    workers: Mutex<HashMap<RunId, tokio::task::JoinHandle<()>>>,
    changed: Notify,
    admission_order: Arc<Mutex<()>>,
}

impl BootstrapSupervisor {
    /// Construct from the actual daemon services, never a synthetic tracking source.
    #[must_use]
    pub fn new(services: BootstrapServices) -> Arc<Self> {
        Arc::new(Self {
            store: services.storage.bootstrap_operation_store(),
            services,
            workers: Mutex::new(HashMap::new()),
            changed: Notify::new(),
            admission_order: Arc::new(Mutex::new(())),
        })
    }

    /// Acknowledge only committed acceptance; matching retries precede mutable preflight.
    pub async fn submit(
        &self,
        id: RunId,
        intent: &BootstrapIntent,
    ) -> Result<BootstrapOperationStatus, BootstrapError> {
        if let Some(existing) = self.store.lookup(id, intent)? {
            return Ok(existing.status);
        }
        if self.services.shutdown.is_cancelled() {
            return Err(BootstrapError::Shutdown);
        }
        let runtime = self
            .services
            .runtime
            .as_ref()
            .ok_or(BootstrapError::NotReady)?;
        let capture = runtime.capture(intent).await?;
        let mut admission = self.services.admission.bootstrap_acceptance().await;
        let reserved = self.store.reserved_run_ids()?;
        let waiting = self.store.pending_admission_runs()?;
        let maximum = admission.reserve(capture.fields().planning_run, &reserved, &waiting);
        let record = self
            .store
            .insert_bounded_if_absent(id, intent, &capture, maximum)?;
        if record.status.planning_run == capture.fields().planning_run {
            admission.commit();
        }
        drop(admission);
        self.changed.notify_waiters();
        Ok(record.status)
    }

    /// Read the durable operation projection.
    pub fn status(&self, id: RunId) -> Result<BootstrapOperationStatus, BootstrapError> {
        Ok(self.record(id)?.status)
    }
    /// Commit monotonic cancellation intent before acknowledgement.
    pub fn cancel(&self, id: RunId) -> Result<BootstrapOperationStatus, BootstrapError> {
        let record = self.store.request_cancel(id)?;
        self.changed.notify_waiters();
        Ok(record.status)
    }
    /// Explicit retry retains the original pins; changed intent requires a new operation.
    pub async fn retry(
        &self,
        id: RunId,
        revision: u64,
    ) -> Result<BootstrapOperationStatus, BootstrapError> {
        // Attention retries share the operation side-effect boundary with reconciliation.
        let workers = self.workers.lock().await;
        if workers.get(&id).is_some_and(|worker| !worker.is_finished()) {
            return Err(BootstrapStoreError::InvalidTransition.into());
        }
        let record = self.record(id)?;
        if !matches!(record.status.state, State::NeedsAttention { .. }) {
            return Err(BootstrapStoreError::InvalidTransition.into());
        }
        if record.status.revision != revision {
            return Err(BootstrapStoreError::StaleRevision.into());
        }
        if record.status.cancel_requested {
            let status = self.store.retry_cancellation(id, revision)?.status;
            drop(workers);
            self.changed.notify_waiters();
            return Ok(status);
        }
        let BootstrapStoredPayload::V1 { capture, .. } = &record.payload else {
            return Err(BootstrapStoreError::UnsupportedPayload.into());
        };
        self.services
            .runtime
            .as_ref()
            .ok_or(BootstrapError::NotReady)?
            .verify(capture)
            .await?;
        let expected = expected(&record)?;
        let inspection = self
            .services
            .storage
            .inspect_run(expected.run_id)
            .await
            .map_err(|_| BootstrapStoreError::CorruptRecord)?;
        let startup = classify_startup(&inspection, &expected)
            .map_err(|_| BootstrapStoreError::CorruptRecord)?;
        if matches!(startup, StartupState::Incomplete(_)) {
            return Err(BootstrapStoreError::InvalidTransition.into());
        }
        prepare_worktree(
            capture,
            record.child.is_some(),
            !matches!(startup, StartupState::Absent),
        )
        .await
        .map_err(|_| crate::bootstrap_runtime::BootstrapRuntimeError::Isolation)?;
        let result = self
            .store
            .retry(id, record.status.revision, capture)?
            .status;
        drop(workers);
        self.changed.notify_waiters();
        Ok(result)
    }
    /// Fence direct/legacy launchers from every reserved bootstrap run.
    pub fn owns_run(&self, run: RunId) -> Result<bool, BootstrapError> {
        Ok(self.store.reserved_run_ids()?.contains(&run))
    }

    fn record(&self, id: RunId) -> Result<Record, BootstrapStoreError> {
        self.store.get(id)?.ok_or(BootstrapStoreError::NotFound)
    }

    /// Rebuild queued/preparing/active operation ownership in persisted order.
    pub async fn reconcile(self: &Arc<Self>) -> Result<(), BootstrapStoreError> {
        let records = self.store.list()?;
        let mut workers = self.workers.lock().await;
        workers.retain(|_, task| !task.is_finished());
        for record in records {
            if record.status.state.phase().is_none()
                || matches!(record.status.state, State::NeedsAttention { .. })
                || !matches!(record.payload, BootstrapStoredPayload::V1 { .. })
                || workers.contains_key(&record.status.operation_id)
            {
                continue;
            }
            // Acquire before spawning so durable queue order survives task scheduling.
            let admission_order = self.admission_order.clone().lock_owned().await;
            let owner = self.clone();
            let id = record.status.operation_id;
            workers.insert(
                id,
                tokio::spawn(async move {
                    if let Err(reason) = owner.process(id, admission_order).await {
                        owner.attention(id, reason);
                    }
                }),
            );
        }
        // Finish the last worker's admission decision before startup exposes new acceptance.
        let _admission_ready = self.admission_order.lock().await;
        Ok(())
    }

    /// Run durable reconciliation until shutdown, then join existing owner tasks.
    /// The main daemon applies its overall grace deadline to this join.
    pub async fn run(self: Arc<Self>) {
        let mut poll = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                () = self.services.shutdown.cancelled() => break,
                () = self.changed.notified() => {},
                _ = poll.tick() => {},
            }
            if let Err(error) = self.reconcile().await {
                tracing::error!(%error, "bootstrap reconciliation failed");
            }
        }
        let workers = std::mem::take(&mut *self.workers.lock().await);
        for (_, worker) in workers {
            let _ = worker.await;
        }
    }

    async fn process(
        &self,
        id: RunId,
        admission_order: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), Attention> {
        let record = self.record(id).map_err(|_| Attention::StorageUnconfirmed)?;
        if let BootstrapStoredPayload::V1 { capture, .. } = &record.payload
            && capture.fields().bootstrap_edit_loop_cap.is_none()
        {
            return Err(Attention::MissingBootstrapPolicy);
        }
        let launch = expected(&record).map_err(|_| Attention::PartialStartup)?;
        if self
            .services
            .engine
            .snapshot_active_runs()
            .await
            .iter()
            .any(|run| run.run_id == launch.run_id)
        {
            // A live owner must settle; never attempt a duplicate start or resume.
            if record.status.cancel_requested {
                let _ = self
                    .services
                    .facade
                    .stop_run(launch.run_id, "bootstrap cancellation requested".into())
                    .await;
            }
            return Ok(());
        }
        let run = launch.run_id;
        let result = self.process_inactive(record, launch, admission_order).await;
        self.services.admission.notify_completed(run).await;
        result
    }

    async fn process_inactive(
        &self,
        record: Record,
        launch: ExpectedBootstrapRun,
        admission_order: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), Attention> {
        let id = record.status.operation_id;
        let inspection = self
            .services
            .storage
            .inspect_run(launch.run_id)
            .await
            .map_err(|_| Attention::StorageUnconfirmed)?;
        let startup =
            classify_startup(&inspection, &launch).map_err(|_| Attention::PartialStartup)?;
        if let StartupState::Terminal(outcome) = startup {
            return self.finish(id, outcome).await;
        }
        if matches!(startup, StartupState::Incomplete(_)) {
            return Err(Attention::PartialStartup);
        }
        if record.status.cancel_requested {
            return self.cancel_inactive(id, &launch).await;
        }
        if matches!(startup, StartupState::Parked { wake_at } if wake_at > chrono::Utc::now()) {
            return Ok(());
        }
        if self.services.shutdown.is_cancelled() {
            return Ok(());
        }
        if !self
            .services
            .admission
            .claim_bootstrap_slot(launch.run_id)
            .await
        {
            return Ok(());
        }
        drop(admission_order);
        self.launch(record, startup, launch).await
    }

    async fn launch(
        &self,
        mut record: Record,
        startup: StartupState,
        expected: ExpectedBootstrapRun,
    ) -> Result<(), Attention> {
        let id = record.status.operation_id;
        let phase = record
            .status
            .state
            .phase()
            .ok_or(Attention::PartialStartup)?;
        if matches!(phase, Phase::QueuedPlanning | Phase::QueuedImplementation) {
            record = match self.store.claim(id, record.status.revision) {
                Ok(claimed) => claimed,
                Err(_)
                    if self
                        .record(id)
                        .is_ok_and(|latest| latest.status.cancel_requested) =>
                {
                    return self.cancel_inactive(id, &expected).await;
                },
                Err(_) => return Err(Attention::StorageUnconfirmed),
            };
        }
        let BootstrapStoredPayload::V1 { capture, .. } = &record.payload else {
            return Err(Attention::PartialStartup);
        };
        if matches!(startup, StartupState::Absent)
            && matches!(phase, Phase::Planning | Phase::Implementing)
        {
            return Err(Attention::PartialStartup);
        }
        let runtime = self
            .services
            .runtime
            .as_ref()
            .ok_or(Attention::ConfigurationChanged)?;
        let runtime = &runtime
            .for_project(&capture.fields().repository)
            .map_err(runtime_attention)?;
        let context = runtime.verify(capture).await.map_err(runtime_attention)?;
        prepare_worktree(
            capture,
            record.child.is_some(),
            !matches!(startup, StartupState::Absent),
        )
        .await?;
        if self
            .record(id)
            .map_err(|_| Attention::StorageUnconfirmed)?
            .status
            .cancel_requested
        {
            return self.cancel_inactive(id, &expected).await;
        }
        if self.services.shutdown.is_cancelled() {
            return Ok(());
        }
        let (graph, config) = self.launch_inputs(&record, runtime, context).await?;
        let publisher = self.services.broadcast.register(expected.run_id).await;
        let tracking =
            TrackingContext::new(self.services.engine.clone(), self.services.storage.clone());
        let started = if matches!(startup, StartupState::Absent) {
            tracking
                .start(
                    self.services.facade.as_ref(),
                    expected.run_id,
                    graph,
                    expected.worktree,
                    config,
                )
                .await
        } else {
            tracking
                .resume(
                    self.services.facade.as_ref(),
                    expected.run_id,
                    expected.worktree,
                )
                .await
        };
        let Ok(handle) = started else {
            self.services.broadcast.deregister(expected.run_id).await;
            return Err(Attention::PartialStartup);
        };
        // Global subscribers discover supervised runs through the same event
        // as ordinary starts/resumes, before their per-run events are forwarded.
        self.services.broadcast.publish_global(
            surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted {
                run_id: expected.run_id,
            },
        );
        let tracked = spawn_tracked_run(
            expected.run_id,
            handle,
            publisher,
            self.services.admission.clone(),
            self.services.broadcast.clone(),
        );
        self.acknowledge_started(&record, expected.run_id).await;
        let outcome = self.wait_run(id, expected.run_id, tracked).await?;
        self.finish(id, outcome).await
    }

    async fn acknowledge_started(&self, record: &Record, run: RunId) {
        if matches!(
            record.status.state.phase(),
            Some(Phase::PreparingPlanning | Phase::PreparingImplementation)
        ) && self
            .store
            .mark_started(record.status.operation_id, record.status.revision)
            .is_err()
        {
            let _ = self
                .services
                .facade
                .stop_run(run, "bootstrap journal changed during start".into())
                .await;
        }
    }

    async fn launch_inputs(
        &self,
        record: &Record,
        runtime: &BootstrapRuntime,
        context: Option<surge_orchestrator::engine::config::ProjectContextSeed>,
    ) -> Result<(surge_core::graph::Graph, EngineRunConfig), Attention> {
        let BootstrapStoredPayload::V1 { intent, capture } = &record.payload else {
            return Err(Attention::PartialStartup);
        };
        let mut config = EngineRunConfig {
            initial_prompt: intent.prompt().into(),
            project_context: context,
            budget: intent.budget(),
            agent_registry: Some(runtime.agents()),
            ..Default::default()
        };
        config.bootstrap.edit_loop_cap = capture
            .fields()
            .bootstrap_edit_loop_cap
            .ok_or(Attention::MissingBootstrapPolicy)?;
        let graph = if let Some(child) = &record.child {
            let checked = crate::bootstrap_continuation::load_child(
                &self.services.storage,
                runtime,
                &capture.fields().planning_worktree,
                child,
            )
            .await
            .map_err(continuation_attention)?;
            config.budget = child.budget();
            config.seed_artifacts = checked.seeds;
            checked.graph
        } else {
            runtime.graph()
        };
        Ok((graph, config))
    }

    async fn wait_run(
        &self,
        id: RunId,
        run: RunId,
        mut tracked: tokio::task::JoinHandle<Result<RunOutcome, crate::tracked_run::TrackingError>>,
    ) -> Result<RunOutcome, Attention> {
        let mut poll = tokio::time::interval(Duration::from_millis(100));
        let mut stopped = false;
        loop {
            tokio::select! {
                result = &mut tracked => return result.map_err(|_| Attention::StorageUnconfirmed)?.map_err(|_| Attention::StorageUnconfirmed),
                _ = poll.tick() => {
                    let state = self.record(id);
                    if !stopped && state.as_ref().map_or(true, |record| record.status.cancel_requested) {
                        let _ = self.services.facade.stop_run(run, "bootstrap operation cancellation requested".into()).await;
                        stopped = true;
                    }
                },
            }
        }
    }

    async fn finish(&self, id: RunId, outcome: RunOutcome) -> Result<(), Attention> {
        let record = self.record(id).map_err(|_| Attention::StorageUnconfirmed)?;
        let run_id = launch_id(&record);
        let result = match outcome {
            RunOutcome::Completed { terminal } if record.child.is_some() => {
                BootstrapTerminal::Completed { run_id, terminal }
            },
            RunOutcome::Completed { .. } | RunOutcome::Aborted { .. }
                if record.status.cancel_requested =>
            {
                BootstrapTerminal::Cancelled
            },
            RunOutcome::Completed { .. } => return self.continue_parent(record).await,
            RunOutcome::Failed { .. } => BootstrapTerminal::Failed {
                run_id,
                reason: BootstrapFailure::RunFailed,
            },
            RunOutcome::Aborted { .. } => BootstrapTerminal::Failed {
                run_id,
                reason: BootstrapFailure::RunAborted,
            },
            RunOutcome::Parked { .. } if record.status.cancel_requested => {
                return self
                    .cancel_inactive(
                        id,
                        &expected(&record).map_err(|_| Attention::PartialStartup)?,
                    )
                    .await;
            },
            RunOutcome::Parked { .. } => return Ok(()),
            _ => return Err(Attention::StorageUnconfirmed),
        };
        self.store
            .settle(id, record.status.revision, &result)
            .map_err(|_| Attention::StorageUnconfirmed)?;
        Ok(())
    }

    async fn continue_parent(&self, mut record: Record) -> Result<(), Attention> {
        let id = record.status.operation_id;
        if record.status.state.phase() == Some(Phase::PreparingPlanning) {
            record = self
                .store
                .mark_started(id, record.status.revision)
                .map_err(|_| Attention::StorageUnconfirmed)?;
        }
        let BootstrapStoredPayload::V1 { intent, capture } = &record.payload else {
            return Err(Attention::PartialStartup);
        };
        let runtime = self
            .services
            .runtime
            .as_ref()
            .ok_or(Attention::ConfigurationChanged)?;
        let runtime = &runtime
            .for_project(&capture.fields().repository)
            .map_err(runtime_attention)?;
        runtime.verify(capture).await.map_err(runtime_attention)?;
        let child = crate::bootstrap_continuation::prepare_child(
            &self.services.engine,
            &self.services.storage,
            runtime,
            record.status.planning_run,
            &capture.fields().planning_worktree,
            intent,
        )
        .await;
        match child {
            Ok(child) => {
                if self
                    .store
                    .continue_planning(id, record.status.revision, &child)
                    .is_err()
                {
                    let latest = self.record(id).map_err(|_| Attention::StorageUnconfirmed)?;
                    if !latest.status.cancel_requested {
                        return Err(Attention::StorageUnconfirmed);
                    }
                    self.store
                        .settle(id, latest.status.revision, &BootstrapTerminal::Cancelled)
                        .map_err(|_| Attention::StorageUnconfirmed)?;
                }
            },
            Err(crate::bootstrap_continuation::ContinuationError::Exhausted) => {
                self.store
                    .settle(
                        id,
                        record.status.revision,
                        &BootstrapTerminal::Failed {
                            run_id: record.status.planning_run,
                            reason: BootstrapFailure::BudgetExhausted,
                        },
                    )
                    .map_err(|_| Attention::StorageUnconfirmed)?;
            },
            Err(error) => return Err(continuation_attention(error)),
        }
        self.changed.notify_waiters();
        Ok(())
    }

    async fn cancel_inactive(
        &self,
        id: RunId,
        launch: &ExpectedBootstrapRun,
    ) -> Result<(), Attention> {
        use crate::bootstrap_cancel::CancellationEvidence;
        let evidence = crate::bootstrap_cancel::cancel_inactive(
            &self.services.storage,
            &self.services.engine,
            id,
            launch,
        )
        .await
        .map_err(|_| Attention::StorageUnconfirmed)?;
        let record = self.record(id).map_err(|_| Attention::StorageUnconfirmed)?;
        let result = match evidence {
            CancellationEvidence::Absent | CancellationEvidence::Aborted => {
                BootstrapTerminal::Cancelled
            },
            CancellationEvidence::Terminal(RunOutcome::Completed { terminal })
                if record.child.is_some() =>
            {
                BootstrapTerminal::Completed {
                    run_id: launch.run_id,
                    terminal,
                }
            },
            CancellationEvidence::Terminal(_) => return Err(Attention::StorageUnconfirmed),
            CancellationEvidence::Active => {
                let _ = self
                    .services
                    .facade
                    .stop_run(launch.run_id, "bootstrap cancellation requested".into())
                    .await;
                return Ok(());
            },
        };
        self.store
            .settle(id, record.status.revision, &result)
            .map_err(|_| Attention::StorageUnconfirmed)?;
        Ok(())
    }

    fn attention(&self, id: RunId, reason: Attention) {
        let result = self
            .record(id)
            .and_then(|record| self.store.block(id, record.status.revision, reason));
        if let Err(error) = result {
            tracing::error!(%id, %error, "bootstrap attention could not be committed");
        }
    }
}

fn launch_id(record: &Record) -> RunId {
    if record.child.is_some() {
        record.status.implementation_run
    } else {
        record.status.planning_run
    }
}
fn expected(record: &Record) -> Result<ExpectedBootstrapRun, BootstrapStoreError> {
    let BootstrapStoredPayload::V1 { intent, capture } = &record.payload else {
        return Err(BootstrapStoreError::UnsupportedPayload);
    };
    let pins = capture.fields();
    Ok(ExpectedBootstrapRun {
        run_id: launch_id(record),
        worktree: if record.child.is_some() {
            pins.implementation_worktree.clone()
        } else {
            pins.planning_worktree.clone()
        },
        initial_prompt: intent.prompt().into(),
        config: surge_core::run_event::RunConfig {
            bootstrap_edit_loop_cap: pins.bootstrap_edit_loop_cap,
            sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
            approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
            budget: record.child.as_ref().map_or_else(
                || intent.budget(),
                surge_core::bootstrap_continuation::BootstrapContinuation::budget,
            ),
        },
        graph_hash: record
            .child
            .as_ref()
            .map_or(pins.graph.digest, |child| child.graph().digest),
    })
}
async fn prepare_worktree(
    capture: &BootstrapCapture,
    child: bool,
    executed: bool,
) -> Result<(), Attention> {
    let pins = capture.fields().clone();
    tokio::task::spawn_blocking(move || {
        use surge_git::run_worktree::{PinnedRunBase, ReconcilePhase, RunWorktreeSpec};
        let base = PinnedRunBase::new(
            pins.repository.clone(),
            pins.git_common_dir,
            pins.base_commit
                .parse()
                .map_err(|_| Attention::WorktreeConflict)?,
        )
        .map_err(|_| Attention::WorktreeConflict)?;
        let (id, path, branch) = if child {
            (
                pins.implementation_run,
                pins.implementation_worktree,
                pins.implementation_branch,
            )
        } else {
            (
                pins.planning_run,
                pins.planning_worktree,
                pins.planning_branch,
            )
        };
        let spec = RunWorktreeSpec::new(id, base, path).map_err(|_| Attention::WorktreeConflict)?;
        if spec.branch() != branch {
            return Err(Attention::WorktreeConflict);
        }
        surge_git::GitManager::new(pins.repository)
            .map_err(|_| Attention::WorktreeConflict)?
            .prepare_run_worktree(
                &spec,
                if executed {
                    ReconcilePhase::AfterExecution
                } else {
                    ReconcilePhase::BeforeExecution
                },
            )
            .map_err(|_| Attention::WorktreeConflict)?;
        Ok(())
    })
    .await
    .map_err(|_| Attention::WorktreeConflict)?
}
fn runtime_attention(error: crate::bootstrap_runtime::BootstrapRuntimeError) -> Attention {
    if matches!(
        error,
        crate::bootstrap_runtime::BootstrapRuntimeError::Credential
    ) {
        Attention::MissingCredential
    } else {
        Attention::ConfigurationChanged
    }
}
fn continuation_attention(error: crate::bootstrap_continuation::ContinuationError) -> Attention {
    match error {
        crate::bootstrap_continuation::ContinuationError::Unconfirmed => {
            Attention::StorageUnconfirmed
        },
        crate::bootstrap_continuation::ContinuationError::BudgetUnconfirmed
        | crate::bootstrap_continuation::ContinuationError::Exhausted => {
            Attention::BudgetUnconfirmed
        },
        crate::bootstrap_continuation::ContinuationError::Invalid => {
            Attention::InvalidMaterialization
        },
    }
}

#[cfg(test)]
#[path = "bootstrap_supervisor_tests.rs"]
mod tests;
