//! Isolated implementation attempts; original bootstrap result is never retried.
mod milestone_retry;
use super::{
    BootstrapError, BootstrapOperationStatus, BootstrapStoreError, BootstrapStoredPayload,
    BootstrapSupervisor, BootstrapTerminal, RunId, RunOutcome, State, TrackingContext, WorkerKey,
    spawn_tracked_run,
};
use std::{collections::HashMap, sync::Arc};
use surge_core::EventPayload;
use surge_core::bootstrap_attempt::{BootstrapAttempt, BootstrapAttemptState as A};
use surge_core::run_verdict::{TerminalOutcome, TerminalVerdict};
use surge_orchestrator::engine::{
    execution_state,
    fork::{ForkRequest, fork_with_parent},
};

impl BootstrapSupervisor {
    /// Reserve and fork one exact failed implementation under its original writer.
    pub async fn continue_implementation(
        self: &Arc<Self>,
        operation: RunId,
        revision: u64,
        source: RunId,
        request: RunId,
    ) -> Result<BootstrapOperationStatus, BootstrapError> {
        let mut workers = self.workers.lock().await;
        if let Some(existing) = self.store.attempt(request)? {
            if existing.operation_id != operation || existing.source_run != source {
                return Err(BootstrapError::Continuation("request identity conflicts"));
            }
            return self.status(operation);
        }
        if self.services.shutdown.is_cancelled() {
            return Err(BootstrapError::Shutdown);
        }
        if workers
            .get(&WorkerKey::Operation(operation))
            .is_some_and(|worker| !worker.is_finished())
            || self
                .services
                .engine
                .snapshot_active_runs()
                .await
                .iter()
                .any(|r| r.run_id == source)
        {
            return Err(BootstrapError::Continuation("source owner is still active"));
        }
        let record = self.record(operation)?;
        validate_source_association(&record.status, revision, source)?;
        if self
            .services
            .storage
            .work_items()
            .for_run(source)
            .map_err(|_| BootstrapError::Continuation("task ownership unavailable"))?
            .is_some()
        {
            return Err(BootstrapError::Continuation(
                "task-owned history requires its own owner",
            ));
        }
        let writer = self
            .services
            .storage
            .open_run_writer(source)
            .await
            .map_err(|_| BootstrapError::Continuation("source writer is unavailable"))?;
        let prepared = self
            .prepare_attempt(operation, revision, source, request, &writer)
            .await;
        let attempt = match prepared {
            Ok(attempt) => attempt,
            Err(error) => {
                let _ = writer.close().await;
                return Err(error);
            },
        };
        let owner = self.clone();
        let _ = register_continuation_worker(
            &mut workers,
            request,
            tokio::spawn(owner.create_attempt(attempt, writer)),
        );
        drop(workers);
        self.status(operation)
    }

    async fn prepare_attempt(
        &self,
        operation: RunId,
        revision: u64,
        source: RunId,
        request: RunId,
        writer: &surge_persistence::runs::RunWriter,
    ) -> Result<BootstrapAttempt, BootstrapError> {
        let record = self.record(operation)?;
        validate_source_association(&record.status, revision, source)?;
        let BootstrapStoredPayload::V1 { capture, intent } = &record.payload else {
            return Err(BootstrapStoreError::UnsupportedPayload.into());
        };
        if !intent.budget().limits.is_unlimited() {
            return Err(BootstrapError::FiniteContinuationBudget);
        }
        let reader = writer.reader();
        let state = execution_state::load(&reader, execution_state::Position::Latest)
            .await
            .map_err(|_| BootstrapError::Continuation("source history is unavailable"))?;
        validate_failed_state(&state)?;
        let (boundary, snapshot, prefix) = select_attempt_boundary(&reader, &state).await?;
        let expected_source = if source == record.status.implementation_run {
            capture.fields().implementation_worktree.clone()
        } else {
            self.services
                .runtime
                .as_ref()
                .ok_or(BootstrapError::NotReady)?
                .attempt_worktree(source)
        };
        validate_checkpoint(
            &reader,
            &snapshot,
            boundary,
            &expected_source,
            &capture.fields().git_common_dir,
        )
        .await?;

        if !matches!(prefix.terminal_verdict, TerminalVerdict::Open)
            || prefix.cursor_source != (execution_state::CursorSource::Checkpoint { seq: boundary })
        {
            return Err(BootstrapError::Continuation(
                "checkpoint is not an executable boundary",
            ));
        }
        let runtime = self
            .services
            .runtime
            .as_ref()
            .ok_or(BootstrapError::NotReady)?;
        let (fingerprint, _) =
            runtime.implementation_runtime(&capture.fields().repository, &prefix.graph)?;
        let attempt = BootstrapAttempt {
            continuation_id: request,
            operation_id: operation,
            source_run: source,
            run: RunId::new(),
            boundary,
            runtime: fingerprint,
            state: A::Reserved,
        };
        self.store.reserve_attempt(revision, &attempt)?;
        Ok(attempt)
    }

