//! Writer task: single-threaded SQLite write loop driven by a bounded mpsc.
//!
//! The writer owns one `rusqlite::Connection` and processes one command at a
//! time. All event INSERTs and view maintenance happen in the same SQL
//! transaction inside the writer task; readers go through a separate r2d2
//! pool and never touch the writer connection.

use super::writer_slot::WriterLease;
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};
use surge_core::{NodeKey, RunId, VersionedEventPayload};
use tokio::sync::{mpsc, oneshot};

use crate::runs::clock::Clock;
use crate::runs::error::WriterError;
use crate::runs::seq::EventSeq;
use crate::runs::types::ArtifactRecord;
use crate::runs::views;

/// Bounded channel size for writer commands. Default 64.
pub const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Commands consumed by the writer task.
///
/// One command is processed at a time; SQL transactions never span multiple
/// commands. `oneshot::Sender` carries the result back to the caller.
pub enum WriterCommand {
    /// Append a single event to the log and run view maintenance in the same tx.
    AppendEvent {
        /// Payload to append.
        payload: Box<VersionedEventPayload>,
        /// Reply channel; the sender returns the assigned `EventSeq` on success.
        reply: oneshot::Sender<Result<EventSeq, WriterError>>,
    },
    /// Append several events atomically inside one transaction.
    AppendBatch {
        /// Payloads to append (preserved order).
        payloads: Vec<VersionedEventPayload>,
        /// Reply channel; returns the assigned `EventSeq`s in input order.
        reply: oneshot::Sender<Result<Vec<EventSeq>, WriterError>>,
    },
    /// Accept one host decision against an unchanged trusted request prefix.
    CommitGateAnswer {
        /// Exact journal prefix validated by the host.
        prefix: EventSeq,
        /// Original request and stage occurrence.
        request: surge_core::execution_recovery::gate_commit::GateCommitRequest,
        /// Original operator response.
        response: serde_json::Value,
        /// Durable accepted response event sequence.
        reply: oneshot::Sender<Result<EventSeq, WriterError>>,
    },
    /// Commit a prepared stage route and its post-route snapshot against one journal prefix.
    CommitStageRoute {
        /// Last event observed while preparing the route.
        prefix: EventSeq,
        /// Ordered routing events to publish together.
        payloads: Vec<VersionedEventPayload>,
        /// Snapshot encoded for the final event sequence of this batch.
        blob: Vec<u8>,
        /// Final committed event sequence.
        reply: oneshot::Sender<Result<EventSeq, WriterError>>,
    },
    /// Persist an artifact (file + DB row).
    StoreArtifact {
        /// Logical artifact name (e.g., `"spec.toml"`).
        name: String,
        /// Raw artifact bytes; the writer hashes and writes to disk.
        content: Vec<u8>,
        /// Node that produced the artifact, if any.
        produced_by: Option<NodeKey>,
        /// Event seq at which the artifact was produced.
        produced_at_seq: EventSeq,
        /// Reply channel; returns the persisted `ArtifactRecord`.
        reply: oneshot::Sender<Result<ArtifactRecord, WriterError>>,
    },
    /// Writes an opaque snapshot blob into `graph_snapshots`.
    ///
    /// Caller is responsible for serializing whatever state they want to
    /// snapshot (typically `serde_json::to_vec(&run_state)` once `RunState`
    /// gains serde derives — currently the snapshot is a caller-encoded
    /// `Vec<u8>` and decoders see only raw bytes via
    /// `RunReader::latest_snapshot_at_or_before`). Storing as an opaque blob
    /// keeps the M2 storage layer agnostic to the snapshot's serde-ability.
    WriteSnapshot {
        /// Event seq the snapshot is anchored to.
        at_seq: EventSeq,
        /// Caller-encoded snapshot bytes (typically `serde_json::to_vec(&state)`).
        blob: Vec<u8>,
        /// Reply channel.
        reply: oneshot::Sender<Result<(), WriterError>>,
    },
    /// Atomically seal a suspension snapshot against an unchanged journal prefix.
    SealSuspension {
        /// Exact prefix captured by the host after writer cleanup.
        fence: surge_core::execution_recovery::SuspensionFence,
        /// Host-encoded recovery snapshot anchored to that prefix.
        blob: Vec<u8>,
        /// Assigned suspension event sequence, after snapshot and event commit together.
        reply: oneshot::Sender<Result<EventSeq, WriterError>>,
    },
    /// Truncate all materialized views and replay from the event log.
    RebuildViews {
        /// Reply channel.
        reply: oneshot::Sender<Result<(), WriterError>>,
    },
    /// Strict-ordering ack — once the writer dequeues this command, every
    /// previously enqueued command has been committed.
    Flush {
        /// Reply channel; `Ok(())` once Flush is processed (everything before is done).
        reply: oneshot::Sender<Result<(), WriterError>>,
    },
    /// Cooperative shutdown. Reply is sent right before the loop exits.
    Shutdown {
        /// Reply channel signalled just before the writer task exits.
        reply: oneshot::Sender<()>,
    },
}

