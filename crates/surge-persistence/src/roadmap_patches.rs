//! Registry-level roadmap patch index for CLI list/show/reject operations.
//!
//! Per-run event logs remain the source of truth for attached patch lifecycle
//! events. This index is the cross-run lookup surface that avoids scanning
//! every run database for common CLI operations.

use std::path::PathBuf;
use std::str::FromStr;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{OptionalExtension, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use surge_core::{
    ContentHash, OperatorConflictChoice, RoadmapPatchApprovalDecision, RoadmapPatchId,
    RoadmapPatchStatus, RoadmapPatchTarget, RunId,
};

use crate::runs::error::StorageError;

/// Filter for registry-level roadmap patch list queries.
#[derive(Debug, Default, Clone)]
pub struct RoadmapPatchIndexFilter {
    /// Only return patches with this lifecycle status.
    pub status: Option<RoadmapPatchStatus>,
    /// Only return patches associated with this project path.
    pub project_path: Option<PathBuf>,
    /// Only return patches attached to this run id.
    pub run_id: Option<RunId>,
    /// Limit returned rows.
    pub limit: Option<usize>,
}

/// Input used to upsert one registry-level roadmap patch index row.
#[derive(Debug, Clone)]
pub struct RoadmapPatchIndexUpsert {
    /// Proposed patch id. Existing rows with the same content hash keep their original id.
    pub patch_id: RoadmapPatchId,
    /// Stable hash of the semantic patch contents.
    pub content_hash: ContentHash,
    /// Run that owns this patch, when already attached to a run.
    pub run_id: Option<RunId>,
    /// Project path this patch belongs to.
    pub project_path: PathBuf,
    /// Target roadmap/run being amended.
    pub target: RoadmapPatchTarget,
    /// Latest lifecycle status.
    pub status: RoadmapPatchStatus,
    /// Stored patch artifact hash, when known.
    pub patch_artifact: Option<ContentHash>,
    /// Stored patch path, when known.
    pub patch_path: Option<PathBuf>,
    /// Approval summary hash, when approval has been requested.
    pub summary_hash: Option<ContentHash>,
    /// Operator decision, when known.
    pub decision: Option<RoadmapPatchApprovalDecision>,
    /// Operator comment, when present.
    pub decision_comment: Option<String>,
    /// Conflict choice selected by the operator, when present.
    pub conflict_choice: Option<OperatorConflictChoice>,
    /// Timestamp for this observation in unix epoch milliseconds.
    pub observed_at_ms: i64,
}

/// One row from the registry-level roadmap patch index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapPatchIndexRecord {
    /// Stable patch identifier.
    pub patch_id: RoadmapPatchId,
    /// Stable hash of the semantic patch contents.
    pub content_hash: ContentHash,
    /// Run that owns this patch, when already attached to a run.
    pub run_id: Option<RunId>,
    /// Project path this patch belongs to.
    pub project_path: PathBuf,
    /// Target roadmap/run being amended.
    pub target: RoadmapPatchTarget,
    /// Latest lifecycle status.
    pub status: RoadmapPatchStatus,
    /// Stored patch artifact hash, when known.
    pub patch_artifact: Option<ContentHash>,
    /// Stored patch path, when known.
    pub patch_path: Option<PathBuf>,
    /// Approval summary hash, when approval has been requested.
    pub summary_hash: Option<ContentHash>,
    /// Operator decision, when known.
    pub decision: Option<RoadmapPatchApprovalDecision>,
    /// Operator comment, when present.
    pub decision_comment: Option<String>,
    /// Conflict choice selected by the operator, when present.
    pub conflict_choice: Option<OperatorConflictChoice>,
    /// First time this row was created in unix epoch milliseconds.
    pub created_at_ms: i64,
    /// Last time this row was updated in unix epoch milliseconds.
    pub updated_at_ms: i64,
}

/// Registry-backed store for roadmap patch lookup metadata.
#[derive(Clone)]
pub struct RoadmapPatchStore {
    pool: Pool<SqliteConnectionManager>,
}

