//! `SqliteStore`: the concrete `Store` backed by SQLite + sqlite-vec.

use crate::error::{io_err, storage_err};
use crate::migrations::run_migrations;
use rb_types::{
    Error, MemoryId, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace, Result,
};
use std::path::Path;

/// One link edge as read by the link-decay job. Public so the daemon's job code
/// can consume it via the read pool. Not part of the `Store` trait: it is a
/// cross-namespace maintenance read, not an engine operation.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRow {
    pub source: MemoryId,
    pub target: MemoryId,
    pub link_type: rb_types::LinkType,
    /// Current (possibly decayed) strength. Used only for change detection so a
    /// pass that recomputes the same value writes nothing.
    pub strength: f32,
    /// Immutable baseline strength captured at link creation. Decay is a pure
    /// function of this and `created_at`, which is what makes the pass idempotent.
    pub base_strength: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Outcome of a one-time namespace rename (W0.3 carryover): how many
/// `memories` rows were re-scoped, how many vec0 rows were re-inserted under
/// the new partition key, and how many rows the target namespace already had
/// before the rename (non-zero only under `merge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamespaceRenameOutcome {
    pub memories: u64,
    pub vectors: u64,
    pub merged_into: u64,
}

/// One page of [`SqliteStore::oplog_changes_since`] replay (W2.7).
///
/// `changes` is the replayable events on this page (bulk-admin rows filtered
/// out). `scanned`/`last_seq` describe the underlying oplog SCAN so the caller
/// paginates correctly even when rows were filtered: keep paging while
/// `scanned == limit`, advancing the cursor to `last_seq`.
#[derive(Debug, Clone, Default)]
pub struct OplogReplayPage {
    pub changes: Vec<rb_types::MemoryChanged>,
    /// Oplog rows examined on this page (≤ the requested limit).
    pub scanned: usize,
    /// Highest oplog seq examined (the next cursor); equals the input `after`
    /// when the page was empty.
    pub last_seq: u64,
}

/// Result of a retroactive [`SqliteStore::scrub`] pass (W2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrubOutcome {
    /// Rows examined (every memory, active and archived).
    pub scanned: u64,
    /// Rows whose content/summary/context had a secret redacted out.
    pub redacted: u64,
    /// Of `redacted`, rows whose embedding-feeding text (content/context)
    /// changed and were therefore marked stale for re-embedding.
    pub reembed_pending: u64,
}

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
    /// Walk the link graph out from `id` up to `depth` hops, returning each
    /// reachable node ONCE with its MINIMUM hop distance (1 = direct neighbor;
    /// the anchor itself is excluded), ordered by hops ascending then id so the
    /// output is deterministic. (W1.5: real hop distances feed the graph
    /// ranking signal instead of incidental walk-order indices.)
    fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<(MemoryId, u8)>>;
    fn list(
        &self,
        ns: &Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>>;
    fn update_memory(&self, id: &MemoryId, updates: &MemoryUpdates) -> Result<()>;
    fn archive_memory(&self, id: &MemoryId) -> Result<()>;
    fn add_link(&self, link: &MemoryLink) -> Result<()>;
    /// Bump `access_count` and stamp `last_accessed_at = now` for `id`.
    /// A missing id is a no-op (best-effort access tracking never errors on absence).
    fn record_access(&self, id: &MemoryId) -> Result<()>;
    /// Bump `access_count` and stamp `last_accessed_at = now` for all `ids` in a
    /// single transaction (one writer round-trip). Missing ids are silently skipped;
    /// duplicate ids are updated once per row (the UPDATE touches each row at most
    /// once regardless of how many times it appears in `ids`). Best-effort: an empty
    /// slice or all-missing ids returns `Ok(())` without touching the DB.
    fn record_accesses(&self, ids: &[MemoryId]) -> Result<()>;
    /// Apply BUFFERED access-tracking bumps in one transaction (W1.8): each
    /// entry adds `count` to `access_count` and advances `last_accessed_at`
    /// monotonically to `last_accessed_at` (an already-newer stamp is kept, so
    /// out-of-order flushes never move the clock backwards). Missing ids are
    /// silently skipped and an empty slice is a no-op (best-effort access
    /// tracking never errors on absence). Under migration 006 these updates
    /// assign no FTS-indexed column, so a flush triggers zero FTS writes.
    fn record_access_bumps(&self, bumps: &[AccessBump]) -> Result<()>;
    /// Mark `old` as superseded by `new` AND archive `old`, in one transaction.
    /// Fails closed (rolls back) if `new` does not exist (FK on `superseded_by`).
    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()>;
    /// Fetch all of `ids` that exist AND belong to `ns`, returned in the SAME
    /// order as `ids` (missing/out-of-namespace ids skipped). One query; fixes
    /// the recall N+1. Links are loaded per returned note.
    fn get_many(&self, ns: &Namespace, ids: &[MemoryId]) -> Result<Vec<MemoryNote>>;
    /// For each id in `ids`, return whether it has at least one ACTIVE
    /// `contradicts` link — inbound OR outbound — where BOTH endpoints are active
    /// (`archived_at IS NULL`) AND live in namespace `ns`. One batched query over
    /// `memory_links` (Feature C, spec §9). Requiring the LOCAL endpoint active too
    /// means an archived memory fetched via `get` is never flagged `contested`.
    /// Scoping both endpoints to `ns` preserves namespace isolation: `memory_links` carries
    /// no namespace and `add_link` permits cross-namespace edges, so without this an
    /// out-of-namespace memory could flag a result `contested`. The returned set
    /// contains exactly the ids that are contested; ids absent from the set are not
    /// contested. Read path; used post-ranking to annotate `MemoryNote.contested`.
    fn active_contradicts(
        &self,
        ns: &Namespace,
        ids: &[MemoryId],
    ) -> Result<std::collections::HashSet<MemoryId>>;
    /// Read up to `limit` ACTIVE memories (across ALL namespaces) whose stamped
    /// `(embedding_model, embedding_input_version)` differs from the current
    /// `(model, input_version)` — the re-embed candidates. Bounded, deterministic
    /// order. Read-only (every mutation goes through the single writer).
    fn memories_for_reembed(
        &self,
        model: &str,
        input_version: &str,
        limit: usize,
    ) -> Result<Vec<MemoryNote>>;
    /// Replace `id`'s stored vector and stamp the row's
    /// `(embedding_model, embedding_input_version)` to the current pair, in one
    /// transaction. This is the ONLY vector-UPDATE path (insert is write-once).
    /// A missing OR archived memory id is `Error::NotFound` — an archived row
    /// must never get its vector resurrected (live-only vec0 partition
    /// invariant; the reembed loop treats it as a per-row skip). The embedding
    /// dimension is validated fail-closed before the write.
    fn update_vector(
        &self,
        id: &MemoryId,
        embedding: &[f32],
        model: &str,
        input_version: &str,
    ) -> Result<()>;
}

/// SQLite-backed store. Owns a single connection (write path); the daemon owns
/// the read pool separately in P1.
pub struct SqliteStore {
    // rusqlite::Connection doesn't impl Debug, so we derive it manually via a wrapper.
    // pub(crate) for intra-crate access (CRUD methods consume it).
    pub(crate) conn: rusqlite::Connection,
    /// Embedding dimension configured at open; used to fail-close dimension mismatches
    /// before touching the DB.
    pub(crate) embedding_dim: usize,
    /// This database's `meta.site_id` (uuid v4, seeded at init), stamped on every
    /// oplog row so logs from multiple machines stay distinguishable after a merge.
    pub(crate) site_id: String,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

/// One buffered access-tracking bump (W1.8): `count` recorded accesses for
/// `id`, the most recent at `last_accessed_at` (unix seconds). The daemon
/// accumulates these off the recall path and flushes them in batches through
/// [`Store::record_access_bumps`], so recall itself issues zero writer ops.
#[derive(Clone, Debug)]
pub struct AccessBump {
    pub id: MemoryId,
    pub count: u64,
    pub last_accessed_at: i64,
}

/// A minimal projection of a memory row for the consolidation scan: only the
/// fields the job and its survivor policy need. Avoids loading full notes/links.
#[derive(Clone, Debug)]
pub struct ConsolidationCandidate {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

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
        Self::init(conn, embedding_dim, embedding_model)
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
        // A zero-dimension embedding produces a malformed `float[0]` vec0 column.
        // Reject it up front rather than letting SQLite fail cryptically later.
        if embedding_dim == 0 {
            return Err(Error::Storage(
                "embedding_dim must be greater than 0".to_string(),
            ));
        }

        // WAL gives concurrent readers + one writer with no SQLITE_BUSY storms.
        // (In-memory DBs ignore WAL and report "memory"; that is fine.)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_err)?;

        // A busy handler keeps the P1 daemon (multiple connections + WAL
        // checkpoints) from hitting immediate SQLITE_BUSY: a contended write
        // waits up to 5s for the lock instead of failing right away.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_err)?;

        run_migrations(&conn)?;

        // Verify the dimension invariant BEFORE touching the vector table: the
        // schema rebuild below re-creates the vec0 table with the configured
        // dim baked into the DDL, so a dim mismatch must fail closed first
        // (rebuilding under a wrong dim would commit a malformed table).
        seed_or_verify_dim(&conn, embedding_dim)?;

        // Dynamic-dimension vector table (vec0 needs the literal dim baked in),
        // created at the current vector schema version — or rebuilt in place
        // from a previous version (W1.1 cosine metric + W1.7 namespace
        // partition, one combined rebuild).
        ensure_vector_schema(&conn, embedding_dim)?;
        if let Some(model) = embedding_model {
            seed_or_verify_model(&conn, model)?;
        }
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

    /// Explicit opt-in for an embedding-model swap: atomically point
    /// `meta.embedding_model` at `new_model` and stale every row's
    /// `embedding_input_version` to the `''` sentinel so the existing reembed
    /// machinery converges the corpus onto the new vector space.
    ///
    /// Returns `true` when a swap occurred (rows were staled). Idempotent:
    /// `false` when `new_model` is already current, or when no model was
    /// recorded yet (legacy/fresh DB — it is seeded without staling, because
    /// the per-row `embedding_model` stamps already drive reembed there).
    pub fn accept_model_change(&self, new_model: &str) -> Result<bool> {
        immediate_tx(&self.conn, || {
            let stored = meta_value(&self.conn, "embedding_model")?;
            if stored.as_deref() == Some(new_model) {
                return Ok(false);
            }
            let swapping = stored.is_some();
            self.conn
                .execute(
                    "INSERT INTO meta (key, value) VALUES ('embedding_model', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![new_model],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if swapping {
                self.conn
                    .execute("UPDATE memories SET embedding_input_version = ''", [])
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }
            Ok(swapping)
        })
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

    /// One-time namespace rename (W0.3 carryover): re-scope EVERY memory row
    /// (active and archived) from `old` to `new` in ONE transaction. The
    /// dogfood-data lifecycle (plan §11) depends on this: memories captured
    /// before a repo pins identity via `.rusty-brain.toml` land under the
    /// heuristic directory-name namespace and need re-scoping once.
    ///
    /// What moves, table by table:
    /// - `memories.namespace` — plain UPDATE. Under migration 006 the `mem_au`
    ///   trigger fires only on content/summary/keywords/tags, and namespace is
    ///   not an FTS-indexed column (`keyword_search` scopes via a JOIN on
    ///   `memories`), so this rewrites ZERO FTS rows.
    /// - `memory_vectors` — namespace is the vec0 PARTITION KEY (W1.7) and
    ///   vec0 supports no partition-key UPDATE, so each moving row is
    ///   point-DELETEd and re-INSERTed under the new key (the two mutation
    ///   patterns the archive and insert paths already prove). Archived rows
    ///   have no vector (W1.7 hygiene), so `vectors <= memories`.
    /// - `memory_links` — carries no namespace column; edges key on memory ids
    ///   and survive untouched.
    /// - `memory_oplog` — ONE `namespace_rename` row recording old, new and
    ///   the row counts; historical oplog rows keep their original namespace
    ///   (the log is history, not state).
    ///
    /// Collision policy: a non-empty `new` is refused with a validation-class
    /// error unless `merge` is set; under `merge` the old rows are appended
    /// and the pre-existing target count is reported in the outcome + oplog.
    /// Exact-string semantics: renaming `project:foo` does NOT touch
    /// `session:foo:*` namespaces. Not on the `Store` trait — like
    /// `accept_model_change` it is a cross-namespace admin op, not an engine
    /// operation.
    pub fn rename_namespace(
        &self,
        old: &Namespace,
        new: &Namespace,
        merge: bool,
    ) -> Result<NamespaceRenameOutcome> {
        let old_str = old.as_db_string();
        let new_str = new.as_db_string();
        if old_str == new_str {
            return Err(Error::InvalidArgument(format!(
                "cannot rename namespace `{old_str}` to itself"
            )));
        }

        immediate_tx(&self.conn, || {
            let count_rows = |ns: &str| -> Result<i64> {
                self.conn
                    .query_row(
                        "SELECT COUNT(*) FROM memories WHERE namespace = ?1",
                        rusqlite::params![ns],
                        |r| r.get(0),
                    )
                    .map_err(storage_err)
            };

            let target_rows = count_rows(&new_str)?;
            if target_rows > 0 && !merge {
                return Err(Error::InvalidArgument(format!(
                    "target namespace `{new_str}` already has {target_rows} memories; \
                     pass --merge to combine them"
                )));
            }
            let source_rows = count_rows(&old_str)?;
            if source_rows == 0 {
                return Err(Error::InvalidArgument(format!(
                    "namespace `{old_str}` has no memories to rename"
                )));
            }

            // Collect the moving vectors BEFORE the memories UPDATE, while the
            // old namespace still identifies them. vec0 fullscan + JOIN is the
            // access pattern `rebuild_vector_table` already proves; the blobs
            // are held in memory for the duration of the transaction, which is
            // acceptable for a one-time helper (dim x 4 bytes per row).
            let moving: Vec<(String, Vec<u8>)> = {
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT v.memory_id, v.embedding
                         FROM memory_vectors v
                         JOIN memories m ON m.memory_id = v.memory_id
                         WHERE m.namespace = ?1",
                    )
                    .map_err(storage_err)?;
                let rows = stmt
                    .query_map(rusqlite::params![old_str], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(storage_err)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(storage_err)?);
                }
                out
            };

            // Re-scope the memories rows. `updated_at` is deliberately NOT
            // bumped: a rename is administrative and must not distort recency
            // ranking or reembed staleness.
            let moved = self
                .conn
                .execute(
                    "UPDATE memories SET namespace = ?1 WHERE namespace = ?2",
                    rusqlite::params![new_str, old_str],
                )
                .map_err(storage_err)?;

            // Re-key the vec0 partition rows: point DELETE + INSERT per row,
            // unchanged embedding bytes.
            let mut vectors: u64 = 0;
            {
                let mut del = self
                    .conn
                    .prepare("DELETE FROM memory_vectors WHERE memory_id = ?1")
                    .map_err(storage_err)?;
                let mut ins = self
                    .conn
                    .prepare(
                        "INSERT INTO memory_vectors (memory_id, namespace, embedding)
                         VALUES (?1, ?2, ?3)",
                    )
                    .map_err(storage_err)?;
                for (id, embedding) in &moving {
                    del.execute(rusqlite::params![id]).map_err(storage_err)?;
                    ins.execute(rusqlite::params![id, new_str, embedding])
                        .map_err(storage_err)?;
                    vectors += 1;
                }
            }

            // One oplog row for the whole bulk op, committed with it. The
            // per-memory `append_oplog` helper does not fit (no single memory
            // id); `memory_id` is the empty sentinel and the payload rides in
            // `details`.
            let details = serde_json::json!({
                "old": old_str,
                "new": new_str,
                "moved": moved,
                "vectors": vectors,
                "merged_into": target_rows,
            })
            .to_string();
            self.conn
                .execute(
                    "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
                     VALUES (?1, 'namespace_rename', '', ?2, ?3, ?4)",
                    rusqlite::params![
                        self.site_id,
                        new_str,
                        chrono::Utc::now().timestamp(),
                        details
                    ],
                )
                .map_err(storage_err)?;

            Ok(NamespaceRenameOutcome {
                memories: moved as u64,
                vectors,
                merged_into: target_rows as u64,
            })
        })
    }

    // One link edge selected for decay. `created_at` is decoded fail-closed.
    // Defined here (not as a `Store` trait method) because the decay job calls
    // it directly through the read pool, outside the engine's namespace scope.

    /// Read up to `limit` link edges for the decay job, newest-irrelevant order
    /// (PK order is fine; decay is per-row and idempotent). One query, no joins.
    pub fn links_for_decay(&self, limit: usize) -> Result<Vec<LinkRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source_id, target_id, link_type, strength, base_strength, created_at
                 FROM memory_links
                 LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(
                // Saturating conversion (matches candidates_for_consolidation /
                // memories_for_recalibration): a raw `as i64` would wrap a huge
                // usize to a negative LIMIT, which SQLite treats as unbounded.
                rusqlite::params![i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            let (src, tgt, lt, strength, base_strength, created) =
                r.map_err(|e| Error::Storage(e.to_string()))?;
            out.push(LinkRow {
                source: src.parse::<MemoryId>()?,
                target: tgt.parse::<MemoryId>()?,
                link_type: rb_types::LinkType::parse(&lt)?,
                // strength is SQLite REAL (f64) narrowed to f32, matching load_links.
                strength: strength as f32,
                base_strength: base_strength as f32,
                created_at: from_ts(created)?,
            });
        }
        Ok(out)
    }

