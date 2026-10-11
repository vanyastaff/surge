//! Normalized transactional verification context. No journal replay per append.
use super::error::WriterError;
use rusqlite::{OptionalExtension, Transaction};
use surge_core::{
    Graph,
    run_event::EventPayload,
    verification_evidence::{
        VerificationContext, VerificationCriteria, VerificationInvalidation, VerificationSubject,
    },
};

pub(super) fn maintain(
    tx: &Transaction<'_>,
    seq: u64,
    payload: &EventPayload,
) -> Result<Option<surge_core::verification_evidence::VerificationClaim>, WriterError> {
    if !matches!(
        payload,
        EventPayload::VerificationSubjectObserved { .. }
            | EventPayload::VerificationCriteriaAccepted { .. }
            | EventPayload::TaskVerified { .. }
            | EventPayload::TaskStatusChanged { .. }
            | EventPayload::RoadmapUpdated { .. }
            | EventPayload::PipelineMaterialized { .. }
            | EventPayload::GraphRevisionAccepted { .. }
    ) {
        return Ok(None);
    }
    let (subject_json, graph_json): (Option<String>, Option<String>) = tx.query_row(
        "SELECT subject_json,graph_json FROM verification_context WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut context = VerificationContext {
        subject: subject_json
            .as_deref()
            .map(serde_json::from_str::<VerificationSubject>)
            .transpose()?,
        ..Default::default()
    };
    let task = match payload {
        EventPayload::VerificationCriteriaAccepted { task_id, .. }
        | EventPayload::TaskVerified { task_id, .. } => Some(task_id),
        _ => None,
    };
    if let Some(task) = task {
        let json: Option<String> = tx
            .query_row(
                "SELECT criteria_json FROM verification_criteria WHERE task_id=?",
                [task.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = json {
            context.criteria.insert(
                task.clone(),
                serde_json::from_str::<VerificationCriteria>(&json)?,
            );
        }
    }
    match context.observe(payload) {
        VerificationInvalidation::All => {
            tx.execute("UPDATE task_ledger SET verified=0", [])?;
            tx.execute("DELETE FROM verification_criteria", [])?;
        },
        VerificationInvalidation::Task(task) => {
            if !context.criteria.contains_key(&task) {
                tx.execute(
                    "DELETE FROM verification_criteria WHERE task_id=?",
                    [task.as_str()],
                )?;
            }
            tx.execute("INSERT INTO task_ledger (task_id,status,verified,updated_seq) VALUES (?,'pending',0,?) ON CONFLICT(task_id) DO UPDATE SET verified=0", rusqlite::params![task.as_str(),seq as i64])?;
        },
        VerificationInvalidation::None => {},
    }
    match payload {
        EventPayload::VerificationSubjectObserved { subject } => {
            tx.execute(
                "UPDATE verification_context SET subject_json=?,updated_seq=? WHERE singleton=1",
                rusqlite::params![
                    subject.as_ref().map(serde_json::to_string).transpose()?,
                    seq as i64
                ],
            )?;
        },
        EventPayload::VerificationCriteriaAccepted { task_id, criteria } => {
            if let Some(criteria) = criteria {
                tx.execute("INSERT INTO verification_criteria VALUES (?,?,?) ON CONFLICT(task_id) DO UPDATE SET criteria_json=excluded.criteria_json,updated_seq=excluded.updated_seq", rusqlite::params![task_id.as_str(),serde_json::to_string(criteria)?,seq as i64])?;
            } else {
                tx.execute(
                    "DELETE FROM verification_criteria WHERE task_id=?",
                    [task_id.as_str()],
                )?;
            }
        },
        EventPayload::RoadmapUpdated { .. } => {
            tx.execute("DELETE FROM verification_criteria", [])?;
        },
        EventPayload::PipelineMaterialized { graph, .. }
        | EventPayload::GraphRevisionAccepted { graph, .. } => {
            tx.execute(
                "UPDATE verification_context SET graph_json=?,updated_seq=? WHERE singleton=1",
                rusqlite::params![serde_json::to_string(graph)?, seq as i64],
            )?;
        },
        EventPayload::TaskVerified {
            task_id,
            node,
            evidence,
            report,
        } => {
            let graph = graph_json
                .as_deref()
                .map(serde_json::from_str::<Graph>)
                .transpose()?;
            let claim = surge_core::verification_evidence::classify_claim(
                graph.as_ref(),
                node,
                task_id,
                *evidence,
                report.as_ref(),
                context.subject.as_ref(),
                context.criteria.get(task_id),
            );
            let valid = claim == surge_core::verification_evidence::VerificationClaim::Verified;
            // Stored bytes, not event metadata alone, must still exist and match.
            let path: Option<String> = tx.query_row("SELECT path FROM artifacts WHERE content_hash=? AND name='verification-report' ORDER BY produced_at_seq DESC LIMIT 1", [evidence.to_string()], |row| row.get(0)).optional()?;
            let stored = path
                .as_ref()
                .and_then(|path| std::fs::read(path).ok())
                .is_some_and(|bytes| surge_core::ContentHash::compute(&bytes) == *evidence);
            if valid
                && stored
                && let (Some(path), Some(binding)) = (
                    path,
                    report.as_ref().and_then(|report| report.binding.as_ref()),
                )
            {
                tx.execute("INSERT INTO verification_proofs VALUES (?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET evidence=excluded.evidence,report_path=excluded.report_path,binding_json=excluded.binding_json", rusqlite::params![task_id.as_str(),evidence.to_string(),path,serde_json::to_string(binding)?])?;
            }
            return Ok(Some(if valid && !stored {
                surge_core::verification_evidence::VerificationClaim::Unbound
            } else {
                claim
            }));
        },
        _ => {},
    }
    Ok(None)
}

/// Historical currentness and its sealed locator, read without migrations or repair.
#[derive(Debug, Clone)]
pub struct VerificationProofRecord {
    /// Stable task identity.
    pub task_id: String,
    /// Fresh at the last durable observation.
    pub verified: bool,
    /// Host-sealed revision and criteria contract.
    pub binding: surge_core::verification_evidence::VerificationBinding,
    /// Immutable stored report locator.
    pub report_path: std::path::PathBuf,
    /// Hash of the sealed report bytes.
    pub evidence: surge_core::ContentHash,
}
impl super::Storage {
    /// Read proof locators without creating storage or applying migrations.
    ///
    /// # Errors
    /// Returns storage errors for absent, unsupported or corrupt journals.
    pub fn inspect_verification_proofs(
        &self,
        run: surge_core::RunId,
    ) -> Result<Vec<VerificationProofRecord>, super::StorageError> {
        let mut conn =
            super::connection::RetainedConnection::read_only(&self.events_db_path(&run))?;
        let tx = conn.transaction()?;
        let mut statement = tx.prepare("SELECT p.task_id,l.verified,p.binding_json,p.report_path,p.evidence FROM verification_proofs p JOIN task_ledger l USING(task_id)")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut proofs = Vec::new();
        for row in rows {
            let (task_id, verified, json, path, hash) = row?;
            let evidence = hash.parse().map_err(|_| {
                super::StorageError::SerializationFailed(serde_json::Error::io(
                    std::io::Error::other("invalid proof content hash"),
                ))
            })?;
            proofs.push(VerificationProofRecord {
                task_id,
                verified,
                binding: serde_json::from_str(&json)?,
                report_path: path.into(),
                evidence,
            });
        }
        Ok(proofs)
    }
}
