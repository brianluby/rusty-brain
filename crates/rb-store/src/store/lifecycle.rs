//! Connection lifecycle: open/init, migrations glue, sqlite-vec registration.

use super::internal::*;
use super::*;
use crate::error::{io_err, storage_err};
use crate::migrations::run_migrations;
use std::path::Path;

const OPEN_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const WAL_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

impl SqliteStore {
    /// Open (or create) a store at `path` with the given embedding dimension.
    ///
    /// Registers sqlite-vec, enables WAL + foreign keys, runs migrations,
    /// creates the dynamic-dim `memory_vectors` table, and enforces the
    /// embedding-dimension invariant fail-closed.
    ///
    /// The embedding-MODEL invariant is NOT enforced here (no model is bound);
    /// any path that serves a real embedding provider must open via
    /// [`SqliteStore::open_with_model`] so a same-dim provider swap cannot
    /// silently mix vector spaces.
    pub fn open(path: &Path, embedding_dim: usize) -> Result<Self> {
        Self::open_inner(path, embedding_dim, None)
    }
    /// Open (or create) a store at `path`, enforcing BOTH embedding invariants:
    /// the dimension and the model identity. Seeds `meta.embedding_model` on
    /// first init; fails closed when the stored model differs from
    /// `embedding_model` (remediation: [`SqliteStore::accept_model_change`]).
    pub fn open_with_model(
        path: &Path,
        embedding_dim: usize,
        embedding_model: &str,
    ) -> Result<Self> {
        Self::open_inner(path, embedding_dim, Some(embedding_model))
    }
    fn open_inner(
        path: &Path,
        embedding_dim: usize,
        embedding_model: Option<&str>,
    ) -> Result<Self> {
        Self::open_inner_with_schema_hook(path, embedding_dim, embedding_model, || {})
    }
    /// The real file-open path with a seam that lets concurrency tests pause
    /// after the optimistic vector-schema check. The production monomorph uses
    /// a zero-sized no-op closure, so current-schema opens pay no callback cost.
    fn open_inner_with_schema_hook<F>(
        path: &Path,
        embedding_dim: usize,
        embedding_model: Option<&str>,
        after_vector_schema_read: F,
    ) -> Result<Self>
    where
        F: FnOnce(),
    {
        register_vec()?;
        // The DB file holds captured memory text: owner-only (0600), parity with
        // the daemon socket. Pre-create a missing file at 0600 so it is never
        // observable at the umask-derived default (Connection::open would create
        // it 0644 and leave a read window until the chmod below). Then chmod
        // fail-closed on every open — tightening a loose pre-W0.5 DB — including
        // any leftover `-wal`/`-shm` siblings: SQLite copies the main file's
        // mode only when it CREATES a sibling, so a 0644 WAL surviving an
        // unclean shutdown would otherwise keep collecting memory content at
        // 0644 for the daemon's whole lifetime.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            use std::os::unix::fs::PermissionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)
                .map_err(|e| {
                    io_err(std::io::Error::other(format!(
                        "create 0600 {}: {e}",
                        path.display()
                    )))
                })?;
            for sibling in ["", "-wal", "-shm"] {
                let mut os = path.as_os_str().to_os_string();
                os.push(sibling);
                let p = std::path::PathBuf::from(os);
                match std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)) {
                    Ok(()) => {}
                    // Absent siblings are the common case; only NotFound passes.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(io_err(std::io::Error::other(format!(
                            "chmod 0600 {}: {e}",
                            p.display()
                        ))))
                    }
                }
            }
        }
        let conn = rusqlite::Connection::open(path).map_err(|e| {
            io_err(std::io::Error::other(format!(
                "open {}: {e}",
                path.display()
            )))
        })?;
        #[cfg(test)]
        wait_for_public_open_test_barrier(path);
        Self::init_with_schema_hook(
            conn,
            embedding_dim,
            embedding_model,
            after_vector_schema_read,
        )
    }
    /// Open an ephemeral in-memory store with the given embedding dimension.
    pub fn open_in_memory(embedding_dim: usize) -> Result<Self> {
        register_vec()?;
        let conn = rusqlite::Connection::open_in_memory().map_err(storage_err)?;
        Self::init(conn, embedding_dim, None)
    }
    /// Shared init path: pragmas, migrations, vectors table, dim invariant,
    /// and (when a model is bound) the model-identity invariant.
    fn init(
        conn: rusqlite::Connection,
        embedding_dim: usize,
        embedding_model: Option<&str>,
    ) -> Result<Self> {
        Self::init_with_schema_hook(conn, embedding_dim, embedding_model, || {})
    }
    fn init_with_schema_hook<F>(
        conn: rusqlite::Connection,
        embedding_dim: usize,
        embedding_model: Option<&str>,
        after_vector_schema_read: F,
    ) -> Result<Self>
    where
        F: FnOnce(),
    {
        // A zero-dimension embedding produces a malformed `float[0]` vec0 column.
        // Reject it up front rather than letting SQLite fail cryptically later.
        if embedding_dim == 0 {
            return Err(Error::Storage(
                "embedding_dim must be greater than 0".to_string(),
            ));
        }

        // A busy handler keeps the P1 daemon (multiple connections + WAL
        // checkpoints) from hitting immediate SQLITE_BUSY: a contended write
        // waits up to 5s for the lock instead of failing right away. Install it
        // before WAL negotiation: changing a zero-byte DB's journal mode is
        // itself a write and two first openers can contend there.
        conn.busy_timeout(OPEN_BUSY_TIMEOUT).map_err(storage_err)?;

        // WAL gives concurrent readers + one writer with no SQLITE_BUSY storms.
        // (In-memory DBs ignore WAL and report "memory"; that is fine.)
        enable_wal(&conn)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_err)?;

        run_migrations(&conn)?;

        // Verify the dimension invariant BEFORE touching the vector table: the
        // schema rebuild below re-creates the vec0 table with the configured
        // dim baked into the DDL, so a dim mismatch must fail closed first
        // (rebuilding under a wrong dim would commit a malformed table).
        seed_or_verify_dim(&conn, embedding_dim)?;

        // Verify the model invariant BEFORE touching the vector table too:
        // `ensure_vector_schema`'s rebuild path prunes/reinserts vectors and
        // rescues-or-drops similarity links in its own committed transaction,
        // which must not happen on a DB we are about to reject for a model
        // mismatch. `seed_or_verify_model` only reads/writes `meta`, so it has
        // no dependency on the vector table existing first.
        if let Some(model) = embedding_model {
            seed_or_verify_model(&conn, model)?;
        }

        // Dynamic-dimension vector table (vec0 needs the literal dim baked in),
        // created at the current vector schema version — or rebuilt in place
        // from a previous version (W1.1 cosine metric + W1.7 namespace
        // partition, one combined rebuild).
        ensure_vector_schema_with_hook(&conn, embedding_dim, after_vector_schema_read)?;
        let site_id = seed_or_get_site_id(&conn)?;

        Ok(Self {
            conn,
            embedding_dim,
            site_id,
        })
    }
    /// This database's `meta.site_id` (uuid v4, seeded at init), stamped on
    /// every `memory_oplog` row.
    pub fn site_id(&self) -> &str {
        &self.site_id
    }
    /// Whether the connection is in autocommit mode (no open transaction).
    ///
    /// `false` after a COMPLETED writer op means the op leaked a transaction —
    /// e.g. a failed COMMIT whose drop-rollback also failed — and every later
    /// op on this connection would die with "cannot start a transaction within
    /// a transaction". The daemon's writer checks this after any op that
    /// returns `Err` and drops + reopens the connection instead of letting the
    /// poison spread (W1.6b, F07/F16).
    pub fn is_autocommit(&self) -> bool {
        self.conn.is_autocommit()
    }
    /// Test-only seam (W1.6b): open a transaction and LEAVE it open, simulating
    /// a writer op that errored out mid-transaction without rolling back (the
    /// failed-COMMIT-then-failed-ROLLBACK poison case, which cannot be induced
    /// through the public write API now that every op rolls back via RAII).
    ///
    /// This method is `pub` only to be reachable from the daemon's writer
    /// tests. Do NOT call it in production code.
    #[doc(hidden)]
    pub fn leave_transaction_open_for_test(&self) -> Result<()> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(storage_err)
    }
    /// Fold the WAL back into the main database file and truncate it to zero.
    ///
    /// Used on graceful daemon shutdown so the on-disk DB is a clean single file
    /// with no trailing WAL frames. On an in-memory or non-WAL connection SQLite
    /// reports the operation as a no-op and returns `SQLITE_OK`, so this never
    /// errors for those DBs.
    ///
    /// Uses `execute_batch` rather than `pragma_query`: rusqlite's `pragma_query`
    /// routes the pragma name through `push_keyword`, which rejects the
    /// parenthesized `wal_checkpoint(TRUNCATE)` form as a non-identifier. A raw
    /// `PRAGMA ...;` statement executed via `execute_batch` has the same
    /// semantics and accepts the argument syntax.
    pub fn checkpoint_truncate(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(storage_err)?;
        Ok(())
    }
    /// Read a value from the key/value `meta` table (e.g. the
    /// `embedding_input_version` seed written by migration 003), or `None` if the
    /// key is absent. A small read helper for invariant checks and the migration
    /// reproducibility gate.
    pub fn meta_value(&self, key: &str) -> Result<Option<String>> {
        meta_value(&self.conn, key)
    }
}

