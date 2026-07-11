//! Shared plumbing used by ≥2 of the other `store::*` submodules.

use super::*;
use crate::error::storage_err;

/// Read one `meta` value; `None` when the key was never seeded.
pub(crate) fn meta_value(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        rusqlite::params![key],
        |r| r.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(storage_err(other)),
    })
}
/// Run `body` inside one `BEGIN IMMEDIATE` transaction with RAII drop-rollback
/// (W1.6a).
///
/// On a body error the `Transaction` guard drops and rolls the transaction
/// back. On a failed COMMIT the guard (consumed by `commit`) still drops and
/// attempts ROLLBACK, because rusqlite's drop path only skips rollback when the
/// connection is already back in autocommit. The manual BEGIN/COMMIT/ROLLBACK
/// pattern this replaces left the connection mid-transaction in exactly the
/// failed-COMMIT case, poisoning every later writer op with "cannot start a
/// transaction within a transaction" (F07/F16). A leaked transaction that even
/// the drop-rollback cannot clear is caught by the writer's post-op
/// `is_autocommit` check (W1.6b).
///
/// Statements inside `body` run on the same connection, so they participate in
/// the transaction without touching the guard.
pub(crate) fn immediate_tx<T>(
    conn: &rusqlite::Connection,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(storage_err)?;
    let value = body()?;
    tx.commit().map_err(storage_err)?;
    Ok(value)
}
/// Append one `memory_oplog` row for a mutation on `memory_id`, resolving the
/// namespace from the memories row (already written within the same
/// transaction for inserts). MUST be called inside the mutation's transaction
/// so the log row commits — or rolls back — with the mutation itself. Callers
/// gate on the mutation having actually changed a row, so the SELECT always
/// resolves; `details` is a small JSON payload (e.g. link type) or empty.
pub(crate) fn append_oplog(
    conn: &rusqlite::Connection,
    site_id: &str,
    op: &str,
    memory_id: &MemoryId,
    details: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
         SELECT ?1, ?2, ?3, namespace, ?4, ?5 FROM memories WHERE memory_id = ?3",
        rusqlite::params![
            site_id,
            op,
            memory_id.to_string(),
            chrono::Utc::now().timestamp(),
            details
        ],
    )
    .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}
pub(crate) fn embedding_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf
}
/// Decode a little-endian `f32` blob (the exact byte layout `embedding_bytes`
/// writes) back into a `Vec<f32>`. Fail closed if the length is not a multiple
/// of four bytes rather than silently truncating a corrupt vector.
pub(crate) fn decode_embedding_bytes(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::Storage(format!(
            "stored embedding blob length {} is not a multiple of 4",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        // chunks_exact(4) yields slices of exactly 4 bytes; the conversion to a
        // [u8; 4] cannot fail, but we handle it explicitly to avoid unwrap.
        let arr: [u8; 4] = chunk
            .try_into()
            .map_err(|_| Error::Storage("embedding chunk was not 4 bytes".to_string()))?;
        out.push(f32::from_le_bytes(arr));
    }
    Ok(out)
}
/// Decode a stored unix-seconds timestamp, failing closed on out-of-range
/// values rather than silently fabricating an epoch-0 datetime.
pub(crate) fn from_ts(secs: i64) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .ok_or_else(|| Error::Storage(format!("timestamp {secs} out of range")))
}
pub(crate) fn parse_id(s: &str) -> Result<MemoryId> {
    s.parse::<MemoryId>()
}