/// Configuration passed to the writer task at spawn.
pub struct WriterConfig {
    /// Run id this writer is bound to. Used in tracing spans.
    pub run_id: RunId,
    /// Path to the per-run SQLite events database file.
    pub events_db_path: PathBuf,
    /// Directory where artifact bytes are written.
    pub artifacts_dir: PathBuf,
    /// Clock used to stamp event `timestamp` columns.
    pub clock: Arc<dyn Clock>,
    /// Interval (seconds) between background `wal_checkpoint(TRUNCATE)` calls.
    pub checkpoint_interval_secs: u64,
}

/// Spawn the writer task. Returns a sender for commands and the join handle.
///
/// `capacity` bounds the mpsc channel; backpressure kicks in once full.
#[must_use]
pub(crate) fn spawn_writer(
    cfg: WriterConfig,
    capacity: usize,
    lease: Arc<WriterLease>,
) -> (
    mpsc::Sender<WriterCommand>,
    tokio::task::JoinHandle<Result<(), WriterError>>,
) {
    let (tx, rx) = mpsc::channel(capacity);
    let join = tokio::spawn(async move { writer_loop(cfg, rx, lease).await });
    (tx, join)
}

async fn writer_loop(
    cfg: WriterConfig,
    mut rx: mpsc::Receiver<WriterCommand>,
    lease: Arc<WriterLease>,
) -> Result<(), WriterError> {
    let span = tracing::info_span!("writer_task", run_id = %cfg.run_id);
    let _enter = span.enter();

    let mut conn = Connection::open(&cfg.events_db_path)?;
    crate::runs::pragmas::apply(&conn, crate::runs::pragmas::PER_RUN_PRAGMAS)?;

    let mut checkpoint_interval = tokio::time::interval(std::time::Duration::from_secs(
        cfg.checkpoint_interval_secs.max(1),
    ));
    checkpoint_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    checkpoint_interval.tick().await; // skip immediate first tick

    loop {
        tokio::select! {
            biased;
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                if !handle_command(&mut conn, &cfg, cmd).await {
                    break;
                }
            }
            _ = checkpoint_interval.tick() => {
                if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
                    tracing::warn!(error = %e, "wal_checkpoint failed");
                }
            }
        }
    }

    tracing::debug!("writer task exiting");
    drop(conn);
    drop(lease);
    Ok(())
}