/// Enable WAL with a bounded retry for SQLite's journal-mode transition.
/// SQLite may return BUSY immediately for this pragma even with a busy handler
/// installed, so two zero-byte first openers need an explicit retry window.
fn enable_wal(conn: &rusqlite::Connection) -> Result<()> {
    let deadline = std::time::Instant::now() + OPEN_BUSY_TIMEOUT;
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(WAL_RETRY_INTERVAL);
            }
            Err(error) => return Err(Error::Storage(format!("enable WAL journal mode: {error}"))),
        }
    }
}

#[cfg(test)]
fn public_open_test_barriers() -> &'static std::sync::Mutex<
    std::collections::HashMap<std::path::PathBuf, std::sync::Arc<std::sync::Barrier>>,
> {
    static BARRIERS: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<std::path::PathBuf, std::sync::Arc<std::sync::Barrier>>,
        >,
    > = std::sync::OnceLock::new();
    BARRIERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn wait_for_public_open_test_barrier(path: &Path) {
    let barrier = public_open_test_barriers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(path)
        .cloned();
    if let Some(barrier) = barrier {
        barrier.wait();
    }
}

#[cfg(test)]
fn install_public_open_test_barrier(path: &Path, barrier: std::sync::Arc<std::sync::Barrier>) {
    let previous = public_open_test_barriers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(path.to_path_buf(), barrier);
    assert!(
        previous.is_none(),
        "test barrier already installed for {path:?}"
    );
}

#[cfg(test)]
fn remove_public_open_test_barrier(path: &Path) {
    let removed = public_open_test_barriers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(path);
    assert!(removed.is_some(), "test barrier missing for {path:?}");
}
/// Caches the result of the one-time process-global sqlite-vec registration.
/// `Ok(())` on success; the error message is cloned on every subsequent call.
static VEC_REGISTERED: std::sync::OnceLock<std::result::Result<(), String>> =
    std::sync::OnceLock::new();
/// Register the sqlite-vec extension so `vec0` virtual tables and the KNN
/// `MATCH` syntax are available on every subsequently opened connection.
///
/// Fails closed: if registration does not return `SQLITE_OK`, this returns an
/// error so the caller never proceeds with a connection that silently lacks
/// the `vec0` module. Runs exactly once per process via `VEC_REGISTERED`.
fn register_vec() -> Result<()> {
    let outcome = VEC_REGISTERED.get_or_init(|| {
        // SAFETY: `sqlite_vec::sqlite3_vec_init` is the FFI entry point published
        // by the sqlite-vec crate. We transmute its address to the
        // `RawAutoExtension` fn-pointer type rusqlite expects, exactly as the
        // sqlite-vec crate does in its own test (transmute of a `*const ()`).
        // The init fn has `'static` validity (it lives in the linked library for
        // the whole program). `register_auto_extension` registers it as a SQLite
        // auto-extension and returns `Err` if SQLite rejects it. Process-global
        // single-execution (and thus idempotency) is guaranteed by the
        // `OnceLock` guard above — we do NOT rely on SQLite's internal dedup.
        #[allow(unsafe_code)]
        #[allow(clippy::missing_transmute_annotations)]
        let result = unsafe {
            rusqlite::auto_extension::register_auto_extension(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            ))
        };
        result.map_err(|e| format!("failed to register sqlite-vec extension: {e}"))
    });
    outcome.clone().map_err(Error::Storage)
}
/// Current vector-table layout, recorded at `meta.vector_schema_version`.
///
/// Version 2 (W1.1 + W1.7, one combined rebuild) = cosine `distance_metric`
/// AND a `namespace` PARTITION KEY. A DB missing the marker carries the
/// version-1 layout (implicit L2 metric, no partition column) and is rebuilt
/// in place exactly once at open; a fresh DB is created directly in final
/// form. One marker covers both properties so the rebuild matrix stays
/// two-state: marker present (current) or absent (full rebuild + cleanup).
const VECTOR_SCHEMA_VERSION: &str = "2";