impl RoadmapPatchStore {
    /// Create a store over the registry DB connection pool.
    #[must_use]
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// Insert or update patch metadata, using `content_hash` for idempotency.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read or written.
    pub fn upsert(
        &self,
        input: &RoadmapPatchIndexUpsert,
    ) -> Result<RoadmapPatchIndexRecord, StorageError> {
        tracing::debug!(
            target: "roadmap_patch_index",
            patch_id = %input.patch_id,
            content_hash = %input.content_hash,
            status = status_label(input.status),
            "roadmap_patch_index_upsert"
        );
        let mut conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let tx = conn.transaction()?;
        let patch_id = existing_patch_id_for_hash(&tx, input.content_hash)?
            .unwrap_or_else(|| input.patch_id.clone());

        tx.execute(
            "INSERT INTO roadmap_patch_index
                (patch_id, content_hash, run_id, project_path, target_json, status,
                 patch_artifact, patch_path, summary_hash, decision, decision_comment,
                 conflict_choice, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(patch_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                run_id = excluded.run_id,
                project_path = excluded.project_path,
                target_json = excluded.target_json,
                status = excluded.status,
                patch_artifact = excluded.patch_artifact,
                patch_path = excluded.patch_path,
                summary_hash = excluded.summary_hash,
                decision = excluded.decision,
                decision_comment = excluded.decision_comment,
                conflict_choice = excluded.conflict_choice,
                updated_at = excluded.updated_at",
            params![
                patch_id.as_str(),
                input.content_hash.to_string(),
                input.run_id.map(|run_id| run_id.to_string()),
                input.project_path.to_string_lossy().to_string(),
                serde_json::to_string(&input.target)?,
                status_label(input.status),
                input.patch_artifact.map(|hash| hash.to_string()),
                input
                    .patch_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                input.summary_hash.map(|hash| hash.to_string()),
                input.decision.map(decision_label),
                input.decision_comment.as_deref(),
                input.conflict_choice.map(conflict_choice_label),
                input.observed_at_ms,
                input.observed_at_ms,
            ],
        )?;
        let record = get_by_patch_id(&tx, &patch_id)?;
        tx.commit()?;
        record.ok_or_else(|| StorageError::MigrationFailed("roadmap patch upsert vanished".into()))
    }

    /// List patch metadata rows matching `filter`.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn list(
        &self,
        filter: &RoadmapPatchIndexFilter,
    ) -> Result<Vec<RoadmapPatchIndexRecord>, StorageError> {
        tracing::debug!(
            target: "roadmap_patch_index",
            status = filter.status.map(status_label),
            project_path = filter.project_path.as_ref().map(|path| path.display().to_string()),
            run_id = filter.run_id.map(|run_id| run_id.to_string()),
            "roadmap_patch_index_list"
        );
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let mut sql = String::from(
            "SELECT patch_id, content_hash, run_id, project_path, target_json, status,
                    patch_artifact, patch_path, summary_hash, decision, decision_comment,
                    conflict_choice, created_at, updated_at
             FROM roadmap_patch_index WHERE 1=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            binds.push(Box::new(status_label(status).to_owned()));
        }
        if let Some(project_path) = &filter.project_path {
            sql.push_str(" AND project_path = ?");
            binds.push(Box::new(project_path.to_string_lossy().to_string()));
        }
        if let Some(run_id) = filter.run_id {
            sql.push_str(" AND run_id = ?");
            binds.push(Box::new(run_id.to_string()));
        }
        sql.push_str(" ORDER BY updated_at DESC, patch_id");
        if let Some(limit) = filter.limit {
            sql.push_str(" LIMIT ?");
            binds.push(Box::new(limit as i64));
        }

        let bind_refs: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(std::convert::AsRef::as_ref).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bind_refs), row_to_record)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Read one patch metadata row by id.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn get(
        &self,
        patch_id: &RoadmapPatchId,
    ) -> Result<Option<RoadmapPatchIndexRecord>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        get_by_patch_id(&conn, patch_id)
    }

    /// Read one patch metadata row by content hash.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn get_by_content_hash(
        &self,
        content_hash: ContentHash,
    ) -> Result<Option<RoadmapPatchIndexRecord>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.query_row(
            "SELECT patch_id, content_hash, run_id, project_path, target_json, status,
                    patch_artifact, patch_path, summary_hash, decision, decision_comment,
                    conflict_choice, created_at, updated_at
             FROM roadmap_patch_index WHERE content_hash = ?",
            params![content_hash.to_string()],
            row_to_record,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Mark a patch rejected without scanning run event logs.
    ///
    /// Applied patches are left unchanged and returned as-is.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read or written.
    pub fn reject(
        &self,
        patch_id: &RoadmapPatchId,
        comment: Option<&str>,
        conflict_choice: Option<OperatorConflictChoice>,
        observed_at_ms: i64,
    ) -> Result<Option<RoadmapPatchIndexRecord>, StorageError> {
        tracing::info!(
            target: "roadmap_patch_index",
            patch_id = %patch_id,
            "roadmap_patch_index_reject"
        );
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.execute(
            "UPDATE roadmap_patch_index
             SET status = ?,
                 decision = ?,
                 decision_comment = COALESCE(?, decision_comment),
                 conflict_choice = COALESCE(?, conflict_choice),
                 updated_at = ?
             WHERE patch_id = ? AND status != ?",
            params![
                status_label(RoadmapPatchStatus::Rejected),
                decision_label(RoadmapPatchApprovalDecision::Reject),
                comment,
                conflict_choice.map(conflict_choice_label),
                observed_at_ms,
                patch_id.as_str(),
                status_label(RoadmapPatchStatus::Applied),
            ],
        )?;
        self.get(patch_id)
    }
}

