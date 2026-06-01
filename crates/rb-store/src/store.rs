//! `SqliteStore`: the concrete `Store` backed by SQLite + sqlite-vec.

use crate::error::{io_err, storage_err};
use crate::migrations::run_migrations;
use rb_types::{
    Error, MemoryId, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace, Result,
};
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
    /// Bump `access_count` and stamp `last_accessed_at = now` for `id`.
    /// A missing id is a no-op (best-effort access tracking never errors on absence).
    fn record_access(&self, id: &MemoryId) -> Result<()>;
    /// Mark `old` as superseded by `new` AND archive `old`, in one transaction.
    /// Fails closed (rolls back) if `new` does not exist (FK on `superseded_by`).
    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()>;
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
        register_vec()?;
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
        register_vec()?;
        let conn = rusqlite::Connection::open_in_memory().map_err(storage_err)?;
        Self::init(conn, embedding_dim)
    }

    /// Shared init path: pragmas, migrations, vectors table, dim invariant.
    fn init(conn: rusqlite::Connection, embedding_dim: usize) -> Result<Self> {
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

        // Dynamic-dimension vector table. vec0 needs the literal dim baked in.
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vectors USING vec0(\
               memory_id TEXT PRIMARY KEY,\
               embedding float[{embedding_dim}]\
             );"
        ))
        .map_err(storage_err)?;

        seed_or_verify_dim(&conn, embedding_dim)?;

        Ok(Self {
            conn,
            embedding_dim,
        })
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
        links,
    })
}