/// Meta key where one-shot vector-rebuild statistics are recorded (JSON:
/// pruned/reinserted vector counts, similarity links rescored/dropped).
const VECTOR_REBUILD_STATS_KEY: &str = "vector_rebuild_v2";
/// The version-2 vec0 DDL. The dimension is baked into the column type (vec0
/// requirement); `namespace TEXT PARTITION KEY` shards the index so KNN scopes
/// per namespace (sqlite-vec 0.1.9 supports both `partition key` columns and
/// `distance_metric=cosine`).
fn vector_table_ddl(embedding_dim: usize) -> String {
    format!(
        "CREATE VIRTUAL TABLE memory_vectors USING vec0(\
           memory_id TEXT PRIMARY KEY,\
           namespace TEXT PARTITION KEY,\
           embedding float[{embedding_dim}] distance_metric=cosine\
         );"
    )
}
/// Upsert one `meta` key.
fn upsert_meta(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(storage_err)?;
    Ok(())
}
/// Ensure `memory_vectors` exists at [`VECTOR_SCHEMA_VERSION`], rebuilding a
/// previous-version table in place when needed (W1.1 cosine metric + W1.7
/// namespace partition + archived-vector cleanup, folded into ONE rebuild).
///
/// This is an open-time code rebuild, not a SQL migration: the vec0 table is
/// created in code with a runtime dim and vec0 implements no `xRename`, so a
/// stash/drop/recreate/re-insert inside one `BEGIN IMMEDIATE` is the only
/// rename-free path. It MUST run on the writer's open path before any read
/// connection opens (single-flight by construction; `StoreHandle::start_inner`
/// sequences the writer open before the read pool spins up) so a large-corpus
/// rebuild cannot starve concurrent opens past `busy_timeout`.
///
/// State matrix:
/// - marker == 2: current layout, nothing to do;
/// - no `memory_vectors` table (fresh DB): create directly in final form;
/// - table without marker (v1 layout, OR a half-created fresh DB after a
///   crash between CREATE and marker): full rebuild — idempotent because the
///   stash SELECT reads only columns both layouts expose.
fn ensure_vector_schema_with_hook<F>(
    conn: &rusqlite::Connection,
    embedding_dim: usize,
    after_schema_read: F,
) -> Result<()>
where
    F: FnOnce(),
{
    if meta_value(conn, "vector_schema_version")?.as_deref() == Some(VECTOR_SCHEMA_VERSION) {
        return Ok(());
    }

    after_schema_read();

    // Only the slow path takes a write lock. Re-read every decision input
    // after BEGIN IMMEDIATE: another opener may have completed initialization
    // while this connection waited for the lock. Keeping the optimistic check
    // above means a current-schema open remains read-only and lock-free.
    // Create-or-rebuild + markers stay in one transaction, so a crash mid-way
    // rolls everything back and the next open retries from the same state.
    immediate_tx(conn, || {
        if meta_value(conn, "vector_schema_version")?.as_deref() == Some(VECTOR_SCHEMA_VERSION) {
            return Ok(());
        }

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'memory_vectors'",
                [],
                |r| r.get(0),
            )
            .map_err(storage_err)?;
        if table_exists > 0 {
            rebuild_vector_table(conn, embedding_dim)?;
        } else {
            conn.execute_batch(&vector_table_ddl(embedding_dim))
                .map_err(storage_err)?;
        }
        upsert_meta(conn, "vector_schema_version", VECTOR_SCHEMA_VERSION)?;
        // Human-legible companion marker (the version gate above is canonical).
        upsert_meta(conn, "vector_metric", "cosine")?;
        Ok(())
    })
}
/// Rebuild `memory_vectors` from a previous layout into the version-2 layout.
/// MUST be called inside an open transaction (see [`ensure_vector_schema`]).
///
/// Folded W1.7 cleanup: the stash JOIN keeps only vectors whose owning memory
/// row exists AND is active (`archived_at IS NULL` — supersede also archives),
/// so vectors for archived/superseded/orphaned rows never enter the new table.
/// Vector bytes are copied unchanged: cosine vs L2 is a query-time metric, no
/// re-embed is needed.
///
/// One-shot link revalidation (W1.1): links produced by the similarity linker
/// (`reason = 'similar'`) were created under the L2 threshold; every such link
/// whose endpoints both still have live vectors is re-scored with
/// `vec_distance_cosine` and DROPPED when above
/// [`rb_types::SIMILARITY_LINK_MAX_COSINE_DISTANCE`] (links the recalibrated
/// linker would not create today). Links with a pruned/missing endpoint vector
/// cannot be re-scored and are left in place (they are inert in recall:
/// archived endpoints are filtered out). Counts are recorded durably at
/// [`VECTOR_REBUILD_STATS_KEY`].
fn rebuild_vector_table(conn: &rusqlite::Connection, embedding_dim: usize) -> Result<()> {
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
        .map_err(storage_err)?;

    // Stash (memory_id, namespace, embedding) for LIVE rows via vec0 fullscan.
    // A plain table created and dropped inside this transaction never survives
    // it (commit drops it; rollback undoes its creation) — no side files, no
    // schema residue.
    conn.execute_batch(
        "CREATE TABLE _vector_rebuild_stash AS
           SELECT v.memory_id AS memory_id,
                  m.namespace AS namespace,
                  v.embedding AS embedding
           FROM memory_vectors v
           JOIN memories m ON m.memory_id = v.memory_id
           WHERE m.archived_at IS NULL;",
    )
    .map_err(storage_err)?;

    let kept: i64 = conn
        .query_row("SELECT COUNT(*) FROM _vector_rebuild_stash", [], |r| {
            r.get(0)
        })
        .map_err(storage_err)?;

    // Re-score similarity-produced links while both endpoint vectors are
    // available in the plain stash (cheaper than KNN; vec_distance_cosine is
    // sqlite-vec's scalar distance over two float32 blobs).
    let max_dist = f64::from(rb_types::SIMILARITY_LINK_MAX_COSINE_DISTANCE);
    let rescored: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memory_links l
               JOIN _vector_rebuild_stash s ON s.memory_id = l.source_id
               JOIN _vector_rebuild_stash t ON t.memory_id = l.target_id
             WHERE l.reason = 'similar'",
            [],
            |r| r.get(0),
        )
        .map_err(storage_err)?;
    let dropped = conn
        .execute(
            "DELETE FROM memory_links WHERE rowid IN (
               SELECT l.rowid FROM memory_links l
                 JOIN _vector_rebuild_stash s ON s.memory_id = l.source_id
                 JOIN _vector_rebuild_stash t ON t.memory_id = l.target_id
               WHERE l.reason = 'similar'
                 AND vec_distance_cosine(s.embedding, t.embedding) > ?1
             )",
            rusqlite::params![max_dist],
        )
        .map_err(storage_err)?;

    // Drop + recreate at the new layout, then re-insert the live vectors
    // (unchanged bytes) with their owning namespace as the partition key.
    conn.execute_batch("DROP TABLE memory_vectors;")
        .map_err(storage_err)?;
    conn.execute_batch(&vector_table_ddl(embedding_dim))
        .map_err(storage_err)?;
    conn.execute(
        "INSERT INTO memory_vectors (memory_id, namespace, embedding)
         SELECT memory_id, namespace, embedding FROM _vector_rebuild_stash",
        [],
    )
    .map_err(storage_err)?;
    conn.execute_batch("DROP TABLE _vector_rebuild_stash;")
        .map_err(storage_err)?;

    // Durable rebuild log: rb-store carries no logging facility, so the
    // one-shot counts live in meta where an operator (or test) can read them.
    let stats = serde_json::json!({
        "pruned_vectors": before - kept,
        "reinserted_vectors": kept,
        "similar_links_rescored": rescored,
        "similar_links_dropped": dropped,
        "at": chrono::Utc::now().timestamp(),
    })
    .to_string();
    upsert_meta(conn, VECTOR_REBUILD_STATS_KEY, &stats)
}
/// Seed `meta.embedding_dim` on first init, or verify it matches on re-open.
/// Fails closed with `Error::DimensionMismatch` on disagreement.
///
/// `INSERT OR IGNORE` + re-read (same idiom as [`seed_or_get_site_id`]): two
/// connections racing the first open both end up validating against the
/// single stored value instead of the second racer's plain `INSERT` hitting
/// `meta`'s PK-uniqueness constraint on `key`.
fn seed_or_verify_dim(conn: &rusqlite::Connection, embedding_dim: usize) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('embedding_dim', ?1)",
        rusqlite::params![embedding_dim.to_string()],
    )
    .map_err(storage_err)?;
    let v = meta_value(conn, "embedding_dim")?
        .ok_or_else(|| Error::Storage("meta.embedding_dim missing after seed".to_string()))?;
    let stored: usize = v
        .parse()
        .map_err(|_| Error::Storage(format!("meta.embedding_dim is not an integer: {v:?}")))?;
    if stored != embedding_dim {
        return Err(Error::DimensionMismatch {
            expected: stored,
            got: embedding_dim,
        });
    }
    Ok(())
}
/// Seed `meta.embedding_model` on first init, or verify it on re-open.
///
/// A legacy DB may have per-row model stamps but no global marker. Recovery is
/// conservative: an empty corpus adopts the configured model, and a populated
/// corpus adopts it only when every row already carries that same model. A
/// disagreement or mixed-model corpus fails closed and requires the explicit
/// `accept_model_change` + re-embed path.
///
/// The common, already-seeded path is read-only. Missing-marker recovery takes
/// an immediate transaction and re-reads the marker under the write lock, so
/// concurrent first opens cannot race the row-stamp check and seed.
fn seed_or_verify_model(conn: &rusqlite::Connection, embedding_model: &str) -> Result<()> {
    if let Some(stored) = meta_value(conn, "embedding_model")? {
        return verify_configured_model(&stored, embedding_model);
    }

    immediate_tx(conn, || {
        // Re-check after acquiring the write lock: another opener may have
        // seeded the marker after the optimistic read above.
        if let Some(stored) = meta_value(conn, "embedding_model")? {
            return verify_configured_model(&stored, embedding_model);
        }

        // At most two values are needed to distinguish empty, uniform, and
        // mixed corpora; keep recovery bounded even if row stamps are corrupt.
        let row_models = {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT embedding_model
                     FROM memories
                     ORDER BY embedding_model
                     LIMIT 2",
                )
                .map_err(storage_err)?;
            let models = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(storage_err)?;
            models
        };

        match row_models.as_slice() {
            [] => {}
            [stored] if stored == embedding_model => {}
            [stored] => {
                return Err(Error::Storage(format!(
                    "embedding model changed (meta.embedding_model is missing, stored rows use: \
                     {stored:?}, configured: {embedding_model:?}); run with \
                     --accept-model-change to mark the corpus, then run \
                     `rusty-brain reembed` until changed=0"
                )))
            }
            models => {
                return Err(Error::Storage(format!(
                    "meta.embedding_model is missing and stored rows contain mixed embedding \
                     models {models:?}; run with --accept-model-change to adopt \
                     {embedding_model:?}; then run `rusty-brain reembed` until changed=0"
                )))
            }
        }

        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embedding_model', ?1)",
            rusqlite::params![embedding_model],
        )
        .map_err(storage_err)?;
        Ok(())
    })
}