    /// Set the `strength` of a single link edge identified by its full PK.
    /// A missing edge is a no-op (0 rows updated); decay is best-effort.
    pub fn set_link_strength(
        &self,
        source: &MemoryId,
        target: &MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
    ) -> Result<()> {
        // Transaction: the UPDATE and its oplog row commit (or roll back)
        // together — link strength is durable graph state replay must reproduce.
        immediate_tx(&self.conn, || {
            let affected = self
                .conn
                .execute(
                    "UPDATE memory_links SET strength = ?1
                     WHERE source_id = ?2 AND target_id = ?3 AND link_type = ?4",
                    rusqlite::params![
                        strength as f64,
                        source.to_string(),
                        target.to_string(),
                        link_type.as_str(),
                    ],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if affected > 0 {
                let details = serde_json::json!({
                    "type": link_type.as_str(),
                    "target": target.to_string(),
                    "strength": strength,
                })
                .to_string();
                append_oplog(
                    &self.conn,
                    &self.site_id,
                    "set_link_strength",
                    source,
                    &details,
                )?;
            }
            Ok(())
        })
    }

    /// Set the EFFECTIVE `importance` of a single memory WITHOUT touching its
    /// `base_importance` author prior (W1.9). This is the importance
    /// recalibration job's only write path; the user-facing `update_memory`
    /// path is the only writer of `base_importance`. Validates the `1..=10`
    /// range fail-closed (matching the insert path). A missing id is a no-op
    /// (0 rows), mirroring `set_confidence`/`set_link_strength`.
    ///
    /// `updated_at` is intentionally NOT bumped: recalibration is a background
    /// maintenance write, not an authorial edit — the same rule `update_vector`
    /// and `rename_namespace` follow. Bumping it would make every nudged memory
    /// look freshly user-modified to list/context, recency ranking, and any
    /// updated_at-based sync each time the job runs. The oplog row below
    /// records the change durably for replay.
    pub fn set_recalibrated_importance(&self, id: &MemoryId, importance: u8) -> Result<()> {
        rb_types::validate_importance(importance)?;
        // Transaction: the UPDATE and its oplog row commit (or roll back)
        // together — importance is durable ranking state replay must reproduce.
        immediate_tx(&self.conn, || {
            let affected = self
                .conn
                .execute(
                    "UPDATE memories SET importance = ?1 WHERE memory_id = ?2",
                    rusqlite::params![importance as i64, id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if affected > 0 {
                let details = serde_json::json!({ "importance": importance }).to_string();
                append_oplog(&self.conn, &self.site_id, "set_importance", id, &details)?;
            }
            Ok(())
        })
    }

    /// Set the `confidence` of a single memory. Validates the `0.0..=1.0` range
    /// fail-closed (matching the insert path). A missing id is a no-op (0 rows).
    /// Used by maintenance/test paths; the engine never mutates confidence on the
    /// shipped recall/remember flow.
    pub fn set_confidence(&self, id: &MemoryId, confidence: f32) -> Result<()> {
        rb_types::validate_confidence(confidence)?;
        // Transaction: the UPDATE and its oplog row commit (or roll back)
        // together — confidence is durable ranking state replay must reproduce.
        immediate_tx(&self.conn, || {
            let affected = self
                .conn
                .execute(
                    "UPDATE memories SET confidence = ?1 WHERE memory_id = ?2",
                    rusqlite::params![confidence as f64, id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if affected > 0 {
                let details = serde_json::json!({ "confidence": confidence }).to_string();
                append_oplog(&self.conn, &self.site_id, "set_confidence", id, &details)?;
            }
            Ok(())
        })
    }

    /// Read a value from the key/value `meta` table (e.g. the
    /// `embedding_input_version` seed written by migration 003), or `None` if the
    /// key is absent. A small read helper for invariant checks and the migration
    /// reproducibility gate.
    pub fn meta_value(&self, key: &str) -> Result<Option<String>> {
        meta_value(&self.conn, key)
    }

    /// Retroactively redact secrets from every stored memory (W2.4
    /// `rusty-brain scrub`). Scans ALL rows — active AND archived, since a
    /// secret at rest is a secret regardless of archive state — applies the
    /// shared `rb_redact` pass to EVERY stored free-text column and rewrites
    /// any row that changed.
    ///
    /// The redacted column set MUST be the union of the FTS-indexed text
    /// columns (migration 001: `content`, `summary`, `keywords`, `tags`) and
    /// the embedding-input columns (`rb_engine::embed_input`: `content`,
    /// `keywords`, `tags`, `context`) — i.e. all five of content, summary,
    /// keywords, tags, context — or a redacted-away secret could still be
    /// discoverable by keyword search or still encoded in the vector after
    /// reembed. FTS resyncs automatically via the `mem_au` trigger, which fires
    /// on UPDATE OF content/summary/keywords/tags (migration 006).
    /// `keywords`/`tags` are JSON arrays; redacting the raw JSON string is safe
    /// — the `[REDACTED:..]` markers contain no quotes (the chars that would
    /// break a JSON string value; the surrounding `[ ]` are inside the value
    /// and harmless), so the array stays valid. A drift guard
    /// (`scrub_covers_every_fts_and_embedding_text_column`) fails if a new
    /// searchable/embedded column is added without teaching scrub.
    ///
    /// Embedding hygiene: a row's embedding stamp is blanked (so the next
    /// `reembed` recomputes a clean vector) ONLY when it is ACTIVE and its
    /// embedding-feeding text (content/keywords/tags/context) changed. Archived
    /// rows have no
    /// live vector — archival deletes the vec0 row, `reembed` skips
    /// `archived_at IS NULL` rows, and `update_vector` refuses archived ids —
    /// so blanking their stamp would inflate `reembed_pending` with work no
    /// reembed pass could ever clear (the "run reembed until changed=0"
    /// guidance would never converge). Their text is still redacted; only the
    /// stamp/count is left alone. `reembed_pending` therefore counts exactly
    /// the rows a follow-up `reembed` will revisit.
    ///
    /// `updated_at` is deliberately NOT bumped: scrub is administrative
    /// hygiene, not an authorial edit (the `rename_namespace` /
    /// `set_recalibrated_importance` rule), so it must not distort recency
    /// ranking. One bulk oplog row records the pass.
    pub fn scrub(&self) -> Result<ScrubOutcome> {
        let outcome = immediate_tx(&self.conn, || {
            let mut scanned = 0u64;
            let mut redacted = 0u64;
            let mut reembed_pending = 0u64;
            // Keyset-paginate by memory_id so peak memory is bounded to one
            // batch, not the whole corpus (scrub is meant to clean exactly the
            // big DBs that a full-table snapshot would OOM). Already-rewritten
            // rows keep their id, so `memory_id > cursor` never revisits them.
            let mut cursor = String::new();
            loop {
                let batch = self.scrub_select_batch(&cursor)?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch[batch.len() - 1].0.clone();
                scanned += batch.len() as u64;
                for (id, content, summary, context, keywords, tags, archived_at) in batch {
                    // keywords/tags are JSON-array strings; redacting the raw
                    // JSON is safe — the `[REDACTED:..]` markers contain no
                    // quotes, so the array stays valid JSON.
                    let cols = [
                        ("content", &content, rb_redact::redact(&content)),
                        ("summary", &summary, rb_redact::redact(&summary)),
                        ("context", &context, rb_redact::redact(&context)),
                        ("keywords", &keywords, rb_redact::redact(&keywords)),
                        ("tags", &tags, rb_redact::redact(&tags)),
                    ];
                    if cols.iter().all(|(_, old, new)| new == *old) {
                        continue; // nothing redactable in this row
                    }
                    redacted += 1;
                    // Blank the embedding stamp only for ACTIVE rows whose
                    // embedding-feeding text (content/context/keywords/tags —
                    // summary does NOT feed the composite document) changed:
                    // reembed only revisits active, stale rows, so an archived
                    // row's blanked stamp would never be cleared.
                    let embed_text_changed = cols
                        .iter()
                        .any(|(name, old, new)| *name != "summary" && new != *old);
                    let blank_embed = archived_at.is_none() && embed_text_changed;
                    if blank_embed {
                        reembed_pending += 1;
                    }
                    // Build the SET from ONLY the columns that actually changed,
                    // so a context-only redaction does not list content/summary/
                    // keywords/tags and needlessly fire the `mem_au` FTS trigger
                    // (`AFTER UPDATE OF content,summary,keywords,tags`).
                    let mut sets: Vec<String> = Vec::new();
                    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                    for (name, old, new) in &cols {
                        if new != *old {
                            sets.push(format!("{name} = ?{}", params.len() + 1));
                            params.push(Box::new(new.clone()));
                        }
                    }
                    if blank_embed {
                        sets.push("embedding_input_version = ''".to_string());
                    }
                    let id_pos = params.len() + 1;
                    params.push(Box::new(id.clone()));
                    let sql = format!(
                        "UPDATE memories SET {} WHERE memory_id = ?{id_pos}",
                        sets.join(", ")
                    );
                    let refs: Vec<&dyn rusqlite::ToSql> =
                        params.iter().map(|b| b.as_ref()).collect();
                    self.conn
                        .execute(&sql, refs.as_slice())
                        .map_err(storage_err)?;
                }
            }

            if redacted > 0 {
                let details = serde_json::json!({
                    "scanned": scanned,
                    "redacted": redacted,
                    "reembed_pending": reembed_pending,
                })
                .to_string();
                self.conn
                    .execute(
                        "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
                         VALUES (?1, 'scrub', '', '', ?2, ?3)",
                        rusqlite::params![self.site_id, chrono::Utc::now().timestamp(), details],
                    )
                    .map_err(storage_err)?;
            }

            Ok(ScrubOutcome {
                scanned,
                redacted,
                reembed_pending,
            })
        })?;

        // Force the pre-redaction WAL frames into the main DB and truncate the
        // -wal file: without this, `scrub` could report success while the old
        // plaintext still sits in the -wal on disk until an arbitrary later
        // checkpoint. Best-effort and only when something changed (a no-op
        // scrub wrote nothing). A no-op on an in-memory DB (no WAL). Residual:
        // freed-page slack in the main DB needs VACUUM/purge (W5b.3) — out of
        // scope here; this closes the obvious recoverable copy.
        if outcome.redacted > 0 {
            self.conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(storage_err)?;
        }
        Ok(outcome)
    }

    /// One keyset page of redactable rows with `memory_id > cursor`, ordered by
    /// `memory_id`, capped at `SCRUB_BATCH`. Split out so the prepared SELECT
    /// statement is dropped before the caller issues UPDATEs for the page.
    #[allow(clippy::type_complexity)]
    fn scrub_select_batch(
        &self,
        cursor: &str,
    ) -> Result<Vec<(String, String, String, String, String, String, Option<i64>)>> {
        const SCRUB_BATCH: i64 = 500;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, content, summary, context, keywords, tags, archived_at \
                 FROM memories WHERE memory_id > ?1 ORDER BY memory_id LIMIT ?2",
            )
            .map_err(storage_err)?;
        let mapped = stmt
            .query_map(rusqlite::params![cursor, SCRUB_BATCH], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for r in mapped {
            out.push(r.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// The highest oplog sequence committed so far (`0` on an empty log).
    /// On the single writer thread, calling this right after a successful op
    /// yields that op's seq (W2.7: stamped onto the broadcast `MemoryChanged`
    /// so consumers can track a replay cursor).
    pub fn last_oplog_seq(&self) -> Result<u64> {
        let seq: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM memory_oplog", [], |r| {
                r.get(0)
            })
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(u64::try_from(seq).unwrap_or(0))
    }

    /// Replay the durable oplog as change events: every entry in `namespace`
    /// with `seq > after`, oldest first, capped at `limit` (W2.7
    /// replay-on-reconnect). Ops that do not describe a single memory's
    /// change (`rename_namespace`) are skipped; `insert` maps to `Created`,
    /// `archive`/`supersede` to `Archived`, and every other mutation
    /// (`update`, `link`, `unlink`, `set_*`) to `Updated`.
    pub fn oplog_changes_since(
        &self,
        namespace: &rb_types::Namespace,
        after: u64,
        limit: usize,
    ) -> Result<OplogReplayPage> {
        use rb_types::{ChangeKind, MemoryChanged};
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, op, memory_id FROM memory_oplog
                 WHERE namespace = ?1 AND seq > ?2
                 ORDER BY seq ASC LIMIT ?3",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    namespace.as_db_string(),
                    i64::try_from(after).unwrap_or(i64::MAX),
                    limit as i64
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut changes = Vec::new();
        // `scanned` counts oplog ROWS examined (≤ limit), and `last_seq` is the
        // highest seq examined — both BEFORE filtering. The caller paginates on
        // these, not on `changes.len()`: a page is the tail only when fewer than
        // `limit` ROWS were scanned, and the cursor must advance past every
        // scanned row (including skipped ones) or a page full of skipped rows
        // would loop forever / drop the rows after it (W2.7 replay bug #2).
        let mut scanned = 0usize;
        let mut last_seq = after;
        for row in rows {
            let (seq, op, id) = row.map_err(|e| Error::Storage(e.to_string()))?;
            scanned += 1;
            last_seq = last_seq.max(u64::try_from(seq).unwrap_or(0));
            // Bulk admin ops (namespace_rename, scrub) carry an EMPTY memory_id
            // sentinel and have no per-memory change identity to replay — skip
            // by the sentinel, not by op string, so a new bulk op can never
            // reach `parse_id("")` (which would error and abort the whole
            // replay, silently dropping every later change — W2.7 bug #1).
            if id.is_empty() {
                continue;
            }
            let kind = match op.as_str() {
                "insert" => ChangeKind::Created,
                "archive" | "supersede" => ChangeKind::Archived,
                // update / link / unlink / set_importance / set_confidence /
                // set_link_strength — and, fail-open, any op string a NEWER
                // writer invents: a replayed Updated is a safe over-notify.
                _ => ChangeKind::Updated,
            };
            changes.push(MemoryChanged {
                id: parse_id(&id)?,
                namespace: namespace.clone(),
                kind,
                seq: Some(u64::try_from(seq).unwrap_or(0)),
            });
        }
        Ok(OplogReplayPage {
            changes,
            scanned,
            last_seq,
        })
    }

    /// Delete a single link edge identified by its full PK. A missing edge is a
    /// no-op. Used by the decay job's `prune_below_floor` policy.
    pub fn delete_link(
        &self,
        source: &MemoryId,
        target: &MemoryId,
        link_type: rb_types::LinkType,
    ) -> Result<()> {
        // Transaction: the DELETE and its oplog row commit (or roll back)
        // together — a pruned edge must be reproducible from the log.
        immediate_tx(&self.conn, || {
            let affected = self
                .conn
                .execute(
                    "DELETE FROM memory_links
                     WHERE source_id = ?1 AND target_id = ?2 AND link_type = ?3",
                    rusqlite::params![source.to_string(), target.to_string(), link_type.as_str(),],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if affected > 0 {
                let details = serde_json::json!({
                    "type": link_type.as_str(),
                    "target": target.to_string(),
                })
                .to_string();
                append_oplog(&self.conn, &self.site_id, "unlink", source, &details)?;
            }
            Ok(())
        })
    }

    /// Find active memories in `ns` whose stored vector is near-identical to the
    /// vector of `id` (similarity `>= threshold`), excluding `id` itself.
    ///
    /// UNIT: `threshold` and the returned similarities are RAW cosine
    /// similarity clamped to `[0, 1]` (`1 - cosine_distance`, negatives clamp
    /// to 0; see [`distance_to_similarity`]) — a threshold of `0.95` admits
    /// candidates with cosine similarity `>= 0.95` (cosine distance `<= 0.05`).
    ///
    /// Namespace-isolated by construction: the KNN scan is scoped to `ns` via
    /// the vec0 partition key, and candidates are re-checked for namespace +
    /// active (`archived_at IS NULL`) in Rust, exactly as `vector_search` does,
    /// so a near-identical memory in another namespace is NEVER returned. Reads
    /// the anchor's OWN stored embedding from `memory_vectors` and runs the same
    /// vec0 KNN MATCH the search path uses. A missing anchor or an anchor with no
    /// stored vector yields an empty result (not an error). Results are sorted by
    /// similarity descending, then id string ascending for a deterministic
    /// tie-break, and truncated to `limit`.
    pub fn near_duplicates(
        &self,
        ns: &Namespace,
        id: &MemoryId,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        // Load the anchor's stored embedding blob. No row => nothing to compare.
        let blob: Option<Vec<u8>> = match self.conn.query_row(
            "SELECT embedding FROM memory_vectors WHERE memory_id = ?1",
            rusqlite::params![id.to_string()],
            |row| row.get::<_, Vec<u8>>(0),
        ) {
            Ok(b) => Some(b),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(Error::Storage(e.to_string())),
        };
        let Some(blob) = blob else {
            return Ok(Vec::new());
        };
        let anchor_vec = decode_embedding_bytes(&blob)?;
        // Defense-in-depth: a stored vector must match the configured dimension.
        if anchor_vec.len() != self.embedding_dim {
            return Err(Error::DimensionMismatch {
                expected: self.embedding_dim,
                got: anchor_vec.len(),
            });
        }

        // sqlite-vec accepts the query vector as a JSON array string (same as
        // vector_search). The namespace PARTITION KEY scopes the KNN scan in
        // SQL; the Rust-side active re-check below is defense-in-depth.
        let query_json =
            serde_json::to_string(&anchor_vec).map_err(|e| Error::Serialization(e.to_string()))?;

        const VEC0_KNN_MAX: i64 = 4096;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        // +1 because the anchor itself is the nearest (distance 0) and is then
        // dropped by the self-exclusion filter below.
        let k_budget = limit_i64
            .saturating_add(1)
            .saturating_mul(10)
            .max(limit_i64)
            .min(VEC0_KNN_MAX);

        let ns_str = ns.as_db_string();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, distance
                 FROM memory_vectors
                 WHERE embedding MATCH ?1
                   AND namespace = ?2
                 ORDER BY distance
                 LIMIT ?3",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![query_json, ns_str, k_budget], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        let self_str = id.to_string();
        let mut out: Vec<(MemoryId, f32)> = Vec::new();
        for r in rows {
            let (cand_str, dist) = r.map_err(|e| Error::Storage(e.to_string()))?;

            // Exclude self.
            if cand_str == self_str {
                continue;
            }

            // Namespace + active filter, fail closed on any non-"no rows" error.
            let active: bool = match self.conn.query_row(
                "SELECT 1 FROM memories WHERE memory_id = ?1 AND namespace = ?2 AND archived_at IS NULL",
                rusqlite::params![cand_str, ns_str],
                |_| Ok(true),
            ) {
                Ok(found) => found,
                Err(rusqlite::Error::QueryReturnedNoRows) => false,
                Err(e) => return Err(Error::Storage(e.to_string())),
            };
            if !active {
                continue;
            }

            let similarity = distance_to_similarity(dist as f32);
            if similarity >= threshold {
                out.push((parse_id(&cand_str)?, similarity));
            }
        }

        // Deterministic order: similarity descending, then id string ascending.
        out.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        out.truncate(limit);
        Ok(out)
    }

    /// Active (`archived_at IS NULL`), non-superseded (`superseded_by IS NULL`)
    /// memories, oldest first then by id, capped at `limit`. The deterministic
    /// ORDER BY makes a consolidation pass reproducible.
    pub fn candidates_for_consolidation(
        &self,
        limit: usize,
    ) -> Result<Vec<ConsolidationCandidate>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, namespace, importance, created_at
                 FROM memories
                 WHERE archived_at IS NULL AND superseded_by IS NULL
                 ORDER BY created_at ASC, memory_id ASC
                 LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(
                rusqlite::params![i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            let (id_str, ns_str, importance, created) =
                r.map_err(|e| Error::Storage(e.to_string()))?;
            out.push(ConsolidationCandidate {
                id: parse_id(&id_str)?,
                namespace: Namespace::parse_db_string(&ns_str)?,
                importance: u8::try_from(importance).map_err(|_| {
                    Error::Storage(format!("importance {importance} out of u8 range"))
                })?,
                created_at: from_ts(created)?,
            });
        }
        Ok(out)
    }
}

/// One active memory's recalibration inputs: the spine fields the importance
/// job reads to recompute `importance`. `last_accessed_at` is the raw stored
/// unix-seconds value (`None` when the memory has never been accessed); the job
/// passes it straight into the pure `recalibrate` function with a single `now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecalRow {
    pub namespace: Namespace,
    pub id: MemoryId,
    /// Current EFFECTIVE importance (what ranking reads). The job uses it only
    /// for change detection — never as a formula input.
    pub importance: u8,
    /// Author-set importance prior (W1.9). The recalibration target is a pure
    /// function of THIS value plus the access signals, bounded to its ±2 band,
    /// which is what makes the job idempotent and author-intent-preserving.
    /// Reads COALESCE to `importance` should the column ever be NULL.
    pub base_importance: u8,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
}

impl SqliteStore {
    /// Read up to `limit` ACTIVE (non-archived) memories with the fields the
    /// importance-recalibration job needs. Bounded by `limit` and ordered by
    /// `created_at DESC` for a stable, deterministic scan. Read-only: issues no
    /// writes (every mutation goes through the single writer via `StoreHandle`).
    pub fn memories_for_recalibration(&self, limit: usize) -> Result<Vec<RecalRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT namespace, memory_id, importance,
                        COALESCE(base_importance, importance) AS base_importance,
                        access_count, last_accessed_at
                 FROM memories
                 WHERE archived_at IS NULL
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(rusqlite::params![i64::try_from(limit).unwrap_or(i64::MAX)])
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            let namespace = Namespace::parse_db_string(
                &row.get::<_, String>("namespace")
                    .map_err(|e| Error::Storage(e.to_string()))?,
            )?;
            let id = parse_id(
                &row.get::<_, String>("memory_id")
                    .map_err(|e| Error::Storage(e.to_string()))?,
            )?;
            let importance = row
                .get::<_, i64>("importance")
                .map_err(|e| Error::Storage(e.to_string()))? as u8;
            let base_importance =
                row.get::<_, i64>("base_importance")
                    .map_err(|e| Error::Storage(e.to_string()))? as u8;
            let access_count = row
                .get::<_, i64>("access_count")
                .map_err(|e| Error::Storage(e.to_string()))?;
            let last_accessed_at = row
                .get::<_, Option<i64>>("last_accessed_at")
                .map_err(|e| Error::Storage(e.to_string()))?;
            out.push(RecalRow {
                namespace,
                id,
                importance,
                base_importance,
                access_count,
                last_accessed_at,
            });
        }
        Ok(out)
    }
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

/// Read one `meta` value; `None` when the key was never seeded.
fn meta_value(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>> {
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
fn immediate_tx<T>(conn: &rusqlite::Connection, body: impl FnOnce() -> Result<T>) -> Result<T> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(storage_err)?;
    let value = body()?;
    tx.commit().map_err(storage_err)?;
    Ok(value)
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
fn ensure_vector_schema(conn: &rusqlite::Connection, embedding_dim: usize) -> Result<()> {
    if meta_value(conn, "vector_schema_version")?.as_deref() == Some(VECTOR_SCHEMA_VERSION) {
        return Ok(());
    }

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'memory_vectors'",
            [],
            |r| r.get(0),
        )
        .map_err(storage_err)?;

    // One transaction for create-or-rebuild + marker: a crash mid-way rolls
    // everything back and the next open retries from the same state.
    immediate_tx(conn, || {
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
fn seed_or_verify_dim(conn: &rusqlite::Connection, embedding_dim: usize) -> Result<()> {
    let existing = meta_value(conn, "embedding_dim")?;

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

/// Seed `meta.embedding_model` on first init (or on the first open of a DB
/// created before the key existed), or verify it matches on re-open. A
/// same-dim provider swap would silently mix vector spaces, so this fails
/// closed with the explicit remediation.
fn seed_or_verify_model(conn: &rusqlite::Connection, embedding_model: &str) -> Result<()> {
    match meta_value(conn, "embedding_model")? {
        Some(stored) => {
            if stored != embedding_model {
                return Err(Error::Storage(format!(
                    "embedding model changed (stored: {stored}, configured: {embedding_model}); \
                     run with --accept-model-change to mark the corpus for re-embedding"
                )));
            }
        }
        None => {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('embedding_model', ?1)",
                rusqlite::params![embedding_model],
            )
            .map_err(storage_err)?;
        }
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

/// Map a `memory_links` INSERT error: a PRIMARY-KEY/UNIQUE violation means the
/// `(source, target, type)` edge already exists, so return the same
/// validation-class "already exists" error `MemoryEngine::link`'s pre-check
/// produces (deterministic across the check-then-act race). Every other error
/// — including a FOREIGN-KEY violation on a missing endpoint — stays Storage.
fn map_link_constraint_err(e: rusqlite::Error, link: &MemoryLink) -> Error {
    if let rusqlite::Error::SqliteFailure(err, _) = &e {
        if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
            || err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        {
            return Error::InvalidArgument(format!(
                "a {} link from {} to {} already exists",
                link.link_type.as_str(),
                link.source_id,
                link.target_id
            ));
        }
    }
    Error::Storage(e.to_string())
}

/// Append one `memory_oplog` row for a mutation on `memory_id`, resolving the
/// namespace from the memories row (already written within the same
/// transaction for inserts). MUST be called inside the mutation's transaction
/// so the log row commits — or rolls back — with the mutation itself. Callers
/// gate on the mutation having actually changed a row, so the SELECT always
/// resolves; `details` is a small JSON payload (e.g. link type) or empty.
fn append_oplog(
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

// ---------------------------------------------------------------------------
// Private codec helpers
// ---------------------------------------------------------------------------

fn json_array(v: &[String]) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Serialization(e.to_string()))
}

fn ts(dt: chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp()
}

fn opt_ts(dt: Option<chrono::DateTime<chrono::Utc>>) -> Option<i64> {
    dt.map(|d| d.timestamp())
}

fn embedding_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf
}

/// Decode a little-endian `f32` blob (the exact byte layout `embedding_bytes`
/// writes) back into a `Vec<f32>`. Fail closed if the length is not a multiple
/// of four bytes rather than silently truncating a corrupt vector.
fn decode_embedding_bytes(bytes: &[u8]) -> Result<Vec<f32>> {
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

/// Convert a vec0 cosine `distance` (`1 - cosine_similarity`, range `[0, 2]`)
/// into RAW cosine similarity clamped to `[0, 1]` (`(1 - d).clamp(0, 1)` —
/// anti-correlated vectors, cos < 0, clamp to 0). Matches the convention used
/// by `rb-search::rank::score_one` since the W1.1 cosine rebuild, and makes
/// `near_duplicates`' threshold a true cosine-similarity bound (0.95 means
/// cos >= 0.95, i.e. cosine distance <= 0.05). A non-finite distance yields `0.0`.
fn distance_to_similarity(distance: f32) -> f32 {
    if distance.is_finite() {
        (1.0 - distance).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn parse_json_array(s: &str) -> Result<Vec<String>> {
    serde_json::from_str(s).map_err(|e| Error::Serialization(e.to_string()))
}

/// Decode a stored unix-seconds timestamp, failing closed on out-of-range
/// values rather than silently fabricating an epoch-0 datetime.
fn from_ts(secs: i64) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .ok_or_else(|| Error::Storage(format!("timestamp {secs} out of range")))
}

/// `None` stays `None`; a present-but-out-of-range value propagates the error.
fn from_opt_ts(secs: Option<i64>) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    secs.map(from_ts).transpose()
}

fn parse_id(s: &str) -> Result<MemoryId> {
    s.parse::<MemoryId>()
}

fn load_links(conn: &rusqlite::Connection, id: &MemoryId) -> Result<Vec<MemoryLink>> {
    let mut stmt = conn
        .prepare(
            "SELECT source_id, target_id, link_type, strength, reason, created_at
             FROM memory_links WHERE source_id = ?1",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut links = Vec::new();
    for r in rows {
        let (src, tgt, lt, strength, reason, created) =
            r.map_err(|e| Error::Storage(e.to_string()))?;
        links.push(MemoryLink {
            source_id: parse_id(&src)?,
            target_id: parse_id(&tgt)?,
            link_type: rb_types::LinkType::parse(&lt)?,
            // `strength` is stored as SQLite REAL (f64) and narrowed to f32 here.
            strength: strength as f32,
            reason,
            created_at: from_ts(created)?,
        });
    }
    Ok(links)
}

fn row_to_note(conn: &rusqlite::Connection, row: &rusqlite::Row<'_>) -> Result<MemoryNote> {
    let id = parse_id(
        &row.get::<_, String>("memory_id")
            .map_err(|e| Error::Storage(e.to_string()))?,
    )?;
    let namespace = Namespace::parse_db_string(
        &row.get::<_, String>("namespace")
            .map_err(|e| Error::Storage(e.to_string()))?,
    )?;
    let memory_type = MemoryType::parse(
        &row.get::<_, String>("memory_type")
            .map_err(|e| Error::Storage(e.to_string()))?,
    )?;
    let g = |c: &str| -> Result<String> {
        row.get::<_, String>(c)
            .map_err(|e| Error::Storage(e.to_string()))
    };
    let gi = |c: &str| -> Result<i64> {
        row.get::<_, i64>(c)
            .map_err(|e| Error::Storage(e.to_string()))
    };
    let go = |c: &str| -> Result<Option<String>> {
        row.get::<_, Option<String>>(c)
            .map_err(|e| Error::Storage(e.to_string()))
    };
    // TODO(P1): batch link loading (avoid N+1 load_links per row in list/get_memory).
    let links = load_links(conn, &id)?;
    Ok(MemoryNote {
        id,
        namespace,
        created_at: from_ts(gi("created_at")?)?,
        updated_at: from_ts(gi("updated_at")?)?,
        content: g("content")?,
        summary: g("summary")?,
        keywords: parse_json_array(&g("keywords")?)?,
        tags: parse_json_array(&g("tags")?)?,
        context: g("context")?,
        memory_type,
        importance: gi("importance")? as u8,
        // `confidence` is stored as SQLite REAL (f64) and narrowed to f32 on load,
        // so round-trips are only exact for f32-representable values.
        confidence: row
            .get::<_, f64>("confidence")
            .map_err(|e| Error::Storage(e.to_string()))? as f32,
        related_files: parse_json_array(&g("related_files")?)?,
        // Checked conversion: a negative DB value must error, not silently wrap
        // into a huge u64.
        access_count: gi("access_count")?
            .try_into()
            .map_err(|_| Error::Storage("access_count is negative in DB".into()))?,
        last_accessed_at: from_opt_ts(
            row.get::<_, Option<i64>>("last_accessed_at")
                .map_err(|e| Error::Storage(e.to_string()))?,
        )?,
        archived_at: from_opt_ts(
            row.get::<_, Option<i64>>("archived_at")
                .map_err(|e| Error::Storage(e.to_string()))?,
        )?,
        superseded_by: row
            .get::<_, Option<String>>("superseded_by")
            .map_err(|e| Error::Storage(e.to_string()))?
            .map(|s| parse_id(&s))
            .transpose()?,
        embedding_model: g("embedding_model")?,
        embedding_input_version: g("embedding_input_version")?,
        links,
        // `contested` (Feature C) is a read-side annotation computed by the engine
        // from `memory_links` after ranking; it is never stored, so loads default
        // it to false.
        contested: false,
        // Provenance (W0.5): nullable by-name decode; rows written before the
        // 004 migration carry NULL and surface as `None`.
        origin_user: go("origin_user")?,
        origin_host: go("origin_host")?,
        origin_agent: go("origin_agent")?,
        origin_source: go("origin_source")?,
        session_id: go("session_id")?,
    })
}

/// Build a safe FTS5 MATCH expression from raw user text (W1.2).
///
/// The query is split on non-alphanumeric boundaries — the same separator rule
/// as the `unicode61` tokenizer family the index uses — and each token is
/// individually double-quoted so FTS5 operators (`-`, `OR`, `NEAR`, `*`, `"`,
/// parens) are always literal text, never syntax. The quoted tokens are then
/// joined with `OR` and ranked by bm25 (`ORDER BY rank` at the call site), so
/// rare query terms dominate and stopword-only matches sink.
///
/// Decision record (W1.2, full-pipeline numbers on the W1.0 eval goldens with
/// the committed all-MiniLM-L6-v2 replay vectors; det = DeterministicProvider):
///
/// | construction        | replay r@5 / MRR  | det r@5 / MRR     | fts_query_rate |
/// |---------------------|-------------------|-------------------|----------------|
/// | whole-query phrase  | 0.9560 / 0.9931   | 0.0486 / 0.0505   | 0.042          |
/// | AND of tokens       | 0.9560 / 0.9931   | 0.1389 / 0.1477   | 0.139          |
/// | OR of tokens (this) | 0.9630 / 0.9838   | 0.7847 / 0.9611   | 1.000          |
///
/// The plan spec hypothesized AND-of-quoted-tokens, but measurement showed AND
/// caps the FTS channel at 13.9% of natural-language goldens (29% with a
/// stopword list) — question words ("how", "is") rarely appear in memory text,
/// so the conjunction fails before content terms can match. The Phase 1 gate
/// requires FTS contribution on >= 80% of goldens; OR-of-tokens reaches 100%
/// and lifts recall@5 and the deterministic gate metrics sharply, at a small
/// replay-MRR cost. Injection safety is identical under either operator: every
/// token is a quoted phrase of length one, so user text is never FTS5 syntax.
/// A trailing-token prefix match (`"tok"*`) was measured under both operators
/// and both tokenizers: no metric moved under AND/unicode61, and it *hurt*
/// under porter (prefix queries bypass the stemmer: replay MRR 0.9838 ->
/// 0.9769), so it is deliberately not emitted.
///
/// A query with no indexable tokens (operators/punctuation only) returns the
/// empty phrase `""`, which FTS5 parses fine and which matches nothing.
fn escape_fts5_query(query: &str) -> String {
    let mut expr = String::new();
    for token in query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        if !expr.is_empty() {
            expr.push_str(" OR ");
        }
        expr.push('"');
        expr.push_str(token);
        expr.push('"');
    }
    if expr.is_empty() {
        // An empty MATCH string is an FTS5 syntax error; an empty quoted
        // phrase is valid and simply matches nothing.
        return "\"\"".to_string();
    }
    expr
}

// ---------------------------------------------------------------------------
// Store impl
// ---------------------------------------------------------------------------

impl Store for SqliteStore {
    fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()> {
        // Defense-in-depth validation before touching the DB. The SQL CHECK
        // constraints are the backstop; these give a clean early error.
        rb_types::validate_importance(note.importance)?;
        // W2.8 taxonomy: a range rejection is guidance for the caller, so it
        // must be validation-class (InvalidArgument travels verbatim over the
        // wire; Storage is replaced with an opaque "internal error").
        rb_types::validate_confidence(note.confidence)?;

        // Take the write lock at BEGIN (IMMEDIATE) instead of deferring it to the
        // first write. This avoids a deferred-transaction upgrade racing another
        // writer mid-transaction; the busy_timeout above makes a contended BEGIN
        // wait rather than fail immediately. Atomicity is unchanged: all writes
        // commit together, and any error rolls the whole transaction back.
        immediate_tx(&self.conn, || {
            self.conn
                .execute(
                    "INSERT INTO memories (
                        memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model, embedding_input_version,
                        origin_user, origin_host, origin_agent, origin_source, session_id,
                        base_importance
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25
                     )",
                    rusqlite::params![
                        note.id.to_string(),
                        note.namespace.as_db_string(),
                        ts(note.created_at),
                        ts(note.updated_at),
                        note.content,
                        note.summary,
                        json_array(&note.keywords)?,
                        json_array(&note.tags)?,
                        note.context,
                        note.memory_type.as_str(),
                        note.importance as i64,
                        note.confidence as f64,
                        json_array(&note.related_files)?,
                        note.access_count as i64,
                        opt_ts(note.last_accessed_at),
                        opt_ts(note.archived_at),
                        note.superseded_by.as_ref().map(|id| id.to_string()),
                        note.embedding_model,
                        note.embedding_input_version,
                        note.origin_user,
                        note.origin_host,
                        note.origin_agent,
                        note.origin_source,
                        note.session_id,
                        // W1.9: the author-set importance prior. Stamped once at
                        // insert; the recalibration job never writes it, so the
                        // bounded-delta formula stays anchored to author intent.
                        note.importance as i64,
                    ],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;

            append_oplog(&self.conn, &self.site_id, "insert", &note.id, "")?;

            if let Some(emb) = embedding {
                // The namespace partition key MUST mirror memories.namespace
                // (vector_search scopes KNN on it). Any path that mutates a
                // memory's namespace MUST re-key its memory_vectors row in the
                // same transaction or vectors strand under the old partition
                // key — `rename_namespace` (the only such path today) does the
                // DELETE+INSERT re-key for exactly this reason.
                self.conn
                    .execute(
                        "INSERT INTO memory_vectors (memory_id, namespace, embedding)
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![
                            note.id.to_string(),
                            note.namespace.as_db_string(),
                            embedding_bytes(emb)
                        ],
                    )
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }

            for link in &note.links {
                self.conn
                    .execute(
                        "INSERT INTO memory_links
                            (source_id, target_id, link_type, strength, base_strength, reason, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            link.source_id.to_string(),
                            link.target_id.to_string(),
                            link.link_type.as_str(),
                            link.strength as f64,
                            // Baseline equals the created strength; decay never
                            // mutates it, so the pass stays idempotent.
                            link.strength as f64,
                            link.reason,
                            ts(link.created_at),
                        ],
                    )
                    .map_err(|e| Error::Storage(e.to_string()))?;
                // One `link` oplog row per edge, same shape as `add_link`'s:
                // a replay consumer could not reconstruct edges from the bare
                // `insert` row alone.
                let details = serde_json::json!({
                    "type": link.link_type.as_str(),
                    "target": link.target_id.to_string(),
                })
                .to_string();
                append_oplog(&self.conn, &self.site_id, "link", &link.source_id, &details)?;
            }
            Ok(())
        })
    }

    fn get_memory(&self, id: &MemoryId) -> Result<Option<MemoryNote>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model, embedding_input_version,
                        origin_user, origin_host, origin_agent, origin_source, session_id
                 FROM memories WHERE memory_id = ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(rusqlite::params![id.to_string()])
            .map_err(|e| Error::Storage(e.to_string()))?;

        match rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            Some(row) => Ok(Some(row_to_note(&self.conn, row)?)),
            None => Ok(None),
        }
    }

    fn keyword_search(&self, ns: &Namespace, query: &str, limit: usize) -> Result<Vec<MemoryId>> {
        let match_expr = escape_fts5_query(query);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.memory_id
                 FROM memories_fts
                 JOIN memories m ON m.rowid = memories_fts.rowid
                 WHERE memories_fts MATCH ?1
                   AND m.namespace = ?2
                   AND m.archived_at IS NULL
                 ORDER BY rank
                 LIMIT ?3",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(
                rusqlite::params![
                    match_expr,
                    ns.as_db_string(),
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut ids = Vec::new();
        for r in rows {
            let s = r.map_err(|e| Error::Storage(e.to_string()))?;
            ids.push(s.parse::<MemoryId>()?);
        }
        Ok(ids)
    }

    /// KNN over the version-2 vec0 table: the `namespace` PARTITION KEY scopes
    /// the scan (and the 4096 hard cap) to in-namespace vectors, and vector
    /// hygiene (archive/supersede delete the vector row; the open-time rebuild
    /// pruned pre-existing archived vectors) keeps the partition live-only — so
    /// a namespace holding <1% of all vectors still fills `limit`. The Rust-side
    /// active/namespace re-check below is defense-in-depth, not load-bearing.
    fn vector_search(
        &self,
        ns: &Namespace,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        if embedding.len() != self.embedding_dim {
            return Err(Error::DimensionMismatch {
                expected: self.embedding_dim,
                got: embedding.len(),
            });
        }

        // sqlite-vec accepts the query vector as a JSON array string.
        let query_json =
            serde_json::to_string(embedding).map_err(|e| Error::Serialization(e.to_string()))?;

        // The namespace partition key scopes the KNN scan in SQL; the modest
        // over-fetch below only buys headroom for the Rust-side
        // defense-in-depth active re-check (the table should contain live
        // vectors only — archive/supersede delete them transactionally).
        //
        // Deviation from the plan: the plan used a CTE with `k = ?` plus an outer
        // `LIMIT`. That would cause a sqlite-vec error: "Only LIMIT or 'k =?' can be
        // provided, not both" (the query planner sees both when it pushes the outer
        // LIMIT into the CTE scan). We instead use a single-level query with LIMIT
        // only, then re-check candidates in Rust.
        //
        // vec0 returns min(LIMIT, partition_rows) without error.
        // sqlite-vec enforces a hard KNN cap of 4096; k_budget must not exceed
        // it. With the partition key the cap now applies to LIVE, IN-NAMESPACE
        // candidates rather than the whole corpus (W1.7).
        const VEC0_KNN_MAX: i64 = 4096;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let k_budget = limit_i64
            .saturating_mul(10)
            .max(limit_i64)
            .min(VEC0_KNN_MAX);

        let ns_str = ns.as_db_string();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, distance
                 FROM memory_vectors
                 WHERE embedding MATCH ?1
                   AND namespace = ?2
                 ORDER BY distance
                 LIMIT ?3",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![query_json, ns_str, k_budget], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        // Defense-in-depth: re-check namespace + active status per candidate.
        let mut out = Vec::new();
        for r in rows {
            let (id_str, dist) = r.map_err(|e| Error::Storage(e.to_string()))?;

            // Check namespace and archived status in one query. Fail closed: a
            // missing row means "not active" (skip), but ANY other DB error must
            // propagate rather than be silently swallowed (which would drop a
            // candidate as if it were out of scope).
            let active: bool = match self.conn.query_row(
                "SELECT 1 FROM memories WHERE memory_id = ?1 AND namespace = ?2 AND archived_at IS NULL",
                rusqlite::params![id_str, ns_str],
                |_| Ok(true),
            ) {
                Ok(found) => found,
                Err(rusqlite::Error::QueryReturnedNoRows) => false,
                Err(e) => return Err(Error::Storage(e.to_string())),
            };

            if active {
                let id = parse_id(&id_str)?;
                out.push((id, dist as f32));
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The recursive CTE `UNION` dedups on (node, depth) pairs, so a cycle can
    /// accumulate O(depth x cycle_length) intermediate rows before the outer
    /// `GROUP BY` flattens them — fine at P0's bounded depth.
    ///
    /// W1.5: the UNION keeps EVERY distinct `(node, d)` pair a multi-path walk
    /// produces, so exposing `d` directly would emit one row per path length.
    /// `MIN(d) ... GROUP BY node` collapses each node to its shortest hop
    /// distance (diamond shapes: the shorter path wins).
    fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<(MemoryId, u8)>> {
        if depth == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "WITH RECURSIVE walk(node, d) AS (
                     SELECT target_id, 1
                     FROM memory_links
                     WHERE source_id = ?1
                     UNION
                     SELECT l.target_id, w.d + 1
                     FROM memory_links l
                     JOIN walk w ON l.source_id = w.node
                     WHERE w.d < ?2
                 )
                 SELECT node, MIN(d) AS hops
                 FROM walk
                 WHERE node <> ?1
                 GROUP BY node
                 ORDER BY MIN(d), node",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![id.to_string(), depth as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            let (s, d) = r.map_err(|e| Error::Storage(e.to_string()))?;
            // d is bounded by `depth` (a u8), so the cast is lossless; clamp
            // defensively rather than trusting the SQL invariant.
            let hops = u8::try_from(d).unwrap_or(u8::MAX);
            out.push((s.parse::<MemoryId>()?, hops));
        }
        Ok(out)
    }

    fn list(
        &self,
        ns: &Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        let min = min_importance.unwrap_or(0) as i64;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model, embedding_input_version,
                        origin_user, origin_host, origin_agent, origin_source, session_id
                 FROM memories
                 WHERE namespace = ?1
                   AND archived_at IS NULL
                   AND importance >= ?2
                 ORDER BY created_at DESC
                 LIMIT ?3",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(rusqlite::params![
                ns.as_db_string(),
                min,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ])
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            out.push(row_to_note(&self.conn, row)?);
        }
        Ok(out)
    }

    fn update_memory(&self, id: &MemoryId, updates: &MemoryUpdates) -> Result<()> {
        // Defense-in-depth validation, consistent with insert_memory, before
        // touching the DB.
        if let Some(imp) = updates.importance {
            rb_types::validate_importance(imp)?;
        }
        if let Some(conf) = updates.confidence {
            rb_types::validate_confidence(conf)?;
        }

        let mut sets: Vec<String> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(content) = &updates.content {
            sets.push(format!("content = ?{}", params.len() + 1));
            params.push(Box::new(content.clone()));
        }
        if let Some(summary) = &updates.summary {
            sets.push(format!("summary = ?{}", params.len() + 1));
            params.push(Box::new(summary.clone()));
        }
        if let Some(importance) = updates.importance {
            sets.push(format!("importance = ?{}", params.len() + 1));
            params.push(Box::new(importance as i64));
            // W1.9: an explicit importance update is the author RE-DECLARING
            // intent, so the prior moves with it. Only this user-facing path
            // re-stamps `base_importance`; the recalibration job writes through
            // `set_recalibrated_importance`, which leaves the prior untouched.
            sets.push(format!("base_importance = ?{}", params.len() + 1));
            params.push(Box::new(importance as i64));
        }
        if let Some(tags) = &updates.tags {
            sets.push(format!("tags = ?{}", params.len() + 1));
            params.push(Box::new(json_array(tags)?));
        }
        if let Some(context) = &updates.context {
            sets.push(format!("context = ?{}", params.len() + 1));
            params.push(Box::new(context.clone()));
        }
        if let Some(confidence) = updates.confidence {
            sets.push(format!("confidence = ?{}", params.len() + 1));
            params.push(Box::new(confidence as f64));
        }

        // If a field that feeds `embedding_input` (content / tags / context — the
        // composite document text) changed, stale this row's embedding stamp so the
        // next `reembed` recomputes its vector. Without this, a tags/context edit
        // would leave the stored vector permanently stale: the version-only reembed
        // scan only revisits rows whose `(model, input_version)` differs from
        // current, and an in-place field edit keeps the current stamp. Empty string
        // is the documented "stale, re-embed me" sentinel. (Content edits are
        // rejected upstream in the engine, but stamping it here keeps the storage
        // invariant self-contained.)
        if updates.content.is_some() || updates.tags.is_some() || updates.context.is_some() {
            sets.push(format!("embedding_input_version = ?{}", params.len() + 1));
            params.push(Box::new(String::new()));
        }

        // An all-None update is a true no-op: do not bump updated_at or issue an
        // UPDATE when nothing changed.
        if sets.is_empty() {
            return Ok(());
        }

        // Bump updated_at only when at least one field is actually changing.
        sets.push(format!("updated_at = ?{}", params.len() + 1));
        params.push(Box::new(chrono::Utc::now().timestamp()));

        // WHERE memory_id bind comes last.
        let id_pos = params.len() + 1;
        params.push(Box::new(id.to_string()));

        let sql = format!(
            "UPDATE memories SET {} WHERE memory_id = ?{}",
            sets.join(", "),
            id_pos
        );

        // Transaction: the UPDATE and its oplog row commit (or roll back) together.
        immediate_tx(&self.conn, || {
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let affected = self
                .conn
                .execute(&sql, refs.as_slice())
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Oplog only on a real change: 0 rows means the id does not exist
            // (a missing-id update stays an Ok no-op and logs nothing).
            if affected > 0 {
                append_oplog(&self.conn, &self.site_id, "update", id, "")?;
            }
            Ok(())
        })
    }

    fn archive_memory(&self, id: &MemoryId) -> Result<()> {
        // Transaction: the archive and its oplog row commit (or roll back) together.
        immediate_tx(&self.conn, || {
            let affected = self
                .conn
                .execute(
                    "UPDATE memories
                     SET archived_at = ?1, updated_at = ?1
                     WHERE memory_id = ?2 AND archived_at IS NULL",
                    rusqlite::params![chrono::Utc::now().timestamp(), id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Missing or already-archived ids are Ok no-ops and log nothing.
            if affected > 0 {
                // Vector hygiene (W1.7): an archived memory must not occupy
                // KNN candidate slots. Same transaction as the archive so the
                // row and its vector commit — or roll back — together. A row
                // stored without an embedding simply deletes nothing.
                self.conn
                    .execute(
                        "DELETE FROM memory_vectors WHERE memory_id = ?1",
                        rusqlite::params![id.to_string()],
                    )
                    .map_err(|e| Error::Storage(e.to_string()))?;
                append_oplog(&self.conn, &self.site_id, "archive", id, "")?;
            }
            Ok(())
        })
    }

    fn add_link(&self, link: &MemoryLink) -> Result<()> {
        // Transaction: the link INSERT and its oplog row commit (or roll back)
        // together (an FK failure on a missing endpoint rolls back both).
        immediate_tx(&self.conn, || {
            self.conn
                .execute(
                    "INSERT INTO memory_links
                        (source_id, target_id, link_type, strength, base_strength, reason, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        link.source_id.to_string(),
                        link.target_id.to_string(),
                        link.link_type.as_str(),
                        link.strength as f64,
                        // Baseline equals the created strength; decay never mutates
                        // it, so the pass stays idempotent.
                        link.strength as f64,
                        link.reason,
                        link.created_at.timestamp(),
                    ],
                )
                // A PK/UNIQUE violation means the (source,target,type) edge
                // already exists. `MemoryEngine::link` is check-then-act, so a
                // caller that loses the race to a concurrent insert reaches
                // here; map it to the SAME deterministic validation-class error
                // the pre-check produces ("already exists") instead of a
                // generic Storage/"internal error". FK violations on a missing
                // endpoint stay Storage (a different ConstraintViolation code).
                .map_err(|e| map_link_constraint_err(e, link))?;
            let details = serde_json::json!({
                "type": link.link_type.as_str(),
                "target": link.target_id.to_string(),
            })
            .to_string();
            append_oplog(&self.conn, &self.site_id, "link", &link.source_id, &details)
        })
    }

    fn record_access(&self, id: &MemoryId) -> Result<()> {
        self.conn
            .execute(
                "UPDATE memories
                 SET access_count = access_count + 1, last_accessed_at = ?1
                 WHERE memory_id = ?2",
                rusqlite::params![chrono::Utc::now().timestamp(), id.to_string()],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    fn record_accesses(&self, ids: &[MemoryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        // Deduplicate to build the IN-list (a row can only be bumped once per SQL
        // UPDATE regardless, but dedup keeps the placeholder list small).
        let mut seen = std::collections::HashSet::new();
        let unique: Vec<&MemoryId> = ids
            .iter()
            .filter(|id| seen.insert(id.to_string()))
            .collect();

        // Build "?2, ?3, ..." placeholders; ?1 is the timestamp.
        let placeholders: Vec<String> = (0..unique.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "UPDATE memories
             SET access_count = access_count + 1, last_accessed_at = ?1
             WHERE memory_id IN ({})",
            placeholders.join(", ")
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(unique.len() + 1);
        params.push(Box::new(chrono::Utc::now().timestamp()));
        for id in &unique {
            params.push(Box::new(id.to_string()));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        self.conn
            .execute(&sql, refs.as_slice())
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    fn record_access_bumps(&self, bumps: &[AccessBump]) -> Result<()> {
        if bumps.is_empty() {
            return Ok(());
        }
        immediate_tx(&self.conn, || {
            let mut stmt = self
                .conn
                .prepare(
                    // Assigns NO FTS-indexed column, so under migration 006 the
                    // mem_au trigger does not fire: a flush is zero FTS writes.
                    "UPDATE memories
                     SET access_count = access_count + ?1,
                         last_accessed_at = CASE
                           WHEN last_accessed_at IS NULL OR last_accessed_at < ?2 THEN ?2
                           ELSE last_accessed_at
                         END
                     WHERE memory_id = ?3",
                )
                .map_err(storage_err)?;
            for bump in bumps {
                // 0 rows affected = missing id: silently skipped (best-effort).
                stmt.execute(rusqlite::params![
                    bump.count,
                    bump.last_accessed_at,
                    bump.id.to_string()
                ])
                .map_err(storage_err)?;
            }
            Ok(())
        })
    }

    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        immediate_tx(&self.conn, || {
            // Point old -> new. FK on superseded_by makes a missing `new` fail here,
            // rolling back the whole transaction (old stays unarchived).
            let affected = self
                .conn
                .execute(
                    "UPDATE memories SET superseded_by = ?1, updated_at = ?2 WHERE memory_id = ?3",
                    rusqlite::params![new.to_string(), now, old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Fail fast on a missing `old`: 0 rows updated means a caller bug, not a
            // silent success. Returning the error rolls the whole transaction back.
            if affected == 0 {
                return Err(Error::NotFound(old.clone()));
            }
            // Archive old (idempotent: only if currently active).
            self.conn
                .execute(
                    "UPDATE memories SET archived_at = ?1, updated_at = ?1
                     WHERE memory_id = ?2 AND archived_at IS NULL",
                    rusqlite::params![now, old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Vector hygiene (W1.7): the superseded (now archived) memory's
            // vector leaves the KNN index in the same transaction. Idempotent:
            // a vectorless or already-cleaned row deletes nothing.
            self.conn
                .execute(
                    "DELETE FROM memory_vectors WHERE memory_id = ?1",
                    rusqlite::params![old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // One `supersede` oplog row covers the whole compound mutation
            // (pointer + archive); the replacement id rides in `details`.
            let details = serde_json::json!({ "new": new.to_string() }).to_string();
            append_oplog(&self.conn, &self.site_id, "supersede", old, &details)
        })
    }

    fn get_many(&self, ns: &Namespace, ids: &[MemoryId]) -> Result<Vec<MemoryNote>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build "?2, ?3, ..." placeholders; ?1 is reserved for the namespace.
        let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                    keywords, tags, context, memory_type, importance, confidence,
                    related_files, access_count, last_accessed_at, archived_at,
                    superseded_by, embedding_model, embedding_input_version,
                    origin_user, origin_host, origin_agent, origin_source, session_id
             FROM memories
             WHERE namespace = ?1 AND memory_id IN ({})",
            placeholders.join(", ")
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 1);
        params.push(Box::new(ns.as_db_string()));
        for id in ids {
            params.push(Box::new(id.to_string()));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(refs.as_slice())
            .map_err(|e| Error::Storage(e.to_string()))?;

        // Decode into an id-keyed map, then re-emit in request order.
        let mut by_id: std::collections::HashMap<MemoryId, MemoryNote> =
            std::collections::HashMap::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            let note = row_to_note(&self.conn, row)?;
            by_id.insert(note.id.clone(), note);
        }

        // Use `get` (not `remove`) so duplicate ids are preserved positionally,
        // honouring the "same order as `ids`" contract.
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(note) = by_id.get(id) {
                out.push(note.clone());
            }
        }
        Ok(out)
    }

    fn active_contradicts(
        &self,
        ns: &Namespace,
        ids: &[MemoryId],
    ) -> Result<std::collections::HashSet<MemoryId>> {
        use std::collections::HashSet;
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        // One batched query: a contradicts link where BOTH the local endpoint
        // (the flagged id) and the far endpoint live in `ns`, and the far endpoint
        // is active. Scoping BOTH endpoints to `ns` keeps the result namespace-pure
        // regardless of caller: `memory_links` carries no namespace and `add_link`
        // permits cross-namespace edges, so a contradiction with a memory in another
        // namespace must neither flag an in-namespace id (`far` scope) nor leak an
        // out-of-namespace id into the result (`loc` scope). Only ACTIVE,
        // in-namespace contradictions count (spec §9). The SELECT returns the local
        // endpoint id for every matching row; we collect the distinct set.
        //
        // Both UNION halves share the same ?1..?N (ids) and ?{N+1} (namespace)
        // positional slots — SQLite parameters are shared across UNION, so binding
        // N+1 values covers both halves. Do NOT double the params.
        let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 1)).collect();
        let in_list = placeholders.join(", ");
        let ns_param = format!("?{}", ids.len() + 1);
        let sql = format!(
            "SELECT l.source_id AS local FROM memory_links l
               JOIN memories far ON far.memory_id = l.target_id
               JOIN memories loc ON loc.memory_id = l.source_id
             WHERE l.link_type = 'contradicts'
               AND far.archived_at IS NULL
               AND loc.archived_at IS NULL
               AND far.namespace = {ns_param}
               AND loc.namespace = {ns_param}
               AND l.source_id IN ({in_list})
             UNION
             SELECT l.target_id AS local FROM memory_links l
               JOIN memories far ON far.memory_id = l.source_id
               JOIN memories loc ON loc.memory_id = l.target_id
             WHERE l.link_type = 'contradicts'
               AND far.archived_at IS NULL
               AND loc.archived_at IS NULL
               AND far.namespace = {ns_param}
               AND loc.namespace = {ns_param}
               AND l.target_id IN ({in_list})"
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 1);
        for id in ids {
            params.push(Box::new(id.to_string()));
        }
        params.push(Box::new(ns.as_db_string()));
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(refs.as_slice())
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut contested: HashSet<MemoryId> = HashSet::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            let s = row
                .get::<_, String>(0)
                .map_err(|e| Error::Storage(e.to_string()))?;
            contested.insert(parse_id(&s)?);
        }
        Ok(contested)
    }

    fn memories_for_reembed(
        &self,
        model: &str,
        input_version: &str,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        // Bounded, deterministic scan (oldest first then by id, mirroring the
        // consolidation candidate order) of active rows whose stamp is stale.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model, embedding_input_version,
                        origin_user, origin_host, origin_agent, origin_source, session_id
                 FROM memories
                 WHERE archived_at IS NULL
                   AND (embedding_model <> ?1 OR embedding_input_version <> ?2)
                 ORDER BY created_at ASC, memory_id ASC
                 LIMIT ?3",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(rusqlite::params![
                model,
                input_version,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ])
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            out.push(row_to_note(&self.conn, row)?);
        }
        Ok(out)
    }

    fn update_vector(
        &self,
        id: &MemoryId,
        embedding: &[f32],
        model: &str,
        input_version: &str,
    ) -> Result<()> {
        // Fail-closed dimension check before any write, identical to the search
        // path: a wrong-length vector must never land in the vec0 table.
        if embedding.len() != self.embedding_dim {
            return Err(Error::DimensionMismatch {
                expected: self.embedding_dim,
                got: embedding.len(),
            });
        }

        immediate_tx(&self.conn, || {
            // Stamp the memory row. 0 rows updated means the id does not exist
            // OR the row is archived: fail closed (NotFound) so the whole
            // transaction rolls back rather than leaving a vector update with
            // no owning LIVE row. The `archived_at IS NULL` guard closes the
            // reembed-vs-archive race: the reembed job reads its candidate set
            // (active rows only) on the read pool, then spends seconds on
            // embedding-API calls before this write reaches the single writer —
            // an Archive/Supersede processed in that window deletes the vec0
            // row, and without the guard the fallback INSERT below would
            // resurrect a vector for the archived row, permanently violating
            // the live-only-partition invariant `vector_search` documents
            // (nothing prunes it again: archive only deletes the vector on the
            // active->archived transition, and the W1.1/W1.7 rebuild is
            // one-shot). The caller (engine reembed loop) counts the error as a
            // per-row skip, and the next candidate scan excludes archived rows,
            // so there is no retry loop.
            // `updated_at` is intentionally NOT bumped: re-embed is a maintenance-only
            // path that refreshes the search vector, leaving the note's user-visible
            // content unchanged. The daemon writer emits no MemoryChanged event for the
            // same reason; bumping updated_at here would make every re-embedded note
            // look freshly modified to list/context and any updated_at-based sync.
            let affected = self
                .conn
                .execute(
                    "UPDATE memories
                     SET embedding_model = ?1, embedding_input_version = ?2
                     WHERE memory_id = ?3 AND archived_at IS NULL",
                    rusqlite::params![model, input_version, id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if affected == 0 {
                return Err(Error::NotFound(id.clone()));
            }

            // Replace the stored vector. vec0 supports UPDATE on the PK; a row
            // that had no vector yet (written without an embedding) is handled by
            // falling back to INSERT when the UPDATE touches nothing.
            let bytes = embedding_bytes(embedding);
            let updated = self
                .conn
                .execute(
                    "UPDATE memory_vectors SET embedding = ?1 WHERE memory_id = ?2",
                    rusqlite::params![bytes, id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if updated == 0 {
                // Resolve the namespace partition key from the owning memories
                // row (proven present AND live by `affected > 0` above, in this
                // same transaction). The `archived_at IS NULL` filter is
                // defense-in-depth on top of the stamp guard: this INSERT must
                // never re-add a vec0 row for an archived memory.
                self.conn
                    .execute(
                        "INSERT INTO memory_vectors (memory_id, namespace, embedding)
                         SELECT memory_id, namespace, ?2 FROM memories
                         WHERE memory_id = ?1 AND archived_at IS NULL",
                        rusqlite::params![id.to_string(), bytes],
                    )
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }
            Ok(())
        })
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
            msg.contains("stored: deterministic") && msg.contains("configured: voyage-3"),
            "refusal names both models: {msg}"
        );
        assert!(
            msg.contains("--accept-model-change"),
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
    fn accept_model_change_swaps_model_and_stales_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("model-swap".to_string());

        {
            let store = SqliteStore::open_with_model(&path, 8, "deterministic").unwrap();
            let mut note =
                MemoryNote::new(ns.clone(), "stale me".to_string(), MemoryType::Insight, 5);
            note.embedding_model = "deterministic".to_string();
            note.embedding_input_version = "v2-composite".to_string();
            store.insert_memory(&note, Some(&[0.1f32; 8])).unwrap();
        }

        // The model-bound open refuses; the opt-in path uses the unbound open.
        let store = SqliteStore::open(&path, 8).unwrap();
        let changed = store.accept_model_change("voyage-3").unwrap();
        assert!(changed, "a real swap reports true");

        let model: String = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(model, "voyage-3", "meta points at the new model");

        let stale: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM memories WHERE embedding_input_version = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let total: i64 = store
            .conn
            .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stale, total, "every row carries the reembed sentinel");
        drop(store);

        // The accepted model now opens cleanly.
        let _reopened = SqliteStore::open_with_model(&path, 8, "voyage-3").unwrap();
    }

    #[test]
    fn accept_model_change_with_current_model_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("model-noop".to_string());

        let store = SqliteStore::open_with_model(&path, 8, "deterministic").unwrap();
        let mut note = MemoryNote::new(ns, "keep stamp".to_string(), MemoryType::Insight, 5);
        note.embedding_model = "deterministic".to_string();
        note.embedding_input_version = "v2-composite".to_string();
        store.insert_memory(&note, Some(&[0.1f32; 8])).unwrap();

        // Same model (e.g. a lingering RB_ACCEPT_MODEL_CHANGE on every restart):
        // no swap, and crucially no corpus-wide re-stale.
        let changed = store.accept_model_change("deterministic").unwrap();
        assert!(!changed, "accepting the current model is a no-op");
        let version: String = store
            .conn
            .query_row(
                "SELECT embedding_input_version FROM memories LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, "v2-composite", "stamps survive a no-op accept");
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

    #[test]
    fn links_for_decay_returns_link_rows_bounded_by_limit() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("decay".to_string());

        // Two real memories to satisfy the FK on memory_links.
        let a = MemoryNote::new(ns.clone(), "source".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "target".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();

        let created = chrono::Utc::now();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: created,
            })
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.source, a.id);
        assert_eq!(row.target, b.id);
        assert_eq!(row.link_type, rb_types::LinkType::References);
        assert!((row.strength - 0.8).abs() < f32::EPSILON);
        // base_strength is captured at creation = the created strength.
        assert!((row.base_strength - 0.8).abs() < f32::EPSILON);
        assert_eq!(row.created_at.timestamp(), created.timestamp());

        // The limit is honoured.
        let none = store.links_for_decay(0).unwrap();
        assert!(none.is_empty(), "limit 0 returns no rows");
    }

    #[test]
    fn set_link_strength_leaves_base_strength_immutable() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("decay-base".to_string());
        let a = MemoryNote::new(ns.clone(), "source".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "target".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        // Decay lowers the running strength but must NOT touch the baseline, so
        // a subsequent recompute from the baseline is reproducible (idempotent).
        store
            .set_link_strength(&a.id, &b.id, rb_types::LinkType::References, 0.2)
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            (rows[0].strength - 0.2).abs() < f32::EPSILON,
            "running value updated"
        );
        assert!(
            (rows[0].base_strength - 0.8).abs() < f32::EPSILON,
            "baseline unchanged by set_link_strength"
        );
    }

    #[test]
    fn set_link_strength_updates_only_the_matching_edge() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("setstr".to_string());
        let a = MemoryNote::new(ns.clone(), "s".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "t".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        store
            .set_link_strength(&a.id, &b.id, rb_types::LinkType::References, 0.25)
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].strength - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn delete_link_removes_only_the_matching_edge() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("dellink".to_string());
        let a = MemoryNote::new(ns.clone(), "s".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "t".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        // A second, differently-typed edge between the same nodes must survive.
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::Extends,
                strength: 0.4,
                reason: "r2".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        store
            .delete_link(&a.id, &b.id, rb_types::LinkType::References)
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1, "only the References edge was deleted");
        assert_eq!(rows[0].link_type, rb_types::LinkType::Extends);
    }
}

#[cfg(test)]
mod insert_tests {
    use super::*;
    use rb_types::{LinkType, MemoryLink, MemoryNote, MemoryType, Namespace};

    fn vec8(seed: f32) -> Vec<f32> {
        (0..8).map(|i| seed + i as f32 * 0.1).collect()
    }

    #[test]
    fn insert_persists_memory_vector_and_links() {
        let store = SqliteStore::open_in_memory(8).unwrap();

        let mut a = MemoryNote::new(
            Namespace::Project("rb".into()),
            "alpha content".into(),
            MemoryType::CodePattern,
            5,
        );
        let mut b = MemoryNote::new(
            Namespace::Project("rb".into()),
            "beta content".into(),
            MemoryType::Insight,
            7,
        );
        b.tags = vec!["x".into(), "y".into()];

        // Insert target first so the link FK is satisfiable.
        store.insert_memory(&b, Some(&vec8(0.5))).unwrap();

        a.keywords = vec!["k1".into()];
        a.related_files = vec!["src/lib.rs".into()];
        a.links = vec![MemoryLink {
            source_id: a.id.clone(),
            target_id: b.id.clone(),
            link_type: LinkType::References,
            strength: 0.8,
            reason: "see beta".into(),
            created_at: a.created_at,
        }];
        store.insert_memory(&a, Some(&vec8(1.0))).unwrap();

        // memories row count
        let n: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);

        // FTS populated via trigger (external-content; requires INSERT trigger from migration 001)
        let fts: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memories_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, 2);

        // vector row stored
        let vn: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vn, 2);

        // link stored
        let ln: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ln, 1);
    }

    #[test]
    fn insert_without_embedding_skips_vector() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let m = MemoryNote::new(Namespace::Global, "no vec".into(), MemoryType::Reference, 3);
        store.insert_memory(&m, None).unwrap();
        let vn: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vn, 0);
    }

    #[test]
    fn insert_rejects_out_of_range_importance() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // importance = 0 is below the valid range 1..=10
        let mut m = MemoryNote::new(Namespace::Global, "bad".into(), MemoryType::Reference, 5);
        m.importance = 0;
        let err = store.insert_memory(&m, None).unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(ref s) if s.contains("importance")),
            "expected invalid argument error about importance, got {err:?}"
        );

        // importance = 11 is above the valid range
        m.importance = 11;
        let err = store.insert_memory(&m, None).unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(ref s) if s.contains("importance")),
            "expected invalid argument error about importance, got {err:?}"
        );
    }

    #[test]
    fn insert_rejects_out_of_range_confidence_as_invalid_argument() {
        // W2.8 taxonomy: a range rejection is caller guidance, so it is
        // validation-class (InvalidArgument travels verbatim over the wire;
        // Storage would surface as an opaque "internal error").
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(Namespace::Global, "bad".into(), MemoryType::Reference, 5);
        for bad in [-0.1f32, 1.1, f32::NAN] {
            m.confidence = bad;
            let err = store.insert_memory(&m, None).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument(ref s) if s.contains("confidence")),
                "expected invalid argument error about confidence for {bad}, got {err:?}"
            );
        }
    }
}