/// Wrap the user query as a single FTS5 phrase so operators (-, OR, NEAR, *, ")
/// are treated as literal text. Internal double-quotes are doubled per FTS5 rules.
fn escape_fts5_query(query: &str) -> String {
    let escaped = query.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

// ---------------------------------------------------------------------------
// Store impl
// ---------------------------------------------------------------------------

impl Store for SqliteStore {
    fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()> {
        // Defense-in-depth validation before touching the DB. The SQL CHECK
        // constraints are the backstop; these give a clean early error.
        if note.importance < 1 || note.importance > 10 {
            return Err(Error::Storage(format!(
                "importance {} is out of range 1..=10",
                note.importance
            )));
        }
        if !(0.0..=1.0).contains(&note.confidence) {
            return Err(Error::Storage(format!(
                "confidence {} is out of range 0.0..=1.0",
                note.confidence
            )));
        }

        // Take the write lock at BEGIN (IMMEDIATE) instead of deferring it to the
        // first write. This avoids a deferred-transaction upgrade racing another
        // writer mid-transaction; the busy_timeout above makes a contended BEGIN
        // wait rather than fail immediately. Atomicity is unchanged: all writes
        // commit together, and any error rolls the whole transaction back.
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|e| Error::Storage(e.to_string()))?;

        let result = (|| -> Result<()> {
            self.conn
                .execute(
                    "INSERT INTO memories (
                        memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18
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
                    ],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;

            if let Some(emb) = embedding {
                self.conn
                    .execute(
                        "INSERT INTO memory_vectors (memory_id, embedding) VALUES (?1, ?2)",
                        rusqlite::params![note.id.to_string(), embedding_bytes(emb)],
                    )
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }

            for link in &note.links {
                self.conn
                    .execute(
                        "INSERT INTO memory_links
                            (source_id, target_id, link_type, strength, reason, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            link.source_id.to_string(),
                            link.target_id.to_string(),
                            link.link_type.as_str(),
                            link.strength as f64,
                            link.reason,
                            ts(link.created_at),
                        ],
                    )
                    .map_err(|e| Error::Storage(e.to_string()))?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT;")
                .map_err(|e| Error::Storage(e.to_string())),
            Err(e) => {
                // Best-effort rollback; surface the original error.
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn get_memory(&self, id: &MemoryId) -> Result<Option<MemoryNote>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model
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

    /// Scale limitation: vec0 KNN cannot filter on namespace/active inside the
    /// query, so we over-fetch a candidate pool of `10 * limit` nearest vectors
    /// and filter by namespace + active state in Rust. If those nearest `10*N`
    /// vectors are dominated by other namespaces or archived rows, FEWER than
    /// `limit` results may be returned. This is acceptable at P0's brute-force
    /// scale; a namespace-aware ANN index (or partitioned vec0 table) is a future
    /// option if recall at the tail becomes a problem.
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

        // Over-fetch: we can't filter by namespace inside vec0's KNN query (adding
        // WHERE on auxiliary columns is not supported in KNN mode). Instead we
        // over-fetch a candidate pool large enough to still yield `limit` in-scope
        // neighbors after filtering, then filter by namespace + active in Rust.
        //
        // Deviation from the plan: the plan used a CTE with `k = ?` plus an outer
        // `LIMIT`. That would cause a sqlite-vec error: "Only LIMIT or 'k =?' can be
        // provided, not both" (the query planner sees both when it pushes the outer
        // LIMIT into the CTE scan). We instead use a single-level query with LIMIT
        // only, then filter candidates in Rust.
        //
        // vec0 returns min(LIMIT, total_rows) without error.
        // sqlite-vec enforces a hard KNN cap of 4096; k_budget must not exceed it.
        const VEC0_KNN_MAX: i64 = 4096;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let k_budget = limit_i64
            .saturating_mul(10)
            .max(limit_i64)
            .min(VEC0_KNN_MAX);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, distance
                 FROM memory_vectors
                 WHERE embedding MATCH ?1
                 ORDER BY distance
                 LIMIT ?2",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![query_json, k_budget], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        // Collect raw candidates, then filter by namespace + active status.
        let ns_str = ns.as_db_string();
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
    /// `SELECT DISTINCT` flattens them — fine at P0's bounded depth.
    fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<MemoryId>> {
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
                 SELECT DISTINCT node
                 FROM walk
                 WHERE node <> ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![id.to_string(), depth as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut ids = Vec::new();
        for r in rows {
            let s = r.map_err(|e| Error::Storage(e.to_string()))?;
            ids.push(s.parse::<MemoryId>()?);
        }
        Ok(ids)
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
                        superseded_by, embedding_model
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
        // touching the DB. (MemoryUpdates has no confidence field.)
        if let Some(imp) = updates.importance {
            if !(1..=10).contains(&imp) {
                return Err(Error::Storage(format!(
                    "importance {imp} is out of range 1..=10"
                )));
            }
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
        }
        if let Some(tags) = &updates.tags {
            sets.push(format!("tags = ?{}", params.len() + 1));
            params.push(Box::new(json_array(tags)?));
        }
        if let Some(context) = &updates.context {
            sets.push(format!("context = ?{}", params.len() + 1));
            params.push(Box::new(context.clone()));
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

        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        self.conn
            .execute(&sql, refs.as_slice())
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    fn archive_memory(&self, id: &MemoryId) -> Result<()> {
        self.conn
            .execute(
                "UPDATE memories
                 SET archived_at = ?1, updated_at = ?1
                 WHERE memory_id = ?2 AND archived_at IS NULL",
                rusqlite::params![chrono::Utc::now().timestamp(), id.to_string()],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    fn add_link(&self, link: &MemoryLink) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO memory_links
                    (source_id, target_id, link_type, strength, reason, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    link.source_id.to_string(),
                    link.target_id.to_string(),
                    link.link_type.as_str(),
                    link.strength as f64,
                    link.reason,
                    link.created_at.timestamp(),
                ],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
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

    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|e| Error::Storage(e.to_string()))?;

        let now = chrono::Utc::now().timestamp();
        let result = (|| -> Result<()> {
            // Point old -> new. FK on superseded_by makes a missing `new` fail here,
            // rolling back the whole transaction (old stays unarchived).
            self.conn
                .execute(
                    "UPDATE memories SET superseded_by = ?1, updated_at = ?2 WHERE memory_id = ?3",
                    rusqlite::params![new.to_string(), now, old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Archive old (idempotent: only if currently active).
            self.conn
                .execute(
                    "UPDATE memories SET archived_at = ?1, updated_at = ?1
                     WHERE memory_id = ?2 AND archived_at IS NULL",
                    rusqlite::params![now, old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            Ok(())
        })();

        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT;")
                .map_err(|e| Error::Storage(e.to_string())),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
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
            matches!(err, Error::Storage(ref s) if s.contains("importance")),
            "expected storage error about importance, got {err:?}"
        );

        // importance = 11 is above the valid range
        m.importance = 11;
        let err = store.insert_memory(&m, None).unwrap_err();
        assert!(
            matches!(err, Error::Storage(ref s) if s.contains("importance")),
            "expected storage error about importance, got {err:?}"
        );
    }

    #[test]
    fn insert_rejects_out_of_range_confidence() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // confidence = -0.1 is below the valid range 0.0..=1.0
        let mut m = MemoryNote::new(Namespace::Global, "bad".into(), MemoryType::Reference, 5);
        m.confidence = -0.1;
        let err = store.insert_memory(&m, None).unwrap_err();
        assert!(
            matches!(err, Error::Storage(ref s) if s.contains("confidence")),
            "expected storage error about confidence, got {err:?}"
        );

        // confidence = 1.1 is above the valid range
        m.confidence = 1.1;
        let err = store.insert_memory(&m, None).unwrap_err();
        assert!(
            matches!(err, Error::Storage(ref s) if s.contains("confidence")),
            "expected storage error about confidence, got {err:?}"
        );
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
    fn escapes_special_query_chars() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "config flag enable-cache value");

        // The '-' would be an FTS5 operator (NOT) if unescaped. After escaping, the
        // query is treated as the literal phrase "enable cache" (unicode61 splits on
        // '-'), which matches the adjacent tokens in the document.
        let found = store.keyword_search(&proj, "enable-cache", 10).unwrap();
        assert_eq!(found, vec![hit.clone()]);

        // A query that is nothing but FTS5 operators must NOT raise a syntax error;
        // escaped, it becomes an empty/operator-free phrase that simply matches nothing.
        let none = store.keyword_search(&proj, "OR AND (", 10).unwrap();
        assert!(none.is_empty());

        // A double-quote is the FTS5 phrase delimiter. Unescaped it would either
        // break the query or let a user inject phrase syntax. escape_fts5_query
        // doubles internal quotes, so this runs safely (no error) and matches
        // nothing here (the document has no literal quote token).
        let quoted = store
            .keyword_search(&proj, "value\" OR config", 10)
            .unwrap();
        assert!(
            quoted.is_empty(),
            "double-quote input must not inject phrase syntax, got {quoted:?}"
        );

        // A bare double-quote alone must also be safe (no panic, no syntax error).
        let lone_quote = store.keyword_search(&proj, "\"", 10).unwrap();
        assert!(lone_quote.is_empty());

        // An asterisk is the FTS5 prefix operator. After escaping (`"enable*"`) the
        // query is a well-formed phrase-with-prefix that FTS5 accepts safely (no
        // syntax error, no injection). It may legitimately match `enable` in the
        // document via prefix; the requirement is only that it runs safely, so we
        // assert it does not error and does not match unrelated rows.
        let star = store.keyword_search(&proj, "enable*", 10).unwrap();
        assert!(
            star == vec![hit.clone()] || star.is_empty(),
            "asterisk input must run safely without injection, got {star:?}"
        );

        // A lone asterisk must also be safe (no panic, no syntax error).
        let lone_star = store.keyword_search(&proj, "*", 10).unwrap();
        assert!(lone_star.is_empty());
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
    fn traverses_up_to_depth() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        let c = node(&store, "c");
        let d = node(&store, "d");
        link(&store, &a, &b); // a -> b
        link(&store, &b, &c); // b -> c
        link(&store, &c, &d); // c -> d

        let mut depth1 = store.graph_neighbors(&a.id, 1).unwrap();
        depth1.sort_by_key(|id| id.to_string());
        let mut want1 = vec![b.id.clone()];
        want1.sort_by_key(|id| id.to_string());
        assert_eq!(depth1, want1);

        let mut depth2 = store.graph_neighbors(&a.id, 2).unwrap();
        depth2.sort_by_key(|id| id.to_string());
        let mut want2 = vec![b.id.clone(), c.id.clone()];
        want2.sort_by_key(|id| id.to_string());
        assert_eq!(depth2, want2);
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
        let mut got = store.graph_neighbors(&a.id, 3).unwrap();
        got.sort_by_key(|id| id.to_string());
        // Neighbors of `a`: b (depth 1). `a` itself is reachable at depth 2 via the
        // back-edge but is excluded by `node <> ?1`. Result is exactly [b].
        let mut want = vec![b.id.clone()];
        want.sort_by_key(|id| id.to_string());
        assert_eq!(got, want, "cycle must terminate with a deduplicated set");
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
        };
        store.update_memory(&m.id, &updates).unwrap();

        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.content, "rewritten unicorn term");
        assert_eq!(got.summary, "new summary");
        assert_eq!(got.importance, 9);
        assert_eq!(got.tags, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(got.context, "new context");
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
                matches!(err, Error::Storage(ref s) if s.contains("importance")),
                "expected storage error about importance for {bad}, got {err:?}"
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
}
