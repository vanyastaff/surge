//! Registry association; filesystem/journal ownership belongs to the daemon.
use super::{
    BootstrapOperationStore, BootstrapState, BootstrapStoreError, Connection, RunId,
    TransactionBehavior, params, read,
};
use rusqlite::OptionalExtension as _;
use surge_core::bootstrap_attempt::{BootstrapAttempt, BootstrapAttemptState};

impl BootstrapOperationStore {
    /// Replay an exact request before mutable runtime validation.
    pub fn attempt(&self, request: RunId) -> Result<Option<BootstrapAttempt>, BootstrapStoreError> {
        let connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        Ok(attempts(&connection, None)?
            .into_iter()
            .find(|a| a.continuation_id == request))
    }

    /// Reserve once under the original failed-operation revision fence.
    pub fn reserve_attempt(
        &self,
        revision: u64,
        attempt: &BootstrapAttempt,
    ) -> Result<BootstrapAttempt, BootstrapStoreError> {
        let mut connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record =
            read(&transaction, attempt.operation_id)?.ok_or(BootstrapStoreError::NotFound)?;
        if record.status.revision != revision {
            return Err(BootstrapStoreError::StaleRevision);
        }
        let failed_original = matches!(record.status.result,
            Some(surge_core::bootstrap_continuation::BootstrapTerminal::Failed { run_id, .. })
                if run_id == attempt.source_run && run_id == record.status.implementation_run);
        let failed_successor = record
            .status
            .continuations
            .iter()
            .any(|a| a.run == attempt.source_run && a.state == BootstrapAttemptState::Failed);
        if record.status.cancel_requested
            || !matches!(record.status.state, BootstrapState::Failed)
            || !(failed_original || failed_successor)
            || attempt.state != BootstrapAttemptState::Reserved
        {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        if record
            .status
            .continuations
            .iter()
            .any(|a| a.source_run == attempt.source_run)
        {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        transaction.execute(
            "INSERT INTO bootstrap_attempts (continuation_id,operation_id,source_run,run,boundary,runtime,state_json) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                attempt.continuation_id.to_string(),
                attempt.operation_id.to_string(),
                attempt.source_run.to_string(),
                attempt.run.to_string(),
                attempt.boundary,
                attempt.runtime.to_string(),
                serde_json::to_string(&attempt.state)?
            ],
        )?;
        let changed = transaction.execute("UPDATE bootstrap_operations SET revision=revision+1 WHERE operation_id=?1 AND revision=?2",
            params![attempt.operation_id.to_string(), revision])?;
        if changed != 1 {
            return Err(BootstrapStoreError::StaleRevision);
        }
        transaction.commit()?;
        Ok(attempt.clone())
    }

    /// Compare-and-swap only the mutable successor state, never its identity.
    pub fn advance_attempt(
        &self,
        request: RunId,
        from: BootstrapAttemptState,
        to: BootstrapAttemptState,
    ) -> Result<(), BootstrapStoreError> {
        use BootstrapAttemptState::{
            Completed, Failed, ForkReady, NeedsAttention, Reserved, Running,
        };
        if !matches!(
            (from, to),
            (Reserved, ForkReady | NeedsAttention)
                | (ForkReady, Running)
                | (Running, Completed | Failed | NeedsAttention)
        ) {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        let mut connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation: String = transaction
            .query_row(
                "SELECT operation_id FROM bootstrap_attempts WHERE continuation_id=?1",
                [request.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(BootstrapStoreError::NotFound)?;
        let changed = transaction.execute("UPDATE bootstrap_attempts SET state_json=?1 WHERE continuation_id=?2 AND state_json=?3",
            params![serde_json::to_string(&to)?,request.to_string(),serde_json::to_string(&from)?])?;
        if changed != 1 {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        let changed = transaction.execute(
            "UPDATE bootstrap_operations SET revision=revision+1 WHERE operation_id=?1",
            [operation],
        )?;
        if changed != 1 {
            return Err(BootstrapStoreError::CorruptRecord);
        }
        transaction.commit()?;
        Ok(())
    }
}

pub(super) fn attempts(
    connection: &Connection,
    operation: Option<RunId>,
) -> Result<Vec<BootstrapAttempt>, BootstrapStoreError> {
    // Read-only history from an older registry remains readable before daemon migration.
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='bootstrap_attempts' AND type='table')",[],|row|row.get(0))?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare("SELECT continuation_id,operation_id,source_run,run,boundary,runtime,state_json FROM bootstrap_attempts WHERE ?1 IS NULL OR operation_id=?1 ORDER BY rowid")?;
    let rows = statement.query_map([operation.map(|id| id.to_string())], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, u64>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    rows.map(|row| {
        let (request, operation, source, run, boundary, runtime, state) = row?;
        let corrupt = || BootstrapStoreError::CorruptRecord;
        Ok(BootstrapAttempt {
            continuation_id: request.parse().map_err(|_| corrupt())?,
            operation_id: operation.parse().map_err(|_| corrupt())?,
            source_run: source.parse().map_err(|_| corrupt())?,
            run: run.parse().map_err(|_| corrupt())?,
            boundary,
            runtime: runtime.parse().map_err(|_| corrupt())?,
            state: serde_json::from_str(&state)?,
        })
    })
    .collect()
}