#[cfg(test)]
mod get_tests {
    use super::*;
    use rb_types::{LinkType, MemoryId, MemoryLink, MemoryNote, MemoryType, Namespace};

    #[test]
    fn get_round_trips_all_fields_and_links() {
        let store = SqliteStore::open_in_memory(8).unwrap();

        let target = MemoryNote::new(
            Namespace::Session {
                project: "rb".into(),
                session_id: "s1".into(),
            },
            "target".into(),
            MemoryType::Entity,
            4,
        );
        store.insert_memory(&target, None).unwrap();

        let mut m = MemoryNote::new(
            Namespace::Project("rb".into()),
            "full content".into(),
            MemoryType::BugFix,
            9,
        );
        m.summary = "a summary".into();
        m.keywords = vec!["alpha".into(), "beta".into()];
        m.tags = vec!["t1".into()];
        m.context = "while fixing X".into();
        m.confidence = 0.75;
        m.related_files = vec!["a.rs".into(), "b.rs".into()];
        m.embedding_model = "voyage-3".into();
        m.links = vec![MemoryLink {
            source_id: m.id.clone(),
            target_id: target.id.clone(),
            link_type: LinkType::Implements,
            strength: 0.6,
            reason: "impl".into(),
            created_at: m.created_at,
        }];
        store.insert_memory(&m, None).unwrap();

        let got = store.get_memory(&m.id).unwrap().expect("memory present");
        assert_eq!(got.id, m.id);
        assert_eq!(got.namespace, Namespace::Project("rb".into()));
        assert_eq!(got.content, "full content");
        assert_eq!(got.summary, "a summary");
        assert_eq!(got.keywords, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(got.tags, vec!["t1".to_string()]);
        assert_eq!(got.context, "while fixing X");
        assert_eq!(got.memory_type, MemoryType::BugFix);
        assert_eq!(got.importance, 9);
        assert!((got.confidence - 0.75).abs() < 1e-6);
        assert_eq!(
            got.related_files,
            vec!["a.rs".to_string(), "b.rs".to_string()]
        );
        assert_eq!(got.embedding_model, "voyage-3");
        assert_eq!(got.created_at.timestamp(), m.created_at.timestamp());
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.links[0].target_id, target.id);
        assert_eq!(got.links[0].link_type, LinkType::Implements);
        assert!((got.links[0].strength - 0.6).abs() < 1e-6);
    }