fn verify_configured_model(stored: &str, embedding_model: &str) -> Result<()> {
    if stored != embedding_model {
        return Err(Error::Storage(format!(
            "embedding model changed (stored: {stored:?}, configured: {embedding_model:?}); \
             run with --accept-model-change to mark the corpus, then run \
             `rusty-brain reembed` until changed=0"
        )));
    }
    Ok(())
}
/// Seed `meta.site_id` (uuid v4) on first init, or read it back on re-open.
/// `INSERT OR IGNORE` + re-read: two connections racing the first open both
/// end up with the single stored value.
fn seed_or_get_site_id(conn: &rusqlite::Connection) -> Result<String> {
    if let Some(existing) = meta_value(conn, "site_id")? {
        return Ok(existing);
    }
    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('site_id', ?1)",
        rusqlite::params![uuid::Uuid::new_v4().to_string()],
    )
    .map_err(storage_err)?;
    meta_value(conn, "site_id")?
        .ok_or_else(|| Error::Storage("meta.site_id missing after seed".to_string()))
}
#[cfg(test)]
mod open_tests {
    #![allow(clippy::panic)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_memory_with_model(store: &SqliteStore, model: &str, content: &str) {
        let mut note = MemoryNote::new(
            Namespace::Project("model-recovery".to_string()),
            content.to_string(),
            MemoryType::Insight,
            5,
        );
        note.embedding_model = model.to_string();
        store.insert_memory(&note, Some(&[0.1f32; 8])).unwrap();
    }

    #[test]
    fn bundled_sqlite_is_at_least_3_53() {
        const MINIMUM_SQLITE_VERSION: i32 = 3_053_000;

        assert!(
            rusqlite::version_number() >= MINIMUM_SQLITE_VERSION,
            "bundled SQLite must be at least 3.53.0, found {}",
            rusqlite::version()
        );
    }

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
    fn open_with_model_seeds_then_reopens_with_same_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let _s = SqliteStore::open_with_model(&path, 8, "deterministic").unwrap();
        }
        let s2 = SqliteStore::open_with_model(&path, 8, "deterministic").unwrap();
        let model: String = s2
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            model, "deterministic",
            "embedding_model seeded at first init"
        );
    }

    #[test]
    fn open_with_a_different_model_refuses_with_remediation_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        // Deterministic-seeded DB; a same-dim provider swap must fail closed.
        {
            let _s = SqliteStore::open_with_model(&path, 8, "deterministic").unwrap();
        }
        let err = SqliteStore::open_with_model(&path, 8, "voyage-3").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("embedding model changed"),
            "refusal names the invariant: {msg}"
        );
        assert!(
            msg.contains("stored: \"deterministic\"") && msg.contains("configured: \"voyage-3\""),
            "refusal names both models: {msg}"
        );
        assert!(
            msg.contains("--accept-model-change") && msg.contains("rusty-brain reembed"),
            "refusal carries the remediation hint: {msg}"
        );
    }

    #[test]
    fn open_with_model_seeds_model_on_a_db_created_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        // Legacy shape: DB created without a bound model (no meta key).
        {
            let _s = SqliteStore::open(&path, 8).unwrap();
        }
        // First model-bound open adopts the configured model.
        let _s = SqliteStore::open_with_model(&path, 8, "deterministic").unwrap();
        // ...and a later swap is then refused.
        let err = SqliteStore::open_with_model(&path, 8, "other-model").unwrap_err();
        assert!(err.to_string().contains("embedding model changed"), "{err}");
    }

    #[test]
    fn missing_model_marker_with_compatible_rows_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let store = SqliteStore::open(&path, 8).unwrap();
            insert_memory_with_model(&store, "model-a", "compatible row");
            assert!(store.meta_value("embedding_model").unwrap().is_none());
        }

        let recovered = SqliteStore::open_with_model(&path, 8, "model-a").unwrap();
        assert_eq!(
            recovered.meta_value("embedding_model").unwrap().as_deref(),
            Some("model-a")
        );
        drop(recovered);

        SqliteStore::open_with_model(&path, 8, "model-a").unwrap();
    }

    #[test]
    fn missing_model_marker_with_incompatible_rows_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let store = SqliteStore::open(&path, 8).unwrap();
            insert_memory_with_model(&store, "model-a", "incompatible row");
        }

        let err = SqliteStore::open_with_model(&path, 8, "model-b").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("meta.embedding_model is missing"), "{msg}");
        assert!(
            msg.contains("stored rows use: \"model-a\"") && msg.contains("configured: \"model-b\""),
            "{msg}"
        );
        assert!(msg.contains("rusty-brain reembed"), "{msg}");

        let legacy = SqliteStore::open(&path, 8).unwrap();
        assert!(legacy.meta_value("embedding_model").unwrap().is_none());
    }

    #[test]
    fn missing_model_marker_with_mixed_rows_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let store = SqliteStore::open(&path, 8).unwrap();
            insert_memory_with_model(&store, "model-a", "first model");
            insert_memory_with_model(&store, "model-b", "second model");
        }

        let err = SqliteStore::open_with_model(&path, 8, "model-a").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mixed embedding models"), "{msg}");
        assert!(msg.contains("model-a") && msg.contains("model-b"), "{msg}");

        let legacy = SqliteStore::open(&path, 8).unwrap();
        assert!(legacy.meta_value("embedding_model").unwrap().is_none());
    }

    #[test]
    fn missing_model_marker_with_empty_legacy_row_stamp_names_it_unambiguously() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let store = SqliteStore::open(&path, 8).unwrap();
            insert_memory_with_model(&store, "", "unstamped legacy row");
        }

        let err = SqliteStore::open_with_model(&path, 8, "model-b").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("stored rows use: \"\""), "{msg}");
        assert!(msg.contains("rusty-brain reembed"), "{msg}");
    }

    #[test]
    fn explicit_accept_recovers_missing_marker_with_incompatible_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");

        {
            let store = SqliteStore::open(&path, 8).unwrap();
            insert_memory_with_model(&store, "model-a", "accept this change");
        }
        assert!(SqliteStore::open_with_model(&path, 8, "model-b").is_err());

        let legacy = SqliteStore::open(&path, 8).unwrap();
        assert!(!legacy.accept_model_change("model-b").unwrap());
        assert_eq!(
            legacy
                .memories_for_reembed("model-b", "v2-composite", 10)
                .unwrap()
                .len(),
            1,
            "the accepted legacy row remains queued for re-embedding"
        );
        drop(legacy);

        SqliteStore::open_with_model(&path, 8, "model-b").unwrap();
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

    #[test]
    fn open_with_zero_dim_fails_closed() {
        // In-memory path.
        let err = SqliteStore::open_in_memory(0).unwrap_err();
        assert!(
            matches!(err, Error::Storage(ref m) if m.contains("embedding_dim must be greater than 0")),
            "zero dim must be rejected with a clear Storage error, got {err:?}"
        );

        // File path.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let err = SqliteStore::open(&path, 0).unwrap_err();
        assert!(
            matches!(err, Error::Storage(ref m) if m.contains("embedding_dim must be greater than 0")),
            "zero dim must be rejected with a clear Storage error, got {err:?}"
        );
    }
}
#[cfg(test)]
mod checkpoint_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    #[test]
    fn checkpoint_truncate_is_ok_on_file_and_memory_dbs() {
        // In-memory DB: journal_mode is "memory"; checkpoint is a harmless no-op.
        let mem = SqliteStore::open_in_memory(8).unwrap();
        mem.checkpoint_truncate().unwrap();

        // File-backed DB in WAL: insert one row, checkpoint, row still present.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = SqliteStore::open(&db, 8).unwrap();
        let ns = Namespace::Project("ckpt".to_string());
        let note = MemoryNote::new(ns, "checkpoint me".to_string(), MemoryType::Insight, 5);
        let id = note.id.clone();
        store.insert_memory(&note, Some(&[0.1f32; 8])).unwrap();

        store.checkpoint_truncate().unwrap();

        let got = store.get_memory(&id).unwrap();
        assert!(got.is_some(), "row survives a wal_checkpoint(TRUNCATE)");
        assert_eq!(got.unwrap().content, "checkpoint me");
    }
}
#[cfg(test)]
mod migration_004_tests {
    use super::*;

    #[test]
    fn migration_004_applies_on_populated_pre_provenance_db() {
        // Build a REAL 003-schema DB (file-discovered migrations 001..003 only),
        // populate it, then open through SqliteStore (which applies 004). Old
        // rows must decode with `None` provenance and the FTS index must be
        // untouched (no backfill UPDATE may churn it).
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            crate::migrations::run_migrations_up_to(&conn, 3).unwrap();
            conn.execute(
                "INSERT INTO memories (memory_id, namespace, created_at, updated_at, content, \
                 summary, keywords, tags, memory_type, importance, confidence, embedding_model) \
                 VALUES ('00000000-0000-4000-8000-000000000001','project:legacy',0,0,\
                 'pre-provenance row','s','[]','[]','insight',5,1.0,'')",
                [],
            )
            .unwrap();
        }

        let store = SqliteStore::open(&db, 8).unwrap();
        let id: MemoryId = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        let got = store.get_memory(&id).unwrap().unwrap();
        assert_eq!(got.content, "pre-provenance row");
        assert!(got.origin_user.is_none(), "old rows keep NULL provenance");
        assert!(got.origin_host.is_none());
        assert!(got.origin_agent.is_none());
        assert!(got.origin_source.is_none());
        assert!(got.session_id.is_none());

        // FTS still matches the old row: 004 ran no UPDATE, so the mem_au
        // trigger never fired and the index is exactly the insert-time one.
        let hits = store
            .keyword_search(&got.namespace, "pre-provenance", 10)
            .unwrap();
        assert!(hits.contains(&id), "FTS untouched by the migration");