async fn handle_command(conn: &mut Connection, cfg: &WriterConfig, cmd: WriterCommand) -> bool {
    match cmd {
        WriterCommand::AppendEvent { payload, reply } => {
            let result = append_event(conn, cfg.clock.as_ref(), &payload);
            let _ = reply.send(result);
        },
        WriterCommand::AppendBatch { payloads, reply } => {
            let result = (|| -> Result<Vec<EventSeq>, WriterError> {
                let mut seqs = Vec::with_capacity(payloads.len());
                let tx = conn.transaction()?;
                for payload in &payloads {
                    let blob = serde_json::to_vec(payload)?;
                    let kind = payload.payload().discriminant_str();
                    let ts = cfg.clock.now_ms();
                    let schema_version = i64::from(payload.schema_version());
                    let seq: i64 = tx.query_row(
                        "INSERT INTO events (timestamp, kind, payload, schema_version)
                         VALUES (?, ?, ?, ?) RETURNING seq",
                        params![ts, kind, blob, schema_version],
                        |row| row.get(0),
                    )?;
                    let seq = EventSeq(seq as u64);
                    views::maintain(&tx, seq, ts, payload.payload())?;
                    seqs.push(seq);
                }
                tx.commit()?;
                Ok(seqs)
            })();
            let _ = reply.send(result);
        },
        WriterCommand::CommitGateAnswer {
            prefix,
            request,
            response,
            reply,
        } => {
            let _ = reply.send(commit_gate_answer(conn, cfg, prefix, &request, response));
        },
        WriterCommand::CommitStageRoute {
            prefix,
            payloads,
            blob,
            reply,
        } => {
            let _ = reply.send(commit_stage_route(conn, cfg, prefix, &payloads, &blob));
        },
        WriterCommand::Flush { reply } => {
            // Strict-ordering ack: by the time the writer dequeues this command,
            // every previously enqueued command has been processed and committed.
            let _ = reply.send(Ok(()));
        },
        WriterCommand::StoreArtifact {
            name,
            content,
            produced_by,
            produced_at_seq,
            reply,
        } => {
            use std::io::Write;

            use rusqlite::OptionalExtension;
            use surge_core::ContentHash;

            let result = (|| -> Result<ArtifactRecord, WriterError> {
                let dir = &cfg.artifacts_dir;
                std::fs::create_dir_all(dir)?;

                let target = dir.join(&name);
                let parent = target.parent().unwrap_or(dir);

                // ContentHash::compute is the M1 constructor for sha256-of-bytes;
                // it lives in surge-core::content_hash and uses the workspace `sha2` dep.
                let hash = ContentHash::compute(&content);
                let size = content.len() as u64;

                let tx = conn.transaction()?;

                let exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM artifacts WHERE id = ?",
                        params![hash.to_string()],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);

                if exists {
                    tracing::debug!(hash = %hash, "artifact dedup: skipping FS write");
                    let row: (String, String) = tx.query_row(
                        "SELECT path, name FROM artifacts WHERE id = ? LIMIT 1",
                        params![hash.to_string()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )?;
                    tx.commit()?;
                    return Ok(ArtifactRecord {
                        id: hash,
                        produced_by_node: produced_by,
                        produced_at_seq,
                        name: row.1,
                        path: dir.join(row.0),
                        size_bytes: size,
                        content_hash: hash,
                    });
                }

                // Atomic write: tmp + rename.
                let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
                tmp.as_file_mut().write_all(&content)?;
                tmp.as_file_mut().sync_all()?;
                tmp.persist(&target).map_err(|e| WriterError::Io(e.error))?;

                let rel_path = name.clone();
                tx.execute(
                    "INSERT INTO artifacts (id, produced_by_node, produced_at_seq, name, path, size_bytes, content_hash)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        hash.to_string(),
                        produced_by.as_ref().map(NodeKey::as_str),
                        produced_at_seq.0 as i64,
                        name,
                        rel_path,
                        size as i64,
                        hash.to_string(),
                    ],
                )?;
                tx.commit()?;

                Ok(ArtifactRecord {
                    id: hash,
                    produced_by_node: produced_by,
                    produced_at_seq,
                    name,
                    path: target,
                    size_bytes: size,
                    content_hash: hash,
                })
            })();
            let _ = reply.send(result);
        },
        WriterCommand::WriteSnapshot {
            at_seq,
            blob,
            reply,
        } => {
            let result = (|| -> Result<(), WriterError> {
                let bytes = blob.len() as i64;
                conn.execute(
                    "INSERT OR REPLACE INTO graph_snapshots (at_seq, snapshot, bytes_compressed)
                     VALUES (?, ?, ?)",
                    params![at_seq.0 as i64, blob, bytes],
                )?;
                Ok(())
            })();
            let _ = reply.send(result);
        },
        WriterCommand::SealSuspension { fence, blob, reply } => {
            let result = seal_suspension(conn, cfg, fence, blob);
            let _ = reply.send(result);
        },
        WriterCommand::RebuildViews { reply } => {
            let result = (|| -> Result<(), WriterError> {
                let tx = conn.transaction()?;
                views::rebuild(&tx)?;

                // Replay all events through views::maintain.
                let mut stmt =
                    tx.prepare("SELECT seq, timestamp, payload FROM events ORDER BY seq")?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        EventSeq(row.get::<_, i64>(0)? as u64),
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })?;
                let collected: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
                drop(stmt);

                for (seq, ts, blob) in collected {
                    let versioned: VersionedEventPayload = serde_json::from_slice(&blob)?;
                    views::maintain(&tx, seq, ts, versioned.payload())?;
                }

                tx.commit()?;
                Ok(())
            })();
            let _ = reply.send(result);
        },
        WriterCommand::Shutdown { reply } => {
            let _ = reply.send(());
            return false;
        },
    }
    true
}