    #[test]
    fn get_missing_returns_none() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        assert!(store.get_memory(&MemoryId::new()).unwrap().is_none());
    }

    #[test]
    fn round_trips_access_count_last_accessed_and_superseded_by() {
        // These three columns never carry non-default values in the other tests,
        // so their decode paths were untested. Exercise them explicitly.
        let store = SqliteStore::open_in_memory(8).unwrap();

        // superseded_by is a FK to memories(memory_id); insert the successor first.
        let successor = MemoryNote::new(
            Namespace::Project("rb".into()),
            "successor".into(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&successor, None).unwrap();

        let accessed = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
            .expect("valid timestamp");

        let mut m = MemoryNote::new(
            Namespace::Project("rb".into()),
            "superseded body".into(),
            MemoryType::CodePattern,
            6,
        );
        m.access_count = 3;
        m.last_accessed_at = Some(accessed);
        m.superseded_by = Some(successor.id.clone());
        store.insert_memory(&m, None).unwrap();

        let got = store.get_memory(&m.id).unwrap().expect("memory present");
        assert_eq!(got.access_count, 3);
        assert_eq!(
            got.last_accessed_at.map(|t| t.timestamp()),
            Some(accessed.timestamp())
        );
        assert_eq!(got.superseded_by, Some(successor.id));
    }
}