    async fn create_attempt(
        self: Arc<Self>,
        mut attempt: BootstrapAttempt,
        writer: surge_persistence::runs::RunWriter,
    ) {
        let result = if let Some(runtime) = &self.services.runtime {
            fork_with_parent(
                &self.services.storage,
                ForkRequest::new(attempt.source_run, attempt.run, attempt.boundary)
                    .with_worktree(runtime.attempt_worktree(attempt.run)),
                &writer,
            )
            .await
        } else {
            Err(surge_orchestrator::engine::EngineError::Internal(
                "runtime unavailable".into(),
            ))
        };
        let closed = writer.close().await;
        let next = if result.is_ok() && closed.is_ok() {
            A::ForkReady
        } else {
            A::NeedsAttention
        };
        if self
            .store
            .advance_attempt(attempt.continuation_id, A::Reserved, next)
            .is_err()
        {
            return;
        }
        if next == A::ForkReady {
            attempt.state = next;
            self.run_attempt(attempt).await;
        }
    }

    pub(super) fn reconcile_attempts(
        self: &Arc<Self>,
        workers: &mut HashMap<WorkerKey, tokio::task::JoinHandle<()>>,
    ) -> Result<(), BootstrapStoreError> {
        for record in self.store.list()? {
            for attempt in record.status.continuations {
                if workers.contains_key(&WorkerKey::Continuation(attempt.continuation_id)) {
                    continue;
                }
                match attempt.state {
                    A::Reserved => self.store.advance_attempt(
                        attempt.continuation_id,
                        A::Reserved,
                        A::NeedsAttention,
                    )?,
                    A::ForkReady | A::Running => {
                        let owner = self.clone();
                        let request = attempt.continuation_id;
                        let worker = tokio::spawn(owner.run_attempt(attempt));
                        let _ = register_continuation_worker(workers, request, worker);
                    },
                    A::NeedsAttention | A::Completed | A::Failed => {},
                }
            }
        }
        Ok(())
    }

    async fn run_attempt(self: Arc<Self>, attempt: BootstrapAttempt) {
        let from = if attempt.state == A::Reserved {
            A::ForkReady
        } else {
            attempt.state
        };
        if from == A::ForkReady
            && self
                .store
                .advance_attempt(attempt.continuation_id, from, A::Running)
                .is_err()
        {
            return;
        }
        let from = if from == A::ForkReady {
            A::Running
        } else {
            from
        };
        let result = self.execute_attempt(&attempt).await;
        let state = match result {
            Ok(RunOutcome::Completed { .. }) => A::Completed,
            Ok(RunOutcome::Failed { .. }) => A::Failed,
            _ => A::NeedsAttention,
        };
        if let Err(error) = self
            .store
            .advance_attempt(attempt.continuation_id, from, state)
        {
            tracing::error!(%error, "continuation settlement remains unconfirmed");
        }
    }