fn existing_patch_id_for_hash(
    conn: &rusqlite::Connection,
    content_hash: ContentHash,
) -> rusqlite::Result<Option<RoadmapPatchId>> {
    conn.query_row(
        "SELECT patch_id FROM roadmap_patch_index WHERE content_hash = ?",
        params![content_hash.to_string()],
        |row| parse_patch_id(row.get(0)?, 0),
    )
    .optional()
}

fn get_by_patch_id(
    conn: &rusqlite::Connection,
    patch_id: &RoadmapPatchId,
) -> Result<Option<RoadmapPatchIndexRecord>, StorageError> {
    conn.query_row(
        "SELECT patch_id, content_hash, run_id, project_path, target_json, status,
                patch_artifact, patch_path, summary_hash, decision, decision_comment,
                conflict_choice, created_at, updated_at
         FROM roadmap_patch_index WHERE patch_id = ?",
        params![patch_id.as_str()],
        row_to_record,
    )
    .optional()
    .map_err(Into::into)
}

fn row_to_record(row: &Row<'_>) -> rusqlite::Result<RoadmapPatchIndexRecord> {
    Ok(RoadmapPatchIndexRecord {
        patch_id: parse_patch_id(row.get(0)?, 0)?,
        content_hash: parse_hash(row.get(1)?, 1)?,
        run_id: parse_optional_run_id(row.get(2)?, 2)?,
        project_path: PathBuf::from(row.get::<_, String>(3)?),
        target: parse_json(row.get(4)?, 4)?,
        status: parse_json_string(row.get(5)?, 5)?,
        patch_artifact: parse_optional_hash(row.get(6)?, 6)?,
        patch_path: optional_path(row.get(7)?),
        summary_hash: parse_optional_hash(row.get(8)?, 8)?,
        decision: parse_optional_json_string(row.get(9)?, 9)?,
        decision_comment: row.get(10)?,
        conflict_choice: parse_optional_json_string(row.get(11)?, 11)?,
        created_at_ms: row.get(12)?,
        updated_at_ms: row.get(13)?,
    })
}

fn parse_patch_id(value: String, column: usize) -> rusqlite::Result<RoadmapPatchId> {
    RoadmapPatchId::from_str(&value).map_err(|error| conversion_error(column, error))
}

fn parse_hash(value: String, column: usize) -> rusqlite::Result<ContentHash> {
    ContentHash::from_str(&value).map_err(|error| conversion_error(column, error))
}

fn parse_optional_hash(
    value: Option<String>,
    column: usize,
) -> rusqlite::Result<Option<ContentHash>> {
    value.map(|inner| parse_hash(inner, column)).transpose()
}

fn parse_optional_run_id(value: Option<String>, column: usize) -> rusqlite::Result<Option<RunId>> {
    value
        .map(|inner| RunId::from_str(&inner).map_err(|error| conversion_error(column, error)))
        .transpose()
}

fn parse_json<T: DeserializeOwned>(value: String, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|error| conversion_error(column, error))
}

fn parse_json_string<T: DeserializeOwned>(value: String, column: usize) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(value))
        .map_err(|error| conversion_error(column, error))
}

fn parse_optional_json_string<T: DeserializeOwned>(
    value: Option<String>,
    column: usize,
) -> rusqlite::Result<Option<T>> {
    value
        .map(|inner| parse_json_string(inner, column))
        .transpose()
}

fn optional_path(value: Option<String>) -> Option<PathBuf> {
    value.map(PathBuf::from)
}