#[cfg(test)]
mod keyword_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert(store: &SqliteStore, ns: Namespace, content: &str) -> rb_types::MemoryId {
        let m = MemoryNote::new(ns, content.into(), MemoryType::Reference, 5);
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
        id
    }

    #[test]
    fn porter_stemming_unifies_inflections() {
        // Migration 005 layers the porter stemmer over unicode61: a query in
        // one inflection must match a document in another ("retries" ->
        // "retri" <- "retry"). Under bare unicode61 this returned nothing.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "we retry failed jobs");
        let found = store.keyword_search(&proj, "retries", 10).unwrap();
        assert_eq!(found, vec![hit]);
    }

    #[test]
    fn finds_matching_and_scopes_to_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "rust async runtime tokio");
        let _miss_ns = insert(&store, Namespace::Global, "rust async runtime tokio");
        let _miss_term = insert(&store, proj.clone(), "completely different topic");

        let found = store.keyword_search(&proj, "tokio", 10).unwrap();
        assert_eq!(found, vec![hit]);
    }

    #[test]
    fn or_of_tokens_matches_natural_language_queries() {
        // W1.2: the keyword leg must be alive for multi-word natural-language
        // queries. The old whole-query-phrase form required every token to be
        // adjacent and in order, so a question never matched; OR-of-tokens
        // matches on any content term and lets bm25 rank by rarity.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "all writes go through tokio channels");
        let _other = insert(&store, proj.clone(), "completely unrelated topic");

        let found = store
            .keyword_search(&proj, "how do we use tokio for writes?", 10)
            .unwrap();
        assert_eq!(
            found,
            vec![hit],
            "a question must match on its content tokens"
        );
    }

    #[test]
    fn escapes_special_query_chars() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "config flag enable-cache value");

        // The '-' would be an FTS5 operator (NOT) if unescaped. Tokenized and
        // quoted, the query becomes `"enable" OR "cache"` and matches the
        // document's tokens (unicode61 splits the document on '-' too).
        let found = store.keyword_search(&proj, "enable-cache", 10).unwrap();
        assert_eq!(found, vec![hit.clone()]);

        // A query that is nothing but FTS5 operator words/punctuation must NOT
        // raise a syntax error. Quoted per-token it is `"OR" OR "AND"` — the
        // literal words, which this document does not contain.
        let none = store.keyword_search(&proj, "OR AND (", 10).unwrap();
        assert!(none.is_empty());

        // A double-quote is the FTS5 phrase delimiter. It is a token separator
        // here, never syntax: the input becomes `"value" OR "OR" OR "config"`,
        // which matches this document on its literal `value`/`config` tokens —
        // exactly what the same input WITHOUT the embedded quote would match.
        // Equivalence is the no-injection property: the quote changed nothing.
        let with_quote = store
            .keyword_search(&proj, "value\" OR config", 10)
            .unwrap();
        let without_quote = store.keyword_search(&proj, "value OR config", 10).unwrap();
        assert_eq!(with_quote, vec![hit.clone()]);
        assert_eq!(
            with_quote, without_quote,
            "an embedded double-quote must not alter query semantics"
        );

        // A bare double-quote alone must also be safe (no panic, no syntax
        // error): no indexable tokens -> the empty phrase, which matches nothing.
        let lone_quote = store.keyword_search(&proj, "\"", 10).unwrap();
        assert!(lone_quote.is_empty());

        // An asterisk is the FTS5 prefix operator. As a separator it vanishes:
        // `enable*` becomes the literal token query `"enable"`, which matches
        // the document (NOT via prefix expansion — see the decision record on
        // escape_fts5_query: trailing prefix match measured as a regression).
        let star = store.keyword_search(&proj, "enable*", 10).unwrap();
        assert_eq!(star, vec![hit.clone()]);

        // A lone asterisk must also be safe (no panic, no syntax error).
        let lone_star = store.keyword_search(&proj, "*", 10).unwrap();
        assert!(lone_star.is_empty());

        // An empty query must be safe and match nothing.
        let empty = store.keyword_search(&proj, "", 10).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn operator_words_are_literal_not_syntax() {
        // NOT/NEAR injection: if `NOT` were parsed as the FTS5 operator,
        // "config NOT value" would EXCLUDE this document (it contains `value`).
        // Tokenized and quoted it is `"config" OR "NOT" OR "value"`, which
        // matches it. Same for a NEAR() construction attempt.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "config flag enable-cache value");

        let not_attempt = store.keyword_search(&proj, "config NOT value", 10).unwrap();
        assert_eq!(
            not_attempt,
            vec![hit.clone()],
            "NOT must be a literal token, not an exclusion operator"
        );

        let near_attempt = store
            .keyword_search(&proj, "NEAR(config value, 2)", 10)
            .unwrap();
        assert_eq!(
            near_attempt,
            vec![hit.clone()],
            "NEAR(...) must be literal tokens, not proximity syntax"
        );

        // Column-filter injection: `summary:config` must not become a
        // column-scoped query; `summary` is just another OR'd token.
        let col_attempt = store.keyword_search(&proj, "summary:config", 10).unwrap();
        assert_eq!(col_attempt, vec![hit]);
    }

    #[test]
    fn excludes_archived() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let id = insert(&store, proj.clone(), "archivable widget");
        store.archive_memory(&id).unwrap();
        let found = store.keyword_search(&proj, "widget", 10).unwrap();
        assert!(found.is_empty());
    }
}