    async fn execute_attempt(
        &self,
        attempt: &BootstrapAttempt,
    ) -> Result<RunOutcome, BootstrapError> {
        let record = self.record(attempt.operation_id)?;
        let BootstrapStoredPayload::V1 { capture, .. } = record.payload else {
            return Err(BootstrapStoreError::UnsupportedPayload.into());
        };
        let runtime = self
            .services
            .runtime
            .as_ref()
            .ok_or(BootstrapError::NotReady)?;
        let reader = self
            .services
            .storage
            .open_run_reader(attempt.run)
            .await
            .map_err(|_| BootstrapError::Continuation("successor history missing"))?;
        let state = execution_state::load(&reader, execution_state::Position::Latest)
            .await
            .map_err(|_| BootstrapError::Continuation("successor history invalid"))?;
        let (fingerprint, catalog) =
            runtime.implementation_runtime(&capture.fields().repository, &state.graph)?;
        drop(reader);
        if fingerprint != attempt.runtime {
            return Err(BootstrapError::Continuation("successor runtime changed"));
        }
        if self.services.shutdown.is_cancelled()
            || !self
                .services
                .admission
                .try_admit_no_queue(attempt.run)
                .await
        {
            return Err(BootstrapError::Continuation(
                "successor admission unavailable",
            ));
        }
        let publisher = self.services.broadcast.register(attempt.run).await;
        let tracking =
            TrackingContext::new(self.services.engine.clone(), self.services.storage.clone());
        let started = tracking
            .resume_scoped(
                &self.services.engine,
                attempt.run,
                runtime.attempt_worktree(attempt.run),
                catalog,
            )
            .await;
        let result = if let Ok(handle) = started {
            self.services.broadcast.publish_global(
                surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted {
                    run_id: attempt.run,
                },
            );
            let tracked = spawn_tracked_run(
                attempt.run,
                handle,
                publisher,
                self.services.admission.clone(),
                self.services.broadcast.clone(),
            );
            tracked.await.ok().and_then(Result::ok)
        } else {
            self.services.broadcast.deregister(attempt.run).await;
            None
        };
        self.services.admission.notify_completed(attempt.run).await;
        result.ok_or(BootstrapError::Continuation(
            "successor completion unconfirmed",
        ))
    }
}

async fn select_attempt_boundary(
    reader: &surge_persistence::runs::RunReader,
    latest: &execution_state::ExecutionState,
) -> Result<(u64, Vec<u8>, execution_state::ExecutionState), BootstrapError> {
    let invalid =
        || BootstrapError::Continuation("no positively proven executable continuation boundary");
    let boundaries = reader.list_snapshots().await.map_err(|_| invalid())?;
    for (index, seq) in boundaries.iter().rev().enumerate() {
        let boundary = seq.as_u64();
        let Some((actual, bytes)) = reader
            .latest_snapshot_at_or_before(*seq)
            .await
            .map_err(|_| invalid())?
        else {
            return Err(invalid());
        };
        if actual != *seq {
            return Err(invalid());
        }
        let prefix = execution_state::load(reader, execution_state::Position::AtSeq(boundary))
            .await
            .map_err(|_| invalid())?;
        if index == 0
            && validate_tail(reader, boundary, latest.position)
                .await
                .is_ok()
        {
            return Ok((boundary, bytes, prefix));
        }
        let snapshot = surge_orchestrator::engine::snapshot::EngineSnapshot::deserialize(&bytes)
            .map_err(|_| invalid())?;
        let tail = reader
            .read_events(
                surge_persistence::runs::EventSeq(boundary + 1)
                    ..surge_persistence::runs::EventSeq(latest.position + 1),
            )
            .await
            .map_err(|_| invalid())?;
        if milestone_retry::prove(&prefix, &snapshot, latest, &tail, *reader.run_id()).is_ok()
            && historically_accepted_tasks(reader, &prefix).await?
        {
            let history = surge_core::run_verdict::TerminalHistory::of(
                tail.iter()
                    .map(|event| (event.seq.as_u64(), &event.payload.payload)),
            );
            if !history.post_terminal_activity().is_empty() {
                return Err(invalid());
            }
            return Ok((boundary, bytes, prefix));
        }
    }
    Err(invalid())
}

