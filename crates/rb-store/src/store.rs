//! `SqliteStore`: the concrete `Store` backed by SQLite + sqlite-vec.

use crate::error::{io_err, storage_err};
use crate::migrations::run_migrations;
use rb_types::{Error, MemoryId, MemoryLink, MemoryNote, MemoryUpdates, Namespace, Result};
use std::path::Path;

/// The synchronous storage trait. The daemon wraps this on blocking threads.
pub trait Store {
    fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()>;
    fn get_memory(&self, id: &MemoryId) -> Result<Option<MemoryNote>>;
    fn keyword_search(&self, ns: &Namespace, query: &str, limit: usize) -> Result<Vec<MemoryId>>;
    fn vector_search(
        &self,
        ns: &Namespace,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>>;
    fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<MemoryId>>;
    fn list(
        &self,
        ns: &Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>>;
    fn update_memory(&self, id: &MemoryId, updates: &MemoryUpdates) -> Result<()>;
    fn archive_memory(&self, id: &MemoryId) -> Result<()>;
    fn add_link(&self, link: &MemoryLink) -> Result<()>;
}

/// SQLite-backed store. Owns a single connection (write path); the daemon owns
/// the read pool separately in P1.
pub struct SqliteStore {
    // rusqlite::Connection doesn't impl Debug, so we derive it manually via a wrapper.
    // pub(crate) for intra-crate access (CRUD methods in Part D); allow dead_code until
    // those methods are added — the field is legitimately used in tests right now.
    #[allow(dead_code)]
    pub(crate) conn: rusqlite::Connection,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Open (or create) a store at `path` with the given embedding dimension.
    ///
    /// Registers sqlite-vec, enables WAL + foreign keys, runs migrations,
    /// creates the dynamic-dim `memory_vectors` table, and enforces the
    /// embedding-dimension invariant fail-closed.
    pub fn open(path: &Path, embedding_dim: usize) -> Result<Self> {
        register_vec();
        let conn = rusqlite::Connection::open(path).map_err(|e| {
            io_err(std::io::Error::other(format!(
                "open {}: {e}",
                path.display()
            )))
        })?;
        Self::init(conn, embedding_dim)
    }

    /// Open an ephemeral in-memory store with the given embedding dimension.
    pub fn open_in_memory(embedding_dim: usize) -> Result<Self> {
        register_vec();
        let conn = rusqlite::Connection::open_in_memory().map_err(storage_err)?;
        Self::init(conn, embedding_dim)
    }

    /// Shared init path: pragmas, migrations, vectors table, dim invariant.
    fn init(conn: rusqlite::Connection, embedding_dim: usize) -> Result<Self> {
        // WAL gives concurrent readers + one writer with no SQLITE_BUSY storms.
        // (In-memory DBs ignore WAL and report "memory"; that is fine.)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_err)?;

        run_migrations(&conn)?;

        // Dynamic-dimension vector table. vec0 needs the literal dim baked in.
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vectors USING vec0(\
               memory_id TEXT PRIMARY KEY,\
               embedding float[{embedding_dim}]\
             );"
        ))
        .map_err(storage_err)?;

        seed_or_verify_dim(&conn, embedding_dim)?;

        Ok(Self { conn })
    }
}

/// Register the sqlite-vec extension so `vec0` virtual tables and the KNN
/// `MATCH` syntax are available on every subsequently opened connection.
fn register_vec() {
    // SAFETY: `sqlite_vec::sqlite3_vec_init` is the FFI entry point published by
    // the sqlite-vec crate. `sqlite3_auto_extension` registers it with SQLite so
    // it runs on each connection opened AFTER this call. We cast the fn pointer
    // exactly as the sqlite-vec crate does in its own test (transmute of a
    // `*const ()`); the target fn-pointer type is inferred from the
    // `sqlite3_auto_extension` argument slot. The init fn is valid for the
    // program's lifetime; re-registration on subsequent `open*` calls is
    // idempotent/benign.
    #[allow(unsafe_code)]
    #[allow(clippy::missing_transmute_annotations)]
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }
}