#[cfg(test)]
mod vector_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_vec(
        store: &SqliteStore,
        ns: Namespace,
        content: &str,
        v: [f32; 8],
    ) -> rb_types::MemoryId {
        let m = MemoryNote::new(ns, content.into(), MemoryType::Insight, 5);
        let id = m.id.clone();
        store.insert_memory(&m, Some(&v)).unwrap();
        id
    }

    #[test]
    fn returns_nearest_first_scoped_to_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());

        let near = insert_vec(
            &store,
            proj.clone(),
            "near",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let far = insert_vec(
            &store,
            proj.clone(),
            "far",
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Different namespace, identical to query: must be excluded by scope.
        let other = insert_vec(
            &store,
            Namespace::Global,
            "other",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let query = [0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let res = store.vector_search(&proj, &query, 10).unwrap();

        let ids: Vec<_> = res.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids, vec![near.clone(), far.clone()]);
        // distances are ascending
        assert!(res[0].1 <= res[1].1);
        assert!(!ids.contains(&other));
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let err = store.vector_search(&proj, &[0.0, 0.0, 0.0], 5).unwrap_err();
        assert!(matches!(
            err,
            Error::DimensionMismatch {
                expected: 8,
                got: 3
            }
        ));
    }
}

#[cfg(test)]
mod namespace_rename_tests {
    #![allow(clippy::panic)]
    use super::*;
    use rb_types::{LinkType, MemoryLink, MemoryNote, MemoryType, Namespace};

    fn insert_vec(store: &SqliteStore, ns: &Namespace, content: &str, v: [f32; 8]) -> MemoryNote {
        let m = MemoryNote::new(ns.clone(), content.into(), MemoryType::Insight, 5);
        store.insert_memory(&m, Some(&v)).unwrap();
        m
    }

    fn count_in(store: &SqliteStore, ns: &Namespace) -> i64 {
        store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE namespace = ?1",
                rusqlite::params![ns.as_db_string()],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn rename_oplog_rows(store: &SqliteStore) -> Vec<(String, String)> {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT namespace, details FROM memory_oplog
                 WHERE op = 'namespace_rename' ORDER BY seq",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn rename_moves_memories_vectors_links_and_logs_one_oplog_row() {
        // The spec fixture: vectors + FTS + links across two namespaces, plus
        // an archived (vectorless) row, then rename ONE namespace.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = Namespace::Project("scratch-dir".into());
        let new = Namespace::Project("rusty-brain".into());
        let other = Namespace::Project("untouched".into());

        let a = insert_vec(
            &store,
            &old,
            "single writer owns the sqlite connection",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let b = insert_vec(
            &store,
            &old,
            "vec0 partitions knn by namespace",
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Archived row: vector pruned by archive (W1.7), but the memories row
        // must still move to the new namespace.
        let archived = insert_vec(
            &store,
            &old,
            "archived rows move too",
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        store.archive_memory(&archived.id).unwrap();
        // A graph edge inside the renamed namespace.
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: LinkType::References,
                strength: 1.0,
                reason: String::new(),
                created_at: a.created_at,
            })
            .unwrap();
        // A bystander namespace that must not move.
        let bystander = insert_vec(
            &store,
            &other,
            "single writer in another namespace",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let outcome = store.rename_namespace(&old, &new, false).unwrap();
        assert_eq!(outcome.memories, 3, "active + archived rows all move");
        assert_eq!(outcome.vectors, 2, "only live rows still have vectors");
        assert_eq!(outcome.merged_into, 0, "target was empty");

        // Old namespace is empty; the new one holds the corpus.
        assert_eq!(count_in(&store, &old), 0);
        assert_eq!(count_in(&store, &new), 3);
        assert!(store.list(&old, None, 10).unwrap().is_empty());
        let listed: Vec<_> = store
            .list(&new, None, 10)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert!(listed.contains(&a.id) && listed.contains(&b.id));

        // FTS leg follows the rename: keyword_search scopes via the memories
        // JOIN, so the same query flips namespaces with the rows.
        let kw_new = store.keyword_search(&new, "writer", 10).unwrap();
        assert!(kw_new.contains(&a.id), "FTS finds the row in the NEW ns");
        assert!(
            store.keyword_search(&old, "writer", 10).unwrap().is_empty(),
            "FTS finds nothing under the OLD ns"
        );

        // vec0 partition keys were re-inserted under the new namespace: KNN
        // under NEW returns both live vectors nearest-first; KNN under OLD is
        // empty (the partition no longer holds rows).
        let query = [0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let knn_new = store.vector_search(&new, &query, 10).unwrap();
        let knn_ids: Vec<_> = knn_new.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(knn_ids, vec![a.id.clone(), b.id.clone()]);
        assert!(store.vector_search(&old, &query, 10).unwrap().is_empty());

        // Graph edges key on memory ids: the link survives untouched.
        assert_eq!(
            store.graph_neighbors(&a.id, 1).unwrap(),
            vec![(b.id.clone(), 1)]
        );

        // The bystander namespace is untouched, in rows and in KNN.
        assert_eq!(count_in(&store, &other), 1);
        let knn_other = store.vector_search(&other, &query, 10).unwrap();
        assert_eq!(knn_other[0].0, bystander.id);

        // Exactly one namespace_rename oplog row, under the NEW namespace,
        // with old/new/counts in details.
        let rows = rename_oplog_rows(&store);
        assert_eq!(rows.len(), 1);
        let (ns, details) = &rows[0];
        assert_eq!(ns, &new.as_db_string());
        let v: serde_json::Value = serde_json::from_str(details).unwrap();
        assert_eq!(v["old"], old.as_db_string());
        assert_eq!(v["new"], new.as_db_string());
        assert_eq!(v["moved"], 3);
        assert_eq!(v["vectors"], 2);
        assert_eq!(v["merged_into"], 0);
    }

    #[test]
    fn rename_refuses_non_empty_target_without_merge_and_changes_nothing() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = Namespace::Project("a".into());
        let b = Namespace::Project("b".into());
        let in_a = insert_vec(&store, &a, "row in a", [1.0; 8]);
        insert_vec(&store, &b, "row in b", [0.5; 8]);

        let err = store.rename_namespace(&a, &b, false).unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "collision must be validation-class, got {err:?}"
        );
        assert!(
            err.to_string().contains("--merge"),
            "refusal carries the remediation hint: {err}"
        );

        // Rolled back: nothing moved, no oplog row.
        assert_eq!(count_in(&store, &a), 1);
        assert_eq!(count_in(&store, &b), 1);
        assert!(store
            .keyword_search(&a, "row", 10)
            .unwrap()
            .contains(&in_a.id));
        assert!(rename_oplog_rows(&store).is_empty());
    }

    #[test]
    fn rename_with_merge_appends_and_reports_pre_existing_count() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = Namespace::Project("a".into());
        let b = Namespace::Project("b".into());
        let m1 = insert_vec(
            &store,
            &a,
            "merge one",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let m2 = insert_vec(
            &store,
            &a,
            "merge two",
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let pre = insert_vec(
            &store,
            &b,
            "already here",
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let outcome = store.rename_namespace(&a, &b, true).unwrap();
        assert_eq!(outcome.memories, 2);
        assert_eq!(outcome.vectors, 2);
        assert_eq!(outcome.merged_into, 1, "pre-existing target rows counted");

        assert_eq!(count_in(&store, &a), 0);
        assert_eq!(count_in(&store, &b), 3);

        // The merged partition serves KNN over old AND pre-existing vectors.
        let knn: std::collections::HashSet<_> = store
            .vector_search(&b, &[0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0], 10)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(knn.contains(&m1.id) && knn.contains(&m2.id) && knn.contains(&pre.id));

        let rows = rename_oplog_rows(&store);
        assert_eq!(rows.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&rows[0].1).unwrap();
        assert_eq!(v["merged_into"], 1);
    }

    #[test]
    fn rename_of_empty_source_is_a_validation_error() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let err = store
            .rename_namespace(
                &Namespace::Project("nothing-here".into()),
                &Namespace::Project("target".into()),
                false,
            )
            .unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
        assert!(err.to_string().contains("no memories"), "{err}");
    }

    #[test]
    fn rename_to_the_same_namespace_is_a_validation_error() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("same".into());
        insert_vec(&store, &ns, "row", [1.0; 8]);
        let err = store.rename_namespace(&ns, &ns, false).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
        assert!(err.to_string().contains("itself"), "{err}");
        assert_eq!(count_in(&store, &ns), 1, "nothing changed");
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;
    use rb_types::{LinkType, MemoryLink, MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(
            Namespace::Project("rb".into()),
            c.into(),
            MemoryType::Entity,
            5,
        );
        store.insert_memory(&m, None).unwrap();
        m
    }

    fn link(store: &SqliteStore, src: &MemoryNote, tgt: &MemoryNote) {
        store
            .add_link(&MemoryLink {
                source_id: src.id.clone(),
                target_id: tgt.id.clone(),
                link_type: LinkType::References,
                strength: 1.0,
                reason: String::new(),
                created_at: src.created_at,
            })
            .unwrap();
    }

    #[test]
    fn traverses_up_to_depth_with_real_hop_distances() {
        // 3-deep chain a -> b -> c -> d: hop values must be 1, 2, 3 (W1.5).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        let c = node(&store, "c");
        let d = node(&store, "d");
        link(&store, &a, &b); // a -> b
        link(&store, &b, &c); // b -> c
        link(&store, &c, &d); // c -> d

        let depth1 = store.graph_neighbors(&a.id, 1).unwrap();
        assert_eq!(depth1, vec![(b.id.clone(), 1)]);

        let depth2 = store.graph_neighbors(&a.id, 2).unwrap();
        assert_eq!(depth2, vec![(b.id.clone(), 1), (c.id.clone(), 2)]);

        let depth3 = store.graph_neighbors(&a.id, 3).unwrap();
        assert_eq!(
            depth3,
            vec![(b.id.clone(), 1), (c.id.clone(), 2), (d.id.clone(), 3)],
            "chain hops must be the real distances 1, 2, 3 in ascending order"
        );
    }

    #[test]
    fn diamond_multiple_paths_keep_minimum_hops() {
        // Two paths from a to d: a -> b -> d (2 hops) and a -> c -> e -> d
        // (3 hops). The recursive UNION dedups (node, depth) PAIRS, so the walk
        // holds both (d, 2) and (d, 3); MIN ... GROUP BY must collapse d to its
        // SHORTEST distance, 2, and return it exactly once (W1.5).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        let c = node(&store, "c");
        let d = node(&store, "d");
        let e = node(&store, "e");
        link(&store, &a, &b); // a -> b
        link(&store, &b, &d); // b -> d   (d at 2 hops)
        link(&store, &a, &c); // a -> c
        link(&store, &c, &e); // c -> e
        link(&store, &e, &d); // e -> d   (d at 3 hops via the long arm)

        let got = store.graph_neighbors(&a.id, 4).unwrap();
        let d_rows: Vec<&(rb_types::MemoryId, u8)> =
            got.iter().filter(|(id, _)| *id == d.id).collect();
        assert_eq!(d_rows.len(), 1, "each node appears exactly once: {got:?}");
        assert_eq!(
            d_rows[0].1, 2,
            "the shorter of the two paths to d must win: {got:?}"
        );

        // Full picture: b and c are direct neighbors (1), e is 2, d is min(2, 3) = 2.
        let mut by_id: Vec<(String, u8)> = got.iter().map(|(id, h)| (id.to_string(), *h)).collect();
        by_id.sort();
        let mut want = vec![
            (b.id.to_string(), 1u8),
            (c.id.to_string(), 1u8),
            (e.id.to_string(), 2u8),
            (d.id.to_string(), 2u8),
        ];
        want.sort();
        assert_eq!(by_id, want);
    }

    #[test]
    fn no_links_returns_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "lonely");
        assert!(store.graph_neighbors(&a.id, 3).unwrap().is_empty());
    }

    #[test]
    fn cycle_terminates_and_dedups() {
        // a -> b and b -> a form a 2-cycle. A naive UNION ALL recursion would loop
        // forever; UNION + DISTINCT must terminate and return each node once.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        link(&store, &a, &b); // a -> b
        link(&store, &b, &a); // b -> a (back-edge forms the cycle)

        // depth >= 2 forces the recursion to revisit `a` via b -> a; it must not
        // hang and must exclude the start node `a` and dedup `b`.
        let got = store.graph_neighbors(&a.id, 3).unwrap();
        // Neighbors of `a`: b at hop 1 (b reappears at depth 3 via the cycle,
        // but MIN-GROUP-BY keeps the shortest). `a` itself is reachable at
        // depth 2 via the back-edge but is excluded by `node <> ?1`.
        assert_eq!(
            got,
            vec![(b.id.clone(), 1)],
            "cycle must terminate with a deduplicated set at minimum hops"
        );
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_imp(
        store: &SqliteStore,
        ns: Namespace,
        content: &str,
        importance: u8,
    ) -> rb_types::MemoryId {
        let mut m = MemoryNote::new(ns, content.into(), MemoryType::Reference, importance);
        // Force distinct created_at ordering by nudging timestamps.
        m.created_at -= chrono::Duration::seconds(importance as i64);
        m.updated_at = m.created_at;
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
        id
    }

    #[test]
    fn orders_by_created_desc_and_filters_importance_and_ns() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        // importance => created_at offset: lower importance = more recent (smaller subtraction)
        let high_recent = insert_imp(&store, proj.clone(), "recent high", 2); // -2s, imp 2
        let mid = insert_imp(&store, proj.clone(), "older mid", 5); // -5s, imp 5
        let low = insert_imp(&store, proj.clone(), "oldest low", 8); // -8s, imp 8
        let _other_ns = insert_imp(&store, Namespace::Global, "global", 1);

        // No importance filter: newest first.
        let all = store.list(&proj, None, 10).unwrap();
        let ids: Vec<_> = all.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec![high_recent.clone(), mid.clone(), low.clone()]);

        // min_importance = 5 keeps mid(5) and low(8), drops high(2).
        let filtered = store.list(&proj, Some(5), 10).unwrap();
        let fids: Vec<_> = filtered.iter().map(|m| m.id.clone()).collect();
        assert_eq!(fids, vec![mid.clone(), low.clone()]);

        // limit respected.
        let limited = store.list(&proj, None, 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, high_recent);
    }

    #[test]
    fn excludes_archived() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let keep = insert_imp(&store, proj.clone(), "keep", 5);
        let drop_id = insert_imp(&store, proj.clone(), "drop", 5);
        store.archive_memory(&drop_id).unwrap();
        let res = store.list(&proj, None, 10).unwrap();
        let ids: Vec<_> = res.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec![keep]);
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    /// Shape-true FAKE secrets, BUILT at runtime from split literals so no
    /// committed source byte forms a scanner-matchable token (GitHub push
    /// protection blocks otherwise). Not real credentials.
    fn aws_key() -> String {
        format!("AKIA{}", "A".repeat(16))
    }
    fn github_token() -> String {
        format!("ghp_{}", "A".repeat(36))
    }

    fn seeded(content: &str, context: &str) -> (SqliteStore, MemoryId) {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(
            Namespace::Project("scrub".into()),
            content.to_string(),
            MemoryType::Insight,
            5,
        );
        m.context = context.to_string();
        m.summary = "a summary".to_string();
        store.insert_memory(&m, None).unwrap();
        (store, m.id)
    }

    #[test]
    fn scrub_redacts_planted_secret_from_content_and_marks_reembed() {
        let secret = aws_key();
        let (store, id) = seeded(
            &format!("deploy key is {secret} do not share"),
            "no secret here",
        );
        let out = store.scrub().unwrap();
        assert_eq!(out.scanned, 1);
        assert_eq!(out.redacted, 1);
        assert_eq!(out.reembed_pending, 1, "content changed -> reembed");

        let got = store.get_memory(&id).unwrap().unwrap();
        assert!(
            !got.content.contains(&secret),
            "plaintext secret must be gone: {}",
            got.content
        );
        assert!(got.content.contains("[REDACTED:aws-key]"));
        assert_eq!(
            got.embedding_input_version, "",
            "embedding stamp blanked for reembed"
        );

        // FTS no longer matches the secret token (mem_au resynced on content).
        let hits = store
            .keyword_search(&Namespace::Project("scrub".into()), &secret, 10)
            .unwrap();
        assert!(
            hits.is_empty(),
            "scrubbed secret must not be FTS-searchable"
        );
    }

    #[test]
    fn scrub_is_idempotent_and_a_clean_db_is_a_noop() {
        let (store, _id) = seeded(&format!("token {} here", github_token()), "");
        let first = store.scrub().unwrap();
        assert_eq!(first.redacted, 1);
        let second = store.scrub().unwrap();
        assert_eq!(second.scanned, 1);
        assert_eq!(second.redacted, 0, "second pass finds nothing to redact");
        assert_eq!(second.reembed_pending, 0);
    }

    #[test]
    fn scrub_redacts_archived_rows_too() {
        let secret = aws_key();
        let (store, id) = seeded(&format!("archived secret {secret}"), "");
        store.archive_memory(&id).unwrap();
        let out = store.scrub().unwrap();
        assert_eq!(
            out.redacted, 1,
            "a secret at rest is scrubbed even if archived"
        );
        // Fix #3: an archived row's text is redacted, but it is NOT counted in
        // reembed_pending — reembed skips archived rows, so counting it would
        // make "reembed until changed=0" never converge.
        assert_eq!(
            out.reembed_pending, 0,
            "archived rows must not be queued for a reembed that can never run"
        );
        let got = store.get_memory(&id).unwrap().unwrap();
        assert!(!got.content.contains(&secret));
    }

    #[test]
    fn scrub_drill_removes_planted_secret_from_the_db_file_at_rest() {
        // W2.4 / Phase 2 gate: the scrub drill on a SEEDED db. A secret that
        // predates the rule (written straight to the store, bypassing
        // capture-time redaction) must be gone from the raw file bytes after
        // scrub + checkpoint — the proof that retroactive redaction reaches
        // disk, not just the in-memory row.
        let secret = aws_key();
        let secret = secret.as_str();
        const SENTINEL: &str = "scrub-drill-sentinel-9b2e";
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("seeded.db");

        {
            let store = SqliteStore::open(&db, 8).unwrap();
            let mut m = MemoryNote::new(
                Namespace::Project("scrub".into()),
                format!("deploy key {secret} and marker {SENTINEL}"),
                MemoryType::Insight,
                5,
            );
            m.summary = format!("uses {secret}");
            store.insert_memory(&m, None).unwrap();

            // Vacuousness guard: the secret really is at rest before scrub.
            // In WAL mode the just-inserted row lives in the -wal file until
            // checkpoint, so grep all three files, not just the main db.
            let mut before = Vec::new();
            for suffix in ["", "-wal", "-shm"] {
                let p = format!("{}{suffix}", db.to_string_lossy());
                if let Ok(bytes) = std::fs::read(&p) {
                    before.extend_from_slice(&bytes);
                }
            }
            assert!(
                before.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "secret must be present before scrub (else the test is vacuous)"
            );

            let out = store.scrub().unwrap();
            assert_eq!(out.redacted, 1);
            // Close the connection so SQLite checkpoints WAL into the main file.
        }

        // Grep the main file plus any WAL/SHM straggler.
        let mut at_rest = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let p = format!("{}{suffix}", db.to_string_lossy());
            if let Ok(bytes) = std::fs::read(&p) {
                at_rest.extend_from_slice(&bytes);
                at_rest.push(0);
            }
        }
        assert!(
            at_rest
                .windows(SENTINEL.len())
                .any(|w| w == SENTINEL.as_bytes()),
            "the non-secret sentinel must survive (proves the row is intact)"
        );
        assert!(
            at_rest
                .windows(b"[REDACTED:".len())
                .any(|w| w == b"[REDACTED:"),
            "a redaction marker must be present at rest"
        );
        assert!(
            !at_rest
                .windows(secret.len())
                .any(|w| w == secret.as_bytes()),
            "the planted secret must be GONE from the db file bytes after scrub"
        );
    }

    #[test]
    fn scrub_covers_every_fts_and_embedding_text_column() {
        // Fix #9: a secret in ANY FTS-indexed or embedding-feeding text column
        // (content, summary, keywords, tags, context) must be redacted and no
        // longer keyword-searchable. keywords/tags were previously NOT scrubbed
        // despite being both FTS columns and embedding inputs.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("scrub".into());
        // Each secret is a valid AWS-key shape: AKIA + exactly 16 [0-9A-Z]
        // (the word padded with 'A' to width 16), distinct per column.
        let key = |w: &str| format!("AKIA{w:A<16}");
        let (c_sec, s_sec, k_sec, t_sec, x_sec) = (
            key("CONTENT"),
            key("SUMMARY"),
            key("KEYWORD"),
            key("TAGS"),
            key("CONTEXT"),
        );
        let mut m = MemoryNote::new(
            proj.clone(),
            format!("content has {c_sec}"),
            MemoryType::Insight,
            5,
        );
        m.summary = format!("summary has {s_sec}");
        m.keywords = vec![k_sec.clone(), "ok".into()];
        m.tags = vec![t_sec.clone()];
        m.context = format!("context has {x_sec}");
        store.insert_memory(&m, None).unwrap();

        let out = store.scrub().unwrap();
        assert_eq!(out.redacted, 1);

        let got = store.get_memory(&m.id).unwrap().unwrap();
        for secret in [&c_sec, &s_sec, &k_sec, &t_sec, &x_sec] {
            let secret = secret.as_str();
            assert!(
                !got.content.contains(secret)
                    && !got.summary.contains(secret)
                    && !got.context.contains(secret)
                    && !got.keywords.iter().any(|k| k.contains(secret))
                    && !got.tags.iter().any(|t| t.contains(secret)),
                "secret {secret} must be redacted from every text column"
            );
            // And gone from FTS (content/summary/keywords/tags are indexed).
            assert!(
                store.keyword_search(&proj, secret, 10).unwrap().is_empty(),
                "scrubbed secret {secret} must not be keyword-searchable"
            );
        }
        // keywords/tags stayed valid JSON arrays (the non-secret element survived).
        assert!(got.keywords.iter().any(|k| k == "ok"), "{:?}", got.keywords);
    }

    #[test]
    fn scrub_appends_one_bulk_oplog_row_only_when_something_changed() {
        let (store, _id) = seeded("clean content, nothing secret", "");
        store.scrub().unwrap();
        let n: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'scrub'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "a no-op scrub logs nothing");
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace};

    #[test]
    fn updates_fields_bumps_timestamp_and_syncs_fts() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(proj.clone(), "original term".into(), MemoryType::Insight, 3);
        m.updated_at -= chrono::Duration::seconds(100);
        store.insert_memory(&m, None).unwrap();

        let updates = MemoryUpdates {
            content: Some("rewritten unicorn term".into()),
            summary: Some("new summary".into()),
            importance: Some(9),
            tags: Some(vec!["alpha".into(), "beta".into()]),
            context: Some("new context".into()),
            confidence: Some(0.4),
        };
        store.update_memory(&m.id, &updates).unwrap();

        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.content, "rewritten unicorn term");
        assert_eq!(got.summary, "new summary");
        assert_eq!(got.importance, 9);
        assert_eq!(got.tags, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(got.context, "new context");
        assert!((got.confidence - 0.4).abs() < f32::EPSILON);
        assert!(got.updated_at.timestamp() > m.updated_at.timestamp());

        // FTS reflects new content, not old.
        let new_hits = store.keyword_search(&proj, "unicorn", 10).unwrap();
        assert_eq!(new_hits, vec![m.id.clone()]);
        let old_hits = store.keyword_search(&proj, "original", 10).unwrap();
        assert!(old_hits.is_empty());
    }

    #[test]
    fn partial_update_leaves_unset_fields() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(
            Namespace::Global,
            "keep me".into(),
            MemoryType::Reference,
            4,
        );
        m.summary = "keep summary".into();
        store.insert_memory(&m, None).unwrap();

        let updates = MemoryUpdates {
            importance: Some(7),
            ..Default::default()
        };
        store.update_memory(&m.id, &updates).unwrap();

        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.importance, 7);
        assert_eq!(got.content, "keep me");
        assert_eq!(got.summary, "keep summary");
    }

    #[test]
    fn update_confidence_alone_persists_and_validates_range() {
        // W2.2: confidence is settable through update_memory, and an
        // out-of-range value fails closed as InvalidArgument before the DB.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let m = MemoryNote::new(Namespace::Global, "trusted".into(), MemoryType::Insight, 5);
        store.insert_memory(&m, None).unwrap();

        store
            .update_memory(
                &m.id,
                &MemoryUpdates {
                    confidence: Some(0.25),
                    ..Default::default()
                },
            )
            .unwrap();
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert!((got.confidence - 0.25).abs() < f32::EPSILON);

        for bad in [-0.1f32, 1.5, f32::NAN] {
            let err = store
                .update_memory(
                    &m.id,
                    &MemoryUpdates {
                        confidence: Some(bad),
                        ..Default::default()
                    },
                )
                .unwrap_err();
            assert!(
                matches!(err, rb_types::Error::InvalidArgument(_)),
                "confidence {bad} must be InvalidArgument, got {err:?}"
            );
        }
        // The valid value persisted earlier survives the rejected writes.
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert!((got.confidence - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn update_missing_is_ok_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let updates = MemoryUpdates {
            importance: Some(5),
            ..Default::default()
        };
        // No row affected; method must not error.
        store.update_memory(&MemoryId::new(), &updates).unwrap();
    }

    #[test]
    fn all_none_update_is_true_noop_keeps_updated_at() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(Namespace::Global, "stable".into(), MemoryType::Reference, 4);
        // Pin updated_at well in the past so a spurious bump would be detectable.
        m.updated_at -= chrono::Duration::seconds(1000);
        store.insert_memory(&m, None).unwrap();
        let before = store.get_memory(&m.id).unwrap().unwrap();

        // All-None update must be a true no-op: updated_at unchanged.
        store
            .update_memory(&m.id, &MemoryUpdates::default())
            .unwrap();

        let after = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(
            after.updated_at.timestamp(),
            before.updated_at.timestamp(),
            "all-None update must not bump updated_at"
        );
    }

    #[test]
    fn editing_embedded_field_stales_embedding_stamp_for_reembed() {
        // Editing a field that feeds embedding_input (tags here) must stale the
        // row's embedding_input_version so the next reembed scan recomputes its
        // vector — otherwise the stored vector stays permanently stale.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(proj, "body".into(), MemoryType::Insight, 5);
        m.embedding_model = "model-x".into();
        m.embedding_input_version = "v2-composite".into();
        store.insert_memory(&m, None).unwrap();

        // Up to date: not a reembed candidate.
        assert!(store
            .memories_for_reembed("model-x", "v2-composite", 10)
            .unwrap()
            .is_empty());

        // Edit tags (an embedded field) -> stamp staled to the empty sentinel.
        store
            .update_memory(
                &m.id,
                &MemoryUpdates {
                    tags: Some(vec!["new".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(
            got.embedding_input_version, "",
            "embedded-field edit stales the stamp"
        );

        // Now a reembed candidate.
        let stale = store
            .memories_for_reembed("model-x", "v2-composite", 10)
            .unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, m.id);
    }

    #[test]
    fn editing_non_embedded_field_does_not_stale_stamp() {
        // Editing importance/summary (NOT part of embedding_input) must leave the
        // stamp intact so it never triggers a needless reembed.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(proj, "body".into(), MemoryType::Insight, 5);
        m.embedding_model = "model-x".into();
        m.embedding_input_version = "v2-composite".into();
        store.insert_memory(&m, None).unwrap();

        store
            .update_memory(
                &m.id,
                &MemoryUpdates {
                    summary: Some("s".into()),
                    importance: Some(7),
                    ..Default::default()
                },
            )
            .unwrap();
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(
            got.embedding_input_version, "v2-composite",
            "non-embedded edit keeps the stamp"
        );
        assert!(store
            .memories_for_reembed("model-x", "v2-composite", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn set_confidence_updates_validates_range_and_noops_on_missing() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(proj, "body".into(), MemoryType::Insight, 5);
        m.confidence = 1.0;
        store.insert_memory(&m, None).unwrap();

        // A valid value lands on the row.
        store.set_confidence(&m.id, 0.75).unwrap();
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert!((got.confidence - 0.75).abs() < 1e-6);

        // Out-of-range and non-finite values fail closed with InvalidArgument,
        // and leave the stored value untouched.
        for bad in [-0.1f32, 1.1, f32::NAN] {
            assert!(matches!(
                store.set_confidence(&m.id, bad),
                Err(Error::InvalidArgument(_))
            ));
        }
        let unchanged = store.get_memory(&m.id).unwrap().unwrap();
        assert!((unchanged.confidence - 0.75).abs() < 1e-6);

        // A missing id is a no-op Ok (0 rows) that touches no existing row.
        store.set_confidence(&MemoryId::new(), 0.2).unwrap();
        let still = store.get_memory(&m.id).unwrap().unwrap();
        assert!((still.confidence - 0.75).abs() < 1e-6);
    }

    #[test]
    fn rejects_out_of_range_importance() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let m = MemoryNote::new(Namespace::Global, "x".into(), MemoryType::Reference, 5);
        store.insert_memory(&m, None).unwrap();

        for bad in [0u8, 11u8] {
            let updates = MemoryUpdates {
                importance: Some(bad),
                ..Default::default()
            };
            let err = store.update_memory(&m.id, &updates).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument(ref s) if s.contains("importance")),
                "expected invalid argument error about importance for {bad}, got {err:?}"
            );
        }
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

    #[test]
    fn archive_sets_timestamp_and_excludes_from_searches() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let m = MemoryNote::new(
            proj.clone(),
            "searchable banana".into(),
            MemoryType::Reference,
            6,
        );
        let emb = [0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        store.insert_memory(&m, Some(&emb)).unwrap();

        // Visible before archive.
        assert_eq!(
            store.keyword_search(&proj, "banana", 10).unwrap(),
            vec![m.id.clone()]
        );
        assert!(!store.list(&proj, None, 10).unwrap().is_empty());

        store.archive_memory(&m.id).unwrap();

        // get_memory still returns it (with archived_at set) — archive is soft.
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert!(got.archived_at.is_some());

        // Excluded from keyword, vector, and list.
        assert!(store
            .keyword_search(&proj, "banana", 10)
            .unwrap()
            .is_empty());
        assert!(store.vector_search(&proj, &emb, 10).unwrap().is_empty());
        assert!(store.list(&proj, None, 10).unwrap().is_empty());
    }

    #[test]
    fn archive_missing_is_ok_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        store.archive_memory(&MemoryId::new()).unwrap();
    }
}

#[cfg(test)]
mod add_link_tests {
    use super::*;
    use rb_types::{LinkType, MemoryLink, MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(
            Namespace::Project("rb".into()),
            c.into(),
            MemoryType::Entity,
            5,
        );
        store.insert_memory(&m, None).unwrap();
        m
    }

    #[test]
    fn add_link_persists_and_is_returned_by_get() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");

        let link = MemoryLink {
            source_id: a.id.clone(),
            target_id: b.id.clone(),
            link_type: LinkType::Supersedes,
            strength: 0.9,
            reason: "newer".into(),
            created_at: a.created_at,
        };
        store.add_link(&link).unwrap();

        let got = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.links[0].target_id, b.id);
        assert_eq!(got.links[0].link_type, LinkType::Supersedes);
        assert!((got.links[0].strength - 0.9).abs() < 1e-6);
        assert_eq!(got.links[0].reason, "newer");
    }

    #[test]
    fn add_link_to_missing_target_fails_fk() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let link = MemoryLink {
            source_id: a.id.clone(),
            target_id: rb_types::MemoryId::new(),
            link_type: LinkType::References,
            strength: 0.5,
            reason: String::new(),
            created_at: a.created_at,
        };
        // foreign_keys=ON => FK violation surfaces as a storage error.
        let err = store.add_link(&link).unwrap_err();
        assert!(matches!(err, Error::Storage(_)));
    }

    #[test]
    fn add_link_duplicate_edge_is_invalid_argument_not_storage() {
        // The (source,target,type) PK already exists: the concurrent-race path
        // through add_link must surface the deterministic "already exists"
        // validation error, not a generic Storage/"internal error".
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        let link = MemoryLink {
            source_id: a.id.clone(),
            target_id: b.id.clone(),
            link_type: LinkType::Contradicts,
            strength: 1.0,
            reason: "x".into(),
            created_at: a.created_at,
        };
        store.add_link(&link).unwrap();
        let err = store.add_link(&link).unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(ref s) if s.contains("already exists")),
            "duplicate edge must map to InvalidArgument, got {err:?}"
        );
    }
}