async fn historically_accepted_tasks(
    reader: &surge_persistence::runs::RunReader,
    prefix: &execution_state::ExecutionState,
) -> Result<bool, BootstrapError> {
    let Some(surge_orchestrator::engine::frames::Frame::Loop(tasks)) = prefix.frames.last() else {
        return Ok(false);
    };
    for item in &tasks.items {
        let Some(id) = item.get("id").and_then(toml::Value::as_str) else {
            return Ok(false);
        };
        let id = surge_core::roadmap::RoadmapTaskId::from(id);
        let Some(task) = prefix.memory.ledger.tasks.get(&id) else {
            return Ok(false);
        };
        let accepted =
            execution_state::load(reader, execution_state::Position::AtSeq(task.updated_seq))
                .await
                .map_err(|_| {
                    BootstrapError::Continuation("historical task verification unavailable")
                })?;
        if !accepted
            .memory
            .ledger
            .tasks
            .get(&id)
            .is_some_and(|historical| {
                historical.verified
                    && historical.status == surge_core::roadmap::RoadmapStatus::Completed
                    && historical.updated_seq == task.updated_seq
                    && historical.last_authority_node == task.last_authority_node
            })
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn check_source_writers(memory: &surge_core::run_state::RunMemory) -> Result<(), BootstrapError> {
    for writer in memory.execution_writers.values() {
        if writer.conflicting_observation {
            return Err(BootstrapError::Continuation("conflicting source writer"));
        }
        if let Some(container) = &writer.container {
            if surge_acp::process_evidence::group_state(container)
                != surge_acp::process_evidence::GroupState::Empty
            {
                return Err(BootstrapError::Continuation(
                    "source process group is not empty",
                ));
            }
        } else if !writer.cleanup_confirmed {
            return Err(BootstrapError::Continuation(
                "source writer settlement is unknown",
            ));
        }
    }
    Ok(())
}

fn accepted_after_boundary(payload: &surge_core::EventPayload) -> bool {
    use surge_core::EventPayload as E;
    matches!(
        payload,
        E::TaskSplit { .. }
            | E::TaskStatusChanged { .. }
            | E::TaskAcceptedByHuman { .. }
            | E::TaskDiscovered { .. }
            | E::TaskVerified { .. }
            | E::StageOutcomeCommitted { .. }
            | E::StageCompleted { .. }
            | E::EdgeTraversed { .. }
            | E::LoopIterationStarted { .. }
            | E::LoopIterationCompleted { .. }
            | E::LoopCompleted { .. }
            | E::RoadmapPatchApplied { .. }
            | E::RoadmapUpdated { .. }
            | E::GraphRevisionAccepted { .. }
            | E::SubgraphEntered { .. }
            | E::SubgraphExited { .. }
    )
}

fn validate_source_association(
    status: &BootstrapOperationStatus,
    revision: u64,
    source: RunId,
) -> Result<(), BootstrapError> {
    if status.revision != revision {
        return Err(BootstrapStoreError::StaleRevision.into());
    }
    let original = matches!(&status.result, Some(BootstrapTerminal::Failed { run_id, .. })
        if *run_id == source && source == status.implementation_run);
    let successor = status
        .continuations
        .iter()
        .any(|attempt| attempt.run == source && attempt.state == A::Failed);
    if status.cancel_requested || status.state != State::Failed || !(original || successor) {
        return Err(BootstrapError::Continuation(
            "source is not this operation's failed implementation",
        ));
    }
    Ok(())
}

async fn validate_checkpoint(
    reader: &surge_persistence::runs::RunReader,
    bytes: &[u8],
    boundary: u64,
    source: &std::path::Path,
    common: &std::path::Path,
) -> Result<(), BootstrapError> {
    let invalid = || BootstrapError::Continuation("checkpoint source identity is unavailable");
    let snapshot = surge_orchestrator::engine::snapshot::EngineSnapshot::deserialize(bytes)
        .map_err(|_| invalid())?;
    let checkpoint = snapshot.workspace_checkpoint.ok_or_else(invalid)?;
    if snapshot.at_seq != boundary
        || snapshot.stage_boundary_seq != boundary
        || checkpoint
            .git_common_dir
            .canonicalize()
            .map_err(|_| invalid())?
            != common.canonicalize().map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    surge_git::checkpoint::tree_identity(&checkpoint).map_err(|_| invalid())?;
    let first = reader
        .read_events(surge_persistence::runs::EventSeq(1)..surge_persistence::runs::EventSeq(2))
        .await
        .map_err(|_| invalid())?;
    let Some(event) = first.first() else {
        return Err(invalid());
    };
    let EventPayload::RunStarted { project_path, .. } = &event.payload.payload else {
        return Err(invalid());
    };
    if project_path.canonicalize().map_err(|_| invalid())?
        != source.canonicalize().map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_failed_state(state: &execution_state::ExecutionState) -> Result<(), BootstrapError> {
    if !matches!(state.terminal_verdict, TerminalVerdict::Terminal(ref claim)
        if matches!(claim.outcome, TerminalOutcome::Failed { .. }))
    {
        return Err(BootstrapError::Continuation(
            "source is not uniquely failed",
        ));
    }
    check_source_writers(&state.memory)?;
    let config = state
        .run_config
        .as_ref()
        .ok_or(BootstrapError::Continuation("source configuration missing"))?;
    validate_supported_config(config)
}

fn validate_supported_config(config: &surge_core::RunConfig) -> Result<(), BootstrapError> {
    if !config.mcp_servers.is_empty() {
        return Err(BootstrapError::ExternalContinuationMcp);
    }
    if !config.budget.limits.is_unlimited() {
        return Err(BootstrapError::FiniteContinuationBudget);
    }
    Ok(())
}

async fn validate_tail(
    reader: &surge_persistence::runs::RunReader,
    boundary: u64,
    position: u64,
) -> Result<(), BootstrapError> {
    let tail = reader
        .read_events(
            surge_persistence::runs::EventSeq(boundary + 1)
                ..surge_persistence::runs::EventSeq(position + 1),
        )
        .await
        .map_err(|_| BootstrapError::Continuation("source tail unavailable"))?;
    let history = surge_core::run_verdict::TerminalHistory::of(
        tail.iter()
            .map(|event| (event.seq.as_u64(), &event.payload.payload)),
    );
    if !history.post_terminal_activity().is_empty() {
        return Err(BootstrapError::Continuation(
            "source has post-terminal execution activity",
        ));
    }
    if tail
        .iter()
        .any(|event| accepted_after_boundary(&event.payload.payload))
    {
        return Err(BootstrapError::Continuation(
            "saved boundary predates accepted task or stage state",
        ));
    }
    Ok(())
}

pub(super) fn register_continuation_worker(
    workers: &mut HashMap<WorkerKey, tokio::task::JoinHandle<()>>,
    request: RunId,
    worker: tokio::task::JoinHandle<()>,
) -> Option<tokio::task::JoinHandle<()>> {
    workers.insert(WorkerKey::Continuation(request), worker)
}

#[cfg(test)]
mod tests {
    use super::{BootstrapError, validate_supported_config};
    use std::time::Duration;

    #[test]
    fn continuation_budget_and_external_mcp_fail_closed_with_zero_unlimited_control() {
        let mut config: surge_core::RunConfig = serde_json::from_value(serde_json::json!({
            "sandbox_default":"workspace-write", "approval_default":"on-request"
        }))
        .unwrap();
        assert!(validate_supported_config(&config).is_ok());
        for limits in [
            surge_core::budget::BudgetLimits {
                tokens: Some(1),
                ..Default::default()
            },
            surge_core::budget::BudgetLimits {
                usd: Some(0.01),
                ..Default::default()
            },
        ] {
            config.budget.limits = limits;
            assert!(matches!(
                validate_supported_config(&config),
                Err(BootstrapError::FiniteContinuationBudget)
            ));
        }
        config.budget.limits = surge_core::budget::BudgetLimits {
            tokens: Some(0),
            usd: Some(0.0),
            ..Default::default()
        };
        assert!(validate_supported_config(&config).is_ok());
        config
            .mcp_servers
            .push(surge_core::mcp_config::McpServerRef::new(
                "external".into(),
                surge_core::mcp_config::McpTransportConfig::Stdio {
                    command: "unused".into(),
                    args: vec![],
                    env: std::collections::HashMap::default(),
                },
                None,
                Duration::from_secs(1),
                false,
            ));
        assert!(matches!(
            validate_supported_config(&config),
            Err(BootstrapError::ExternalContinuationMcp)
        ));
    }
}