/// The single-event production transaction shared with informational recovery.
pub(crate) fn append_event(
    conn: &mut Connection,
    clock: &dyn Clock,
    payload: &VersionedEventPayload,
) -> Result<EventSeq, WriterError> {
    let tx = conn.transaction()?;
    let seq = append_event_in(&tx, clock, payload)?;
    tx.commit()?;
    Ok(seq)
}

/// Caller retains the same actual writer lease and transaction through validation.
pub(crate) fn append_event_in(
    tx: &rusqlite::Transaction<'_>,
    clock: &dyn Clock,
    payload: &VersionedEventPayload,
) -> Result<EventSeq, WriterError> {
    let blob = serde_json::to_vec(payload)?;
    let kind = payload.payload().discriminant_str();
    let ts = clock.now_ms();
    let schema_version = i64::from(payload.schema_version());
    let seq: i64 = tx.query_row(
        "INSERT INTO events (timestamp, kind, payload, schema_version)
         VALUES (?, ?, ?, ?) RETURNING seq",
        params![ts, kind, blob, schema_version],
        |row| row.get(0),
    )?;
    let seq = EventSeq(seq as u64);
    views::maintain(tx, seq, ts, payload.payload())?;
    Ok(seq)
}

fn commit_stage_route(
    conn: &mut Connection,
    cfg: &WriterConfig,
    prefix: EventSeq,
    payloads: &[VersionedEventPayload],
    blob: &[u8],
) -> Result<EventSeq, WriterError> {
    use surge_core::run_event::EventPayload;
    let all = payloads;
    // Task-scoped records (a split, a human override) lead a route batch so
    // they commit atomically with the route and its snapshot.
    let leading = all
        .iter()
        .take_while(|payload| {
            matches!(
                payload.payload(),
                EventPayload::TaskSplit { .. }
                    | EventPayload::TaskAcceptedByHuman { .. }
                    | EventPayload::RequirementRevised { .. }
            )
        })
        .count();
    if leading > 2 {
        return Err(WriterError::OperationRejected(
            "stage route carries too many task records".into(),
        ));
    }
    let payloads = &all[leading..];
    if !(2..=3).contains(&payloads.len())
        || !matches!(payloads[0].payload(), EventPayload::EdgeTraversed { .. })
        || !matches!(payloads[1].payload(), EventPayload::StageCompleted { .. })
        || (payloads.len() == 3
            && !matches!(
                payloads[2].payload(),
                EventPayload::StageRouteCommitted { .. }
                    | EventPayload::GateStageRouteCommitted { .. }
            ))
    {
        return Err(WriterError::OperationRejected(
            "stage route requires an edge and stage completion".into(),
        ));
    }
    let tx = conn.transaction()?;
    let current: u64 = tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
        row.get(0)
    })?;
    if current != prefix.as_u64() {
        return Err(WriterError::OperationRejected(
            "stage route journal prefix changed before commit".into(),
        ));
    }
    if let EventPayload::StageRouteCommitted {
        invocation,
        outcome_commit_seq,
    } = payloads
        .last()
        .map(VersionedEventPayload::payload)
        .ok_or_else(|| WriterError::OperationRejected("empty stage route".into()))?
    {
        let accepted: Option<(String, String)> = tx.query_row(
            "SELECT node_id,outcome FROM stage_outcome_commits WHERE invocation=? AND committed_seq=? AND routed_seq IS NULL",
            params![invocation.as_ulid().to_string(),outcome_commit_seq], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        let matches = accepted.is_some_and(|(node, outcome)|
            matches!(payloads[0].payload(), EventPayload::EdgeTraversed { from, .. } if from.as_str()==node)
            && matches!(payloads[1].payload(), EventPayload::StageCompleted { node: completed_node, outcome: completed_outcome } if completed_node.as_str()==node && completed_outcome.as_str()==outcome));
        if !matches {
            return Err(WriterError::OperationRejected(
                "stage route contradicts accepted invocation identity".into(),
            ));
        }
    }
    if let Some(EventPayload::GateStageRouteCommitted {
        request,
        stage_entry_seq,
        outcome_commit_seq,
    }) = payloads.last().map(VersionedEventPayload::payload)
    {
        let accepted: Option<(String,String)> = tx.query_row(
            "SELECT node_id,outcome FROM gate_stage_commits WHERE request_id=? AND stage_entry_seq=? AND committed_seq=? AND disposition=? AND routed_seq IS NULL",
            params![request.as_ulid().to_string(),stage_entry_seq,outcome_commit_seq,"\"route\""], |row|Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        let matches = accepted.is_some_and(|(node,outcome)|
            matches!(payloads[0].payload(), EventPayload::EdgeTraversed { from,.. } if from.as_str()==node)
            && matches!(payloads[1].payload(), EventPayload::StageCompleted { node:completed,outcome:actual } if completed.as_str()==node && actual.as_str()==outcome));
        if !matches {
            return Err(WriterError::OperationRejected(
                "gate route contradicts accepted decision identity".into(),
            ));
        }
    }
    let mut final_seq = prefix;
    for payload in all {
        let timestamp = cfg.clock.now_ms();
        let assigned: u64 = tx.query_row(
            "INSERT INTO events(timestamp,kind,payload,schema_version) VALUES(?,?,?,?) RETURNING seq",
            params![timestamp, payload.payload().discriminant_str(), serde_json::to_vec(payload)?, payload.schema_version()],
            |row| row.get(0),
        )?;
        final_seq = EventSeq(assigned);
        views::maintain(&tx, final_seq, timestamp, payload.payload())?;
    }
    tx.execute(
        "INSERT INTO graph_snapshots(at_seq,snapshot,bytes_compressed) VALUES(?,?,?)",
        params![final_seq.as_u64(), blob, blob.len() as u64],
    )?;
    tx.commit()?;
    Ok(final_seq)
}

fn seal_suspension(
    conn: &mut Connection,
    cfg: &WriterConfig,
    fence: surge_core::execution_recovery::SuspensionFence,
    blob: Vec<u8>,
) -> Result<EventSeq, WriterError> {
    if fence.control_generation == 0 || !fence.cleanup_confirmed {
        return Err(WriterError::Internal(
            "suspension requires confirmed cleanup and a nonzero control generation".into(),
        ));
    }
    let tx = conn.transaction()?;
    let prefix: u64 = tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
        row.get(0)
    })?;
    if prefix != fence.snapshot_seq {
        return Err(WriterError::OperationRejected(
            "suspension snapshot prefix changed before commit".into(),
        ));
    }
    let payload =
        VersionedEventPayload::new(surge_core::run_event::EventPayload::RunSuspended { fence });
    let timestamp = cfg.clock.now_ms();
    let seq: u64 = tx.query_row(
        "INSERT INTO events(timestamp,kind,payload,schema_version) VALUES(?,?,?,?) RETURNING seq",
        params![
            timestamp,
            payload.payload().discriminant_str(),
            serde_json::to_vec(&payload)?,
            payload.schema_version()
        ],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO graph_snapshots(at_seq,snapshot,bytes_compressed) VALUES(?,?,?)",
        params![prefix, &blob, blob.len() as u64],
    )?;
    views::maintain(&tx, EventSeq(seq), timestamp, payload.payload())?;
    tx.commit()?;
    Ok(EventSeq(seq))
}