#[cfg(test)]
mod access_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(
            Namespace::Project("rb".into()),
            c.into(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&m, None).unwrap();
        m
    }

    #[test]
    fn record_access_bumps_count_and_sets_last_accessed() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "accessed");
        // Fresh note: access_count 0, last_accessed_at None.
        let before = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(before.access_count, 0);
        assert!(before.last_accessed_at.is_none());

        store.record_access(&a.id).unwrap();
        let after = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(after.access_count, 1);
        assert!(after.last_accessed_at.is_some());

        // A second access increments again.
        store.record_access(&a.id).unwrap();
        assert_eq!(store.get_memory(&a.id).unwrap().unwrap().access_count, 2);
    }

    #[test]
    fn record_access_missing_id_is_ok_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // No row updated; must not error (best-effort access tracking).
        store.record_access(&MemoryId::new()).unwrap();
    }

    #[test]
    fn record_accesses_bumps_all_ids_in_one_transaction() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "access_a");
        let b = node(&store, "access_b");

        // Both bumped in one call.
        store
            .record_accesses(&[a.id.clone(), b.id.clone()])
            .unwrap();

        let got_a = store.get_memory(&a.id).unwrap().unwrap();
        let got_b = store.get_memory(&b.id).unwrap().unwrap();
        assert_eq!(got_a.access_count, 1, "a must be bumped");
        assert_eq!(got_b.access_count, 1, "b must be bumped");
        assert!(got_a.last_accessed_at.is_some());
        assert!(got_b.last_accessed_at.is_some());
    }

    #[test]
    fn record_accesses_missing_id_is_silently_skipped() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "present");
        let missing = MemoryId::new();

        // A missing id must not cause an error; the present id is still bumped.
        store.record_accesses(&[missing, a.id.clone()]).unwrap();

        let got = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(got.access_count, 1, "present id must be bumped");
    }

    #[test]
    fn record_accesses_duplicate_ids_bump_once() {
        // The UPDATE … WHERE memory_id IN (…) touches each row at most once
        // even when the same id appears multiple times in the slice.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "dup_target");

        store
            .record_accesses(&[a.id.clone(), a.id.clone()])
            .unwrap();

        let got = store.get_memory(&a.id).unwrap().unwrap();
        // The SQL UPDATE deduplicates: the row is bumped exactly once.
        assert_eq!(
            got.access_count, 1,
            "duplicate ids in the slice must not double-bump (SQL IN-list dedup)"
        );
    }

    #[test]
    fn record_accesses_empty_slice_is_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // Must not panic or error.
        store.record_accesses(&[]).unwrap();
    }

    #[test]
    fn record_access_bumps_applies_counts_and_monotonic_timestamps() {
        // W1.8 batched-bump semantics: each entry adds its accumulated count
        // and advances last_accessed_at monotonically (an older flush can
        // never move the clock backwards).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "bump_a");
        let b = node(&store, "bump_b");

        store
            .record_access_bumps(&[
                AccessBump {
                    id: a.id.clone(),
                    count: 3,
                    last_accessed_at: 100,
                },
                AccessBump {
                    id: b.id.clone(),
                    count: 1,
                    last_accessed_at: 50,
                },
            ])
            .unwrap();

        let got_a = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(got_a.access_count, 3, "count accumulates, not +1");
        assert_eq!(
            got_a.last_accessed_at.map(|t| t.timestamp()),
            Some(100),
            "stamp is the buffered access time"
        );
        let got_b = store.get_memory(&b.id).unwrap().unwrap();
        assert_eq!(got_b.access_count, 1);
        assert_eq!(got_b.last_accessed_at.map(|t| t.timestamp()), Some(50));

        // A later flush carrying an OLDER timestamp adds its count but keeps
        // the newer stamp.
        store
            .record_access_bumps(&[AccessBump {
                id: a.id.clone(),
                count: 2,
                last_accessed_at: 40,
            }])
            .unwrap();
        let again = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(again.access_count, 5, "3 + 2 across two flushes");
        assert_eq!(
            again.last_accessed_at.map(|t| t.timestamp()),
            Some(100),
            "last_accessed_at is monotonic"
        );
    }

    #[test]
    fn record_access_bumps_missing_id_skipped_and_empty_is_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "bump_present");

        // Empty slice: no transaction, no error.
        store.record_access_bumps(&[]).unwrap();

        // A missing id is silently skipped; present ids in the same batch land.
        store
            .record_access_bumps(&[
                AccessBump {
                    id: MemoryId::new(),
                    count: 7,
                    last_accessed_at: 100,
                },
                AccessBump {
                    id: a.id.clone(),
                    count: 1,
                    last_accessed_at: 100,
                },
            ])
            .unwrap();
        let got = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(got.access_count, 1);
    }

    #[test]
    fn supersede_sets_superseded_by_and_archives_old() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old decision");
        let new = node(&store, "new decision");

        store.supersede(&old.id, &new.id).unwrap();

        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert_eq!(got.superseded_by.as_ref(), Some(&new.id));
        assert!(got.archived_at.is_some(), "superseded note is archived");
        // The new note is untouched.
        let new_got = store.get_memory(&new.id).unwrap().unwrap();
        assert!(new_got.superseded_by.is_none());
        assert!(new_got.archived_at.is_none());
    }

    #[test]
    fn supersede_excludes_old_from_keyword_and_list() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let old = node(&store, "supersede excludes me");
        let new = node(&store, "supersede keeps me");
        store.supersede(&old.id, &new.id).unwrap();

        // old is archived -> excluded from keyword + list; new remains.
        let kw = store.keyword_search(&proj, "supersede", 10).unwrap();
        assert!(kw.contains(&new.id));
        assert!(!kw.contains(&old.id));
        let listed: Vec<MemoryId> = store
            .list(&proj, None, 10)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert!(listed.contains(&new.id));
        assert!(!listed.contains(&old.id));
    }

    #[test]
    fn supersede_missing_new_target_fails_fk() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old");
        // superseded_by REFERENCES memories(memory_id); a missing target must fail
        // the FK and leave the old note unchanged (transaction rolled back).
        // foreign_keys=ON is set in SqliteStore::init, so the FK is enforced
        // immediately on the UPDATE statement (SQLite FKs are not deferred by default).
        let err = store.supersede(&old.id, &MemoryId::new()).unwrap_err();
        assert!(matches!(err, Error::Storage(_)));
        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert!(got.superseded_by.is_none(), "rolled back: no superseded_by");
        assert!(got.archived_at.is_none(), "rolled back: not archived");
    }

    #[test]
    fn supersede_missing_old_errors() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // `new` exists so the FK is satisfiable; `old` does NOT exist. The first
        // UPDATE affects 0 rows, which must fail fast (NotFound) and roll back —
        // never a silent Ok(()).
        let new = node(&store, "new decision");
        let missing_old = MemoryId::new();

        let err = store.supersede(&missing_old, &new.id).unwrap_err();
        assert!(matches!(err, Error::NotFound(ref id) if *id == missing_old));

        // No partial write: `new` is untouched (not archived, not superseded).
        let new_got = store.get_memory(&new.id).unwrap().unwrap();
        assert!(new_got.superseded_by.is_none());
        assert!(new_got.archived_at.is_none());
    }
}

#[cfg(test)]
mod get_many_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, ns: &Namespace, c: &str) -> MemoryId {
        let m = MemoryNote::new(ns.clone(), c.into(), MemoryType::Insight, 5);
        store.insert_memory(&m, None).unwrap();
        m.id
    }

    #[test]
    fn get_many_returns_notes_in_request_order() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let a = node(&store, &ns, "alpha");
        let b = node(&store, &ns, "bravo");
        let c = node(&store, &ns, "charlie");

        // Request in a non-storage order; result must follow request order.
        let got = store
            .get_many(&ns, &[c.clone(), a.clone(), b.clone()])
            .unwrap();
        let ids: Vec<MemoryId> = got.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![c, a, b]);
    }

    #[test]
    fn get_many_skips_missing_and_out_of_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let other = Namespace::Project("other".into());
        let in_ns = node(&store, &proj, "in scope");
        let foreign = node(&store, &other, "foreign");
        let missing = MemoryId::new();

        let got = store
            .get_many(&proj, &[missing, foreign.clone(), in_ns.clone()])
            .unwrap();
        let ids: Vec<MemoryId> = got.iter().map(|n| n.id.clone()).collect();
        // Only the in-namespace, existing id is returned.
        assert_eq!(ids, vec![in_ns]);
    }

    #[test]
    fn get_many_empty_input_returns_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        assert!(store.get_many(&ns, &[]).unwrap().is_empty());
    }

    #[test]
    fn get_many_loads_links_for_each_note() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let a = node(&store, &ns, "src");
        let b = node(&store, &ns, "dst");
        store
            .add_link(&rb_types::MemoryLink {
                source_id: a.clone(),
                target_id: b.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "rel".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        let got = store.get_many(&ns, std::slice::from_ref(&a)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].links.len(), 1);
        assert_eq!(got[0].links[0].target_id, b);
    }

    #[test]
    fn get_many_preserves_duplicate_ids_positionally() {
        // Request [a, a, b]: the contract says "same order as ids", including
        // duplicates. A remove()-based reorder would collapse [a, a, b] to [a, b];
        // get()-based must emit [a, a, b].
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let a = node(&store, &ns, "alpha");
        let b = node(&store, &ns, "bravo");

        let got = store
            .get_many(&ns, &[a.clone(), a.clone(), b.clone()])
            .unwrap();
        let ids: Vec<MemoryId> = got.iter().map(|n| n.id.clone()).collect();
        assert_eq!(
            ids,
            vec![a.clone(), a, b],
            "duplicate id must appear twice in output"
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
mod near_duplicates_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_vec(
        store: &SqliteStore,
        ns: Namespace,
        content: &str,
        v: [f32; 8],
    ) -> rb_types::MemoryId {
        let m = MemoryNote::new(ns, content.into(), MemoryType::Insight, 5);
        let id = m.id.clone();
        store.insert_memory(&m, Some(&v)).unwrap();
        id
    }

    #[test]
    fn returns_same_namespace_twin_and_never_crosses_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj_a = Namespace::Project("a".into());
        let proj_b = Namespace::Project("b".into());

        // Anchor in A.
        let anchor = insert_vec(
            &store,
            proj_a.clone(),
            "anchor",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Near-identical twin in A (same direction => cosine distance ~0 => sim ~1).
        let twin = insert_vec(
            &store,
            proj_a.clone(),
            "twin",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // A clearly different vector in A (orthogonal => cosine distance ~1 => sim ~0.0).
        let _different = insert_vec(
            &store,
            proj_a.clone(),
            "different",
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Near-identical to anchor BUT in namespace B: must NEVER be returned.
        let foreign = insert_vec(
            &store,
            proj_b.clone(),
            "foreign twin",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let dups = store.near_duplicates(&proj_a, &anchor, 0.95, 10).unwrap();
        let ids: Vec<rb_types::MemoryId> = dups.iter().map(|(id, _)| id.clone()).collect();

        assert!(ids.contains(&twin), "the same-namespace twin must be found");
        assert!(
            !ids.contains(&anchor),
            "the anchor itself must be excluded (self)"
        );
        assert!(
            !ids.contains(&foreign),
            "a near-identical memory in another namespace must NEVER be returned"
        );
        // The orthogonal vector has similarity ~0.0, well below the 0.95 threshold.
        assert_eq!(ids, vec![twin], "only the above-threshold twin is returned");
        // Reported similarity for an identical vector is at/near 1.0.
        assert!(
            dups[0].1 >= 0.95,
            "twin similarity must meet the threshold, got {}",
            dups[0].1
        );
    }

    #[test]
    fn missing_anchor_or_no_vector_returns_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("a".into());
        // Anchor id that does not exist at all.
        let ghost = rb_types::MemoryId::new();
        assert!(store
            .near_duplicates(&ns, &ghost, 0.95, 10)
            .unwrap()
            .is_empty());

        // A memory that exists but was inserted WITHOUT an embedding has no vector
        // row, so there is nothing to KNN against: empty, not an error.
        let no_vec = MemoryNote::new(ns.clone(), "no vector".into(), MemoryType::Insight, 5);
        let no_vec_id = no_vec.id.clone();
        store.insert_memory(&no_vec, None).unwrap();
        assert!(store
            .near_duplicates(&ns, &no_vec_id, 0.95, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn excludes_archived_candidates() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("a".into());
        let anchor = insert_vec(
            &store,
            ns.clone(),
            "anchor",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let twin = insert_vec(
            &store,
            ns.clone(),
            "twin",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Archive the twin: it must drop out of the candidate set.
        store.archive_memory(&twin).unwrap();
        let dups = store.near_duplicates(&ns, &anchor, 0.95, 10).unwrap();
        assert!(
            dups.is_empty(),
            "an archived candidate must not be returned, got {dups:?}"
        );
    }

    #[test]
    fn memories_for_recalibration_carries_access_fields_and_excludes_archived() {
        let store = SqliteStore::open_in_memory(8).unwrap();

        // Two active rows in different namespaces; one archived row.
        let mut a = MemoryNote::new(
            Namespace::Global,
            "frequently accessed".into(),
            MemoryType::Insight,
            5,
        );
        a.access_count = 7;
        a.last_accessed_at = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0);
        store.insert_memory(&a, None).unwrap();

        let b = MemoryNote::new(
            Namespace::Project("rb".into()),
            "never accessed".into(),
            MemoryType::Reference,
            3,
        );
        store.insert_memory(&b, None).unwrap();

        let gone = MemoryNote::new(Namespace::Global, "archived".into(), MemoryType::Insight, 9);
        let gone_id = gone.id.clone();
        store.insert_memory(&gone, None).unwrap();
        store.archive_memory(&gone_id).unwrap();

        let rows = store.memories_for_recalibration(100).unwrap();

        // Archived row excluded; exactly the two active rows returned.
        assert_eq!(rows.len(), 2, "archived rows must be excluded");
        assert!(
            rows.iter().all(|r| r.id != gone_id),
            "archived id must not appear"
        );

        let row_a = rows
            .iter()
            .find(|r| r.id == a.id)
            .expect("active row a must be present");
        assert_eq!(row_a.namespace, Namespace::Global);
        assert_eq!(row_a.importance, 5);
        assert_eq!(row_a.base_importance, 5, "insert stamps the author prior");
        assert_eq!(row_a.access_count, 7);
        assert_eq!(row_a.last_accessed_at, Some(1_700_000_000));

        let row_b = rows
            .iter()
            .find(|r| r.id == b.id)
            .expect("active row b must be present");
        assert_eq!(row_b.namespace, Namespace::Project("rb".into()));
        assert_eq!(row_b.importance, 3);
        assert_eq!(row_b.base_importance, 3, "insert stamps the author prior");
        assert_eq!(row_b.access_count, 0);
        assert_eq!(row_b.last_accessed_at, None);
    }

    #[test]
    fn set_recalibrated_importance_moves_effective_but_never_the_author_prior() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let m = MemoryNote::new(Namespace::Global, "anchored".into(), MemoryType::Insight, 4);
        store.insert_memory(&m, None).unwrap();
        let stored_before = store.get_memory(&m.id).unwrap().expect("note present");

        store.set_recalibrated_importance(&m.id, 6).unwrap();

        let row = store
            .memories_for_recalibration(10)
            .unwrap()
            .into_iter()
            .find(|r| r.id == m.id)
            .expect("row present");
        assert_eq!(row.importance, 6, "effective importance moved");
        assert_eq!(
            row.base_importance, 4,
            "author prior must survive the job write"
        );

        let note = store.get_memory(&m.id).unwrap().expect("note present");
        assert_eq!(note.importance, 6, "ranking reads the effective value");
        assert_eq!(
            note.updated_at, stored_before.updated_at,
            "a maintenance write must NOT bump updated_at (the \
             update_vector/rename_namespace rule): recalibrated rows must not \
             look freshly user-modified"
        );
    }

    #[test]
    fn set_recalibrated_importance_validates_range_and_ignores_missing_id() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // Out-of-range importance fails closed (matching the insert path).
        let err = store
            .set_recalibrated_importance(&MemoryId::new(), 11)
            .unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "11 must be rejected, got {err:?}"
        );
        // Missing id with a valid value is a best-effort no-op.
        store
            .set_recalibrated_importance(&MemoryId::new(), 5)
            .unwrap();
    }

    #[test]
    fn explicit_importance_update_redeclares_the_author_prior() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let m = MemoryNote::new(
            Namespace::Global,
            "re-declared".into(),
            MemoryType::Insight,
            3,
        );
        store.insert_memory(&m, None).unwrap();

        // Simulate a prior recalibration: effective drifts, prior anchored.
        store.set_recalibrated_importance(&m.id, 5).unwrap();

        // The USER explicitly sets importance: both effective AND prior move —
        // an explicit update is the author re-declaring intent.
        store
            .update_memory(
                &m.id,
                &MemoryUpdates {
                    importance: Some(9),
                    ..Default::default()
                },
            )
            .unwrap();

        let row = store
            .memories_for_recalibration(10)
            .unwrap()
            .into_iter()
            .find(|r| r.id == m.id)
            .expect("row present");
        assert_eq!(row.importance, 9);
        assert_eq!(
            row.base_importance, 9,
            "an explicit user update re-anchors the author prior"
        );
    }

    #[test]
    fn memories_for_recalibration_respects_limit() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        for i in 0..5 {
            let m = MemoryNote::new(
                Namespace::Global,
                format!("note {i}"),
                MemoryType::Insight,
                4,
            );
            store.insert_memory(&m, None).unwrap();
        }
        let rows = store.memories_for_recalibration(3).unwrap();
        assert_eq!(rows.len(), 3, "limit must bound the row count");
    }
}

#[cfg(test)]
mod contradiction_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{LinkType, MemoryLink, MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(Namespace::Global, c.into(), MemoryType::Insight, 5);
        store.insert_memory(&m, None).unwrap();
        m
    }

    fn contradicts(store: &SqliteStore, src: &MemoryNote, tgt: &MemoryNote) {
        store
            .add_link(&MemoryLink {
                source_id: src.id.clone(),
                target_id: tgt.id.clone(),
                link_type: LinkType::Contradicts,
                strength: 1.0,
                reason: "conflicting claims".into(),
                created_at: src.created_at,
            })
            .unwrap();
    }

    #[test]
    fn empty_input_returns_empty_set() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        assert!(store
            .active_contradicts(&Namespace::Global, &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn flags_both_endpoints_of_an_active_contradicts_link() {
        // A contradicts B (directed). BOTH endpoints must be flagged: outbound for
        // the source, inbound for the target.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a says x");
        let b = node(&store, "b says not-x");
        contradicts(&store, &a, &b);

        let flagged = store
            .active_contradicts(&Namespace::Global, &[a.id.clone(), b.id.clone()])
            .unwrap();
        assert!(flagged.contains(&a.id), "source flagged (outbound)");
        assert!(flagged.contains(&b.id), "target flagged (inbound)");
    }

    #[test]
    fn uncontested_memory_is_not_flagged() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        // A references B — NOT a contradiction.
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: LinkType::References,
                strength: 1.0,
                reason: String::new(),
                created_at: a.created_at,
            })
            .unwrap();
        let flagged = store
            .active_contradicts(&Namespace::Global, &[a.id.clone(), b.id.clone()])
            .unwrap();
        assert!(flagged.is_empty(), "references is not a contradiction");
    }

    #[test]
    fn contradiction_with_archived_endpoint_is_not_active() {
        // A contradicts B, then B is archived. The contradiction is no longer
        // "active" — A must NOT be flagged (the contradicting memory is gone).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a survives");
        let b = node(&store, "b archived");
        contradicts(&store, &a, &b);
        store.archive_memory(&b.id).unwrap();

        let flagged = store
            .active_contradicts(&Namespace::Global, &[a.id.clone(), b.id.clone()])
            .unwrap();
        assert!(
            !flagged.contains(&a.id),
            "contradiction with an archived memory is inactive"
        );
    }

    #[test]
    fn archived_local_endpoint_is_not_flagged() {
        // get() can return an archived memory; even with a live contradicts edge to
        // an ACTIVE partner, an archived LOCAL row must not be flagged contested
        // (Feature C: "ACTIVE contradicts" requires BOTH endpoints active).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a active");
        let b = node(&store, "b will be archived");
        contradicts(&store, &a, &b);
        store.archive_memory(&b.id).unwrap();
        let flagged = store
            .active_contradicts(&Namespace::Global, std::slice::from_ref(&b.id))
            .unwrap();
        assert!(
            !flagged.contains(&b.id),
            "an archived local endpoint must not be flagged contested"
        );
    }

    #[test]
    fn cross_namespace_contradiction_does_not_flag() {
        // `memory_links` carries no namespace and add_link permits cross-namespace
        // edges. A contradicts B where A is Global and B lives in another namespace:
        // querying Global must NOT flag A, or B's existence leaks across the
        // isolation boundary (the bug this scoping fixes).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "global claim"); // Namespace::Global
        let other = Namespace::Project("secret".into());
        let b = MemoryNote::new(other.clone(), "hidden claim".into(), MemoryType::Insight, 5);
        store.insert_memory(&b, None).unwrap();
        contradicts(&store, &a, &b);

        // Queried in Global: A is not flagged — its only contradiction is in another ns.
        let in_global = store
            .active_contradicts(&Namespace::Global, &[a.id.clone(), b.id.clone()])
            .unwrap();
        assert!(
            in_global.is_empty(),
            "a cross-namespace contradiction must not flag any Global id"
        );

        // Queried in the OTHER namespace: B sees A as the far endpoint, but A is
        // Global, so B is still not flagged — both directions stay isolated.
        let in_other = store
            .active_contradicts(&other, &[a.id.clone(), b.id.clone()])
            .unwrap();
        assert!(
            in_other.is_empty(),
            "neither endpoint is flagged from either namespace's view"
        );
    }
}

