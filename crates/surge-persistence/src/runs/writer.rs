//! Writer task: single-threaded SQLite write loop driven by a bounded mpsc.
//!
//! The writer owns one `rusqlite::Connection` and processes one command at a
//! time. All event INSERTs and view maintenance happen in the same SQL
//! transaction inside the writer task; readers go through a separate r2d2
//! pool and never touch the writer connection.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, params};
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
        payload: VersionedEventPayload,
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
pub fn spawn_writer(
    cfg: WriterConfig,
    capacity: usize,
) -> (
    mpsc::Sender<WriterCommand>,
    tokio::task::JoinHandle<Result<(), WriterError>>,
) {
    let (tx, rx) = mpsc::channel(capacity);
    let join = tokio::spawn(async move { writer_loop(cfg, rx).await });
    (tx, join)
}

async fn writer_loop(
    cfg: WriterConfig,
    mut rx: mpsc::Receiver<WriterCommand>,
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
    Ok(())
}

async fn handle_command(
    conn: &mut Connection,
    cfg: &WriterConfig,
    cmd: WriterCommand,
) -> bool {
    match cmd {
        WriterCommand::AppendEvent { payload, reply } => {
            let result = (|| -> Result<EventSeq, WriterError> {
                let blob = serde_json::to_vec(&payload)?;
                let kind = payload.payload().discriminant_str();
                let ts = cfg.clock.now_ms();
                let schema_version = i64::from(payload.schema_version());

                let tx = conn.transaction()?;
                let seq: i64 = tx.query_row(
                    "INSERT INTO events (timestamp, kind, payload, schema_version)
                     VALUES (?, ?, ?, ?) RETURNING seq",
                    params![ts, kind, blob, schema_version],
                    |row| row.get(0),
                )?;
                let seq = EventSeq(seq as u64);
                views::maintain(&tx, seq, ts, payload.payload())?;
                tx.commit()?;
                Ok(seq)
            })();
            let _ = reply.send(result);
        }
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
        }
        WriterCommand::Flush { reply } => {
            // Strict-ordering ack: by the time the writer dequeues this command,
            // every previously enqueued command has been processed and committed.
            let _ = reply.send(Ok(()));
        }
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
        }
        WriterCommand::Shutdown { reply } => {
            let _ = reply.send(());
            return false;
        }
        // WriteSnapshot / RebuildViews — Task 5.4
        cmd => {
            // Stub: tell caller the command is not yet wired in this skeleton commit.
            stub_reject(cmd);
        }
    }
    true
}

fn stub_reject(cmd: WriterCommand) {
    use WriterCommand::*;
    let err = || WriterError::Internal("command not yet implemented in skeleton".into());
    match cmd {
        WriteSnapshot { reply, .. } => {
            let _ = reply.send(Err(err()));
        }
        RebuildViews { reply } => {
            let _ = reply.send(Err(err()));
        }
        AppendEvent { .. }
        | AppendBatch { .. }
        | StoreArtifact { .. }
        | Flush { .. }
        | Shutdown { .. } => {
            unreachable!("handled in handle_command")
        }
    }
}