fn conversion_error<E>(column: usize, error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

const fn status_label(status: RoadmapPatchStatus) -> &'static str {
    match status {
        RoadmapPatchStatus::Drafted => "drafted",
        RoadmapPatchStatus::PendingApproval => "pending_approval",
        RoadmapPatchStatus::Approved => "approved",
        RoadmapPatchStatus::Applied => "applied",
        RoadmapPatchStatus::Rejected => "rejected",
        RoadmapPatchStatus::Superseded => "superseded",
    }
}

const fn decision_label(decision: RoadmapPatchApprovalDecision) -> &'static str {
    match decision {
        RoadmapPatchApprovalDecision::Approve => "approve",
        RoadmapPatchApprovalDecision::Edit => "edit",
        RoadmapPatchApprovalDecision::Reject => "reject",
    }
}

const fn conflict_choice_label(choice: OperatorConflictChoice) -> &'static str {
    match choice {
        OperatorConflictChoice::DeferToNextMilestone => "defer_to_next_milestone",
        OperatorConflictChoice::AbortCurrentRun => "abort_current_run",
        OperatorConflictChoice::CreateFollowUpRun => "create_follow_up_run",
        OperatorConflictChoice::RejectPatch => "reject_patch",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::registry::open_registry_pool;

    fn store() -> RoadmapPatchStore {
        let tmp = tempfile::tempdir().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        RoadmapPatchStore::new(pool)
    }

    fn upsert(id: &str, content: &[u8], status: RoadmapPatchStatus) -> RoadmapPatchIndexUpsert {
        RoadmapPatchIndexUpsert {
            patch_id: RoadmapPatchId::new(id).unwrap(),
            content_hash: ContentHash::compute(content),
            run_id: None,
            project_path: PathBuf::from("/tmp/project"),
            target: RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: ".ai-factory/ROADMAP.md".into(),
            },
            status,
            patch_artifact: None,
            patch_path: None,
            summary_hash: None,
            decision: None,
            decision_comment: None,
            conflict_choice: None,
            observed_at_ms: 1_700_000_000_001,
        }
    }

    #[test]
    fn duplicate_content_hash_keeps_existing_patch_id() {
        let store = store();
        let first = store
            .upsert(&upsert(
                "rpatch-one",
                b"same patch",
                RoadmapPatchStatus::Drafted,
            ))
            .unwrap();
        let second = store
            .upsert(&upsert(
                "rpatch-two",
                b"same patch",
                RoadmapPatchStatus::PendingApproval,
            ))
            .unwrap();

        assert_eq!(second.patch_id, first.patch_id);
        assert_eq!(second.status, RoadmapPatchStatus::PendingApproval);
        assert!(
            store
                .get(&RoadmapPatchId::new("rpatch-two").unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn list_filters_by_status_and_reject_is_idempotent() {
        let store = store();
        let pending = store
            .upsert(&upsert(
                "rpatch-pending",
                b"pending patch",
                RoadmapPatchStatus::PendingApproval,
            ))
            .unwrap();
        let applied = store
            .upsert(&upsert(
                "rpatch-applied",
                b"applied patch",
                RoadmapPatchStatus::Applied,
            ))
            .unwrap();

        let listed = store
            .list(&RoadmapPatchIndexFilter {
                status: Some(RoadmapPatchStatus::PendingApproval),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].patch_id, pending.patch_id);

        let rejected = store
            .reject(
                &pending.patch_id,
                Some("no longer needed"),
                Some(OperatorConflictChoice::RejectPatch),
                1_700_000_000_500,
            )
            .unwrap()
            .unwrap();
        assert_eq!(rejected.status, RoadmapPatchStatus::Rejected);
        assert_eq!(
            rejected.decision,
            Some(RoadmapPatchApprovalDecision::Reject)
        );
        assert_eq!(
            rejected.decision_comment.as_deref(),
            Some("no longer needed")
        );
        assert_eq!(
            rejected.conflict_choice,
            Some(OperatorConflictChoice::RejectPatch)
        );

        let still_applied = store
            .reject(
                &applied.patch_id,
                Some("too late"),
                Some(OperatorConflictChoice::RejectPatch),
                1_700_000_000_600,
            )
            .unwrap()
            .unwrap();
        assert_eq!(still_applied.status, RoadmapPatchStatus::Applied);
        assert_eq!(still_applied.decision_comment, None);
    }
}