        // The oplog table now exists and new mutations log into it.
        store.archive_memory(&id).unwrap();
        let ops: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_oplog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ops, 1, "post-migration mutation appends to the oplog");
    }
}
#[cfg(test)]
mod fts_tokenizer_migration_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    #[test]
    fn migration_005_rebuilds_fts_on_populated_pre_porter_db() {
        // Standing rule (W1.1): every schema migration ships with a test
        // against a POPULATED prior-version DB. Build a real 004-schema DB
        // (file-discovered migrations 001..004: bare-unicode61 FTS index),
        // populate it through the old index's insert trigger, then open via
        // SqliteStore (which applies 005). The porter rebuild must re-index
        // the pre-existing rows, and the sync triggers must survive the
        // virtual-table swap.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            crate::migrations::run_migrations_up_to(&conn, 4).unwrap();
            conn.execute(
                "INSERT INTO memories (memory_id, namespace, created_at, updated_at, content, \
                 summary, keywords, tags, memory_type, importance, confidence, embedding_model) \
                 VALUES ('00000000-0000-4000-8000-000000000005','project:legacy',0,0,\
                 'we retry failed jobs','s','[]','[]','insight',5,1.0,'')",
                [],
            )
            .unwrap();
            // Sanity precondition: the OLD index does not stem, so the
            // inflected query has nothing to match yet.
            let pre: i64 = conn
                .query_row(
                    "SELECT count(*) FROM memories_fts WHERE memories_fts MATCH '\"retries\"'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(pre, 0, "pre-005 unicode61 index must not stem");
        }

        let store = SqliteStore::open(&db, 8).unwrap();
        let ns = Namespace::Project("legacy".into());
        let id: MemoryId = "00000000-0000-4000-8000-000000000005".parse().unwrap();

        // The 'rebuild' step re-indexed the OLD row under porter: an inflected
        // query now matches it.
        let hits = store.keyword_search(&ns, "retries", 10).unwrap();
        assert_eq!(hits, vec![id.clone()], "old row re-indexed under porter");

        // Exactly the populated rows are indexed (no loss, no duplication).
        let fts_rows: i64 = store
            .conn
            .query_row("SELECT count(*) FROM memories_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts_rows, 1, "rebuild preserves the row count");

        // The recreated table carries the new tokenizer in its DDL.
        let ddl: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='memories_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            ddl.contains("porter"),
            "memories_fts DDL must record the porter tokenizer, got {ddl:?}"
        );

        // mem_ai survived the swap: a post-migration insert is searchable.
        let fresh = MemoryNote::new(
            ns.clone(),
            "daemon reconnects after restarts".into(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&fresh, None).unwrap();
        let found = store.keyword_search(&ns, "reconnecting", 10).unwrap();
        assert_eq!(found, vec![fresh.id.clone()], "insert trigger still syncs");

        // mem_au survived the swap: an update re-syncs the index.
        store
            .conn
            .execute(
                "UPDATE memories SET content = 'writer thread serializes mutations' \
                 WHERE memory_id = ?1",
                rusqlite::params![fresh.id.to_string()],
            )
            .unwrap();
        let updated = store
            .keyword_search(&ns, "serializing mutation", 10)
            .unwrap();
        assert_eq!(updated, vec![fresh.id], "update trigger still syncs");
        let stale = store.keyword_search(&ns, "reconnecting", 10).unwrap();
        assert!(stale.is_empty(), "old content must leave the index");
    }
}
#[cfg(test)]
mod mem_au_narrowing_migration_tests {
    use super::*;
    use rb_types::Namespace;

