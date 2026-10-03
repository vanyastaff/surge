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
