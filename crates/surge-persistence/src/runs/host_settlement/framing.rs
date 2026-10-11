//! Exact raw-row framing. A digest is not an execution receipt or folded-outcome authorization.
use super::super::{connection, error::StorageError, subscribe::SUBSCRIBE_BATCH_MAX};
use rusqlite::Transaction;
use sha2::{Digest, Sha256};
use surge_core::RunId;

fn invalid(message: &'static str) -> StorageError {
    StorageError::OperationRejected(message.into())
}

pub(super) fn header(run: RunId, identity: &[u8], sealed: u64) -> Result<Vec<u8>, StorageError> {
    if !matches!(
        (identity.first(), identity.len()),
        (Some(1), 65) | (Some(2), 97)
    ) {
        return Err(invalid("unsupported journal identity framing"));
    }
    i64::try_from(sealed).map_err(|_| invalid("sealed sequence exceeds SQLite domain"))?;
    let id = run.to_string();
    if id.parse::<RunId>().ok() != Some(run) {
        return Err(invalid("noncanonical run identifier"));
    }
    let length = u32::try_from(id.len()).map_err(|_| invalid("run identifier length overflow"))?;
    let mut bytes = b"surge.host-settlement.journal-prefix.v1\0".to_vec();
    bytes.extend(length.to_le_bytes());
    bytes.extend(id.as_bytes());
    bytes.extend(identity);
    bytes.extend(sealed.to_le_bytes());
    Ok(bytes)
}
pub(super) struct RawRow<'a> {
    pub sequence: i64,
    pub timestamp: i64,
    pub version: i64,
    pub kind: &'a str,
    pub payload: &'a [u8],
}
pub(super) fn row_frame(
    mut output: impl FnMut(&[u8]),
    row: &RawRow<'_>,
) -> Result<(), StorageError> {
    let sequence = u64::try_from(row.sequence).map_err(|_| invalid("negative journal sequence"))?;
    if sequence == 0 {
        return Err(invalid("zero journal sequence"));
    }
    let version =
        u32::try_from(row.version).map_err(|_| invalid("stored schema version outside u32"))?;
    let decoded = surge_core::migrate_payload(version, row.payload)
        .map_err(|error| StorageError::MigrationFailed(error.to_string()))?;
    if decoded.discriminant_str() != row.kind {
        return Err(invalid("stored event kind disagrees with decoded payload"));
    }
    let kind_length =
        u32::try_from(row.kind.len()).map_err(|_| invalid("event kind length overflow"))?;
    let payload_length =
        u64::try_from(row.payload.len()).map_err(|_| invalid("event payload length overflow"))?;
    output(&sequence.to_le_bytes());
    output(&row.timestamp.to_le_bytes());
    output(&version.to_le_bytes());
    output(&kind_length.to_le_bytes());
    output(row.kind.as_bytes());
    output(&payload_length.to_le_bytes());
    output(row.payload);
    Ok(())
}
fn counts(tx: &Transaction<'_>, sealed: i64) -> Result<(), StorageError> {
    let (maximum, count): (i64, i64) = tx.query_row(
        "SELECT COALESCE(MAX(seq),0),COUNT(*) FROM events",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if maximum != sealed || count != sealed {
        return Err(invalid(
            "journal count or maximum differs from sealed prefix",
        ));
    }
    Ok(())
}
/// One actual read transaction covers both aggregate checks and every streamed row.
pub(super) fn snapshot(
    connection: &mut connection::ManagedConnection,
    run: RunId,
    identity: &[u8],
    sealed: u64,
) -> Result<[u8; 32], StorageError> {
    let mut hash = Sha256::new();
    hash.update(header(run, identity, sealed)?);
    let sealed_sql =
        i64::try_from(sealed).map_err(|_| invalid("sealed sequence exceeds SQLite domain"))?;
    let tx = connection.transaction()?;
    counts(&tx, sealed_sql)?;
    let mut previous = 0_i64;
    let mut total_bytes = 0_u64;
    while previous < sealed_sql {
        let mut statement=tx.prepare("SELECT seq,timestamp,schema_version,kind,payload FROM events WHERE seq>? ORDER BY seq LIMIT ?")?;
        let mut rows = statement.query(rusqlite::params![previous, SUBSCRIBE_BATCH_MAX as i64])?;
        let mut batch = 0_usize;
        while let Some(row) = rows.next()? {
            let sequence: i64 = row.get(0)?;
            if previous.checked_add(1) != Some(sequence) || sequence > sealed_sql {
                return Err(invalid("journal gap or suffix outside sealed prefix"));
            }
            let timestamp: i64 = row.get(1)?;
            let version: i64 = row.get(2)?;
            let kind_value = row.get_ref(3)?;
            let payload_value = row.get_ref(4)?;
            let kind = kind_value
                .as_str()
                .map_err(|_| invalid("journal kind is not UTF-8 text"))?;
            let payload = payload_value
                .as_blob()
                .map_err(|_| invalid("journal payload is not a blob"))?;
            let row_bytes = u64::try_from(kind.len())
                .ok()
                .and_then(|n| n.checked_add(u64::try_from(payload.len()).ok()?))
                .and_then(|n| n.checked_add(32))
                .ok_or_else(|| invalid("journal row byte count overflow"))?;
            total_bytes = total_bytes
                .checked_add(row_bytes)
                .ok_or_else(|| invalid("journal total byte count overflow"))?;
            row_frame(
                |bytes| hash.update(bytes),
                &RawRow {
                    sequence,
                    timestamp,
                    version,
                    kind,
                    payload,
                },
            )?;
            previous = sequence;
            batch += 1;
        }
        if batch == 0 {
            return Err(invalid("journal ended before sealed prefix"));
        }
    }
    counts(&tx, sealed_sql)?;
    tx.commit()?;
    Ok(hash.finalize().into())
}