    /// FTS row-version probe (W1.8): fts5 appends new inverted-index segment
    /// rows to the `memories_fts_data` shadow table on EVERY index write —
    /// including the delete+reinsert cycle the old broad `mem_au` ran on
    /// metadata-only updates — so `(count, max id)` moves iff the index was
    /// written. The pre-006 half of the test below proves the probe detects
    /// churn (it is not vacuously stable).
    fn fts_index_state(conn: &rusqlite::Connection) -> (i64, i64) {
        conn.query_row(
            "SELECT count(*), coalesce(max(id), 0) FROM memories_fts_data",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn migration_006_narrows_mem_au_on_populated_pre_006_db() {
        // Standing rule (W1.1): every schema migration ships with a test
        // against a POPULATED prior-version DB. Build a real 005-schema DB,
        // demonstrate the F08 write amplification the old trigger caused
        // (access bump => FTS churn), then open via SqliteStore (006 applies)
        // and prove the same bumps are now ZERO FTS writes while indexed-column
        // edits still re-sync the index.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            crate::migrations::run_migrations_up_to(&conn, 5).unwrap();
            conn.execute(
                "INSERT INTO memories (memory_id, namespace, created_at, updated_at, content, \
                 summary, keywords, tags, memory_type, importance, confidence, embedding_model) \
                 VALUES ('00000000-0000-4000-8000-000000000006','project:legacy',0,0,\
                 'recall must not rewrite the index','s','[]','[]','insight',5,1.0,'')",
                [],
            )
            .unwrap();

            // Pre-006 churn proof: a metadata-only access bump fires the broad
            // AFTER UPDATE trigger and rewrites the row's index entries.
            let before = fts_index_state(&conn);
            conn.execute(
                "UPDATE memories SET access_count = access_count + 1, last_accessed_at = 1 \
                 WHERE memory_id = '00000000-0000-4000-8000-000000000006'",
                [],
            )
            .unwrap();
            assert_ne!(
                before,
                fts_index_state(&conn),
                "pre-006 the broad trigger churns FTS on a pure access bump \
                 (this also proves the probe detects index writes)"
            );
        }

        let store = SqliteStore::open(&db, 8).unwrap();
        let ns = Namespace::Project("legacy".into());
        let id: MemoryId = "00000000-0000-4000-8000-000000000006".parse().unwrap();

        // The narrowed trigger is live and names exactly the indexed columns.
        let ddl: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name='mem_au'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            ddl.contains("AFTER UPDATE OF"),
            "mem_au must be column-scoped post-006, got {ddl:?}"
        );
        for col in ["content", "summary", "keywords", "tags"] {
            assert!(ddl.contains(col), "mem_au OF-list must keep {col}: {ddl:?}");
        }

        // The trigger swap never touched the index: the old row still matches.
        let hits = store.keyword_search(&ns, "recall", 10).unwrap();
        assert_eq!(hits, vec![id.clone()], "index survives the migration");

        // Post-006: access bumps — both the single and the batched (W1.8
        // flush) paths — trigger ZERO FTS writes.
        let before = fts_index_state(&store.conn);
        store.record_access(&id).unwrap();
        store
            .record_access_bumps(&[AccessBump {
                id: id.clone(),
                count: 2,
                last_accessed_at: 123,
            }])
            .unwrap();
        assert_eq!(
            fts_index_state(&store.conn),
            before,
            "access bumps must not write FTS"
        );
        // The bumps themselves landed (the trigger narrowing lost no writes).
        let got = store.get_memory(&id).unwrap().unwrap();
        assert_eq!(got.access_count, 4, "1 (pre-006) + 1 + 2");

        // An indexed-column edit STILL re-syncs the index.
        store
            .conn
            .execute(
                "UPDATE memories SET content = 'porter stems serialized mutations' \
                 WHERE memory_id = ?1",
                rusqlite::params![id.to_string()],
            )
            .unwrap();
        let after_edit = fts_index_state(&store.conn);
        assert_ne!(after_edit, before, "content edits still write the index");
        let hits = store.keyword_search(&ns, "serialized", 10).unwrap();
        assert_eq!(hits, vec![id.clone()], "new content searchable");
        let stale = store.keyword_search(&ns, "recall", 10).unwrap();
        assert!(stale.is_empty(), "old content left the index");

        // Archive (archived_at/updated_at only) is churn-free too.
        store.archive_memory(&id).unwrap();
        assert_eq!(
            fts_index_state(&store.conn),
            after_edit,
            "archive must not write FTS"
        );
    }
}
#[cfg(test)]
mod base_importance_migration_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    #[test]
    fn migration_007_backfills_base_importance_on_populated_pre_007_db() {
        // Standing rule (W1.1): every schema migration ships with a test
        // against a POPULATED prior-version DB. Build a real 006-schema DB
        // with rows across the importance range, open via SqliteStore (007
        // applies), and prove every row's author prior was backfilled from
        // its pre-007 importance and that post-migration writes keep the
        // prior/effective split intact.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            crate::migrations::run_migrations_up_to(&conn, 6).unwrap();
            // The pre-007 schema must genuinely lack the column, or this test
            // would not be exercising the ALTER + backfill at all.
            let has: i64 = conn
                .query_row(
                    "SELECT count(*) FROM pragma_table_info('memories') \
                     WHERE name='base_importance'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(has, 0, "pre-007 schema must lack base_importance");
            for (suffix, importance) in [(1, 1i64), (2, 7), (3, 10)] {
                conn.execute(
                    &format!(
                        "INSERT INTO memories (memory_id, namespace, created_at, updated_at, \
                         content, summary, keywords, tags, memory_type, importance, confidence, \
                         embedding_model) \
                         VALUES ('00000000-0000-4000-8000-00000000000{suffix}','project:legacy',\
                         {suffix},0,'legacy row {suffix}','s','[]','[]','insight',{importance},\
                         1.0,'')"
                    ),
                    [],
                )
                .unwrap();
            }
        }

        // Open via SqliteStore: 007 applies on top of the populated DB.
        let store = SqliteStore::open(&db, 8).unwrap();
        let rows = store.memories_for_recalibration(10).unwrap();
        assert_eq!(rows.len(), 3, "all legacy rows survive the migration");
        for row in &rows {
            assert_eq!(
                row.base_importance, row.importance,
                "backfill must anchor the prior at the pre-007 importance ({})",
                row.id
            );
        }

        // FTS untouched by the migration (mem_au is column-scoped post-006 and
        // the backfill assigns no indexed column): legacy content still matches.
        let ns = Namespace::Project("legacy".into());
        let hits = store.keyword_search(&ns, "legacy", 10).unwrap();
        assert_eq!(hits.len(), 3, "index survives the migration");

        // A post-migration insert stamps the prior...
        let m = MemoryNote::new(ns.clone(), "fresh row".into(), MemoryType::Insight, 8);
        store.insert_memory(&m, None).unwrap();
        let fresh = store
            .memories_for_recalibration(10)
            .unwrap()
            .into_iter()
            .find(|r| r.id == m.id)
            .expect("fresh row present");
        assert_eq!(fresh.base_importance, 8, "insert stamps the author prior");

        // ...and a job write on a MIGRATED row moves only the effective value,
        // leaving the backfilled prior anchored.
        let legacy_id: MemoryId = "00000000-0000-4000-8000-000000000003".parse().unwrap();
        store.set_recalibrated_importance(&legacy_id, 8).unwrap();
        let legacy = store
            .memories_for_recalibration(10)
            .unwrap()
            .into_iter()
            .find(|r| r.id == legacy_id)
            .expect("legacy row present");
        assert_eq!(legacy.importance, 8, "job write moved the effective value");
        assert_eq!(
            legacy.base_importance, 10,
            "backfilled prior survives a job write"
        );
    }
}
#[cfg(test)]
#[cfg(unix)]
mod db_perms_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn db_file_is_0600_after_create_and_wal_inherits() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("perm.db");
        let store = SqliteStore::open(&db, 8).unwrap();
        assert_eq!(mode_of(&db), 0o600, "db file must be owner-only");

        // Force a write so the -wal sibling exists; SQLite creates it copying
        // the main file's (already-tightened) mode.
        let n = MemoryNote::new(
            Namespace::Project("perm".into()),
            "perm probe".into(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&n, None).unwrap();
        let wal = dir.path().join("perm.db-wal");
        if wal.exists() {
            assert_eq!(mode_of(&wal), 0o600, "-wal must inherit owner-only");
        }
    }

    #[test]
    fn reopen_tightens_a_pre_existing_loose_db_file() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("loose.db");
        drop(SqliteStore::open(&db, 8).unwrap());
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(SqliteStore::open(&db, 8).unwrap());
        assert_eq!(mode_of(&db), 0o600, "open must tighten a loose pre-W0.5 DB");
    }

    #[test]
    fn reopen_tightens_a_pre_existing_loose_wal_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("loose-wal.db");
        drop(SqliteStore::open(&db, 8).unwrap());
        // An unclean daemon death leaves the -wal behind (it is only removed on
        // a clean close); a pre-W0.5 install left it 0644 and SQLite reuses the
        // file as-is on reopen, so open must tighten it too.
        let wal = dir.path().join("loose-wal.db-wal");
        std::fs::write(&wal, b"").unwrap();
        std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
        let store = SqliteStore::open(&db, 8).unwrap();
        let n = MemoryNote::new(
            Namespace::Project("perm".into()),
            "wal probe".into(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&n, None).unwrap();
        assert!(wal.exists(), "-wal must exist after a write");
        assert_eq!(
            mode_of(&wal),
            0o600,
            "open must tighten a loose pre-existing -wal"
        );
    }
}
#[cfg(test)]
mod vector_schema_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    const DIM: usize = 8;

    fn insert_vec(store: &SqliteStore, ns: &Namespace, content: &str, v: &[f32]) -> MemoryId {
        let m = MemoryNote::new(ns.clone(), content.into(), MemoryType::Insight, 5);
        let id = m.id.clone();
        store.insert_memory(&m, Some(v)).unwrap();
        id
    }

    fn vector_row_count(store: &SqliteStore, id: &MemoryId) -> i64 {
        store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_vectors WHERE memory_id = ?1",
                rusqlite::params![id.to_string()],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// The L2-vs-cosine distinguishing test (spec W1.1), with NON-UNIT vectors
    /// through the store API. Candidate `aligned` points exactly along the
    /// query direction but with magnitude 10 (L2 distance 9.0 from the query);
    /// candidate `close_l2` sits a short straight-line hop away (L2 ~0.894)
    /// but 36.87 degrees off in angle (cosine distance 0.4). L2 ordering
    /// returns `close_l2` first; cosine MUST return `aligned` first.
    #[test]
    fn cosine_metric_ranks_by_angle_not_magnitude() {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        let ns = Namespace::Project("cosine".into());

        let aligned = insert_vec(
            &store,
            &ns,
            "aligned big magnitude",
            &[10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let close_l2 = insert_vec(
            &store,
            &ns,
            "close in euclidean terms",
            &[0.6, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let query = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let res = store.vector_search(&ns, &query, 10).unwrap();
        let ids: Vec<MemoryId> = res.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(
            ids,
            vec![aligned, close_l2],
            "cosine must rank the angle-aligned non-unit vector first; \
             L2 would rank the short-straight-line candidate first"
        );
        // Distances are cosine distances: 0.0 for the aligned vector, 0.4 for
        // the 0.6/0.8 one (cos = 0.6).
        assert!(res[0].1.abs() < 1e-5, "aligned cosine distance ~0");
        assert!(
            (res[1].1 - 0.4).abs() < 1e-5,
            "off-angle cosine distance ~0.4, got {}",
            res[1].1
        );
    }

    /// The FROZEN pre-W1.1 in-code vec0 DDL (L2 metric, no partition key).
    /// Shared by every v1-fixture builder so the replicated legacy schema
    /// cannot drift between tests.
    ///
    /// CONVENTION (accepted interpretation of the W1.1 "committed populated
    /// previous-schema DB fixtures" standing rule): prior-version DBs are
    /// CONSTRUCTED at test time from (a) the committed, immutable migration
    /// SQL files via `run_migrations` and (b) — for the vec0 virtual table,
    /// which migrations never owned — this FROZEN replica of the exact
    /// pre-W1.1 in-code DDL (verified byte-equal to the deleted ensure-schema
    /// DDL at the time W1.1 landed). Do NOT "modernize" this string: its whole
    /// value is that it matches what real pre-Phase-1 dogfood DBs actually
    /// contain. A committed binary .db fixture was considered and rejected:
    /// sqlite-vec shadow-table bytes are version-sensitive and unreviewable in
    /// diffs, while this builder is reviewable and runs on every platform in
    /// CI.
    const V1_VEC0_DDL: &str = "CREATE VIRTUAL TABLE memory_vectors USING vec0(\
           memory_id TEXT PRIMARY KEY,\
           embedding float[8]\
         );";

    /// Open a fresh v1-schema DB at `path`: committed migrations + the frozen
    /// pre-W1.1 vec0 DDL + the `embedding_dim` meta seed.
    fn open_v1_schema(path: &std::path::Path) -> rusqlite::Connection {
        register_vec().unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        run_migrations(&conn).unwrap();
        conn.execute_batch(V1_VEC0_DDL).unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embedding_dim', '8')",
            [],
        )
        .unwrap();
        conn
    }

    /// Build a POPULATED version-1 schema DB (old vec0 DDL: L2 metric, no
    /// partition key; vectors for archived rows; L2-era similarity links)
    /// exactly as pre-W1.1 code would have, using the committed migrations
    /// plus the frozen old in-code DDL (see `V1_VEC0_DDL`).
    fn build_v1_fixture(
        path: &std::path::Path,
    ) -> (MemoryId, MemoryId, MemoryId, MemoryId, MemoryId) {
        let conn = open_v1_schema(path);

        let raw_mem = |id: &MemoryId, ns: &str, archived: bool| {
            conn.execute(
                "INSERT INTO memories (memory_id, namespace, created_at, updated_at, content,
                    summary, keywords, tags, memory_type, importance, confidence,
                    embedding_model, archived_at)
                 VALUES (?1, ?2, 0, 0, ?3, 's', '[]', '[]', 'insight', 5, 1.0, '', ?4)",
                rusqlite::params![
                    id.to_string(),
                    ns,
                    format!("content {id}"),
                    if archived { Some(1i64) } else { None }
                ],
            )
            .unwrap();
        };
        let raw_vec = |id: &MemoryId, v: &[f32]| {
            conn.execute(
                "INSERT INTO memory_vectors (memory_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![id.to_string(), embedding_bytes(v)],
            )
            .unwrap();
        };
        let raw_link = |src: &MemoryId, tgt: &MemoryId, reason: &str| {
            conn.execute(
                "INSERT INTO memory_links
                    (source_id, target_id, link_type, strength, base_strength, reason, created_at)
                 VALUES (?1, ?2, 'references', 0.8, 0.8, ?3, 0)",
                rusqlite::params![src.to_string(), tgt.to_string(), reason],
            )
            .unwrap();
        };

        let m1 = MemoryId::new(); // ns a, active, NON-UNIT vector (byte-identity probe)
        let m2 = MemoryId::new(); // ns a, active, same direction as m1
        let m3 = MemoryId::new(); // ns a, active, orthogonal to m1
        let m4 = MemoryId::new(); // ns a, ARCHIVED with a leftover vector (prune target)
        let m5 = MemoryId::new(); // ns b, active

        raw_mem(&m1, "project:a", false);
        raw_mem(&m2, "project:a", false);
        raw_mem(&m3, "project:a", false);
        raw_mem(&m4, "project:a", true);
        raw_mem(&m5, "project:b", false);

        raw_vec(&m1, &[3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        raw_vec(&m2, &[6.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // Orthogonal to m1 but a SHORT L2 hop from small vectors: the exact
        // bug class the revalidation targets (L2 said "near", cosine says
        // "unrelated").
        raw_vec(&m3, &[0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0]);
        raw_vec(&m4, &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        raw_vec(&m5, &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

        // Orphan vector with NO owning memories row (vec0 enforces no FK):
        // must be pruned by the rebuild's JOIN.
        let orphan = MemoryId::new();
        raw_vec(&orphan, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);

        // L2-era links:
        //  - m1 -> m2 'similar': cosine distance 0 (same direction) => KEPT.
        //  - m1 -> m3 'similar': cosine distance 1.0 > 0.18 => DROPPED.
        //  - m2 -> m3 'llm': not similarity-produced => KEPT regardless.
        //  - m1 -> m4 'similar': endpoint archived (vector pruned) => cannot
        //    be re-scored => KEPT.
        raw_link(&m1, &m2, "similar");
        raw_link(&m1, &m3, "similar");
        raw_link(&m2, &m3, "llm");
        raw_link(&m1, &m4, "similar");

        (m1, m2, m3, m4, m5)
    }

    fn link_exists(store: &SqliteStore, src: &MemoryId, tgt: &MemoryId) -> bool {
        let n: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_links WHERE source_id = ?1 AND target_id = ?2",
                rusqlite::params![src.to_string(), tgt.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        n > 0
    }

    #[test]
    fn v1_db_rebuilds_once_to_cosine_partitioned_prunes_and_revalidates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.db");
        let (m1, m2, m3, m4, m5) = build_v1_fixture(&path);

        // The open path performs the one-shot rebuild.
        let store = SqliteStore::open(&path, DIM).unwrap();

        // Markers set.
        assert_eq!(
            store
                .meta_value("vector_schema_version")
                .unwrap()
                .as_deref(),
            Some("2")
        );
        assert_eq!(
            store.meta_value("vector_metric").unwrap().as_deref(),
            Some("cosine")
        );

        // New DDL really carries the partition key + cosine metric.
        let ddl: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='memory_vectors'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ddl.contains("PARTITION KEY"), "ddl: {ddl}");
        assert!(ddl.contains("distance_metric=cosine"), "ddl: {ddl}");

        // Cleanup: archived (m4) + orphan vectors pruned; live ones kept.
        let total: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 4, "m1, m2, m3, m5 survive; archived + orphan pruned");
        assert_eq!(vector_row_count(&store, &m4), 0, "archived vector pruned");

        // Vector BYTES are unchanged (no re-embed): the stored blob for the
        // non-unit m1 round-trips bit-for-bit through the rebuild.
        let blob: Vec<u8> = store
            .conn
            .query_row(
                "SELECT embedding FROM memory_vectors WHERE memory_id = ?1",
                rusqlite::params![m1.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob,
            embedding_bytes(&[3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            "rebuild must copy vector bytes unchanged"
        );

        // Revalidation: the L2-era 'similar' link to an orthogonal vector is
        // dropped; the same-direction 'similar' link, the non-similarity
        // ('llm') link, and the unscoreable archived-endpoint link survive.
        assert!(link_exists(&store, &m1, &m2), "near similar link kept");
        assert!(!link_exists(&store, &m1, &m3), "far similar link dropped");
        assert!(link_exists(&store, &m2, &m3), "non-similarity link kept");
        assert!(link_exists(&store, &m1, &m4), "unscoreable link kept");

        // Durable rebuild stats.
        let stats_raw = store.meta_value("vector_rebuild_v2").unwrap().unwrap();
        let stats: serde_json::Value = serde_json::from_str(&stats_raw).unwrap();
        assert_eq!(stats["pruned_vectors"], 2, "stats: {stats}");
        assert_eq!(stats["reinserted_vectors"], 4, "stats: {stats}");
        assert_eq!(stats["similar_links_rescored"], 2, "stats: {stats}");
        assert_eq!(stats["similar_links_dropped"], 1, "stats: {stats}");

        // KNN is namespace-partitioned and cosine-ordered after the rebuild.
        let ns_a = Namespace::Project("a".into());
        let ns_b = Namespace::Project("b".into());
        let query = [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let in_a = store.vector_search(&ns_a, &query, 10).unwrap();
        let a_ids: Vec<MemoryId> = in_a.iter().map(|(id, _)| id.clone()).collect();
        assert!(a_ids.contains(&m1) && a_ids.contains(&m2) && a_ids.contains(&m3));
        assert!(!a_ids.contains(&m4), "archived row not searchable");
        assert!(!a_ids.contains(&m5), "ns-b row never leaks into ns-a KNN");
        let in_b = store.vector_search(&ns_b, &query, 10).unwrap();
        let b_ids: Vec<MemoryId> = in_b.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(b_ids, vec![m5.clone()]);

        // Reopen: the marker short-circuits — no second rebuild, stats and
        // contents identical.
        drop(store);
        let store2 = SqliteStore::open(&path, DIM).unwrap();
        assert_eq!(
            store2.meta_value("vector_rebuild_v2").unwrap().unwrap(),
            stats_raw,
            "second open must not rebuild again"
        );
        let total2: i64 = store2
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total2, 4);
    }

    #[test]
    fn fresh_db_is_created_in_final_form_without_rebuild_stats() {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        assert_eq!(
            store
                .meta_value("vector_schema_version")
                .unwrap()
                .as_deref(),
            Some("2")
        );
        // No rebuild ran on a fresh DB: the stats key is absent.
        assert!(store.meta_value("vector_rebuild_v2").unwrap().is_none());
        let ddl: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='memory_vectors'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ddl.contains("PARTITION KEY"), "ddl: {ddl}");
        assert!(ddl.contains("distance_metric=cosine"), "ddl: {ddl}");
    }

    #[test]
    fn concurrent_fresh_vector_schema_opens_agree_on_all_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent-fresh.db");

        // Leave a genuinely fresh dynamic-vector state: all committed SQL
        // migrations exist, but dimension/model/site markers and the vec0
        // table do not. This isolates the vector-schema race from the separate
        // migration runner while exercising marker seeding in both opens.
        register_vec().unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            run_migrations(&conn).unwrap();
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let spawn_open = |path: std::path::PathBuf, barrier: std::sync::Arc<std::sync::Barrier>| {
            std::thread::spawn(move || -> std::result::Result<[String; 5], String> {
                let store = SqliteStore::open_inner_with_schema_hook(
                    &path,
                    DIM,
                    Some("deterministic"),
                    move || {
                        barrier.wait();
                    },
                )
                .map_err(|error| error.to_string())?;
                let value = |key| {
                    store
                        .meta_value(key)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("missing meta.{key}"))
                };
                Ok([
                    value("embedding_dim")?,
                    value("embedding_model")?,
                    value("vector_schema_version")?,
                    value("vector_metric")?,
                    store.site_id().to_string(),
                ])
            })
        };

        let first = spawn_open(path.clone(), std::sync::Arc::clone(&barrier));
        let second = spawn_open(path, barrier);
        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();

        assert_eq!(first, second, "both opens must observe one committed state");
        assert_eq!(
            &first[..4],
            ["8", "deterministic", VECTOR_SCHEMA_VERSION, "cosine"],
            "dimension, model, schema version, and metric markers agree"
        );
    }

    #[test]
    fn concurrent_zero_byte_public_opens_agree_on_all_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zero-byte-concurrent.db");
        assert!(!path.exists(), "the public opens must create the database");

        // The path-scoped test seam pauses both connections immediately after
        // Connection::open, so the calls below race the complete public init
        // path: WAL negotiation, migrations, marker seeding, and vec0 create.
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        install_public_open_test_barrier(&path, barrier);
        let spawn_open = |path: std::path::PathBuf| {
            std::thread::spawn(move || -> std::result::Result<[String; 5], String> {
                let store = SqliteStore::open_with_model(&path, DIM, "deterministic")
                    .map_err(|error| error.to_string())?;
                let value = |key| {
                    store
                        .meta_value(key)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("missing meta.{key}"))
                };
                Ok([
                    value("embedding_dim")?,
                    value("embedding_model")?,
                    value("vector_schema_version")?,
                    value("vector_metric")?,
                    store.site_id().to_string(),
                ])
            })
        };

        let first = spawn_open(path.clone());
        let second = spawn_open(path.clone());
        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();
        remove_public_open_test_barrier(&path);

        assert_eq!(first, second, "both opens must observe one committed state");
        assert_eq!(
            &first[..4],
            ["8", "deterministic", VECTOR_SCHEMA_VERSION, "cosine"],
            "dimension, model, schema version, and metric markers agree"
        );
    }

    /// Reproducible task #54 microbenchmark. Run with:
    /// `cargo test --release -p rb-store vector_schema_current_open_benchmark \
    /// -- --ignored --nocapture`
    #[test]
    #[ignore = "manual release-mode microbenchmark"]
    fn vector_schema_current_open_benchmark() {
        const ITERATIONS: u32 = 20_000;
        const SAMPLES: usize = 7;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("current-schema-bench.db");
        let store = SqliteStore::open_with_model(&path, DIM, "deterministic").unwrap();

        // Warm SQLite's page cache and both statement paths before timing.
        for _ in 0..1_000 {
            ensure_vector_schema_with_hook(&store.conn, DIM, || {}).unwrap();
            immediate_tx(&store.conn, || {
                std::hint::black_box(meta_value(&store.conn, "vector_schema_version")?);
                Ok(())
            })
            .unwrap();
        }

        let measure_optimistic = || {
            let started = std::time::Instant::now();
            for _ in 0..ITERATIONS {
                ensure_vector_schema_with_hook(&store.conn, DIM, || {}).unwrap();
            }
            started.elapsed().as_nanos() as f64 / f64::from(ITERATIONS)
        };
        let measure_forced_lock = || {
            let started = std::time::Instant::now();
            for _ in 0..ITERATIONS {
                immediate_tx(&store.conn, || {
                    std::hint::black_box(meta_value(&store.conn, "vector_schema_version")?);
                    Ok(())
                })
                .unwrap();
            }
            started.elapsed().as_nanos() as f64 / f64::from(ITERATIONS)
        };

        let mut optimistic_samples = Vec::with_capacity(SAMPLES);
        let mut forced_lock_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            // Alternate order to avoid consistently giving either path the
            // warmer cache or later CPU state.
            if sample % 2 == 0 {
                optimistic_samples.push(measure_optimistic());
                forced_lock_samples.push(measure_forced_lock());
            } else {
                forced_lock_samples.push(measure_forced_lock());
                optimistic_samples.push(measure_optimistic());
            }
        }
        optimistic_samples.sort_by(f64::total_cmp);
        forced_lock_samples.sort_by(f64::total_cmp);
        let optimistic_median = optimistic_samples[SAMPLES / 2];
        let forced_lock_median = forced_lock_samples[SAMPLES / 2];
        println!(
            "vector_schema_current_open iterations_per_sample={ITERATIONS} samples={SAMPLES} \
             optimistic_median_ns_per_op={optimistic_median:.1} \
             forced_immediate_median_ns_per_op={forced_lock_median:.1} \
             forced_overhead_ratio={:.2}x",
            forced_lock_median / optimistic_median
        );
    }

    /// W1.7 acceptance: a namespace whose live rows are <1% of all vectors
    /// still fills `limit`. 1000 near-query vectors live in a big namespace;
    /// 5 (~0.5%) live in a small one. The pre-partition code over-fetched
    /// `10 * limit = 50` GLOBAL candidates — all from the big namespace — and
    /// returned nothing for the small one; the partition key scopes the scan.
    #[test]
    fn sub_one_percent_namespace_still_fills_limit() {
        let store = SqliteStore::open_in_memory(4).unwrap();
        let big = Namespace::Project("big".into());
        let small = Namespace::Project("small".into());

        for i in 0..1000u32 {
            // All big-namespace vectors are nearly query-aligned (cosine
            // distance ~0), i.e. globally nearer than every small-ns vector.
            let v = [1.0, 1e-4 * (i as f32 + 1.0), 0.0, 0.0];
            let m = MemoryNote::new(big.clone(), format!("big {i}"), MemoryType::Insight, 5);
            store.insert_memory(&m, Some(&v)).unwrap();
        }
        let mut small_ids = Vec::new();
        for i in 0..5u32 {
            // 60 degrees off the query: cosine distance 0.5, far behind every
            // big-namespace vector in global order.
            let v = [0.5, 0.0, 0.866, 1e-4 * (i as f32 + 1.0)];
            let m = MemoryNote::new(small.clone(), format!("small {i}"), MemoryType::Insight, 5);
            small_ids.push(m.id.clone());
            store.insert_memory(&m, Some(&v)).unwrap();
        }

        let query = [1.0, 0.0, 0.0, 0.0];
        let res = store.vector_search(&small, &query, 5).unwrap();
        assert_eq!(
            res.len(),
            5,
            "a <1% namespace must still fill limit under the partitioned index"
        );
        let got: std::collections::HashSet<String> =
            res.iter().map(|(id, _)| id.to_string()).collect();
        let want: std::collections::HashSet<String> =
            small_ids.iter().map(|id| id.to_string()).collect();
        assert_eq!(got, want, "exactly the small-namespace rows are returned");
    }

    /// Phase 1 gate (plan §4): "a 10k-archived-vector scenario still returns
    /// correct live-namespace results". A v1-schema DB carries 10_000 leftover
    /// vectors for ARCHIVED rows (pre-W1.7 archive never pruned vec0) in the
    /// SAME namespace as 5 live rows — and every archived vector is nearly
    /// query-aligned, i.e. strictly CLOSER to the query than any live vector,
    /// so a pruning failure would crowd the live rows out of the KNN entirely.
    /// The W1.1/W1.7 open-time rebuild must prune all 10k, after which
    /// live-namespace search fills `limit` with exactly the live rows. (At
    /// steady state the scenario is structurally impossible: archive/supersede
    /// delete the vec0 row in-transaction and `update_vector` refuses archived
    /// rows — this exercises the legacy-DB path the gate names.)
    #[test]
    fn ten_k_archived_vector_scenario_returns_correct_live_namespace_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1-10k.db");

        let mut live_ids: Vec<MemoryId> = Vec::new();
        {
            let conn = open_v1_schema(&path);
            // One transaction + prepared statements: 10_005 rows stay fast.
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            {
                let mut mem_stmt = conn
                    .prepare(
                        "INSERT INTO memories (memory_id, namespace, created_at, updated_at,
                            content, summary, keywords, tags, memory_type, importance,
                            confidence, embedding_model, archived_at)
                         VALUES (?1, 'project:live', 0, 0, ?2, 's', '[]', '[]', 'insight', 5,
                                 1.0, '', ?3)",
                    )
                    .unwrap();
                let mut vec_stmt = conn
                    .prepare("INSERT INTO memory_vectors (memory_id, embedding) VALUES (?1, ?2)")
                    .unwrap();
                for i in 0..10_000u32 {
                    let id = MemoryId::new();
                    // Nearly query-aligned (cosine distance ~0 from [1,0,..]).
                    let v = [1.0, 1e-4 * (i as f32 + 1.0), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
                    mem_stmt
                        .execute(rusqlite::params![
                            id.to_string(),
                            format!("archived {i}"),
                            Some(1i64)
                        ])
                        .unwrap();
                    vec_stmt
                        .execute(rusqlite::params![id.to_string(), embedding_bytes(&v)])
                        .unwrap();
                }
                for i in 0..5u32 {
                    let id = MemoryId::new();
                    // 60 degrees off the query (cosine distance 0.5): strictly
                    // FARTHER than every archived vector.
                    let v = [0.5, 0.0, 0.866, 1e-4 * (i as f32 + 1.0), 0.0, 0.0, 0.0, 0.0];
                    mem_stmt
                        .execute(rusqlite::params![
                            id.to_string(),
                            format!("live {i}"),
                            None::<i64>
                        ])
                        .unwrap();
                    vec_stmt
                        .execute(rusqlite::params![id.to_string(), embedding_bytes(&v)])
                        .unwrap();
                    live_ids.push(id);
                }
            }
            conn.execute_batch("COMMIT").unwrap();
        }

        // Open through SqliteStore: the one-shot W1.1/W1.7 rebuild runs.
        let store = SqliteStore::open(&path, DIM).unwrap();

        let total: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 5, "all 10k archived-row vectors must be pruned");
        let stats_raw = store.meta_value("vector_rebuild_v2").unwrap().unwrap();
        let stats: serde_json::Value = serde_json::from_str(&stats_raw).unwrap();
        assert_eq!(stats["pruned_vectors"], 10_000, "stats: {stats}");
        assert_eq!(stats["reinserted_vectors"], 5, "stats: {stats}");

        let ns = Namespace::Project("live".into());
        let query = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let res = store.vector_search(&ns, &query, 5).unwrap();
        assert_eq!(
            res.len(),
            5,
            "live-namespace search must fill limit after the 10k prune"
        );
        let got: std::collections::HashSet<String> =
            res.iter().map(|(id, _)| id.to_string()).collect();
        let want: std::collections::HashSet<String> =
            live_ids.iter().map(|id| id.to_string()).collect();
        assert_eq!(got, want, "exactly the live rows are returned");
    }

    #[test]
    fn is_autocommit_reflects_a_leaked_transaction() {
        // The W1.6b poison probe: the writer uses `is_autocommit` to detect a
        // connection stranded mid-transaction; the test seam strands one.
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        assert!(store.is_autocommit(), "fresh connection is in autocommit");
        store.leave_transaction_open_for_test().unwrap();
        assert!(
            !store.is_autocommit(),
            "an open transaction must be visible to the poison probe"
        );
    }
}
