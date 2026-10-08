//! Durable bootstrap acceptance and compare-and-swap state transitions.
//!
//! Environment capture belongs to the daemon, outside every transaction. Call
//! `lookup` before capture, then `insert_if_absent` to resolve a concurrent retry.

use crate::SqliteConnectionManager;
use r2d2::Pool;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use surge_core::bootstrap_continuation::{BootstrapContinuation, BootstrapTerminal};
use surge_core::bootstrap_operation::{
    BOOTSTRAP_VERSION, BootstrapAttentionReason, BootstrapCapture, BootstrapInputError,
    BootstrapIntent, BootstrapOperationStatus, BootstrapPhase, BootstrapState,
};
use surge_core::{ContentHash, RunId};

/// Typed refusal or storage failure; callers acknowledge only committed results.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapStoreError {
    /// SQLite failure, including transaction commit failures.
    #[error("bootstrap journal database failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Pool unavailable.
    #[error("bootstrap journal connection pool unavailable")]
    Pool,
    /// Serialization failed without logging the input payload.
    #[error("bootstrap journal serialization failed")]
    Serialization(#[from] serde_json::Error),
    /// Validated contract rejected input.
    #[error(transparent)]
    Input(#[from] BootstrapInputError),
    /// A repeated ID carried different client intent.
    #[error("bootstrap operation ID already belongs to different intent")]
    IntentConflict,
    /// No durable record exists.
    #[error("bootstrap operation not found")]
    NotFound,
    /// Another claimant or cancellation changed this operation.
    #[error("bootstrap operation revision changed")]
    StaleRevision,
    /// The transition is not allowed from the recorded state.
    #[error("bootstrap operation transition refused")]
    InvalidTransition,
    /// Explicit retry did not restore the original captured identities.
    #[error("bootstrap pinned inputs have changed; use a new operation ID for changed intent")]
    PinnedInputsChanged,
    /// Durable bootstrap ownership is at the configured admission bound.
    #[error("bootstrap admission queue is full")]
    QueueFull,
    /// Future payloads may be inspected but not executed.
    #[error("bootstrap payload version is not supported for execution")]
    UnsupportedPayload,
    /// Stored fingerprints, identities, or state are inconsistent.
    #[error("bootstrap journal integrity check failed")]
    CorruptRecord,
}

/// Readable payload versions. Unknown payload bytes are never guessed or executed.
#[derive(Debug, Clone, PartialEq)]
pub enum BootstrapStoredPayload {
    /// Current allowlisted payload.
    V1 {
        /// Validated immutable caller intent.
        intent: BootstrapIntent,
        /// Original environmental pins.
        capture: Box<BootstrapCapture>,
    },
    /// Preserve status visibility for a future version without decoding its payload.
    Unsupported {
        /// Stored future format version.
        version: u32,
    },
}

/// Full persisted record for daemon supervision; IPC status omits captured content.
#[derive(Debug, Clone, PartialEq)]
pub struct BootstrapOperationRecord {
    /// Nonsecret presentation and concurrency metadata.
    pub status: BootstrapOperationStatus,
    /// Immutable caller-only fingerprint.
    pub intent_fingerprint: ContentHash,
    /// Independent environment-capture fingerprint.
    pub capture_fingerprint: ContentHash,
    /// Validated supported payload or explicit future-version marker.
    pub payload: BootstrapStoredPayload,
    /// Immutable child launch intent once approved continuation commits.
    pub child: Option<BootstrapContinuation>,
}

/// Focused store over the existing registry SQLite pool.
#[derive(Clone)]
pub struct BootstrapOperationStore {
    pool: Pool<SqliteConnectionManager>,
}

impl BootstrapOperationStore {
    /// Construct using the migrated registry pool.
    #[must_use]
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// Inspect an operation, including unknown payload versions.
    pub fn get(&self, id: RunId) -> Result<Option<BootstrapOperationRecord>, BootstrapStoreError> {
        let connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        read(&connection, id)
    }

    /// Idempotency lookup before mutable environment validation/capture.
    pub fn lookup(
        &self,
        id: RunId,
        intent: &BootstrapIntent,
    ) -> Result<Option<BootstrapOperationRecord>, BootstrapStoreError> {
        let record = self.get(id)?;
        if let Some(record) = &record {
            compare_intent(record, intent)?;
        }
        Ok(record)
    }

    /// Commit a new operation, or return the concurrent winner with identical intent.
    ///
    /// Does no filesystem, runtime, or credential lookup. Captured input is ignored
    /// for an existing matching intent, even when current environmental pins differ.
    pub fn insert_if_absent(
        &self,
        id: RunId,
        intent: &BootstrapIntent,
        capture: &BootstrapCapture,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.insert_with_limit(id, intent, capture, None)
    }

    /// Atomically compare intent and bound all nonterminal durable ownership.
    /// The daemon holds its shared admission guard when deriving this limit.
    pub fn insert_bounded_if_absent(
        &self,
        id: RunId,
        intent: &BootstrapIntent,
        capture: &BootstrapCapture,
        maximum_nonterminal: usize,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.insert_with_limit(id, intent, capture, Some(maximum_nonterminal))
    }

    fn insert_with_limit(
        &self,
        id: RunId,
        intent: &BootstrapIntent,
        capture: &BootstrapCapture,
        maximum_nonterminal: Option<usize>,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        let mut connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(record) = read(&transaction, id)? {
            compare_intent(&record, intent)?;
            transaction.commit()?;
            return Ok(record);
        }
        if let Some(maximum) = maximum_nonterminal {
            let count: u64 = transaction.query_row(
                "SELECT COUNT(*) FROM bootstrap_operations WHERE payload_version <> ?1 OR COALESCE(json_extract(state_json, '$.state'), '') NOT IN ('completed','failed','cancelled')",
                [BOOTSTRAP_VERSION], |row| row.get(0),
            )?;
            if count >= maximum as u64 {
                return Err(BootstrapStoreError::QueueFull);
            }
        }
        let pins = capture.fields();
        // A reserved run cannot belong to any other operation, in either role.
        let occupied: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM bootstrap_operations WHERE planning_run IN (?1,?2) OR implementation_run IN (?1,?2))",
            params![pins.planning_run.to_string(), pins.implementation_run.to_string()], |row| row.get(0),
        )?;
        if occupied {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        transaction.execute(
            "INSERT INTO bootstrap_operations (operation_id, planning_run, implementation_run, payload_version, intent_fingerprint, capture_fingerprint, intent_json, capture_json, state_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id.to_string(), pins.planning_run.to_string(), pins.implementation_run.to_string(), BOOTSTRAP_VERSION,
                intent.fingerprint()?.to_string(), capture.fingerprint()?.to_string(), serde_json::to_string(intent)?,
                serde_json::to_string(capture)?, serde_json::to_string(&BootstrapState::Pending { phase: BootstrapPhase::QueuedPlanning })?],
        )?;
        let record = read(&transaction, id)?.ok_or(BootstrapStoreError::CorruptRecord)?;
        transaction.commit()?;
        Ok(record)
    }

    /// All journal records in durable admission order, including attention and future versions.
    pub fn list(&self) -> Result<Vec<BootstrapOperationRecord>, BootstrapStoreError> {
        let connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let mut statement = connection
            .prepare("SELECT operation_id FROM bootstrap_operations ORDER BY queue_sequence")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                let id = id.parse().map_err(|_| BootstrapStoreError::CorruptRecord)?;
                read(&connection, id)?.ok_or(BootstrapStoreError::CorruptRecord)
            })
            .collect()
    }

    /// Confirm valid startup evidence for the already-claimed reserved run.
    pub fn mark_started(
        &self,
        id: RunId,
        revision: u64,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.change(id, Some(revision), |record| {
            executable(record)?;
            let phase = match record.status.state {
                BootstrapState::Pending {
                    phase: BootstrapPhase::PreparingPlanning,
                } => BootstrapPhase::Planning,
                BootstrapState::Pending {
                    phase: BootstrapPhase::PreparingImplementation,
                } => BootstrapPhase::Implementing,
                _ => return Err(BootstrapStoreError::InvalidTransition),
            };
            Ok(BootstrapState::Pending { phase })
        })
    }

    /// Commit validated child launch data only after confirmed parent completion.
    /// Artifact/graph I/O validation belongs to the daemon before this transaction.
    pub fn continue_planning(
        &self,
        id: RunId,
        revision: u64,
        child: &BootstrapContinuation,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        let mut connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = read(&transaction, id)?.ok_or(BootstrapStoreError::NotFound)?;
        if record.status.revision != revision {
            return Err(BootstrapStoreError::StaleRevision);
        }
        executable(&record)?;
        if record.status.state
            != (BootstrapState::Pending {
                phase: BootstrapPhase::Planning,
            })
            || record.child.is_some()
            || child.parent() != record.status.planning_run
        {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        let BootstrapStoredPayload::V1 { intent, .. } = &record.payload else {
            return Err(BootstrapStoreError::UnsupportedPayload);
        };
        if child.budget().policy != intent.budget().policy
            || intent.budget().limits.tokens.is_some_and(|limit| {
                child
                    .budget()
                    .limits
                    .tokens
                    .is_none_or(|remaining| remaining > limit)
            })
            || intent.budget().limits.usd.is_some_and(|limit| {
                child
                    .budget()
                    .limits
                    .usd
                    .is_none_or(|remaining| remaining > limit)
            })
        {
            return Err(BootstrapStoreError::InvalidTransition);
        }
        let next = BootstrapState::Pending {
            phase: BootstrapPhase::QueuedImplementation,
        };
        let changed = transaction.execute("UPDATE bootstrap_operations SET child_json=?1,state_json=?2,revision=revision+1 WHERE operation_id=?3 AND revision=?4 AND cancel_requested=0 AND child_json IS NULL",
            params![serde_json::to_string(child)?, serde_json::to_string(&next)?, id.to_string(), revision])?;
        if changed != 1 {
            return Err(BootstrapStoreError::StaleRevision);
        }
        let committed = read(&transaction, id)?.ok_or(BootstrapStoreError::CorruptRecord)?;
        transaction.commit()?;
        Ok(committed)
    }

    /// Settle only a typed result after the owner confirms durable execution evidence.
    pub fn settle(
        &self,
        id: RunId,
        revision: u64,
        result: &BootstrapTerminal,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        let mut connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = read(&transaction, id)?.ok_or(BootstrapStoreError::NotFound)?;
        if record.status.revision != revision {
            return Err(BootstrapStoreError::StaleRevision);
        }
        if !matches!(record.payload, BootstrapStoredPayload::V1 { .. }) {
            return Err(BootstrapStoreError::UnsupportedPayload);
        }
        let phase = record
            .status
            .state
            .phase()
            .ok_or(BootstrapStoreError::InvalidTransition)?;
        let active = phase_run(
            phase,
            record.status.planning_run,
            record.status.implementation_run,
        );
        let state = match result {
            BootstrapTerminal::Completed { run_id, .. }
                if *run_id == record.status.implementation_run
                    && *run_id == active
                    && record.child.is_some() =>
            {
                BootstrapState::Completed
            },
            BootstrapTerminal::Failed { run_id, .. } if *run_id == active => BootstrapState::Failed,
            BootstrapTerminal::Cancelled if record.status.cancel_requested => {
                BootstrapState::Cancelled
            },
            _ => return Err(BootstrapStoreError::InvalidTransition),
        };
        let changed = transaction.execute("UPDATE bootstrap_operations SET state_json=?1,result_json=?2,revision=revision+1 WHERE operation_id=?3 AND revision=?4",
            params![serde_json::to_string(&state)?,serde_json::to_string(result)?,id.to_string(),revision])?;
        if changed != 1 {
            return Err(BootstrapStoreError::StaleRevision);
        }
        let committed = read(&transaction, id)?.ok_or(BootstrapStoreError::CorruptRecord)?;
        transaction.commit()?;
        Ok(committed)
    }

    /// Reserved identities in every payload version, including terminal operations.
    /// Ownership is independent of payload decoding and must fence legacy launchers.
    pub fn reserved_run_ids(
        &self,
    ) -> Result<std::collections::HashSet<RunId>, BootstrapStoreError> {
        let connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let mut statement = connection.prepare(
            "SELECT planning_run FROM bootstrap_operations UNION SELECT implementation_run FROM bootstrap_operations",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| id.parse().map_err(|_| BootstrapStoreError::CorruptRecord))
            .collect()
    }

    /// Runnable pre-start identities in durable order, excluding active/parked phases.
    /// Callers serialize physical reservations separately; returned identities may already
    /// hold a provisional slot and must not be reserved twice.
    pub fn pending_admission_runs(&self) -> Result<Vec<RunId>, BootstrapStoreError> {
        let connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let mut statement = connection.prepare(
            "SELECT planning_run, implementation_run, payload_version, intent_fingerprint,
                    capture_fingerprint, intent_json, capture_json, state_json, revision,
                    cancel_requested, queue_sequence, child_json, result_json, operation_id
             FROM bootstrap_operations
             WHERE payload_version=?1 AND cancel_requested=0
               AND json_extract(state_json, '$.state')='pending'
               AND json_extract(state_json, '$.phase') IN
                   ('queued_planning','preparing_planning','queued_implementation','preparing_implementation')
             ORDER BY queue_sequence",
        )?;
        // Read eligibility and validated payload from the same SQLite snapshot. A
        // concurrent cancellation cannot change a row between selection and decode.
        let rows = statement
            .query_map([BOOTSTRAP_VERSION], |row| {
                Ok((row.get::<_, String>(13)?, stored_row(row)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, row)| {
                let id = id.parse().map_err(|_| BootstrapStoreError::CorruptRecord)?;
                let record = decode(id, row)?;
                match record.status.state {
                    BootstrapState::Pending {
                        phase: BootstrapPhase::QueuedPlanning | BootstrapPhase::PreparingPlanning,
                    } => Ok(record.status.planning_run),
                    BootstrapState::Pending {
                        phase:
                            BootstrapPhase::QueuedImplementation
                            | BootstrapPhase::PreparingImplementation,
                    } => Ok(record.status.implementation_run),
                    _ => Err(BootstrapStoreError::CorruptRecord),
                }
            })
            .collect()
    }

    /// Queued records in durable acceptance order; attention never resumes implicitly.
    pub fn queued(&self) -> Result<Vec<BootstrapOperationRecord>, BootstrapStoreError> {
        let connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let mut statement = connection
            .prepare("SELECT operation_id FROM bootstrap_operations ORDER BY queue_sequence")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut queued = Vec::new();
        for id in ids {
            let id = id.parse().map_err(|_| BootstrapStoreError::CorruptRecord)?;
            let record = read(&connection, id)?.ok_or(BootstrapStoreError::CorruptRecord)?;
            if !record.status.cancel_requested
                && matches!(
                    record.status.state,
                    BootstrapState::Pending {
                        phase: BootstrapPhase::QueuedPlanning
                            | BootstrapPhase::QueuedImplementation
                    }
                )
                && matches!(record.payload, BootstrapStoredPayload::V1 { .. })
            {
                queued.push(record);
            }
        }
        Ok(queued)
    }

    /// Claim one queued phase. Caller owns admission accounting around this CAS.
    pub fn claim(
        &self,
        id: RunId,
        revision: u64,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.change(id, Some(revision), |record| {
            executable(record)?;
            let phase = match record.status.state {
                BootstrapState::Pending {
                    phase: BootstrapPhase::QueuedPlanning,
                } => BootstrapPhase::PreparingPlanning,
                BootstrapState::Pending {
                    phase: BootstrapPhase::QueuedImplementation,
                } => BootstrapPhase::PreparingImplementation,
                _ => return Err(BootstrapStoreError::InvalidTransition),
            };
            Ok(BootstrapState::Pending { phase })
        })
    }

    /// Record monotonic cancellation intent. Terminal outcomes are never relabeled.
    pub fn request_cancel(
        &self,
        id: RunId,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.change(id, None, |record| {
            if record.status.cancel_requested {
                return Ok(record.status.state.clone());
            }
            Ok(record.status.state.phase().map_or_else(
                || record.status.state.clone(),
                |phase| BootstrapState::Cancelling { phase },
            ))
        })
    }

    /// Preserve the phase and reserved run when execution needs operator attention.
    pub fn block(
        &self,
        id: RunId,
        revision: u64,
        reason: BootstrapAttentionReason,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.change(id, Some(revision), |record| {
            if !matches!(record.payload, BootstrapStoredPayload::V1 { .. }) {
                return Err(BootstrapStoreError::UnsupportedPayload);
            }
            let (BootstrapState::Pending { phase } | BootstrapState::Cancelling { phase }) =
                record.status.state
            else {
                return Err(BootstrapStoreError::InvalidTransition);
            };
            let run_id = match phase {
                BootstrapPhase::QueuedPlanning
                | BootstrapPhase::PreparingPlanning
                | BootstrapPhase::Planning => record.status.planning_run,
                BootstrapPhase::QueuedImplementation
                | BootstrapPhase::PreparingImplementation
                | BootstrapPhase::Implementing => record.status.implementation_run,
            };
            Ok(BootstrapState::NeedsAttention {
                phase,
                run_id: Some(run_id),
                reason,
            })
        })
    }

    /// Retry only cancellation settlement after attention; never restore execution eligibility.
    pub fn retry_cancellation(
        &self,
        id: RunId,
        revision: u64,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        self.change(id, Some(revision), |record| {
            if !matches!(record.payload, BootstrapStoredPayload::V1 { .. }) {
                return Err(BootstrapStoreError::UnsupportedPayload);
            }
            let BootstrapState::NeedsAttention { phase, .. } = record.status.state else {
                return Err(BootstrapStoreError::InvalidTransition);
            };
            if !record.status.cancel_requested {
                return Err(BootstrapStoreError::InvalidTransition);
            }
            Ok(BootstrapState::Cancelling { phase })
        })
    }

    /// Explicit operator retry after the caller re-resolves original pinned inputs.
    /// This method compares pins; it performs no environment verification itself.
    pub fn retry(
        &self,
        id: RunId,
        revision: u64,
        restored: &BootstrapCapture,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        let fingerprint = restored.fingerprint()?;
        self.change(id, Some(revision), |record| {
            executable(record)?;
            if record.capture_fingerprint != fingerprint {
                return Err(BootstrapStoreError::PinnedInputsChanged);
            }
            let BootstrapState::NeedsAttention { phase, .. } = record.status.state else {
                return Err(BootstrapStoreError::InvalidTransition);
            };
            Ok(BootstrapState::Pending { phase })
        })
    }

    fn change(
        &self,
        id: RunId,
        revision: Option<u64>,
        apply: impl FnOnce(&BootstrapOperationRecord) -> Result<BootstrapState, BootstrapStoreError>,
    ) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
        let mut connection = self.pool.get().map_err(|_| BootstrapStoreError::Pool)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = read(&transaction, id)?.ok_or(BootstrapStoreError::NotFound)?;
        if revision.is_some_and(|expected| expected != record.status.revision) {
            return Err(BootstrapStoreError::StaleRevision);
        }
        let state = apply(&record)?;
        if state != record.status.state {
            let cancelled = record.status.cancel_requested
                || matches!(state, BootstrapState::Cancelling { .. });
            let changed = transaction.execute(
                "UPDATE bootstrap_operations SET state_json=?1, cancel_requested=?2, revision=revision+1 WHERE operation_id=?3 AND revision=?4",
                params![serde_json::to_string(&state)?, cancelled, id.to_string(), record.status.revision],
            )?;
            if changed != 1 {
                return Err(BootstrapStoreError::StaleRevision);
            }
            record = read(&transaction, id)?.ok_or(BootstrapStoreError::CorruptRecord)?;
        }
        transaction.commit()?;
        Ok(record)
    }
}

fn phase_run(phase: BootstrapPhase, planning: RunId, implementation: RunId) -> RunId {
    match phase {
        BootstrapPhase::QueuedPlanning
        | BootstrapPhase::PreparingPlanning
        | BootstrapPhase::Planning => planning,
        BootstrapPhase::QueuedImplementation
        | BootstrapPhase::PreparingImplementation
        | BootstrapPhase::Implementing => implementation,
    }
}

fn executable(record: &BootstrapOperationRecord) -> Result<(), BootstrapStoreError> {
    if !matches!(record.payload, BootstrapStoredPayload::V1 { .. }) {
        return Err(BootstrapStoreError::UnsupportedPayload);
    }
    if record.status.cancel_requested {
        return Err(BootstrapStoreError::InvalidTransition);
    }
    Ok(())
}

fn compare_intent(
    record: &BootstrapOperationRecord,
    intent: &BootstrapIntent,
) -> Result<(), BootstrapStoreError> {
    if record.intent_fingerprint != intent.fingerprint()? {
        return Err(BootstrapStoreError::IntentConflict);
    }
    Ok(())
}

struct StoredRow {
    planning_run: String,
    implementation_run: String,
    version: u32,
    intent_fingerprint: String,
    capture_fingerprint: String,
    intent_json: String,
    capture_json: String,
    state_json: String,
    revision: u64,
    cancelled: bool,
    sequence: u64,
    child_json: Option<String>,
    result_json: Option<String>,
}

/// Resolve exact ownership and decode through the journal integrity checks.
pub(super) fn read_capture_for_run(
    connection: &Connection,
    run_id: RunId,
) -> Result<Option<BootstrapCapture>, BootstrapStoreError> {
    let mut statement = connection.prepare(
        "SELECT operation_id FROM bootstrap_operations WHERE planning_run=?1 OR implementation_run=?1 LIMIT 2",
    )?;
    let ids = statement
        .query_map([run_id.to_string()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > 1 {
        return Err(BootstrapStoreError::CorruptRecord);
    }
    let Some(id) = ids.first() else {
        return Ok(None);
    };
    let id = id.parse().map_err(|_| BootstrapStoreError::CorruptRecord)?;
    let record = read(connection, id)?.ok_or(BootstrapStoreError::CorruptRecord)?;
    match record.payload {
        BootstrapStoredPayload::V1 { capture, .. } => Ok(Some(*capture)),
        BootstrapStoredPayload::Unsupported { .. } => Err(BootstrapStoreError::UnsupportedPayload),
    }
}

fn read(
    connection: &Connection,
    id: RunId,
) -> Result<Option<BootstrapOperationRecord>, BootstrapStoreError> {
    let row = connection.query_row(
        "SELECT planning_run, implementation_run, payload_version, intent_fingerprint, capture_fingerprint, intent_json, capture_json, state_json, revision, cancel_requested, queue_sequence, child_json, result_json FROM bootstrap_operations WHERE operation_id=?",
        [id.to_string()], stored_row,
    ).optional()?;
    row.map(|row| decode(id, row)).transpose()
}

fn stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok(StoredRow {
        planning_run: row.get(0)?,
        implementation_run: row.get(1)?,
        version: row.get(2)?,
        intent_fingerprint: row.get(3)?,
        capture_fingerprint: row.get(4)?,
        intent_json: row.get(5)?,
        capture_json: row.get(6)?,
        state_json: row.get(7)?,
        revision: row.get(8)?,
        cancelled: row.get(9)?,
        sequence: row.get(10)?,
        child_json: row.get(11)?,
        result_json: row.get(12)?,
    })
}

fn decode(id: RunId, row: StoredRow) -> Result<BootstrapOperationRecord, BootstrapStoreError> {
    let intent_fingerprint: ContentHash = row
        .intent_fingerprint
        .parse()
        .map_err(|_| BootstrapStoreError::CorruptRecord)?;
    let capture_fingerprint: ContentHash = row
        .capture_fingerprint
        .parse()
        .map_err(|_| BootstrapStoreError::CorruptRecord)?;
    let planning_run = row
        .planning_run
        .parse()
        .map_err(|_| BootstrapStoreError::CorruptRecord)?;
    let implementation_run = row
        .implementation_run
        .parse()
        .map_err(|_| BootstrapStoreError::CorruptRecord)?;
    let payload = if row.version == BOOTSTRAP_VERSION {
        let intent: BootstrapIntent = serde_json::from_str(&row.intent_json)?;
        let capture: BootstrapCapture = serde_json::from_str(&row.capture_json)?;
        if intent.fingerprint()? != intent_fingerprint
            || capture.fingerprint()? != capture_fingerprint
            || capture.fields().planning_run != planning_run
            || capture.fields().implementation_run != implementation_run
        {
            return Err(BootstrapStoreError::CorruptRecord);
        }
        BootstrapStoredPayload::V1 {
            intent,
            capture: Box::new(capture),
        }
    } else {
        BootstrapStoredPayload::Unsupported {
            version: row.version,
        }
    };
    let state: BootstrapState = serde_json::from_str(&row.state_json)?;
    // The repository owns persisted cross-field invariants; never hand an
    // incoherent supported record to the scheduler and silently skip it.
    validate_state(&state, row.cancelled, planning_run, implementation_run)?;
    let record = BootstrapOperationRecord {
        status: BootstrapOperationStatus {
            operation_id: id,
            planning_run,
            implementation_run,
            state,
            result: row
                .result_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            revision: row.revision,
            cancel_requested: row.cancelled,
            queue_sequence: row.sequence,
            payload_version: row.version,
        },
        intent_fingerprint,
        capture_fingerprint,
        payload,
        child: row
            .child_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
    };
    validate_launch(&record)?;
    Ok(record)
}

fn validate_launch(record: &BootstrapOperationRecord) -> Result<(), BootstrapStoreError> {
    if !matches!(record.payload, BootstrapStoredPayload::V1 { .. }) {
        return Ok(());
    }
    if record
        .child
        .as_ref()
        .is_some_and(|child| child.parent() != record.status.planning_run)
    {
        return Err(BootstrapStoreError::CorruptRecord);
    }
    if let Some(phase) = record.status.state.phase() {
        let child_phase = phase_run(
            phase,
            record.status.planning_run,
            record.status.implementation_run,
        ) == record.status.implementation_run;
        if child_phase != record.child.is_some() || record.status.result.is_some() {
            return Err(BootstrapStoreError::CorruptRecord);
        }
        return Ok(());
    }
    let valid = match (&record.status.state, &record.status.result) {
        (BootstrapState::Completed, Some(BootstrapTerminal::Completed { run_id, .. })) => {
            *run_id == record.status.implementation_run && record.child.is_some()
        },
        (BootstrapState::Failed, Some(BootstrapTerminal::Failed { run_id, .. })) => {
            *run_id
                == if record.child.is_some() {
                    record.status.implementation_run
                } else {
                    record.status.planning_run
                }
        },
        (BootstrapState::Cancelled, Some(BootstrapTerminal::Cancelled)) => {
            record.status.cancel_requested
        },
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(BootstrapStoreError::CorruptRecord)
    }
}

fn validate_state(
    state: &BootstrapState,
    cancelled: bool,
    planning: RunId,
    implementation: RunId,
) -> Result<(), BootstrapStoreError> {
    let coherent = match state {
        BootstrapState::Pending { .. } => !cancelled,
        BootstrapState::NeedsAttention { phase, run_id, .. } => {
            let expected = match phase {
                BootstrapPhase::QueuedPlanning
                | BootstrapPhase::PreparingPlanning
                | BootstrapPhase::Planning => planning,
                BootstrapPhase::QueuedImplementation
                | BootstrapPhase::PreparingImplementation
                | BootstrapPhase::Implementing => implementation,
            };
            *run_id == Some(expected)
        },
        BootstrapState::Cancelling { .. } | BootstrapState::Cancelled => cancelled,
        BootstrapState::Completed | BootstrapState::Failed => true,
    };
    if coherent {
        Ok(())
    } else {
        Err(BootstrapStoreError::CorruptRecord)
    }
}

#[cfg(test)]
mod tests;
