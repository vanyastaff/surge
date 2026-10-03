//! Registry-owned persistent tasks. Transactions never span provisioning or engine awaits.
mod control;
pub mod recovery_cycles;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{fs::File, path::PathBuf};
use surge_core::{
    ContentHash, Graph, RunId,
    id::{WorkItemId, WorkItemProjectId},
    work_item::*,
};

/// Task mutation or durable storage failure.
#[derive(Debug, thiserror::Error)]
pub enum WorkItemError {
    /// SQLite operation failed.
    #[error("task storage: {0}")]
    Sql(#[from] rusqlite::Error),
    /// Stored task JSON is malformed.
    #[error("task data: {0}")]
    Json(#[from] serde_json::Error),
    /// Registry connection is unavailable.
    #[error("task pool: {0}")]
    Pool(#[from] r2d2::Error),
    /// Host lock filesystem operation failed.
    #[error("task filesystem: {0}")]
    Io(#[from] std::io::Error),
    /// Requested task or attempt does not exist.
    #[error("task not found")]
    NotFound,
    /// Optimistic or ownership precondition changed.
    #[error("task conflict: {0}")]
    Conflict(String),
    /// Caller or persisted launch input violates the contract.
    #[error("invalid task input: {0}")]
    Invalid(String),
    /// Another host holds launch ownership or admission is full.
    #[error("task launch already claimed")]
    Busy,
}
type Result<T> = std::result::Result<T, WorkItemError>;
#[derive(Serialize, Deserialize)]
enum OperationResult {
    Complete(WorkItemResult),
    Start(RunId),
    Control { run: RunId, generation: u64 },
}
/// Authoritative admission of this exact immutable operation body.
pub enum WorkItemAdmission {
    /// No operation receipt was committed.
    Absent,
    /// The operation was admitted; its current result may still be pending.
    Accepted(WorkItemResult),
    /// This body was refused, or its start was durably rejected and released.
    Rejected(String),
}
/// Registry store; every call releases its database connection before returning.
#[derive(Clone)]
pub struct WorkItemStore {
    pool: Pool<SqliteConnectionManager>,
    home: PathBuf,
}
/// Host-only launch ownership. The OS file lock is retained until this value drops.
#[derive(Clone)]
pub struct WorkItemLaunchClaim {
    // Sharing the open file description keeps one lock alive across both the
    // daemon supervisor and the engine task without creating a new authority.
    lock: std::sync::Arc<File>,
    path: PathBuf,
    run: RunId,
    token: String,
    binding: WorkItemBinding,
}
impl WorkItemLaunchClaim {
    /// Run reserved under this ownership.
    pub fn run(&self) -> RunId {
        self.run
    }
    /// Immutable reservation association.
    pub fn binding(&self) -> &WorkItemBinding {
        &self.binding
    }
}
impl crate::runs::Storage {
    /// Task store using the canonical registry and home.
    pub fn work_items(&self) -> WorkItemStore {
        WorkItemStore {
            pool: self.registry_pool.clone(),
            home: self.home.clone(),
        }
    }
}
fn verify_lock_identity(file: &File, path: &std::path::Path) -> Result<()> {
    let opened = file.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    if !current.is_file() || current.file_type().is_symlink() {
        return Err(WorkItemError::Conflict(
            "launch lock identity changed".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != current.dev() || opened.ino() != current.ino() {
            return Err(WorkItemError::Conflict("launch lock was replaced".into()));
        }
    }
    #[cfg(not(unix))]
    let _ = opened;
    Ok(())
}
fn json<T: serde::de::DeserializeOwned>(text: String) -> rusqlite::Result<T> {
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}
fn record(conn: &Connection, item: WorkItemId) -> Result<WorkItemRecord> {
    conn.query_row("SELECT id,project_id,title,accepted_revision,version,archived_at_ms,workspace,active_run,generation FROM work_items WHERE id=?",[item.to_string()],|row| {
        let parse = |index| -> rusqlite::Result<WorkItemId> { let value:String=row.get(index)?;value.parse().map_err(|error|rusqlite::Error::FromSqlConversionFailure(index,rusqlite::types::Type::Text,Box::new(error))) };
        let project:String=row.get(1)?;
        let run:Option<String>=row.get(7)?;
        Ok(WorkItemRecord { id:parse(0)?,project:project.parse().map_err(|error|rusqlite::Error::FromSqlConversionFailure(1,rusqlite::types::Type::Text,Box::new(error)))?,title:row.get(2)?,accepted_revision:row.get(3)?,version:row.get(4)?,archived_at_ms:row.get(5)?,workspace:json(row.get(6)?)?,active_run:run.map(|value|value.parse()).transpose().map_err(|error|rusqlite::Error::FromSqlConversionFailure(7,rusqlite::types::Type::Text,Box::new(error)))?,generation:row.get(8)? })
    }).optional()?.ok_or(WorkItemError::NotFound)
}
fn revision(conn: &Connection, item: WorkItemId, number: u64) -> Result<WorkItemRevision> {
    Ok(conn.query_row(
        "SELECT payload FROM work_item_revisions WHERE item=? AND revision=?",
        params![item.to_string(), number],
        |row| json(row.get(0)?),
    )?)
}
fn attempt(conn: &Connection, run: RunId) -> Result<WorkItemAttempt> {
    Ok(conn.query_row("SELECT payload,state,usage_seq,input_tokens,output_tokens,known_cost_usd,usage_unknown FROM work_item_attempts WHERE run=?",[run.to_string()],|row| {
        let mut value:WorkItemAttempt=json(row.get(0)?)?;
        let current:u64=conn.query_row("SELECT accepted_revision FROM work_items WHERE id=?",[value.item.to_string()],|row|row.get(0))?;
        value.accepted_revision_relation=if current==value.binding.revision{AcceptedRevisionRelation::MatchesCurrent}else{AcceptedRevisionRelation::Superseded};
        value.state=json(format!("\"{}\"",row.get::<_,String>(1)?))?;
        value.usage_seq=row.get(2)?;value.input_tokens=row.get(3)?;value.output_tokens=row.get(4)?;value.known_cost_usd=row.get(5)?;value.usage_unknown=row.get(6)?;Ok(value)
    })?)
}
fn detail(conn: &Connection, item: WorkItemId) -> Result<WorkItemDetail> {
    let item = record(conn, item)?;
    let pr = conn
        .query_row(
            "SELECT payload FROM work_item_pr WHERE item=?",
            [item.id.to_string()],
            |row| json(row.get(0)?),
        )
        .optional()?;
    let usage=conn.query_row("SELECT COUNT(*),COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0),COALESCE(SUM(known_cost_usd),0),COALESCE(SUM(usage_unknown),0) FROM work_item_attempts WHERE item=?",[item.id.to_string()],|row|Ok(WorkItemUsage{runs:row.get(0)?,input_tokens:row.get(1)?,output_tokens:row.get(2)?,known_cost_usd:row.get(3)?,unknown_runs:row.get(4)?}))?;
    let control = item
        .active_run
        .map(|run| control::read_control(conn, run, None))
        .transpose()?
        .flatten();
    Ok(WorkItemDetail {
        control,
        revision: revision(conn, item.id, item.accepted_revision)?,
        item,
        pr,
        usage,
    })
}
fn save_record(tx: &Transaction<'_>, item: &WorkItemRecord) -> Result<()> {
    tx.execute("UPDATE work_items SET accepted_revision=?,version=?,archived_at_ms=?,active_run=?,generation=? WHERE id=?",params![item.accepted_revision,item.version,item.archived_at_ms,item.active_run.map(|run|run.to_string()),item.generation,item.id.to_string()])?;
    Ok(())
}
fn check(item: &WorkItemRecord, version: u64, inactive: bool) -> Result<()> {
    if item.version != version {
        return Err(WorkItemError::Conflict("stale item version".into()));
    }
    if item.archived_at_ms.is_some() {
        return Err(WorkItemError::Conflict("task archived".into()));
    }
    if inactive && item.active_run.is_some() {
        return Err(WorkItemError::Conflict(
            "task still owns an active attempt".into(),
        ));
    }
    Ok(())
}
fn command_hash(command: &WorkItemCommand) -> Result<String> {
    Ok(ContentHash::compute(&serde_json::to_vec(command)?).to_string())
}
fn response(conn: &Connection, result: OperationResult) -> Result<WorkItemResult> {
    match result {
        OperationResult::Complete(value) => Ok(value),
        OperationResult::Control { run, generation } => Ok(WorkItemResult::Control(Box::new(
            control::read_control(conn, run, Some(generation))?.ok_or(WorkItemError::NotFound)?,
        ))),
        OperationResult::Start(run) => {
            let value = attempt(conn, run)?;
            if value.state == WorkItemAttemptState::Rejected {
                return Err(WorkItemError::Conflict(
                    value.diagnostic.unwrap_or_else(|| "start rejected".into()),
                ));
            }
            Ok(WorkItemResult::Attempt(Box::new(value)))
        },
    }
}
impl WorkItemStore {
    /// Inspect admission without attempting execution or inferring it from error text.
    pub fn operation_admission(&self, command: &WorkItemCommand) -> Result<WorkItemAdmission> {
        let Some(operation) = command.operation_id() else {
            return Ok(WorkItemAdmission::Absent);
        };
        let conn = self.pool.get()?;
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT body_hash,result FROM work_item_operations WHERE operation=?",
                [operation.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((hash, raw)) = row else {
            return Ok(WorkItemAdmission::Absent);
        };
        if hash != command_hash(command)? {
            return Ok(WorkItemAdmission::Rejected(
                "operation identity reused with different body".into(),
            ));
        }
        let result: OperationResult = serde_json::from_str(&raw)?;
        if let OperationResult::Start(run) = &result {
            let value = attempt(&conn, *run)?;
            if value.state == WorkItemAttemptState::Rejected {
                return Ok(WorkItemAdmission::Rejected(
                    value.diagnostic.unwrap_or_else(|| "start rejected".into()),
                ));
            }
        }
        response(&conn, result).map(WorkItemAdmission::Accepted)
    }
    /// Retain the first post-admission diagnostic on the exact current control.
    /// Historical operation replay cannot annotate a newer generation.
    pub fn record_control_diagnostic(
        &self,
        control: &surge_core::execution_recovery::WorkItemExecutionControl,
        diagnostic: &str,
    ) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut current) = control::read_control(&tx, control.run, None)? else {
            return Err(WorkItemError::NotFound);
        };
        if current.generation == control.generation
            && current.operation == control.operation
            && current.diagnostic.is_none()
        {
            current.diagnostic = Some(diagnostic.to_owned());
            control::write_control(&tx, &current)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Persist a recovery refusal only for the still-current owned control generation.
    /// An older failed Continue cannot overwrite a newer manual action.
    pub fn mark_control_recovery_required(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &surge_core::execution_recovery::WorkItemExecutionControl,
        diagnostic: &str,
    ) -> Result<()> {
        verify_lock_identity(&claim.lock, &claim.path)?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let valid: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_item_attempts a JOIN work_items i ON i.id=a.item WHERE a.run=? AND a.generation=? AND a.claim_token=? AND i.active_run=a.run AND i.generation=a.generation AND i.archived_at_ms IS NULL AND a.state IN ('reserved','launched','attention','suspended'))",
            params![claim.run.to_string(), claim.binding.generation, claim.token], |row|row.get(0),
        )?;
        let mut control =
            control::read_control(&tx, claim.run, None)?.ok_or(WorkItemError::NotFound)?;
        if !valid
            || expected.run != claim.run
            || expected.item != claim.binding.item
            || expected.attempt_generation != claim.binding.generation
            || control.item != claim.binding.item
            || control.attempt_generation != claim.binding.generation
            || control.generation != expected.generation
            || control.operation != expected.operation
            || control.state
                != surge_core::execution_recovery::ExecutionControlState::ContinueReserved
        {
            return Err(WorkItemError::Conflict("obsolete recovery refusal".into()));
        }
        let mut value = attempt(&tx, claim.run)?;
        value.state = WorkItemAttemptState::Attention;
        value.diagnostic = Some(diagnostic.to_owned());
        tx.execute("UPDATE work_item_attempts SET state='attention',payload=? WHERE run=? AND generation=? AND claim_token=?",
            params![serde_json::to_string(&value)?,claim.run.to_string(),claim.binding.generation,claim.token])?;
        control.state = surge_core::execution_recovery::ExecutionControlState::Attention;
        control.diagnostic = Some(diagnostic.to_owned());
        control::write_control(&tx, &control)?;
        tx.execute(
            "UPDATE work_items SET version=version+1 WHERE id=? AND active_run=? AND generation=?",
            params![
                claim.binding.item.to_string(),
                claim.run.to_string(),
                claim.binding.generation
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Read accepted context and bounded indexed usage.
    pub fn show(&self, item: WorkItemId) -> Result<WorkItemDetail> {
        detail(&*self.pool.get()?, item)
    }
    /// Look up one explicit run association; legacy runs stay unassociated.
    pub fn for_run(&self, run: RunId) -> Result<Option<WorkItemAttempt>> {
        let conn = self.pool.get()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_item_attempts WHERE run=?)",
            [run.to_string()],
            |row| row.get(0),
        )?;
        if exists {
            Ok(Some(attempt(&conn, run)?))
        } else {
            Ok(None)
        }
    }
    /// Return original mutation result before checking current item version.
    pub fn replay(&self, command: &WorkItemCommand) -> Result<Option<WorkItemResult>> {
        let conn = self.pool.get()?;
        self.replay_conn(&conn, command)
    }
    fn replay_conn(
        &self,
        conn: &Connection,
        command: &WorkItemCommand,
    ) -> Result<Option<WorkItemResult>> {
        let Some(operation) = command.operation_id() else {
            return Ok(None);
        };
        let value: Option<(String, String)> = conn
            .query_row(
                "SELECT body_hash,result FROM work_item_operations WHERE operation=?",
                [operation.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((hash, result)) = value else {
            return Ok(None);
        };
        if hash != command_hash(command)? {
            return Err(WorkItemError::Conflict(
                "operation identity reused with different body".into(),
            ));
        }
        response(conn, serde_json::from_str(&result)?).map(Some)
    }
    /// Apply one mutation. Host creation intent and frozen configuration are never caller-authored workspace metadata.
    pub fn mutate(
        &self,
        command: &WorkItemCommand,
        workspace: Option<&WorkItemWorkspace>,
        config: Option<&str>,
        actor: &str,
        now: i64,
    ) -> Result<WorkItemResult> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(result) = self.replay_conn(&tx, command)? {
            return Ok(result);
        }
        let result = match command {
            WorkItemCommand::Create {
                operation_id,
                title,
                requirements,
                ..
            } => Self::create(
                &tx,
                *operation_id,
                title,
                requirements,
                workspace,
                actor,
                now,
            )?,
            WorkItemCommand::Start {
                item,
                expected_version,
                graph,
                ..
            } => self.reserve(
                &tx,
                *item,
                *expected_version,
                graph,
                config.ok_or_else(|| WorkItemError::Invalid("missing frozen config".into()))?,
            )?,
            WorkItemCommand::Suspend { .. } | WorkItemCommand::Continue { .. } => {
                self.request_control(&tx, command)?
            },
            _ => self.change(&tx, command, actor, now)?,
        };
        let operation = command
            .operation_id()
            .ok_or_else(|| WorkItemError::Invalid("query is not mutation".into()))?;
        tx.execute(
            "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
            params![
                operation.to_string(),
                command_hash(command)?,
                serde_json::to_string(&result)?
            ],
        )?;
        let output = response(&tx, result)?;
        tx.commit()?;
        Ok(output)
    }
    fn create(
        tx: &Transaction<'_>,
        operation: surge_core::id::WorkItemOperationId,
        title: &str,
        requirements: &WorkItemRequirements,
        workspace: Option<&WorkItemWorkspace>,
        actor: &str,
        now: i64,
    ) -> Result<OperationResult> {
        if title.trim().is_empty() || title.len() > 1024 {
            return Err(WorkItemError::Invalid(
                "task title empty or oversized".into(),
            ));
        }
        let workspace = workspace
            .ok_or_else(|| WorkItemError::Invalid("host workspace intent missing".into()))?;
        let repository = workspace.repository.to_string_lossy();
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM work_item_projects WHERE repository=?",
                [repository.as_ref()],
                |row| row.get(0),
            )
            .optional()?;
        let project = existing.map_or_else(
            || Ok(WorkItemProjectId::new()),
            |value| {
                value
                    .parse()
                    .map_err(|_| WorkItemError::Invalid("invalid project ID".into()))
            },
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO work_item_projects(id,repository,checkout) VALUES(?,?,?)",
            params![
                project.to_string(),
                repository.as_ref(),
                workspace.checkout.to_string_lossy()
            ],
        )?;
        let item: WorkItemId = operation
            .as_ulid()
            .to_string()
            .parse()
            .map_err(|_| WorkItemError::Invalid("operation ID".into()))?;
        tx.execute("INSERT INTO work_items(id,project_id,title,accepted_revision,version,workspace) VALUES(?,?,?,1,1,?)",params![item.to_string(),project.to_string(),title,serde_json::to_string(workspace)?])?;
        let revision = WorkItemRevision {
            revision: 1,
            requirements: requirements.clone(),
            hash: requirements.hash()?,
            actor: actor.into(),
            accepted_proposal: None,
            accepted_at_ms: now,
        };
        tx.execute(
            "INSERT INTO work_item_revisions(item,revision,payload) VALUES(?,1,?)",
            params![item.to_string(), serde_json::to_string(&revision)?],
        )?;
        Ok(OperationResult::Complete(WorkItemResult::Detail(Box::new(
            detail(tx, item)?,
        ))))
    }
    fn reserve(
        &self,
        tx: &Transaction<'_>,
        id: WorkItemId,
        version: u64,
        graph: &Graph,
        config: &str,
    ) -> Result<OperationResult> {
        let mut item = record(tx, id)?;
        check(&item, version, true)?;
        let ordinal: u64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal),0)+1 FROM work_item_attempts WHERE item=?",
            [id.to_string()],
            |row| row.get(0),
        )?;
        let accepted = revision(tx, id, item.accepted_revision)?;
        let run = RunId::new();
        item.generation += 1;
        item.active_run = Some(run);
        item.version += 1;
        let binding = WorkItemBinding {
            item: id,
            revision: accepted.revision,
            requirements_hash: accepted.hash,
            generation: item.generation,
        };
        let value = WorkItemAttempt {
            item: id,
            run,
            ordinal,
            binding,
            accepted_revision_relation: AcceptedRevisionRelation::MatchesCurrent,
            graph: Box::new(graph.clone()),
            config: config.into(),
            state: WorkItemAttemptState::Reserved,
            diagnostic: None,
            usage_seq: 0,
            input_tokens: 0,
            output_tokens: 0,
            known_cost_usd: 0.0,
            usage_unknown: true,
        };
        tx.execute("INSERT INTO work_item_attempts(run,item,ordinal,generation,state,payload) VALUES(?,?,?,?,'reserved',?)",params![run.to_string(),id.to_string(),ordinal,item.generation,serde_json::to_string(&value)?])?;
        save_record(tx, &item)?;
        Ok(OperationResult::Start(run))
    }
    fn change(
        &self,
        tx: &Transaction<'_>,
        command: &WorkItemCommand,
        actor: &str,
        now: i64,
    ) -> Result<OperationResult> {
        let (id, version, inactive) = match command {
            WorkItemCommand::Edit {
                item,
                expected_version,
                ..
            }
            | WorkItemCommand::AcceptProposal {
                item,
                expected_version,
                ..
            }
            | WorkItemCommand::Archive {
                item,
                expected_version,
                ..
            } => (*item, *expected_version, true),
            WorkItemCommand::Discuss {
                item,
                expected_version,
                ..
            }
            | WorkItemCommand::AttachPr {
                item,
                expected_version,
                ..
            } => (*item, *expected_version, false),
            _ => return Err(WorkItemError::Invalid("unsupported mutation".into())),
        };
        let mut item = record(tx, id)?;
        check(&item, version, inactive)?;
        match command {
            WorkItemCommand::Edit {
                expected_revision,
                requirements,
                ..
            } => accept(
                tx,
                &mut item,
                *expected_revision,
                requirements,
                actor,
                now,
                None,
            )?,
            WorkItemCommand::AcceptProposal {
                expected_revision,
                proposal,
                ..
            } => {
                let entry: WorkItemDiscussion = tx.query_row(
                    "SELECT payload FROM work_item_discussion WHERE item=? AND sequence=?",
                    params![id.to_string(), proposal],
                    |row| json(row.get(0)?),
                )?;
                let requirements = entry.proposal.ok_or_else(|| {
                    WorkItemError::Invalid("discussion is not a revision proposal".into())
                })?;
                accept(
                    tx,
                    &mut item,
                    *expected_revision,
                    &requirements,
                    actor,
                    now,
                    Some(*proposal),
                )?;
            },
            WorkItemCommand::Archive { .. } => item.archived_at_ms = Some(now),
            WorkItemCommand::Discuss { body, proposal, .. } => {
                if body.trim().is_empty() || body.len() > 131_072 {
                    return Err(WorkItemError::Invalid("discussion empty/oversized".into()));
                }
                let sequence: u64 = tx.query_row(
                    "SELECT COALESCE(MAX(sequence),0)+1 FROM work_item_discussion WHERE item=?",
                    [id.to_string()],
                    |row| row.get(0),
                )?;
                let entry = WorkItemDiscussion {
                    sequence,
                    actor: actor.into(),
                    body: body.clone(),
                    proposal: proposal.clone(),
                    created_at_ms: now,
                };
                tx.execute(
                    "INSERT INTO work_item_discussion(item,sequence,payload) VALUES(?,?,?)",
                    params![id.to_string(), sequence, serde_json::to_string(&entry)?],
                )?;
            },
            WorkItemCommand::AttachPr { pr, .. } => {
                pr.validate().map_err(WorkItemError::Invalid)?;
                let existing: Option<WorkItemPr> = tx
                    .query_row(
                        "SELECT payload FROM work_item_pr WHERE item=?",
                        [id.to_string()],
                        |row| json(row.get(0)?),
                    )
                    .optional()?;
                if existing.as_ref().is_some_and(|value| value != pr) {
                    return Err(WorkItemError::Conflict(
                        "task already owns a different PR".into(),
                    ));
                }
                tx.execute(
                    "INSERT OR IGNORE INTO work_item_pr(item,payload) VALUES(?,?)",
                    params![id.to_string(), serde_json::to_string(pr)?],
                )?;
            },
            _ => return Err(WorkItemError::Invalid("unsupported change".into())),
        }
        item.version += 1;
        save_record(tx, &item)?;
        Ok(OperationResult::Complete(WorkItemResult::Detail(Box::new(
            detail(tx, id)?,
        ))))
    }
    /// Obtain a stable OS lock and a SQL claimant token for exactly this generation.
    pub fn claim(&self, run: RunId) -> Result<WorkItemLaunchClaim> {
        let locks = self.home.join("work-items/locks");
        std::fs::create_dir_all(&locks)?;
        let path = locks.join(format!("{run}.lock"));
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(WorkItemError::Invalid(
                "launch lock cannot be a symlink".into(),
            ));
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => WorkItemError::Busy,
            std::fs::TryLockError::Error(error) => WorkItemError::Io(error),
        })?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = attempt(&tx, run)?;
        let item = record(&tx, value.item)?;
        if !value.state.is_active()
            || item.active_run != Some(run)
            || item.generation != value.binding.generation
        {
            return Err(WorkItemError::Conflict(
                "reservation ownership changed".into(),
            ));
        }
        let token = RunId::new().to_string();
        tx.execute(
            "UPDATE work_item_attempts SET claim_token=? WHERE run=? AND generation=?",
            params![token, run.to_string(), item.generation],
        )?;
        tx.commit()?;
        verify_lock_identity(&lock, &path)?;
        Ok(WorkItemLaunchClaim {
            lock: std::sync::Arc::new(lock),
            path,
            run,
            token,
            binding: value.binding,
        })
    }
    /// Validate host ownership at every engine boundary, never caller configuration.
    pub fn validate_claim(&self, claim: &WorkItemLaunchClaim) -> Result<WorkItemAttempt> {
        verify_lock_identity(&claim.lock, &claim.path)?;
        let conn = self.pool.get()?;
        let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM work_item_attempts a JOIN work_items i ON i.id=a.item WHERE a.run=? AND a.generation=? AND a.claim_token=? AND i.active_run=a.run AND i.generation=a.generation AND i.archived_at_ms IS NULL AND a.state IN ('reserved','launched','attention','suspended'))",params![claim.run.to_string(),claim.binding.generation,claim.token],|row|row.get(0))?;
        if !valid {
            return Err(WorkItemError::Conflict("stale launch claim".into()));
        }
        attempt(&conn, claim.run)
    }
    /// Settle only this run's historical attempt; ownership release is run/generation scoped.
    pub fn settle(
        &self,
        run: RunId,
        generation: u64,
        state: WorkItemAttemptState,
        diagnostic: Option<String>,
    ) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut value = attempt(&tx, run)?;
        if value.binding.generation != generation {
            return Err(WorkItemError::Conflict("wrong generation".into()));
        }
        if !value.state.is_active() {
            return Ok(());
        }
        value.state = state;
        value.diagnostic = diagnostic;
        let state_text = serde_json::to_string(&state)?;
        let state_text = state_text.trim_matches('"');
        tx.execute(
            "UPDATE work_item_attempts SET state=?,payload=? WHERE run=? AND generation=?",
            params![
                state_text,
                serde_json::to_string(&value)?,
                run.to_string(),
                generation
            ],
        )?;
        if let Some(mut control) = control::read_control(&tx, run, None)? {
            if !state.is_active() {
                control.state = surge_core::execution_recovery::ExecutionControlState::Terminal;
                control::write_control(&tx, &control)?;
            } else if state == WorkItemAttemptState::Attention {
                control.state = surge_core::execution_recovery::ExecutionControlState::Attention;
                control::write_control(&tx, &control)?;
            }
        }
        if !state.is_active() {
            tx.execute("UPDATE work_items SET active_run=NULL,version=version+1 WHERE id=? AND active_run=? AND generation=?",params![value.item.to_string(),run.to_string(),generation])?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Read immutable requirements of an explicit attempt.
    pub fn requirements(&self, value: &WorkItemAttempt) -> Result<WorkItemRevision> {
        revision(&*self.pool.get()?, value.item, value.binding.revision)
    }
}
fn accept(
    tx: &Transaction<'_>,
    item: &mut WorkItemRecord,
    expected: u64,
    requirements: &WorkItemRequirements,
    actor: &str,
    now: i64,
    proposal: Option<u64>,
) -> Result<()> {
    if item.accepted_revision != expected {
        return Err(WorkItemError::Conflict("stale accepted revision".into()));
    }
    item.accepted_revision += 1;
    let revision = WorkItemRevision {
        revision: item.accepted_revision,
        requirements: requirements.clone(),
        hash: requirements.hash()?,
        actor: actor.into(),
        accepted_proposal: proposal,
        accepted_at_ms: now,
    };
    tx.execute(
        "INSERT INTO work_item_revisions(item,revision,payload) VALUES(?,?,?)",
        params![
            item.id.to_string(),
            revision.revision,
            serde_json::to_string(&revision)?
        ],
    )?;
    Ok(())
}

/// Maximum rows returned by a cold task operator page.
pub const MAX_PAGE_SIZE: u32 = 100;
fn page_size(limit: u32) -> Result<u32> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        Err(WorkItemError::Invalid(format!(
            "page limit must be 1..={MAX_PAGE_SIZE}"
        )))
    } else {
        Ok(limit)
    }
}
fn finish_page<T>(
    mut entries: Vec<T>,
    limit: u32,
    cursor: impl Fn(&T) -> String,
) -> WorkItemPage<T> {
    let next_cursor = if entries.len() > limit as usize {
        entries.pop();
        entries.last().map(cursor)
    } else {
        None
    };
    WorkItemPage {
        entries,
        next_cursor,
    }
}
impl WorkItemStore {
    /// Bounded, stable pages. History inspection never reads a run journal.
    pub fn query(&self, command: &WorkItemCommand) -> Result<WorkItemResult> {
        let conn = self.pool.get()?;
        match command {
            WorkItemCommand::Show { item } => {
                Ok(WorkItemResult::Detail(Box::new(detail(&conn, *item)?)))
            },
            WorkItemCommand::List { after, limit } => {
                let limit = page_size(*limit)?;
                let after = after
                    .as_deref()
                    .map(|value| {
                        value
                            .parse::<WorkItemId>()
                            .map(|id| id.to_string())
                            .map_err(|_| WorkItemError::Invalid("invalid task cursor".into()))
                    })
                    .transpose()?
                    .unwrap_or_default();
                let mut stmt =
                    conn.prepare("SELECT id FROM work_items WHERE id>? ORDER BY id LIMIT ?")?;
                let ids = stmt
                    .query_map(params![after, limit + 1], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let entries = ids
                    .into_iter()
                    .map(|id| {
                        record(
                            &conn,
                            id.parse()
                                .map_err(|_| WorkItemError::Invalid("invalid task ID".into()))?,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(WorkItemResult::Items(finish_page(entries, limit, |item| {
                    item.id.to_string()
                })))
            },
            WorkItemCommand::Revisions { item, after, limit } => {
                let entries = self.history::<WorkItemRevision>(
                    &conn,
                    *item,
                    "work_item_revisions",
                    "revision",
                    after,
                    *limit,
                )?;
                Ok(WorkItemResult::Revisions(finish_page(
                    entries,
                    *limit,
                    |entry| entry.revision.to_string(),
                )))
            },
            WorkItemCommand::Discussion { item, after, limit } => {
                let entries = self.history::<WorkItemDiscussion>(
                    &conn,
                    *item,
                    "work_item_discussion",
                    "sequence",
                    after,
                    *limit,
                )?;
                Ok(WorkItemResult::Discussion(finish_page(
                    entries,
                    *limit,
                    |entry| entry.sequence.to_string(),
                )))
            },
            WorkItemCommand::Attempts { item, after, limit } => {
                let limit = page_size(*limit)?;
                record(&conn, *item)?;
                let after = number_cursor(after)?;
                let mut stmt=conn.prepare("SELECT run FROM work_item_attempts WHERE item=? AND ordinal>? ORDER BY ordinal LIMIT ?")?;
                let runs = stmt
                    .query_map(params![item.to_string(), after, limit + 1], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let entries = runs
                    .into_iter()
                    .map(|run| {
                        attempt(
                            &conn,
                            run.parse()
                                .map_err(|_| WorkItemError::Invalid("invalid run ID".into()))?,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(WorkItemResult::Attempts(finish_page(
                    entries,
                    limit,
                    |entry| entry.ordinal.to_string(),
                )))
            },
            _ => Err(WorkItemError::Invalid("mutation is not query".into())),
        }
    }
    fn history<T: serde::de::DeserializeOwned>(
        &self,
        conn: &Connection,
        item: WorkItemId,
        table: &str,
        column: &str,
        after: &Option<String>,
        limit: u32,
    ) -> Result<Vec<T>> {
        let limit = page_size(limit)?;
        record(conn, item)?;
        let after = number_cursor(after)?;
        // Identifiers are fixed private call-site literals, never client strings.
        let sql = format!(
            "SELECT payload FROM {table} WHERE item=? AND {column}>? ORDER BY {column} LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map(params![item.to_string(), after, limit + 1], |row| {
                json(row.get(0)?)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    /// Bounded global sweep cursor, for reconciliation and incremental usage.
    pub fn scan_attempts(
        &self,
        after: Option<RunId>,
        limit: u32,
    ) -> Result<WorkItemPage<WorkItemAttempt>> {
        let limit = page_size(limit)?;
        let conn = self.pool.get()?;
        let mut stmt =
            conn.prepare("SELECT run FROM work_item_attempts WHERE run>? ORDER BY run LIMIT ?")?;
        let runs = stmt
            .query_map(
                params![
                    after.map_or_else(String::new, |id| id.to_string()),
                    limit + 1
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let entries = runs
            .into_iter()
            .map(|run| {
                attempt(
                    &conn,
                    run.parse()
                        .map_err(|_| WorkItemError::Invalid("invalid run ID".into()))?,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(finish_page(entries, limit, |entry| entry.run.to_string()))
    }
    /// Incrementally sync at most 256 durable events. Cursor CAS prevents repeat charging.
    pub fn sync_usage(&self, run: RunId) -> Result<()> {
        let value = self.for_run(run)?.ok_or(WorkItemError::NotFound)?;
        let db = self
            .home
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite");
        let observed = self.read_usage(&db, &value);
        let conn = self.pool.get()?;
        match observed {
            Ok((seq, input, output, cost, unpriced, unknown)) => {
                conn.execute("UPDATE work_item_attempts SET usage_seq=?,input_tokens=input_tokens+?,output_tokens=output_tokens+?,known_cost_usd=known_cost_usd+?,usage_unpriced=?,usage_unknown=? WHERE run=? AND usage_seq=?",params![seq,input,output,cost,unpriced,unknown,run.to_string(),value.usage_seq])?;
                Ok(())
            },
            Err(error) => {
                conn.execute(
                    "UPDATE work_item_attempts SET usage_unknown=1 WHERE run=?",
                    [run.to_string()],
                )?;
                Err(error)
            },
        }
    }
    fn read_usage(
        &self,
        db: &std::path::Path,
        value: &WorkItemAttempt,
    ) -> Result<(u64, u64, u64, f64, bool, bool)> {
        let conn = Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let mut binding_statement=conn.prepare("SELECT payload,schema_version FROM events WHERE kind='WorkItemAttemptBound' ORDER BY seq LIMIT 2")?;
        let bindings = binding_statement
            .query_map([], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, u32>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if bindings.len() != 1 {
            return Err(WorkItemError::Invalid(
                "usage journal lacks unique task binding".into(),
            ));
        }
        let payload = surge_core::migrate_payload(bindings[0].1, &bindings[0].0)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        if !matches!(payload,surge_core::EventPayload::WorkItemAttemptBound{context} if context.binding()==&value.binding)
        {
            return Err(WorkItemError::Invalid(
                "usage journal binding mismatch".into(),
            ));
        }
        let mut stmt = conn.prepare(
            "SELECT seq,payload,schema_version FROM events WHERE seq>? ORDER BY seq LIMIT 257",
        )?;
        let rows = stmt
            .query_map([value.usage_seq], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut seq = value.usage_seq;
        let mut input = 0;
        let mut output = 0;
        let mut cost = 0.0;
        let mut unpriced: bool = self.pool.get()?.query_row(
            "SELECT usage_unpriced FROM work_item_attempts WHERE run=?",
            [value.run.to_string()],
            |row| row.get(0),
        )?;
        for (index, (next, blob, version)) in rows.iter().take(256).enumerate() {
            if *next != seq + 1 {
                return Err(WorkItemError::Invalid("usage history discontinuity".into()));
            }
            let payload = surge_core::migrate_payload(*version, blob)
                .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
            if value.usage_seq == 0
                && index == 0
                && !matches!(payload, surge_core::EventPayload::RunStarted { .. })
            {
                return Err(WorkItemError::Invalid(
                    "usage history has no trusted origin".into(),
                ));
            }
            if let surge_core::EventPayload::TokensConsumed {
                prompt_tokens,
                output_tokens,
                cost_usd,
                ..
            } = payload
            {
                input += u64::from(prompt_tokens);
                output += u64::from(output_tokens);
                if let Some(amount) = cost_usd.filter(|amount| amount.is_finite() && *amount >= 0.0)
                {
                    cost += amount;
                } else {
                    unpriced = true;
                }
            }
            seq = *next;
        }
        Ok((
            seq,
            input,
            output,
            cost,
            unpriced,
            unpriced || rows.len() > 256 || seq == 0,
        ))
    }
}
fn number_cursor(after: &Option<String>) -> Result<u64> {
    after.as_deref().map_or(Ok(0), |value| {
        value
            .parse()
            .map_err(|_| WorkItemError::Invalid("invalid history cursor".into()))
    })
}
impl WorkItemStore {
    /// Whether provisioning was acknowledged; loss after acknowledgment must never recreate.
    pub fn workspace_prepared(&self, item: WorkItemId) -> Result<bool> {
        Ok(self.pool.get()?.query_row(
            "SELECT workspace_prepared FROM work_items WHERE id=?",
            [item.to_string()],
            |row| row.get(0),
        )?)
    }
    /// Acknowledge successful registered provisioning under the same host claim.
    pub fn mark_workspace_prepared(&self, claim: &WorkItemLaunchClaim) -> Result<()> {
        self.validate_claim(claim)?;
        let changed=self.pool.get()?.execute("UPDATE work_items SET workspace_prepared=1 WHERE id=? AND active_run=? AND generation=? AND EXISTS(SELECT 1 FROM work_item_attempts WHERE run=? AND claim_token=?)",params![claim.binding.item.to_string(),claim.run.to_string(),claim.binding.generation,claim.run.to_string(),claim.token])?;
        if changed != 1 {
            return Err(WorkItemError::Conflict(
                "provisioning acknowledgment lost ownership".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::WorkItemOperationId;
    async fn fixture() -> (
        tempfile::TempDir,
        std::sync::Arc<crate::runs::Storage>,
        WorkItemDetail,
    ) {
        let home = tempfile::tempdir().unwrap();
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let intent = WorkItemWorkspace {
            repository: home.path().join("repo/.git"),
            checkout: home.path().join("repo"),
            path: home.path().join("retained"),
            ownership: RunId::new().to_string(),
            branch: "retained".into(),
            base_commit: "a".repeat(40),
        };
        let command = WorkItemCommand::Create {
            operation_id: WorkItemOperationId::new(),
            project: intent.checkout.clone(),
            title: "Acceptance".into(),
            requirements: req("Original"),
        };
        let WorkItemResult::Detail(detail) = storage
            .work_items()
            .mutate(&command, Some(&intent), None, "human", 1)
            .unwrap()
        else {
            panic!("detail")
        };
        (home, storage, *detail)
    }
    fn req(text: &str) -> WorkItemRequirements {
        WorkItemRequirements::new(text.into(), vec!["Fixed acceptance".into()]).unwrap()
    }
    fn start(item: &WorkItemRecord) -> WorkItemCommand {
        WorkItemCommand::Start {
            operation_id: WorkItemOperationId::new(),
            item: item.id,
            expected_version: item.version,
            graph: Box::new(
                toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap(),
            ),
            quota_recovery: None,
        }
    }
    fn reserve(store: &WorkItemStore, item: &WorkItemRecord) -> WorkItemAttempt {
        let WorkItemResult::Attempt(attempt) = store
            .mutate(&start(item), None, Some("{}"), "host", 2)
            .unwrap()
        else {
            panic!("attempt")
        };
        *attempt
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_revision_is_immutable_and_replay_precedes_stale_cas() {
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        let edit = WorkItemCommand::Edit {
            operation_id: WorkItemOperationId::new(),
            item: created.item.id,
            expected_version: created.item.version,
            expected_revision: 1,
            requirements: req("Accepted amendment"),
        };
        let edited = store.mutate(&edit, None, None, "human", 2).unwrap();
        assert_eq!(
            serde_json::to_value(&edited).unwrap(),
            serde_json::to_value(store.mutate(&edit, None, None, "human", 3).unwrap()).unwrap()
        );
        assert_eq!(
            store
                .requirements(&WorkItemAttempt {
                    binding: WorkItemBinding {
                        item: created.item.id,
                        revision: 1,
                        requirements_hash: created.revision.hash,
                        generation: 1
                    },
                    ..reserve(&store, &store.show(created.item.id).unwrap().item)
                })
                .unwrap()
                .requirements
                .text(),
            "Original"
        );
        let changed = match edit {
            WorkItemCommand::Edit {
                operation_id,
                item,
                expected_version,
                expected_revision,
                ..
            } => WorkItemCommand::Edit {
                operation_id,
                item,
                expected_version,
                expected_revision,
                requirements: req("Different"),
            },
            _ => unreachable!(),
        };
        assert!(matches!(
            store.replay(&changed),
            Err(WorkItemError::Conflict(_))
        ));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn active_attempt_blocks_acceptance_and_archive_but_allows_discussion() {
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        let attempt = reserve(&store, &created.item);
        let current = store.show(created.item.id).unwrap();
        assert!(
            store
                .mutate(
                    &WorkItemCommand::Archive {
                        operation_id: WorkItemOperationId::new(),
                        item: current.item.id,
                        expected_version: current.item.version
                    },
                    None,
                    None,
                    "human",
                    2
                )
                .is_err()
        );
        assert!(
            store
                .mutate(
                    &WorkItemCommand::Edit {
                        operation_id: WorkItemOperationId::new(),
                        item: current.item.id,
                        expected_version: current.item.version,
                        expected_revision: 1,
                        requirements: req("wrong")
                    },
                    None,
                    None,
                    "human",
                    2
                )
                .is_err()
        );
        store
            .mutate(
                &WorkItemCommand::Discuss {
                    operation_id: WorkItemOperationId::new(),
                    item: current.item.id,
                    expected_version: current.item.version,
                    body: "Proposed only".into(),
                    proposal: Some(req("Proposed")),
                },
                None,
                None,
                "agent",
                2,
            )
            .unwrap();
        assert_eq!(
            store.requirements(&attempt).unwrap().requirements.text(),
            "Original"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lock_fences_second_host_and_reacquire_invalidates_old_sql_claim() {
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        let attempt = reserve(&store, &created.item);
        let claim = store.claim(attempt.run).unwrap();
        assert!(matches!(store.claim(attempt.run), Err(WorkItemError::Busy)));
        store.validate_claim(&claim).unwrap();
        drop(claim);
        let second = store.claim(attempt.run).unwrap();
        store.validate_claim(&second).unwrap();
        assert!(
            store
                .home
                .join("work-items/locks")
                .join(format!("{}.lock", attempt.run))
                .exists()
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_launch_owner_keeps_os_lock_until_last_clone_drops() {
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        let attempt = reserve(&store, &created.item);
        let supervisor = store.claim(attempt.run).unwrap();
        let task = supervisor.clone();
        drop(supervisor);
        store.validate_claim(&task).unwrap();
        assert!(matches!(store.claim(attempt.run), Err(WorkItemError::Busy)));
        drop(task);
        let replacement = store.claim(attempt.run).unwrap();
        store.validate_claim(&replacement).unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interprocess_launch_lock_blocks_other_host_and_releases_on_exit() {
        let (home, storage, created) = fixture().await;
        let store = storage.work_items();
        let attempt = reserve(&store, &created.item);
        let claim = store.claim(attempt.run).unwrap();
        let run_probe = |expected: &str| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "work_items::tests::cross_process_claim_probe",
                    "--nocapture",
                ])
                .env("SURGE_TEST_LOCK_HOME", home.path())
                .env("SURGE_TEST_LOCK_RUN", attempt.run.to_string())
                .env("SURGE_TEST_LOCK_EXPECT", expected)
                .output()
                .unwrap()
        };
        let blocked = run_probe("busy");
        assert!(
            blocked.status.success(),
            "{}",
            String::from_utf8_lossy(&blocked.stderr)
        );
        drop(claim);
        let released = run_probe("free");
        assert!(
            released.status.success(),
            "{}",
            String::from_utf8_lossy(&released.stderr)
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cross_process_claim_probe() {
        let Some(home) = std::env::var_os("SURGE_TEST_LOCK_HOME") else {
            return;
        };
        let storage = crate::runs::Storage::open(std::path::PathBuf::from(home))
            .await
            .unwrap();
        let run = std::env::var("SURGE_TEST_LOCK_RUN")
            .unwrap()
            .parse()
            .unwrap();
        let claim = storage.work_items().claim(run);
        if std::env::var("SURGE_TEST_LOCK_EXPECT").unwrap() == "busy" {
            assert!(matches!(claim, Err(WorkItemError::Busy)));
        } else {
            assert!(claim.is_ok());
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn old_terminal_settlement_cannot_release_new_attempt_and_workspace_is_retained() {
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        let old = reserve(&store, &created.item);
        std::fs::create_dir_all(&created.item.workspace.path).unwrap();
        std::fs::write(created.item.workspace.path.join("untracked"), "keep").unwrap();
        store
            .settle(
                old.run,
                old.binding.generation,
                WorkItemAttemptState::Completed,
                None,
            )
            .unwrap();
        let next = reserve(&store, &store.show(created.item.id).unwrap().item);
        store
            .settle(
                old.run,
                old.binding.generation,
                WorkItemAttemptState::Failed,
                None,
            )
            .unwrap();
        assert_eq!(
            store.show(created.item.id).unwrap().item.active_run,
            Some(next.run)
        );
        assert_eq!(next.ordinal, 2);
        assert_eq!(next.binding.revision, 1);
        store
            .settle(
                next.run,
                next.binding.generation,
                WorkItemAttemptState::Aborted,
                None,
            )
            .unwrap();
        let item = store.show(created.item.id).unwrap().item;
        store
            .mutate(
                &WorkItemCommand::Archive {
                    operation_id: WorkItemOperationId::new(),
                    item: item.id,
                    expected_version: item.version,
                },
                None,
                None,
                "human",
                4,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(item.workspace.path.join("untracked")).unwrap(),
            "keep"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn histories_have_stable_bounded_cursors_and_no_guess_backfill() {
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        for _ in 0..205 {
            let current = store.show(created.item.id).unwrap().item;
            store
                .mutate(
                    &WorkItemCommand::Discuss {
                        operation_id: WorkItemOperationId::new(),
                        item: current.id,
                        expected_version: current.version,
                        body: "Discussion".into(),
                        proposal: None,
                    },
                    None,
                    None,
                    "human",
                    2,
                )
                .unwrap();
        }
        let mut after = None;
        let mut seen = vec![];
        loop {
            let WorkItemResult::Discussion(page) = store
                .query(&WorkItemCommand::Discussion {
                    item: created.item.id,
                    after,
                    limit: 2,
                })
                .unwrap()
            else {
                panic!("page")
            };
            assert!(page.entries.len() <= 2);
            seen.extend(page.entries.iter().map(|row| row.sequence));
            after = page.next_cursor;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(seen, (1..=205).collect::<Vec<_>>());
        assert!(
            store
                .query(&WorkItemCommand::List {
                    after: None,
                    limit: 101
                })
                .is_err()
        );
        assert!(store.for_run(RunId::new()).unwrap().is_none());
        assert_eq!(store.show(created.item.id).unwrap().usage.unknown_runs, 0);
        reserve(&store, &store.show(created.item.id).unwrap().item);
        assert_eq!(store.show(created.item.id).unwrap().usage.unknown_runs, 1);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suspension_intent_does_not_authorize_continue_without_confirmed_journal_fence() {
        use surge_core::execution_recovery::ExecutionControlState;
        let (_home, storage, created) = fixture().await;
        let store = storage.work_items();
        let original = reserve(&store, &created.item);
        let request = WorkItemCommand::Suspend {
            operation_id: WorkItemOperationId::new(),
            item: created.item.id,
            expected_version: store.show(created.item.id).unwrap().item.version,
        };
        let result = store.mutate(&request, None, None, "operator", 3).unwrap();
        let WorkItemResult::Control(control) = result else {
            panic!("durable suspend intent")
        };
        assert_eq!(control.state, ExecutionControlState::SuspendRequested);
        assert_eq!(control.run, original.run);
        assert!(control.fence.is_none());
        let continued = WorkItemCommand::Continue {
            operation_id: WorkItemOperationId::new(),
            item: created.item.id,
            expected_version: store.show(created.item.id).unwrap().item.version,
            new_session: false,
        };
        assert!(store.mutate(&continued, None, None, "operator", 4).is_err());
        let WorkItemResult::Control(replayed) = store.replay(&request).unwrap().unwrap() else {
            panic!("replay")
        };
        assert_eq!(*replayed, *control);
        assert_eq!(
            store.show(created.item.id).unwrap().item.active_run,
            Some(original.run)
        );
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use surge_core::{
        EventPayload, SessionId, VersionedEventPayload, approvals::ApprovalPolicy,
        id::WorkItemOperationId, run_event::RunConfig, sandbox::SandboxMode,
    };
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn durable_usage_pages_are_charged_once_and_unknown_pricing_stays_unknown() {
        let home = tempfile::tempdir().unwrap();
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let requirements =
            WorkItemRequirements::new("Cost oracle".into(), vec!["No double charge".into()])
                .unwrap();
        let workspace = WorkItemWorkspace {
            repository: home.path().join(".git"),
            checkout: home.path().into(),
            path: home.path().join("retained"),
            ownership: RunId::new().to_string(),
            branch: "cost".into(),
            base_commit: "a".repeat(40),
        };
        let create = WorkItemCommand::Create {
            operation_id: WorkItemOperationId::new(),
            project: home.path().into(),
            title: "Cost".into(),
            requirements: requirements.clone(),
        };
        let WorkItemResult::Detail(detail) = store
            .mutate(&create, Some(&workspace), None, "human", 1)
            .unwrap()
        else {
            panic!("detail")
        };
        let graph: Graph =
            toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
        let start = WorkItemCommand::Start {
            operation_id: WorkItemOperationId::new(),
            item: detail.item.id,
            expected_version: 1,
            graph: Box::new(graph),
            quota_recovery: None,
        };
        let WorkItemResult::Attempt(attempt) =
            store.mutate(&start, None, Some("{}"), "host", 2).unwrap()
        else {
            panic!("attempt")
        };
        assert!(store.sync_usage(attempt.run).is_err());
        assert_eq!(store.show(detail.item.id).unwrap().usage.unknown_runs, 1);
        let writer = storage
            .create_run(attempt.run, &workspace.path, None)
            .await
            .unwrap();
        let mut events = vec![
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: workspace.path.clone(),
                initial_prompt: "cost".into(),
                config: RunConfig {
                    bootstrap_edit_loop_cap: None,
                    budget: Default::default(),
                    sandbox_default: SandboxMode::WorkspaceWrite,
                    approval_default: ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: vec![],
                },
            },
            EventPayload::WorkItemAttemptBound {
                context: WorkItemContext::new(attempt.binding.clone(), requirements).unwrap(),
            },
        ];
        let session = SessionId::new();
        for _ in 0..300 {
            events.push(EventPayload::TokensConsumed {
                session,
                prompt_tokens: 2,
                output_tokens: 3,
                cache_hits: 0,
                model: "fixture".into(),
                cost_usd: Some(0.5),
            });
        }
        writer
            .append_events(events.into_iter().map(VersionedEventPayload::new).collect())
            .await
            .unwrap();
        store.sync_usage(attempt.run).unwrap();
        assert_eq!(store.show(detail.item.id).unwrap().usage.unknown_runs, 1);
        store.sync_usage(attempt.run).unwrap();
        store.sync_usage(attempt.run).unwrap();
        let usage = store.show(detail.item.id).unwrap().usage;
        assert_eq!(usage.runs, 1);
        assert_eq!(usage.input_tokens, 600);
        assert_eq!(usage.output_tokens, 900);
        assert_eq!(usage.known_cost_usd, 150.0);
        assert_eq!(usage.unknown_runs, 0);
        writer
            .append_event(VersionedEventPayload::new(EventPayload::TokensConsumed {
                session,
                prompt_tokens: 7,
                output_tokens: 11,
                cache_hits: 0,
                model: "subscription-unpriced".into(),
                cost_usd: None,
            }))
            .await
            .unwrap();
        store.sync_usage(attempt.run).unwrap();
        store.sync_usage(attempt.run).unwrap();
        let usage = store.show(detail.item.id).unwrap().usage;
        assert_eq!(usage.input_tokens, 607);
        assert_eq!(usage.known_cost_usd, 150.0);
        assert_eq!(usage.unknown_runs, 1);
        writer.close().await.unwrap();
    }
}