fn commit_gate_answer(
    conn: &mut Connection,
    cfg: &WriterConfig,
    prefix: EventSeq,
    request: &surge_core::execution_recovery::gate_commit::GateCommitRequest,
    response: serde_json::Value,
) -> Result<EventSeq, WriterError> {
    let tx = conn.transaction()?;
    let actual: u64 = tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
        row.get(0)
    })?;
    if actual != prefix.as_u64() {
        return Err(WriterError::OperationRejected(
            "gate answer prefix changed".into(),
        ));
    }
    let entry: Vec<u8> = tx.query_row(
        "SELECT payload FROM events WHERE seq=?",
        [request.stage_entry_seq()],
        |row| row.get(0),
    )?;
    let original: Vec<u8> = tx.query_row(
        "SELECT payload FROM events WHERE seq=?",
        [request.requested_seq()],
        |row| row.get(0),
    )?;
    let entry: VersionedEventPayload = serde_json::from_slice(&entry)?;
    let original: VersionedEventPayload = serde_json::from_slice(&original)?;
    if !matches!(entry.payload(),surge_core::EventPayload::StageEntered {node,..} if node==request.node())
        || !matches!(original.payload(),surge_core::EventPayload::HumanInputRequested {node,session:None,call_id:Some(call),..}
            if node==request.node() && surge_core::id::GateRequestId::from_event_call_id(call)==Some(request.request()))
    {
        return Err(WriterError::OperationRejected(
            "gate answer has no original request occurrence".into(),
        ));
    }
    let latest_entry: u64 = tx.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM events WHERE kind='StageEntered'",
        [],
        |row| row.get(0),
    )?;
    let closed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE seq>? AND kind IN ('HumanInputResolved','HumanInputTimedOut','StageCompleted','RunCompleted','RunFailed','RunAborted'))",[request.requested_seq()],|row|row.get(0))?;
    if latest_entry != request.stage_entry_seq() || closed {
        return Err(WriterError::OperationRejected(
            "gate request is no longer pending".into(),
        ));
    }
    let payload = VersionedEventPayload::new(surge_core::EventPayload::HumanInputResolved {
        node: request.node().clone(),
        call_id: Some(request.request().to_string()),
        response,
    });
    let ts = cfg.clock.now_ms();
    let seq: u64 = tx.query_row(
        "INSERT INTO events(timestamp,kind,payload,schema_version) VALUES(?,?,?,?) RETURNING seq",
        params![
            ts,
            payload.payload().discriminant_str(),
            serde_json::to_vec(&payload)?,
            payload.schema_version()
        ],
        |row| row.get(0),
    )?;
    let seq = EventSeq(seq);
    views::maintain(&tx, seq, ts, payload.payload())?;
    tx.commit()?;
    Ok(seq)
}
