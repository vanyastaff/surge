//! Exclusive host preparation before freezing sources in the actual launch workspace.
pub(in crate::work_items) mod secure_lock;
use super::*;
use secure_lock::PreparationLock;

/// Host-only owning capability. Neither cloneable nor serializable; dropping abandons
/// only this token while its OS lock is still held. No attempt exists until finalization.
pub struct StartPreparation {
    store: WorkItemStore,
    lock: PreparationLock,
    item: WorkItemId,
    token: String,
    operation: String,
    body_hash: String,
    snapshot: String,
    lock_identity: String,
    workspace: WorkItemWorkspace,
    prepared: bool,
    consumed: bool,
}
impl StartPreparation {
    /// Original host-owned workspace; config discovery still uses its checkout.
    pub fn workspace(&self) -> &WorkItemWorkspace {
        &self.workspace
    }
    /// True only after a genuine earlier launch claim acknowledged preparation.
    pub fn workspace_prepared(&self) -> bool {
        self.prepared
    }
    /// Consume this capability and commit one immutable ordinary Start reservation.
    pub fn finalize(mut self, command: &WorkItemCommand, config: &str) -> Result<WorkItemResult> {
        self.lock.verify()?;
        if command_hash(command)? != self.body_hash
            || command.operation_id().map(|id| id.to_string()).as_deref() != Some(&self.operation)
        {
            return Err(WorkItemError::Conflict(
                "preparation operation changed".into(),
            ));
        }
        let mut conn = self.store.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: Option<(String,String,String,String,String)> = tx.query_row("SELECT token,operation,body_hash,snapshot,lock_identity FROM work_item_start_preparations WHERE item=? AND state='preparing'", [self.item.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if stored
            != Some((
                self.token.clone(),
                self.operation.clone(),
                self.body_hash.clone(),
                self.snapshot.clone(),
                self.lock_identity.clone(),
            ))
            || snapshot(&tx, self.item)? != self.snapshot
        {
            return Err(WorkItemError::Conflict(
                "task changed during Start preparation".into(),
            ));
        }
        if self.lock.identity()? != self.lock_identity {
            return Err(WorkItemError::Conflict(
                "preparation lock identity changed".into(),
            ));
        }
        let WorkItemCommand::Start {
            item,
            expected_version,
            graph,
            ..
        } = command
        else {
            return Err(WorkItemError::Invalid("preparation requires Start".into()));
        };
        let consumed=tx.execute("UPDATE work_item_start_preparations SET state='consumed' WHERE item=? AND token=? AND state='preparing'",params![self.item.to_string(),self.token])?;
        if consumed != 1 {
            return Err(WorkItemError::Conflict(
                "preparation capability changed".into(),
            ));
        }
        // The ordinary reserve boundary rejects every preparing row. Consumption is
        // private and transactional: a later failure restores this row on rollback.
        let result = self
            .store
            .reserve(&tx, *item, *expected_version, graph, config)?;
        tx.execute(
            "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
            params![
                self.operation,
                self.body_hash,
                serde_json::to_string(&result)?
            ],
        )?;
        let output = response(&tx, result)?;
        tx.commit()?;
        self.consumed = true;
        Ok(output)
    }
}
impl Drop for StartPreparation {
    fn drop(&mut self) {
        if self.consumed {
            return;
        }
        if let Ok(conn) = self.store.pool.get()
            && let Err(error) = conn.execute("UPDATE work_item_start_preparations SET state='abandoned' WHERE item=? AND token=? AND state='preparing'",params![self.item.to_string(),self.token]) {
                tracing::warn!(%error,"could not abandon Start preparation");
        }
    }
}
fn snapshot(conn: &Connection, item: WorkItemId) -> Result<String> {
    let record = record(conn, item)?;
    let accepted = revision(conn, item, record.accepted_revision)?;
    let prepared: bool = conn.query_row(
        "SELECT workspace_prepared FROM work_items WHERE id=?",
        [item.to_string()],
        |r| r.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT payload FROM work_item_execution_controls WHERE item=? ORDER BY run,generation",
    )?;
    let controls = statement
        .query_map([item.to_string()], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(serde_json::to_string(&(
        record, accepted, prepared, controls,
    ))?)
}
fn guarded_item(command: &WorkItemCommand) -> Option<WorkItemId> {
    match command {
        WorkItemCommand::Start { item, .. }
        | WorkItemCommand::Edit { item, .. }
        | WorkItemCommand::AcceptProposal { item, .. }
        | WorkItemCommand::Archive { item, .. }
        | WorkItemCommand::Suspend { item, .. }
        | WorkItemCommand::Continue { item, .. } => Some(*item),
        _ => None,
    }
}
impl WorkItemStore {
    /// Reserve preparation; callers must check immutable replay before any filesystem work.
    /// Exact committed replay is refused here so it cannot be mistaken for effect authority.
    pub fn begin_start_preparation(&self, command: &WorkItemCommand) -> Result<StartPreparation> {
        if self.replay(command)?.is_some() {
            return Err(WorkItemError::Conflict(
                "Start already admitted; inspect original result".into(),
            ));
        }
        let WorkItemCommand::Start {
            item,
            expected_version,
            operation_id,
            ..
        } = command
        else {
            return Err(WorkItemError::Invalid("preparation requires Start".into()));
        };
        let lock = PreparationLock::acquire_task(&self.home, *item)?;
        lock.verify()?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if self.replay_conn(&tx, command)?.is_some() {
            return Err(WorkItemError::Conflict(
                "Start already admitted; inspect original result".into(),
            ));
        }
        let lock_identity = lock.identity()?;
        verify_stable_identity(&tx, *item, &lock_identity)?;
        let value = record(&tx, *item)?;
        check(&value, *expected_version, true)?;
        let snapshot = snapshot(&tx, *item)?;
        let prepared: bool = tx.query_row(
            "SELECT workspace_prepared FROM work_items WHERE id=?",
            [item.to_string()],
            |r| r.get(0),
        )?;
        let token = ulid::Ulid::new().to_string();
        let operation = operation_id.to_string();
        let body_hash = command_hash(command)?;
        // Holding the SAME persisted stable lock proves any earlier preparing owner exited.
        tx.execute("INSERT INTO work_item_start_preparations(item,token,operation,body_hash,snapshot,lock_identity,state) VALUES(?,?,?,?,?,?,'preparing') ON CONFLICT(item) DO UPDATE SET token=excluded.token,operation=excluded.operation,body_hash=excluded.body_hash,snapshot=excluded.snapshot,lock_identity=excluded.lock_identity,state='preparing'",params![item.to_string(),token,operation,body_hash,snapshot,lock_identity])?;
        lock.verify()?;
        tx.commit()?;
        Ok(StartPreparation {
            store: self.clone(),
            lock,
            item: *item,
            token,
            operation,
            body_hash,
            snapshot,
            lock_identity,
            workspace: value.workspace,
            prepared,
            consumed: false,
        })
    }
    // Acquire outside SQL. A held lock proves dead preparation and keeps reclamation
    // serialized with a concurrent begin until the lifecycle mutation commits.
    pub(super) fn reclaim_start_preparation(
        &self,
        command: &WorkItemCommand,
    ) -> Result<Option<PreparationLock>> {
        let Some(item) = guarded_item(command) else {
            return Ok(None);
        };
        let active: bool = self.pool.get()?.query_row("SELECT EXISTS(SELECT 1 FROM work_item_start_preparations WHERE item=? AND state='preparing')",[item.to_string()],|r|r.get(0))?;
        if !active {
            return Ok(None);
        }
        let lock = PreparationLock::acquire_task(&self.home, item)?;
        lock.verify()?;
        let conn = self.pool.get()?;
        verify_stable_identity(&conn, item, &lock.identity()?)?;
        conn.execute("UPDATE work_item_start_preparations SET state='abandoned' WHERE item=? AND state='preparing'",[item.to_string()])?;
        Ok(Some(lock))
    }
    pub(super) fn check_start_preparation(
        &self,
        conn: &Connection,
        command: &WorkItemCommand,
    ) -> Result<()> {
        let Some(item) = guarded_item(command) else {
            return Ok(());
        };
        let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM work_item_start_preparations WHERE item=? AND state='preparing')",[item.to_string()],|r|r.get(0))?;
        if active {
            Err(WorkItemError::Busy)
        } else {
            Ok(())
        }
    }
}

fn verify_stable_identity(conn: &Connection, item: WorkItemId, identity: &str) -> Result<()> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT lock_identity FROM work_item_start_preparations WHERE item=?",
            [item.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    if stored.as_deref().is_some_and(|stored| stored != identity) {
        return Err(WorkItemError::Conflict(
            "stable Start preparation lock was replaced".into(),
        ));
    }
    Ok(())
}
