//! Host opening-order barriers. Inspection is evidence, never RPC authority.
use super::*;
use surge_core::{
    ContentHash,
    id::{ExecutionWriterId, StageInvocationId},
};

impl WorkItemStore {
    /// Commit a unique opening barrier before the provider RPC. Replays fail.
    pub fn admit_recipe_opening(
        &self,
        writer: ExecutionWriterId,
        invocation: StageInvocationId,
        runtime: &str,
        launch_hash: &ContentHash,
    ) -> Result<()> {
        if writer.as_ulid() == ulid::Ulid::nil()
            || invocation.as_ulid() == ulid::Ulid::nil()
            || runtime.trim().is_empty()
            || runtime.len() > 256
            || runtime.chars().any(char::is_control)
        {
            return Err(WorkItemError::Invalid(
                "invalid recipe opening identity".into(),
            ));
        }
        let conn = self.pool.get()?;
        conn.execute("INSERT INTO recipe_opening_admissions(execution_writer,invocation,runtime,launch_hash) VALUES(?,?,?,?)",
            params![writer.to_string(), invocation.to_string(), runtime, launch_hash.to_string()])?;
        Ok(())
    }
    /// Host-only configured snapshot association before the provider RPC.
    pub fn attach_admitted_configured_pin(
        &self,
        writer: ExecutionWriterId,
        pin: &ContentHash,
    ) -> Result<()> {
        let conn = self.pool.get()?;
        let changed = conn.execute("UPDATE recipe_opening_admissions SET configured_pin=? WHERE execution_writer=? AND configured_pin IS NULL AND exhaustion_receipt IS NULL",params![pin.to_string(),writer.to_string()])?;
        if changed != 1 {
            return Err(WorkItemError::Conflict(
                "recipe pin association refused".into(),
            ));
        }
        Ok(())
    }
    /// A changed source remains audit evidence, never a reusable configured snapshot.
    pub fn invalidate_admitted_configured_pin(&self, writer: ExecutionWriterId) -> Result<()> {
        self.pool.get()?.execute(
            "UPDATE recipe_opening_admissions SET configured_pin=NULL WHERE execution_writer=?",
            [writer.to_string()],
        )?;
        Ok(())
    }
    /// Latest comparable exhaustion for an exact project and frozen recipe.
    pub fn inspect_current_recipe_exhaustion(
        &self,
        project: &surge_core::id::WorkItemProjectId,
        expected: &recovery_cycles::FrozenQuotaCandidate,
        now_ms: i64,
    ) -> Result<Option<recovery_cycles::FreshTypedExhaustion>> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let receipt: Option<Option<String>> = tx.query_row(
            "SELECT exhaustion_receipt FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=? ORDER BY epoch DESC LIMIT 1",
            params![expected.candidate().runtime(), expected.launch_hash().to_string()], |row| row.get(0)).optional()?;
        match receipt.flatten() {
            Some(receipt) => {
                recovery_cycles::inspect_current_receipt(&tx, &receipt, project, expected, now_ms)
            },
            None => Ok(None),
        }
    }
}

/// Publish only for the exact actual opening, and never past a newer barrier.
pub(super) fn publish_receipt(
    conn: &Connection,
    opened: &surge_core::execution_recovery::OpenedSession,
    receipt: &str,
) -> Result<()> {
    let Some(writer) = opened.execution_writer.as_ref() else {
        return Ok(());
    };
    let existing: Option<Option<String>> = conn.query_row(
        "SELECT exhaustion_receipt FROM recipe_opening_admissions WHERE execution_writer=? AND invocation=? AND runtime=? AND launch_hash=?",
        params![writer.writer().to_string(), opened.descriptor.invocation().to_string(), opened.descriptor.runtime(), opened.descriptor.launch_hash().to_string()], |row| row.get(0)).optional()?;
    match existing {
        Some(Some(original)) if original != receipt => {
            return Err(WorkItemError::Conflict(
                "recipe opening already has a different exhaustion origin".into(),
            ));
        },
        Some(Some(_)) | None => return Ok(()),
        Some(None) => {},
    }
    conn.execute(
        "UPDATE recipe_opening_admissions SET exhaustion_receipt=? WHERE execution_writer=? AND invocation=? AND runtime=? AND launch_hash=? AND epoch=(SELECT MAX(epoch) FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=?) AND exhaustion_receipt IS NULL",
        params![receipt, writer.writer().to_string(), opened.descriptor.invocation().to_string(), opened.descriptor.runtime(), opened.descriptor.launch_hash().to_string(), opened.descriptor.runtime(), opened.descriptor.launch_hash().to_string()])?;
    Ok(())
}

/// Exact configured source identity is additional to the broad opening-order barrier.
pub(super) fn inspect_pinned_on_connection(
    conn: &Connection,
    project: &surge_core::id::WorkItemProjectId,
    expected: &recovery_cycles::FrozenQuotaCandidate,
    now_ms: i64,
) -> Result<Option<recovery_cycles::FreshTypedExhaustion>> {
    if expected.model().is_none() {
        return Ok(None);
    }
    let Some(pin) = expected.configured_pin() else {
        return Ok(None);
    };
    let row: Option<(Option<String>,Option<String>)> = conn.query_row("SELECT exhaustion_receipt,configured_pin FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=? ORDER BY epoch DESC LIMIT 1",params![expected.candidate().runtime(),expected.launch_hash().to_string()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((Some(receipt), Some(actual))) = row else {
        return Ok(None);
    };
    if actual != pin.to_string() {
        return Ok(None);
    }
    recovery_cycles::inspect_current_receipt(conn, &receipt, project, expected, now_ms)
}
