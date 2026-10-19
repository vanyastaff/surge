//! Correctness version for recomputable views. Schema installation never certifies replay.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use surge_core::migrate_payload;

use super::{EventSeq, StorageError, WriterError, views};

const CURRENT: i64 = 1;

fn version(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    let present: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='projection_semantics')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        return Ok(None);
    }
    conn.query_row(
        "SELECT version FROM projection_semantics WHERE singleton=1",
        [],
        |row| row.get(0),
    )
    .optional()
}

// Call inside the same read transaction that consumes normalized view rows.
pub(super) fn require_current(conn: &Connection) -> Result<(), StorageError> {
    let observed = version(conn)?;
    if observed == Some(CURRENT) {
        Ok(())
    } else {
        Err(StorageError::MigrationFailed(format!(
            "unconfirmed or unsupported projection semantics {observed:?}; expected {CURRENT}; exclusive owned replay required"
        )))
    }
}

// Called only with both run writer guards, before any writer/pool publication.
pub(super) fn upgrade_owned(conn: &mut Connection) -> Result<(), WriterError> {
    match version(conn)? {
        Some(CURRENT) => Ok(()),
        Some(0) => rebuild_owned(conn),
        other => Err(WriterError::Internal(format!(
            "unsupported projection semantics {other:?}; expected 0 or {CURRENT}"
        ))),
    }
}

// Shared by owner startup and explicit rebuild. Stream one journal row at a time;
// immutable checkpoints and the journal itself never enter the reset set.
pub(super) fn rebuild_owned(conn: &mut Connection) -> Result<(), WriterError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    match version(&tx)? {
        Some(0 | CURRENT) => {},
        other => {
            return Err(WriterError::Internal(format!(
                "unsupported projection semantics {other:?}; expected 0 or {CURRENT}"
            )));
        },
    }
    views::rebuild(&tx)?;
    {
        let mut statement =
            tx.prepare("SELECT seq,timestamp,payload,schema_version FROM events ORDER BY seq")?;
        let mut rows = statement.query([])?;
        let mut expected_seq = 1;
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let timestamp: i64 = row.get(1)?;
            let bytes: Vec<u8> = row.get(2)?;
            let recorded: i64 = row.get(3)?;
            let schema = u32::try_from(recorded).map_err(|error| {
                WriterError::Internal(format!("invalid event schema at seq={seq}: {error}"))
            })?;
            let seq = u64::try_from(seq)
                .ok()
                .filter(|seq| *seq > 0)
                .ok_or_else(|| WriterError::Internal("invalid replay event sequence".into()))?;
            if seq != expected_seq {
                return Err(WriterError::Internal(format!(
                    "incomplete replay journal: expected seq={expected_seq}, got seq={seq}"
                )));
            }
            let payload = migrate_payload(schema, &bytes).map_err(|error| {
                WriterError::Internal(format!("event migration failed at seq={seq}: {error}"))
            })?;
            views::maintain(&tx, EventSeq(seq), timestamp, &payload)?;
            // SQLite sequence values are positive i64, so the u64 successor cannot overflow.
            expected_seq = seq + 1;
        }
    }
    tx.execute(
        "UPDATE projection_semantics SET version=? WHERE singleton=1",
        [CURRENT],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::migrations::{PER_RUN_MIGRATIONS, apply};
    use surge_core::{EventPayload, VersionedEventPayload};

    fn database() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PER_RUN_MIGRATIONS, &MockClock::new(100)).unwrap();
        conn
    }

    #[test]
    fn replay_uses_legacy_schema_and_preserves_raw_bytes() {
        let mut conn = database();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "payload": EventPayload::StageEntered { node: "old_worker".parse().unwrap(), attempt: 2 },
        })).unwrap();
        conn.execute("INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(1,42,'StageEntered',?,1)",[&bytes]).unwrap();
        upgrade_owned(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), Some(CURRENT));
        let row: (String, i64, i64, i64) = conn
            .query_row(
                "SELECT node_id,attempt,started_seq,started_at FROM stage_executions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, ("old_worker".into(), 2, 1, 42));
        assert_eq!(
            conn.query_row("SELECT payload FROM events WHERE seq=1", [], |row| row
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            bytes
        );
        rebuild_owned(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), Some(CURRENT));
    }

    #[test]
    fn explicit_rebuild_refuses_future_event_schema_even_with_current_decodable_bytes() {
        let mut conn = database();
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(EventPayload::RunAborted {
            reason: "current decodable payload".into(),
        }))
        .unwrap();
        conn.execute("INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(1,42,'RunAborted',?,?)",rusqlite::params![&bytes,i64::from(surge_core::migrations::MAX_SUPPORTED_VERSION)+1]).unwrap();
        conn.execute("INSERT INTO stage_executions(node_id,attempt,started_seq,started_at) VALUES('prior_view',1,1,42)",[]).unwrap();
        let error = rebuild_owned(&mut conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("event migration failed at seq=1"),
            "{error}"
        );
        assert_eq!(version(&conn).unwrap(), Some(0));
        assert_eq!(
            conn.query_row("SELECT node_id FROM stage_executions", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "prior_view"
        );
        assert_eq!(
            conn.query_row("SELECT payload FROM events WHERE seq=1", [], |row| row
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn replay_unavailable_cost_does_not_promote_legacy_aggregate_to_known_spend() {
        let mut conn = database();
        let session = surge_core::SessionId::new();
        for (index, payload) in [
            EventPayload::StageEntered {
                node: "worker".parse().unwrap(),
                attempt: 1,
            },
            EventPayload::SessionOpened {
                node: "worker".parse().unwrap(),
                session,
                agent: "fixture".into(),
                agent_id: None,
                opened: None,
                handoff: None,
            },
            EventPayload::TokensConsumed {
                session,
                prompt_tokens: 11,
                output_tokens: 7,
                cache_hits: 0,
                model: "fixture".into(),
                cost_usd: None,
            },
        ]
        .into_iter()
        .enumerate()
        {
            let payload = VersionedEventPayload::new(payload);
            conn.execute(
                "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(?,42,?,?,?)",
                rusqlite::params![
                    index as i64 + 1,
                    payload.payload().discriminant_str(),
                    serde_json::to_vec(&payload).unwrap(),
                    payload.schema_version()
                ],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO stage_executions(node_id,attempt,started_seq,started_at,cost_usd) VALUES('worker',1,1,42,99)",[]).unwrap();
        upgrade_owned(&mut conn).unwrap();
        let row: (Option<f64>, bool, i64, i64) = conn
            .query_row(
                "SELECT known_cost_usd,cost_unknown,tokens_in,tokens_out FROM stage_executions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (None, true, 11, 7));
    }
}
