//! Failure-only registry projection; never grants a claim or private input capability.
use crate::work_items::{
    self, Result, WorkItemError,
    recovery_cycles::{self, RecoveryCycle},
};
use rusqlite::{Connection, OptionalExtension, params};
use surge_core::{
    RunId,
    execution_recovery::{ExecutionControlState, WorkItemExecutionControl},
    work_item::{WorkItemAttempt, WorkItemAttemptState},
};

pub(super) fn known(error: &WorkItemError) -> bool {
    matches!(
        error,
        WorkItemError::PrivateInputsRecoveryRequired
            | WorkItemError::PrivateInputsCorrupt
            | WorkItemError::PrivateInputsUnsafe
            | WorkItemError::PrivateInputsUnsupported
    )
}
fn checked(changed: usize) -> Result<()> {
    if changed != 1 {
        return Err(WorkItemError::Conflict(
            "private recovery observation changed".into(),
        ));
    }
    Ok(())
}
pub(super) fn attention(
    conn: &Connection,
    observed: &WorkItemAttempt,
    control: Option<&WorkItemExecutionControl>,
    version: u64,
    diagnostic: &str,
) -> Result<()> {
    if serde_json::to_value(work_items::attempt(conn, observed.run)?)?
        != serde_json::to_value(observed)?
        || work_items::control::read_control(conn, observed.run, None)?.as_ref() != control
    {
        return Err(WorkItemError::Conflict(
            "private recovery observation changed".into(),
        ));
    }
    let raw_attempt: String = conn.query_row(
        "SELECT payload FROM work_item_attempts WHERE run=?",
        [observed.run.to_string()],
        |row| row.get(0),
    )?;
    let mut attempt = observed.clone();
    attempt.state = WorkItemAttemptState::Attention;
    attempt.diagnostic = Some(diagnostic.into());
    checked(conn.execute(
        "UPDATE work_item_attempts SET state='attention',payload=? WHERE run=? AND generation=? AND payload=?",
        params![serde_json::to_string(&attempt)?, observed.run.to_string(), observed.binding.generation, raw_attempt],
    )?)?;
    if let Some(observed_control) = control {
        let raw_control: String = conn.query_row(
            "SELECT payload FROM work_item_execution_controls WHERE run=? AND generation=?",
            params![
                observed_control.run.to_string(),
                observed_control.generation
            ],
            |row| row.get(0),
        )?;
        let mut control = observed_control.clone();
        control.state = ExecutionControlState::Attention;
        control.diagnostic = attempt.diagnostic;
        checked(conn.execute(
            "UPDATE work_item_execution_controls SET state='attention',payload=? WHERE run=? AND generation=? AND payload=?",
            params![serde_json::to_string(&control)?, control.run.to_string(), control.generation, raw_control],
        )?)?;
    }
    checked(conn.execute(
        "UPDATE work_items SET version=version+1 WHERE id=? AND version=? AND active_run=? AND generation=? AND accepted_revision=? AND archived_at_ms IS NULL",
        params![observed.item.to_string(), version, observed.run.to_string(), observed.binding.generation, observed.binding.revision],
    )?)
}
#[derive(PartialEq)]
pub(super) struct Wakes {
    registry: Option<(String, Option<i64>)>,
    cycles: Vec<RecoveryCycle>,
}
pub(super) fn registry(conn: &Connection, run: RunId) -> Result<Option<(String, Option<i64>)>> {
    Ok(conn
        .query_row(
            "SELECT status,wake_at FROM runs WHERE id=?",
            [run.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}
impl Wakes {
    pub(super) fn read(conn: &Connection, run: RunId) -> Result<Self> {
        let registry = registry(conn, run)?;
        let mut query = conn.prepare("SELECT invocation,cycle_generation FROM work_item_quota_cycles WHERE run=? AND wake_at_ms IS NOT NULL AND closed=0 ORDER BY invocation,cycle_generation")?;
        let identities = query
            .query_map([run.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let cycles = identities
            .into_iter()
            .map(|(invocation, generation)| {
                recovery_cycles::read_cycle(conn, run, &invocation, generation)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { registry, cycles })
    }
    pub(super) fn disarm(&self, conn: &Connection, run: RunId) -> Result<()> {
        for cycle in &self.cycles {
            checked(conn.execute("UPDATE work_item_quota_cycles SET wake_at_ms=NULL WHERE run=? AND invocation=? AND cycle_generation=? AND revision=? AND wake_at_ms IS ?", params![run.to_string(),cycle.invocation,cycle.generation,cycle.revision,cycle.wake.as_ref().map(|wake|wake.due_at_ms())])?)?;
        }
        if let Some((status, Some(wake))) = &self.registry {
            checked(conn.execute(
                "UPDATE runs SET wake_at=NULL WHERE id=? AND status=? AND wake_at IS ?",
                params![run.to_string(), status, wake],
            )?)?;
        }
        Ok(())
    }
}
