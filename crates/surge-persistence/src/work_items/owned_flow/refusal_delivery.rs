//! Existing-only journal publication under the same actual normal-writer lease.
use super::{launch, refusal_owner::DeliveryResources};
use crate::runs::{EventSeq, file_lock::FileLock, writer, writer_slot::WriterLease};
use crate::work_items::{Result, WorkItemError, start_preparation::secure_lock::PreparationLock};
#[cfg(not(windows))]
use rusqlite::Connection;
use rusqlite::{OpenFlags, OptionalExtension, Transaction, params};
use std::sync::Arc;
use surge_core::{
    ContentHash, RunId, VersionedEventPayload, run_event::EventPayload as E,
    work_item::OwnedFlowWakeRefusalReceipt,
};

pub(super) struct JournalOwner {
    connection: Option<crate::runs::connection::ManagedConnection>,
    _lease: Arc<WriterLease>,
}
fn unavailable() -> WorkItemError {
    WorkItemError::IndependentAcceptedDataInvalid
}
fn read_receipt(
    resources: &DeliveryResources,
    key: ContentHash,
    hash: ContentHash,
) -> Result<Option<OwnedFlowWakeRefusalReceipt>> {
    let conn = resources.pool.get()?;
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT hash,body FROM owned_flow_wake_refusals WHERE key=?",
            [key.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((stored_hash, body)) = row else {
        return Ok(None);
    };
    let receipt: OwnedFlowWakeRefusalReceipt = serde_json::from_str(&body)?;
    if stored_hash != hash.to_string() || receipt.hash()? != hash || key != hash {
        return Err(unavailable());
    }
    Ok(Some(receipt))
}
fn acquire(resources: &DeliveryResources, run: RunId) -> Result<JournalOwner> {
    let token = resources
        .writers
        .try_acquire_sync(run)
        .map_err(|_| unavailable())?
        .ok_or(WorkItemError::Busy)?;
    #[cfg(windows)]
    let namespace = crate::state_home::SqliteNamespaceOwner::existing(
        &resources
            .home
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
    )?;
    let path = resources
        .home
        .join("runs")
        .join(run.to_string())
        .join("events.sqlite.lock");
    let file_lock = FileLock::try_acquire_existing(&path, run).map_err(|error| match error {
        crate::runs::OpenError::WriterAlreadyHeld { .. } => WorkItemError::Busy,
        crate::runs::OpenError::Io(error) => WorkItemError::Io(error),
        _ => unavailable(),
    })?;
    Ok(JournalOwner {
        _lease: Arc::new(WriterLease {
            _token: token,
            _file_lock: file_lock,
            #[cfg(windows)]
            namespace,
        }),
        connection: None,
    })
}
/// A failed append/ACK retains the acquired writer lease and connection for the next paced attempt.
pub(super) fn deliver(
    resources: &DeliveryResources,
    guard: &PreparationLock,
    run_hint: RunId,
    key: ContentHash,
    hash: ContentHash,
    uncertain: bool,
    owner: &mut Option<JournalOwner>,
) -> Result<()> {
    guard.verify()?;
    let Some(receipt) = read_receipt(resources, key, hash)? else {
        return if uncertain {
            Ok(())
        } else {
            Err(unavailable())
        };
    };
    if receipt.run() != run_hint {
        return Err(unavailable());
    }
    let independent = {
        let mut conn = resources.pool.get()?;
        let tx = conn.transaction()?;
        let independent = launch::accepted_launch(&tx, run_hint)?;
        let binding = launch::independent_binding(&tx, &independent, run_hint)?;
        if receipt.operation() != independent.operation
            || receipt.binding() != &binding.binding
            || independent.identity != guard.identity()?
        {
            return Err(unavailable());
        }
        tx.commit()?;
        independent
    };
    if owner.is_none() {
        *owner = Some(acquire(resources, run_hint)?);
    }
    let Some(owner) = owner else {
        return Err(unavailable());
    };
    if owner.connection.is_none() {
        let path = resources
            .home
            .join("runs")
            .join(run_hint.to_string())
            .join("events.sqlite");
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(unavailable());
        }
        #[cfg(windows)]
        let conn = crate::runs::connection::RetainedConnection::open_owned(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            owner._lease.namespace.clone(),
        )?;
        #[cfg(not(windows))]
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        crate::runs::pragmas::apply(&conn, crate::runs::pragmas::PER_RUN_PRAGMAS)
            .map_err(|_| unavailable())?;
        owner.connection = Some(conn);
    }
    let Some(conn) = &mut owner.connection else {
        return Err(unavailable());
    };
    let tx = conn.transaction()?;
    let existing = validate_journal(&tx, &independent, Some((&receipt, hash)))?;
    let sequence = if let Some(sequence) = existing {
        sequence
    } else {
        writer::append_event_in(
            &tx,
            resources.clock.as_ref(),
            &VersionedEventPayload::new(E::OwnedFlowWakeRefused {
                receipt: Box::new(receipt.clone()),
            }),
        )
        .map_err(|_| unavailable())?
    };
    guard.verify()?;
    tx.commit()?;
    // Revalidate the actual durable occurrence before acknowledging the separate outbox.
    let tx = conn.transaction()?;
    if validate_journal(&tx, &independent, Some((&receipt, hash)))? != Some(sequence) {
        return Err(unavailable());
    }
    let mut registry = resources.pool.get()?;
    let ack = registry.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let stored: (String, Option<u64>) = ack.query_row(
        "SELECT hash,delivered_event_seq FROM owned_flow_wake_refusal_outbox WHERE key=?",
        [key.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored.0 != hash.to_string() || stored.1.is_some_and(|seq| seq != sequence.as_u64()) {
        return Err(unavailable());
    }
    ack.execute("UPDATE owned_flow_wake_refusal_outbox SET delivered_event_seq=? WHERE key=? AND hash=? AND delivered_event_seq IS NULL",
        params![sequence.as_u64(), key.to_string(), hash.to_string()])?;
    guard.verify()?;
    ack.commit()?;
    tx.commit()?;
    Ok(())
}
/// Independent original startup workspace/artifact anchor, before blaming a manifest.
pub(super) fn validate_original_startup(
    home: &std::path::Path,
    independent: &launch::AcceptedLaunch,
) -> Result<()> {
    let path = home
        .join("runs")
        .join(independent.receipt.run.to_string())
        .join("events.sqlite");
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unavailable());
    }
    let mut connection = crate::runs::connection::RetainedConnection::read_only(&path)?;
    let tx = connection.transaction()?;
    validate_journal(&tx, independent, None)?;
    tx.commit()?;
    Ok(())
}
fn validate_journal(
    tx: &Transaction<'_>,
    independent: &launch::AcceptedLaunch,
    expected: Option<(&OwnedFlowWakeRefusalReceipt, ContentHash)>,
) -> Result<Option<EventSeq>> {
    let mut statement = tx.prepare("SELECT seq,payload,schema_version FROM events ORDER BY seq")?;
    let mut rows = statement.query([])?;
    let (mut started, mut graph, mut binding, mut inputs, mut artifact) =
        (false, false, false, false, false);
    let mut startup = true;
    let mut previous = 0_u64;
    let mut found = None;
    while let Some(row) = rows.next()? {
        let sequence: u64 = row.get(0)?;
        if previous.checked_add(1) != Some(sequence) {
            return Err(unavailable());
        }
        previous = sequence;
        let bytes: Vec<u8> = row.get(1)?;
        let version: u32 = row.get(2)?;
        let payload = surge_core::migrate_payload(version, &bytes).map_err(|_| unavailable())?;
        match &payload {
            E::RunStarted {
                project_path,
                initial_prompt,
                ..
            } if !started && sequence == 1 => {
                if project_path != &independent.source.workspace.path
                    || initial_prompt != independent.source.contract.raw_prompt()
                {
                    return Err(unavailable());
                }
                started = true;
            },
            E::RunStarted { .. } => return Err(unavailable()),
            E::PipelineMaterialized {
                graph: accepted,
                graph_hash,
            } if startup && !graph => {
                if accepted.as_ref() != independent.source.contract.graph()
                    || *graph_hash != independent.source.contract.graph_hash()
                {
                    return Err(unavailable());
                }
                graph = true;
            },
            E::PipelineMaterialized { .. } => return Err(unavailable()),
            E::WorkItemAttemptBound { context } if startup && !binding => {
                if context.binding() != &independent.receipt.binding
                    || context.flow() != Some(&independent.source.contract)
                {
                    return Err(unavailable());
                }
                binding = true;
            },
            E::WorkItemAttemptBound { .. } => return Err(unavailable()),
            E::OwnedFlowInputsBound { manifest } if startup && !inputs => {
                if manifest.operation() != independent.operation
                    || manifest.run() != independent.receipt.run
                    || manifest.binding() != &independent.receipt.binding
                    || manifest.workspace() != &independent.source.workspace
                    || manifest.workspace_owner() != independent.receipt.workspace_owner
                {
                    return Err(unavailable());
                }
                inputs = true;
            },
            E::OwnedFlowInputsBound { .. } => return Err(unavailable()),
            E::ArtifactProduced {
                name,
                path,
                artifact: identity,
                ..
            } if startup && name == "accepted_flow_contract" => {
                if artifact || *identity != independent.receipt.binding.requirements_hash {
                    return Err(unavailable());
                }
                let bytes = std::fs::read(path)?;
                if ContentHash::compute(&bytes) != *identity
                    || bytes != independent.source.contract.canonical_bytes()?
                {
                    return Err(unavailable());
                }
                artifact = true;
            },
            E::OwnedFlowWakeRefused { receipt: existing } => {
                if let Some((receipt, hash)) = expected
                    && existing.run() == receipt.run()
                    && existing.lineage() == receipt.lineage()
                {
                    if existing.as_ref() != receipt || existing.hash()? != hash || found.is_some() {
                        return Err(unavailable());
                    }
                    found = Some(EventSeq(sequence));
                }
            },
            _ => {},
        }
        startup &= matches!(
            payload,
            E::RunStarted { .. }
                | E::PipelineMaterialized { .. }
                | E::ArtifactProduced { .. }
                | E::WorkItemAttemptBound { .. }
                | E::OwnedFlowInputsBound { .. }
        );
    }
    if !(started && graph && binding && inputs && artifact) {
        return Err(unavailable());
    }
    Ok(found)
}