/// Seed `meta.embedding_dim` on first init, or verify it matches on re-open.
/// Fails closed with `Error::DimensionMismatch` on disagreement.
fn seed_or_verify_dim(conn: &rusqlite::Connection, embedding_dim: usize) -> Result<()> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='embedding_dim'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(storage_err(other)),
        })?;

    match existing {
        Some(v) => {
            let stored: usize = v.parse().map_err(|_| {
                Error::Storage(format!("meta.embedding_dim is not an integer: {v:?}"))
            })?;
            if stored != embedding_dim {
                return Err(Error::DimensionMismatch {
                    expected: stored,
                    got: embedding_dim,
                });
            }
        }
        None => {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('embedding_dim', ?1)",
                rusqlite::params![embedding_dim.to_string()],
            )
            .map_err(storage_err)?;
        }
    }
    Ok(())
}

impl Store for SqliteStore {
    fn insert_memory(&self, _note: &MemoryNote, _embedding: Option<&[f32]>) -> Result<()> {
        unimplemented!("next cluster")
    }
    fn get_memory(&self, _id: &MemoryId) -> Result<Option<MemoryNote>> {
        unimplemented!("next cluster")
    }
    fn keyword_search(
        &self,
        _ns: &Namespace,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryId>> {
        unimplemented!("next cluster")
    }
    fn vector_search(
        &self,
        _ns: &Namespace,
        _embedding: &[f32],
        _limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        unimplemented!("next cluster")
    }
    fn graph_neighbors(&self, _id: &MemoryId, _depth: u8) -> Result<Vec<MemoryId>> {
        unimplemented!("next cluster")
    }
    fn list(
        &self,
        _ns: &Namespace,
        _min_importance: Option<u8>,
        _limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        unimplemented!("next cluster")
    }
    fn update_memory(&self, _id: &MemoryId, _updates: &MemoryUpdates) -> Result<()> {
        unimplemented!("next cluster")
    }
    fn archive_memory(&self, _id: &MemoryId) -> Result<()> {
        unimplemented!("next cluster")
    }
    fn add_link(&self, _link: &MemoryLink) -> Result<()> {
        unimplemented!("next cluster")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;

    #[test]
    fn open_in_memory_creates_schema_and_seeds_dim() {
        let store = SqliteStore::open_in_memory(1024).unwrap();
        let c = &store.conn;

        let table = |name: &str| -> bool {
            let n: i64 = c
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![name],
                    |r| r.get(0),
                )
                .unwrap();
            n == 1
        };

        assert!(table("meta"), "meta exists");
        assert!(table("memories"), "memories exists");
        assert!(table("memory_links"), "memory_links exists");
        assert!(table("memories_fts"), "memories_fts exists");
        assert!(
            table("memory_vectors"),
            "memory_vectors created in code at open"
        );

        // embedding_dim seeded to the requested value.
        let dim: String = c
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_dim'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dim, "1024", "embedding_dim seeded");

        // foreign_keys pragma is ON.
        let fk: i64 = c
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys ON");
    }

    #[test]
    fn open_persists_and_reopen_same_dim_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let _s = SqliteStore::open(&path, 768).unwrap();
        }
        // Re-open with the SAME dim: succeeds, dim unchanged.
        let s2 = SqliteStore::open(&path, 768).unwrap();
        let dim: String = s2
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_dim'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dim, "768");
    }

    #[test]
    fn reopen_with_different_dim_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let _s = SqliteStore::open(&path, 768).unwrap();
        }
        // Re-open with a DIFFERENT dim must fail closed.
        let err = SqliteStore::open(&path, 1024).unwrap_err();
        match err {
            Error::DimensionMismatch { expected, got } => {
                assert_eq!(expected, 768, "stored dim is the expected");
                assert_eq!(got, 1024, "requested dim is what we got");
            }
            other => panic!("expected DimensionMismatch, got {other:?}"),
        }
    }

    #[test]
    fn wal_mode_enabled_for_file_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let s = SqliteStore::open(&path, 256).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "file DB uses WAL");
    }
}