#[cfg(test)]
mod oplog_tests {
    use super::*;
    use rb_types::{
        ChangeKind, LinkType, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    };

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let n = MemoryNote::new(
            Namespace::Project("oplog".into()),
            c.to_string(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&n, None).unwrap();
        n
    }

    /// All `(op, memory_id, namespace, site_id, details)` rows in seq order.
    fn oplog_rows(store: &SqliteStore) -> Vec<(String, String, String, String, String)> {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT op, memory_id, namespace, site_id, details
                 FROM memory_oplog ORDER BY seq",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn site_id_is_seeded_as_uuid_and_stable_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("site.db");
        let first = SqliteStore::open(&db, 8).unwrap().site_id().to_string();
        assert_eq!(first.len(), 36, "site_id is a uuid: {first}");
        let second = SqliteStore::open(&db, 8).unwrap().site_id().to_string();
        assert_eq!(first, second, "site_id must survive reopen");
    }

    #[test]
    fn insert_appends_one_oplog_row_with_namespace_and_site_id() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let n = node(&store, "first");
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 1);
        let (op, mid, ns, site, details) = &rows[0];
        assert_eq!(op, "insert");
        assert_eq!(mid, &n.id.to_string());
        assert_eq!(ns, &n.namespace.as_db_string());
        assert_eq!(site, store.site_id());
        assert_eq!(
            store.meta_value("site_id").unwrap().as_deref(),
            Some(store.site_id()),
            "cached site_id must equal the meta seed"
        );
        assert_eq!(details, "");
    }

    #[test]
    fn insert_with_links_appends_one_link_oplog_row_per_edge() {
        // Links carried on the note must each produce a `link` oplog row (same
        // shape as add_link's), or a replay consumer cannot reconstruct edges.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let target = node(&store, "target");
        let mut n = MemoryNote::new(
            Namespace::Project("oplog".into()),
            "linked at insert".to_string(),
            MemoryType::Insight,
            5,
        );
        n.links.push(MemoryLink {
            source_id: n.id.clone(),
            target_id: target.id.clone(),
            link_type: LinkType::Extends,
            strength: 0.8,
            reason: "carried on the note".into(),
            created_at: chrono::Utc::now(),
        });
        store.insert_memory(&n, None).unwrap();

        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 3, "target insert + insert + one link row");
        assert_eq!(rows[1].0, "insert");
        assert_eq!(rows[1].1, n.id.to_string());
        let (op, mid, _, _, details) = &rows[2];
        assert_eq!(op, "link");
        assert_eq!(mid, &n.id.to_string());
        let details: serde_json::Value = serde_json::from_str(details).unwrap();
        assert_eq!(details["type"], "extends");
        assert_eq!(details["target"], target.id.to_string());
    }

    #[test]
    fn set_recalibrated_importance_appends_oplog_only_on_real_change() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let n = node(&store, "to recalibrate");

        // Missing-id write: best-effort no-op, no oplog row.
        store
            .set_recalibrated_importance(&MemoryId::new(), 5)
            .unwrap();
        assert_eq!(oplog_rows(&store).len(), 1, "only the insert is logged");

        // A real job write logs a `set_importance` row a replay consumer can
        // reproduce (importance is durable ranking state).
        store.set_recalibrated_importance(&n.id, 7).unwrap();
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 2);
        let (op, mid, _, _, details) = &rows[1];
        assert_eq!(op, "set_importance");
        assert_eq!(mid, &n.id.to_string());
        let details: serde_json::Value = serde_json::from_str(details).unwrap();
        assert_eq!(details["importance"], 7);
    }

    #[test]
    fn update_appends_oplog_only_on_real_change() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let n = node(&store, "to update");

        // All-None update: true no-op, no oplog row.
        store
            .update_memory(&n.id, &MemoryUpdates::default())
            .unwrap();
        // Missing-id update: Ok no-op, no oplog row.
        store
            .update_memory(
                &MemoryId::new(),
                &MemoryUpdates {
                    summary: Some("s".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(oplog_rows(&store).len(), 1, "only the insert is logged");

        store
            .update_memory(
                &n.id,
                &MemoryUpdates {
                    summary: Some("new summary".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].0, "update");
        assert_eq!(rows[1].1, n.id.to_string());
    }

    #[test]
    fn archive_appends_oplog_once_and_rearchive_logs_nothing() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let n = node(&store, "to archive");
        store.archive_memory(&n.id).unwrap();
        store.archive_memory(&n.id).unwrap(); // idempotent no-op
        store.archive_memory(&MemoryId::new()).unwrap(); // missing no-op
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 2, "insert + exactly one archive");
        assert_eq!(rows[1].0, "archive");
    }

    #[test]
    fn supersede_appends_oplog_with_replacement_id_in_details() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old");
        let new = node(&store, "new");
        store.supersede(&old.id, &new.id).unwrap();
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 3, "two inserts + one supersede");
        assert_eq!(rows[2].0, "supersede");
        assert_eq!(rows[2].1, old.id.to_string());
        let details: serde_json::Value = serde_json::from_str(&rows[2].4).unwrap();
        assert_eq!(details["new"], new.id.to_string());
    }

    #[test]
    fn add_link_appends_oplog_with_type_and_target_in_details() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: LinkType::Extends,
                strength: 0.9,
                reason: "a extends b".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 3, "two inserts + one link");
        assert_eq!(rows[2].0, "link");
        assert_eq!(rows[2].1, a.id.to_string());
        let details: serde_json::Value = serde_json::from_str(&rows[2].4).unwrap();
        assert_eq!(details["type"], "extends");
        assert_eq!(details["target"], b.id.to_string());
    }

    #[test]
    fn oplog_changes_since_replays_kind_mapped_namespace_scoped_after_cursor() {
        // W2.7 replay-on-reconnect: the oplog reads back as change events —
        // kind-mapped, namespace-scoped, strictly after the cursor, oldest
        // first, with seq stamped for cursor tracking.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("oplog".into());
        let a = node(&store, "a"); // seq 1: insert
        let b = node(&store, "b"); // seq 2: insert
        store
            .update_memory(
                &a.id,
                &MemoryUpdates {
                    importance: Some(7),
                    ..Default::default()
                },
            )
            .unwrap(); // seq 3: update
        store.archive_memory(&b.id).unwrap(); // seq 4: archive

        // A foreign-namespace write must never leak into the replay.
        let foreign = MemoryNote::new(
            Namespace::Project("other".into()),
            "foreign".to_string(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&foreign, None).unwrap(); // seq 5

        assert_eq!(store.last_oplog_seq().unwrap(), 5);

        let all = store.oplog_changes_since(&ns, 0, 100).unwrap();
        let kinds: Vec<_> = all.changes.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ChangeKind::Created,
                ChangeKind::Created,
                ChangeKind::Updated,
                ChangeKind::Archived
            ]
        );
        assert!(
            all.changes.iter().all(|e| e.namespace == ns),
            "namespace-scoped"
        );
        let seqs: Vec<_> = all.changes.iter().map(|e| e.seq.unwrap()).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4], "oldest first, seq stamped");
        assert_eq!(
            all.scanned, 4,
            "4 in-namespace rows scanned (foreign excluded)"
        );
        assert_eq!(all.last_seq, 4, "cursor advances to the last scanned seq");

        // Strictly after a cursor.
        let after = store.oplog_changes_since(&ns, 2, 100).unwrap();
        assert_eq!(
            after
                .changes
                .iter()
                .map(|e| e.seq.unwrap())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );

        // Limit caps the scan.
        let capped = store.oplog_changes_since(&ns, 0, 2).unwrap();
        assert_eq!(capped.changes.len(), 2);
        assert_eq!(capped.scanned, 2);

        // Empty log tail: nothing after the max cursor.
        let tail = store.oplog_changes_since(&ns, 5, 100).unwrap();
        assert!(tail.changes.is_empty());
        assert_eq!(tail.scanned, 0);
        assert_eq!(tail.last_seq, 5, "empty page keeps the input cursor");
    }

    #[test]
    fn oplog_replay_skips_bulk_admin_rows_and_paginates_past_them() {
        // W2.7 bugs #1+#2: a namespace_rename row (empty memory_id, real
        // namespace) must be SKIPPED (not crash parse_id("")) AND counted in
        // `scanned` so pagination advances past it instead of looping or
        // dropping later rows.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("bulk".into());
        let other = Namespace::Project("src".into());
        let insert = |s: &SqliteStore, n: &Namespace, c: &str| -> MemoryId {
            let m = MemoryNote::new(n.clone(), c.into(), MemoryType::Insight, 5);
            s.insert_memory(&m, None).unwrap();
            m.id
        };

        let a = insert(&store, &ns, "a"); // seq 1: insert (bulk)
        insert(&store, &other, "moving"); // seq 2: insert (src)
                                          // Rename INTO `bulk` writes a namespace_rename row under `bulk`
                                          // with an empty memory_id sentinel.
        store.rename_namespace(&other, &ns, true).unwrap(); // seq 3: bulk row under `bulk`
        let c = insert(&store, &ns, "c"); // seq 4: insert (bulk)

        let page = store.oplog_changes_since(&ns, 0, 100).unwrap();
        // The rename row is skipped, so no empty-id bulk row leaks into replay.
        let ids: Vec<_> = page.changes.iter().map(|e| e.id.clone()).collect();
        assert!(ids.contains(&a) && ids.contains(&c));
        assert!(
            page.changes.iter().all(|e| !e.id.to_string().is_empty()),
            "no empty-id bulk row leaked into replay"
        );
        // ...but `scanned`/`last_seq` cover EVERY scanned row incl. the skipped.
        assert_eq!(page.last_seq, store.last_oplog_seq().unwrap());

        // Paginate with limit=1 so the rename row lands mid-stream: the cursor
        // must still advance past it and reach every real change.
        let mut after = 0u64;
        let mut seen = Vec::new();
        loop {
            let page = store.oplog_changes_since(&ns, after, 1).unwrap();
            if page.scanned == 0 {
                break;
            }
            seen.extend(page.changes.iter().map(|e| e.id.clone()));
            after = page.last_seq;
        }
        assert!(
            seen.contains(&a) && seen.contains(&c),
            "every real change is reached across pages despite the skipped bulk row"
        );
    }

    #[test]
    fn set_confidence_appends_oplog_only_on_real_change() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let n = node(&store, "to score");
        store.set_confidence(&MemoryId::new(), 0.5).unwrap(); // missing no-op
        store.set_confidence(&n.id, 0.4).unwrap();
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 2, "insert + exactly one set_confidence");
        assert_eq!(rows[1].0, "set_confidence");
        assert_eq!(rows[1].1, n.id.to_string());
        let details: serde_json::Value = serde_json::from_str(&rows[1].4).unwrap();
        let logged = details["confidence"].as_f64().unwrap();
        assert!(
            (logged - 0.4).abs() < 1e-6,
            "confidence in details: {logged}"
        );
    }

    #[test]
    fn set_link_strength_and_delete_link_append_oplog_with_edge_details() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: LinkType::Extends,
                strength: 0.9,
                reason: "a extends b".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        store
            .set_link_strength(&a.id, &b.id, LinkType::Extends, 0.5)
            .unwrap();
        // Missing-edge ops stay silent no-ops (decay is best-effort): no rows.
        store
            .set_link_strength(&a.id, &b.id, LinkType::Contradicts, 0.5)
            .unwrap();
        store
            .delete_link(&a.id, &b.id, LinkType::Contradicts)
            .unwrap();
        store.delete_link(&a.id, &b.id, LinkType::Extends).unwrap();

        let rows = oplog_rows(&store);
        assert_eq!(
            rows.len(),
            5,
            "two inserts + link + set_link_strength + unlink"
        );
        assert_eq!(rows[3].0, "set_link_strength");
        assert_eq!(rows[3].1, a.id.to_string());
        let details: serde_json::Value = serde_json::from_str(&rows[3].4).unwrap();
        assert_eq!(details["type"], "extends");
        assert_eq!(details["target"], b.id.to_string());
        assert!((details["strength"].as_f64().unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(rows[4].0, "unlink");
        assert_eq!(rows[4].1, a.id.to_string());
        let details: serde_json::Value = serde_json::from_str(&rows[4].4).unwrap();
        assert_eq!(details["type"], "extends");
        assert_eq!(details["target"], b.id.to_string());
    }

    #[test]
    fn failed_insert_rolls_back_memory_and_oplog_together() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // A link to a missing target violates the FK AFTER both the memories row
        // and the oplog row were written inside the transaction: everything must
        // roll back together.
        let mut n = MemoryNote::new(
            Namespace::Project("oplog".into()),
            "doomed".into(),
            MemoryType::Insight,
            5,
        );
        n.links = vec![MemoryLink {
            source_id: n.id.clone(),
            target_id: MemoryId::new(), // does not exist
            link_type: LinkType::References,
            strength: 0.5,
            reason: String::new(),
            created_at: n.created_at,
        }];
        assert!(store.insert_memory(&n, None).is_err());

        let memories: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(memories, 0, "memories insert rolled back");
        assert!(
            oplog_rows(&store).is_empty(),
            "oplog row must roll back with the failed mutation"
        );
    }

    #[test]
    fn failed_add_link_rolls_back_oplog() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let err = store.add_link(&MemoryLink {
            source_id: a.id.clone(),
            target_id: MemoryId::new(), // missing target: FK failure
            link_type: LinkType::References,
            strength: 0.5,
            reason: String::new(),
            created_at: chrono::Utc::now(),
        });
        assert!(err.is_err());
        let rows = oplog_rows(&store);
        assert_eq!(rows.len(), 1, "only the insert is logged; no link row");
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    #[test]
    fn provenance_fields_round_trip_through_insert_and_get() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut n = MemoryNote::new(
            Namespace::Project("prov".into()),
            "with provenance".into(),
            MemoryType::Insight,
            5,
        );
        n.origin_user = Some("alice".into());
        n.origin_host = Some("devbox".into());
        n.origin_agent = Some("claude-code".into());
        n.origin_source = Some("hook".into());
        n.session_id = Some("s-123".into());
        store.insert_memory(&n, None).unwrap();

        let got = store.get_memory(&n.id).unwrap().unwrap();
        assert_eq!(got.origin_user.as_deref(), Some("alice"));
        assert_eq!(got.origin_host.as_deref(), Some("devbox"));
        assert_eq!(got.origin_agent.as_deref(), Some("claude-code"));
        assert_eq!(got.origin_source.as_deref(), Some("hook"));
        assert_eq!(got.session_id.as_deref(), Some("s-123"));

        // The list projection decodes them too (same by-name path, explicit
        // column list).
        let listed = store.list(&n.namespace, None, 10).unwrap();
        assert_eq!(listed[0].origin_source.as_deref(), Some("hook"));
        // And get_many.
        let many = store.get_many(&n.namespace, &[n.id.clone()]).unwrap();
        assert_eq!(many[0].session_id.as_deref(), Some("s-123"));
    }

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
    //! W1.1 (cosine metric) + W1.7 (namespace partition + vector hygiene):
    //! the open-time rebuild, the recalibrated metric, and the live-only
    //! partitioned KNN behavior.
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
    fn archive_deletes_vector_row_in_same_transaction() {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        let ns = Namespace::Project("hygiene".into());
        let id = insert_vec(&store, &ns, "to archive", &[1.0; DIM]);
        assert_eq!(vector_row_count(&store, &id), 1);

        store.archive_memory(&id).unwrap();
        assert_eq!(
            vector_row_count(&store, &id),
            0,
            "archive must delete the vec0 row"
        );
        // The memory row itself survives (soft delete).
        assert!(store
            .get_memory(&id)
            .unwrap()
            .unwrap()
            .archived_at
            .is_some());
    }

    #[test]
    fn supersede_deletes_old_vector_row_and_rolls_back_atomically() {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        let ns = Namespace::Project("hygiene".into());
        let old = insert_vec(&store, &ns, "old", &[1.0; DIM]);
        let new = insert_vec(&store, &ns, "new", &[0.5; DIM]);

        // Failure path FIRST: superseding by a missing id fails the FK and the
        // whole transaction (including the vector DELETE) rolls back.
        let ghost = MemoryId::new();
        assert!(store.supersede(&old, &ghost).is_err());
        assert_eq!(
            vector_row_count(&store, &old),
            1,
            "rolled-back supersede must NOT delete the vector"
        );

        // Success path: old's vector leaves with the same transaction.
        store.supersede(&old, &new).unwrap();
        assert_eq!(
            vector_row_count(&store, &old),
            0,
            "supersede must delete the superseded memory's vec0 row"
        );
        assert_eq!(vector_row_count(&store, &new), 1, "successor vector kept");
    }

    #[test]
    fn update_vector_insert_fallback_lands_in_owning_namespace_partition() {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        let ns = Namespace::Project("fallback".into());
        let other = Namespace::Project("elsewhere".into());

        // Stored WITHOUT an embedding: update_vector takes the INSERT path and
        // must resolve the partition key from the owning memories row.
        let m = MemoryNote::new(
            ns.clone(),
            "vectorless at first".into(),
            MemoryType::Insight,
            5,
        );
        store.insert_memory(&m, None).unwrap();
        let v = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        store.update_vector(&m.id, &v, "det", "v2").unwrap();

        let hits = store.vector_search(&ns, &v, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, m.id, "found in its own namespace partition");
        assert!(
            store.vector_search(&other, &v, 5).unwrap().is_empty(),
            "absent from every other partition"
        );
    }

    /// The reembed-vs-archive race (Phase 1 review): `update_vector` must NOT
    /// resurrect a KNN vector for a memory archived after the reembed job read
    /// its candidate set but before the write reached the single writer. The
    /// stamp UPDATE skips archived rows fail-closed (`NotFound` — the engine
    /// counts it as a per-row skip and the next scan excludes archived rows),
    /// and the fallback INSERT's SELECT is guarded too, preserving the
    /// live-only vec0 partition invariant `vector_search` documents.
    #[test]
    fn update_vector_does_not_resurrect_archived_memory_vector() {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        let ns = Namespace::Project("race".into());
        let id = insert_vec(&store, &ns, "soon archived", &[1.0; DIM]);

        // W1.7: archive deletes the vec0 row in the same transaction...
        store.archive_memory(&id).unwrap();
        assert_eq!(vector_row_count(&store, &id), 0);

        // ...and a reembed write that raced past the candidate scan fails
        // closed instead of re-INSERTing the vector via the fallback path.
        let err = store
            .update_vector(&id, &[0.5; DIM], "det", "v2")
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
        assert_eq!(
            vector_row_count(&store, &id),
            0,
            "update_vector must not resurrect a vec0 row for an archived memory"
        );
        // The whole transaction rolled back: the stamp did not move either.
        let note = store.get_memory(&id).unwrap().unwrap();
        assert_ne!(
            note.embedding_model, "det",
            "the embedding stamp must not be updated on an archived row"
        );
    }

    #[test]
    fn failed_mid_transaction_insert_rolls_back_and_restores_autocommit() {
        // W1.6a: a constraint violation AFTER the transaction opened (the
        // memories INSERT succeeded; the link INSERT hits the FK on a missing
        // target) must roll the whole op back via the RAII guard and leave the
        // connection in autocommit, so the next write runs cleanly.
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        let ns = Namespace::Project("raii".into());

        let mut bad = MemoryNote::new(ns.clone(), "doomed insert".into(), MemoryType::Insight, 5);
        bad.links.push(MemoryLink {
            source_id: bad.id.clone(),
            target_id: MemoryId::new(), // does not exist -> FK violation
            link_type: rb_types::LinkType::References,
            strength: 0.5,
            reason: "ghost target".into(),
            created_at: chrono::Utc::now(),
        });
        let err = store.insert_memory(&bad, Some(&[0.1; DIM])).unwrap_err();
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");

        assert!(
            store.is_autocommit(),
            "the failed op must not leave an open transaction"
        );
        assert!(
            store.get_memory(&bad.id).unwrap().is_none(),
            "the partial memories INSERT must roll back with the failed link"
        );

        // The same connection accepts the next write (no transaction poison).
        let ok = MemoryNote::new(ns, "clean follow-up".into(), MemoryType::Insight, 5);
        store.insert_memory(&ok, Some(&[0.2; DIM])).unwrap();
        assert!(store.get_memory(&ok.id).unwrap().is_some());
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
