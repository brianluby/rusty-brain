//! The `Store` trait implementation and core CRUD-adjacent inherent methods.

use super::internal::*;
use super::*;
use crate::error::storage_err;

impl SqliteStore {
    /// Insert benchmark fixtures in one transaction per caller-provided batch.
    ///
    /// This is feature-gated because normal writes must continue through the
    /// daemon's single-writer path. The eval harness uses it only to keep
    /// corpus construction outside the timed load phase.
    #[cfg(feature = "bench-utils")]
    #[doc(hidden)]
    pub fn insert_memory_batch_for_benchmark(&self, rows: &[(MemoryNote, Vec<f32>)]) -> Result<()> {
        for (note, _) in rows {
            rb_types::validate_importance(note.importance)?;
            rb_types::validate_confidence(note.confidence)?;
            for anchor in &note.anchors {
                anchor.validate()?;
            }
        }
        immediate_tx(&self.conn, || {
            for (note, embedding) in rows {
                self.insert_memory_tx_body(note, Some(embedding))?;
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

    /// The insert transaction body (memories row + oplog + anchors + vector +
    /// links), shared by [`Store::insert_memory`] (which wraps it in its own
    /// `immediate_tx`) and the atomic review merge (which runs it INSIDE the
    /// merge's single transaction — PRD 2026-07-02 review, atomicity fix).
    /// MUST be called inside an open transaction.
    pub(crate) fn insert_memory_tx_body(
        &self,
        note: &MemoryNote,
        embedding: Option<&[f32]>,
    ) -> Result<()> {
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

        // Typed code anchors (PRD 2026-07-02): kind-split value columns
        // (`path` for file anchors, `ref` for commit/symbol — the 009
        // CHECK pins the split). Values are stored NORMALIZED
        // (rb_types::normalize_anchor_value) so the anchor-filter SQL can
        // compare by plain equality. Exact duplicates (post-normalization,
        // incl. the line range) collapse to ONE row here — repeated CLI
        // flags / `--batch` fan-out must not accumulate copies, and a
        // UNIQUE constraint cannot do it (SQLite treats the NULL
        // path/ref/line columns as pairwise distinct). First-seen order
        // is preserved.
        let mut seen_anchors = std::collections::HashSet::new();
        for anchor in &note.anchors {
            let value = rb_types::normalize_anchor_value(anchor.kind, &anchor.value);
            if !seen_anchors.insert((
                rb_types::anchor_kind_str(anchor.kind),
                value.clone(),
                anchor.start_line,
                anchor.end_line,
            )) {
                continue;
            }
            let is_file = anchor.kind == rb_types::AnchorKind::File;
            self.conn
                .execute(
                    "INSERT INTO memory_anchors
                            (memory_id, namespace, kind, path, start_line, end_line, ref)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        note.id.to_string(),
                        note.namespace.as_db_string(),
                        rb_types::anchor_kind_str(anchor.kind),
                        is_file.then_some(value.as_str()),
                        anchor.start_line.map(i64::from),
                        anchor.end_line.map(i64::from),
                        (!is_file).then_some(value.as_str()),
                    ],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
        }

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
fn json_array(v: &[String]) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Serialization(e.to_string()))
}
fn ts(dt: chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp()
}
fn opt_ts(dt: Option<chrono::DateTime<chrono::Utc>>) -> Option<i64> {
    dt.map(|d| d.timestamp())
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
/// `None` stays `None`; a present-but-out-of-range value propagates the error.
fn from_opt_ts(secs: Option<i64>) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    secs.map(from_ts).transpose()
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
/// Load a memory's typed code anchors (PRD 2026-07-02), decoded from the
/// kind-split columns: `path` carries a file anchor's value, `ref` a
/// commit/symbol's. Ordered by insertion (rowid) so round-trips preserve the
/// caller's anchor order.
fn load_anchors(conn: &rusqlite::Connection, id: &MemoryId) -> Result<Vec<rb_types::MemoryAnchor>> {
    let mut stmt = conn
        .prepare(
            "SELECT kind, path, start_line, end_line, ref
             FROM memory_anchors WHERE memory_id = ?1 ORDER BY id",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut anchors = Vec::new();
    for r in rows {
        let (kind, path, start, end, reference) = r.map_err(|e| Error::Storage(e.to_string()))?;
        let kind = rb_types::parse_anchor_kind(&kind)?;
        let value = path.or(reference).ok_or_else(|| {
            Error::Storage("memory_anchors row carries neither path nor ref".to_string())
        })?;
        let to_line = |v: Option<i64>| -> Result<Option<u32>> {
            v.map(|n| {
                u32::try_from(n)
                    .map_err(|_| Error::Storage("anchor line out of range in DB".to_string()))
            })
            .transpose()
        };
        anchors.push(rb_types::MemoryAnchor {
            kind,
            value,
            start_line: to_line(start)?,
            end_line: to_line(end)?,
        });
    }
    Ok(anchors)
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
    // TODO(P1): batch relation loading — BOTH load_links and load_anchors are
    // per-row queries (N+1 in list/get_many paths) and candidates for a single
    // batched IN-list fetch per page of rows.
    let links = load_links(conn, &id)?;
    let anchors = load_anchors(conn, &id)?;
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
        anchors,
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
/// Build the filtered-list SELECT — the exact SQL + positional params
/// `list_filtered` executes — for `ns`, `filter`, `limit`. Split out so
/// tests can `EXPLAIN QUERY PLAN` the real query (the anchor semi-join
/// index guarantee). Every fragment is a FIXED literal (no caller data is
/// ever interpolated); caller values ride numbered parameters exclusively.
/// Performs the same defense-in-depth `filter.validate()` as the execution
/// path (this also rejects empty anchor-filter values fail-closed).
#[allow(clippy::vec_box)]
fn build_list_filtered_query(
    ns: &Namespace,
    filter: &rb_types::RecallFilter,
    limit: usize,
) -> Result<(String, Vec<Box<dyn rusqlite::ToSql>>)> {
    filter.validate()?;

    let mut sql = String::from(
        "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                keywords, tags, context, memory_type, importance, confidence,
                related_files, access_count, last_accessed_at, archived_at,
                superseded_by, embedding_model, embedding_input_version,
                origin_user, origin_host, origin_agent, origin_source, session_id
         FROM memories m
         WHERE m.namespace = ?1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(ns.as_db_string())];
    fn push(
        sql: &mut String,
        params: &mut Vec<Box<dyn rusqlite::ToSql>>,
        clause: &str,
        value: Box<dyn rusqlite::ToSql>,
    ) {
        params.push(value);
        sql.push_str(" AND ");
        sql.push_str(clause);
        sql.push_str(&format!("?{}", params.len()));
    }

    match filter.state {
        rb_types::MemoryState::Active => sql.push_str(" AND m.archived_at IS NULL"),
        rb_types::MemoryState::Archived => sql.push_str(" AND m.archived_at IS NOT NULL"),
        rb_types::MemoryState::All => {}
    }
    if let Some(min) = filter.min_importance {
        push(
            &mut sql,
            &mut params,
            "m.importance >= ",
            Box::new(min as i64),
        );
    }
    if let Some(max) = filter.max_importance {
        push(
            &mut sql,
            &mut params,
            "m.importance <= ",
            Box::new(max as i64),
        );
    }
    if let Some(min) = filter.min_confidence {
        push(
            &mut sql,
            &mut params,
            "m.confidence >= ",
            Box::new(f64::from(min)),
        );
    }
    if let Some(max) = filter.max_confidence {
        push(
            &mut sql,
            &mut params,
            "m.confidence <= ",
            Box::new(f64::from(max)),
        );
    }
    if let Some(since) = filter.since {
        // `created_at` is stored whole-second, so a FRACTIONAL lower bound
        // rounds UP: flooring 12:00:00.5 would wrongly admit a row created
        // at 12:00:00 (which `RecallFilter::matches` rejects).
        let bound = since.timestamp() + i64::from(since.timestamp_subsec_nanos() > 0);
        push(&mut sql, &mut params, "m.created_at >= ", Box::new(bound));
    }
    if let Some(until) = filter.until {
        // Flooring the UPPER bound is exact for whole-second storage: a
        // stored second <= floor(until) is <= until, and floor(until)+1
        // is > until whenever `until` carries a fraction.
        push(
            &mut sql,
            &mut params,
            "m.created_at <= ",
            Box::new(until.timestamp()),
        );
    }
    if !filter.types.is_empty() {
        // Any-of over the canonical db strings, one placeholder per type.
        let start = params.len() + 1;
        let placeholders: Vec<String> = (0..filter.types.len())
            .map(|i| format!("?{}", start + i))
            .collect();
        sql.push_str(&format!(
            " AND m.memory_type IN ({})",
            placeholders.join(", ")
        ));
        for t in &filter.types {
            params.push(Box::new(t.as_str().to_string()));
        }
    }
    if !filter.sources.is_empty() {
        // Any-of; a NULL origin_source never matches an IN list, so
        // pre-provenance rows are correctly excluded.
        let start = params.len() + 1;
        let placeholders: Vec<String> = (0..filter.sources.len())
            .map(|i| format!("?{}", start + i))
            .collect();
        sql.push_str(&format!(
            " AND m.origin_source IN ({})",
            placeholders.join(", ")
        ));
        for s in &filter.sources {
            params.push(Box::new(s.clone()));
        }
    }
    for tag in &filter.tags {
        // All-of: one EXISTS per required tag over the JSON tags array
        // (json1 ships with the bundled SQLite).
        push(
            &mut sql,
            &mut params,
            "EXISTS (SELECT 1 FROM json_each(m.tags) WHERE json_each.value = ",
            Box::new(tag.clone()),
        );
        sql.push(')');
    }
    for anchor in &filter.anchors {
        // All-of (like tags): one namespace-scoped IN semi-join per
        // anchor constraint, probing the kind-matched value column
        // (`path` for file anchors, `ref` for commit/symbol — the same
        // split insert_memory writes). A semi-join, NOT a correlated
        // EXISTS (PR #59 review, EXPLAIN-verified at 100k rows): the
        // correlated form was always planned as a full `memories` scan
        // with a per-row idx_memory_anchors_memory probe, so a selective
        // anchor filter cost O(active memories); the IN subquery is
        // evaluated once via idx_memory_anchors_path / _ref — O(matching
        // anchors) — which is what the wide indexes exist for (pinned by
        // `anchor_filters_probe_the_wide_anchor_indexes`). Scoping the
        // subquery on `a.namespace` (which mirrors memories.namespace;
        // rename re-keys both) is what makes the leading index column
        // usable and never changes the result set. Values compare by
        // plain equality because BOTH sides are normalized: stored
        // values at insert, the filter value here. Semantics are pinned
        // to `RecallFilter::matches` by the
        // `anchor_filter_agrees_with_recall_filter_matches` drift test.
        let value_col = if anchor.kind == rb_types::AnchorKind::File {
            "path"
        } else {
            "ref"
        };
        params.push(Box::new(ns.as_db_string()));
        let ns_param = params.len();
        params.push(Box::new(rb_types::anchor_kind_str(anchor.kind).to_string()));
        let kind_param = params.len();
        params.push(Box::new(rb_types::normalize_anchor_value(
            anchor.kind,
            &anchor.value,
        )));
        let value_param = params.len();
        sql.push_str(&format!(
            " AND m.memory_id IN (SELECT a.memory_id FROM memory_anchors a
                   WHERE a.namespace = ?{ns_param}
                     AND a.kind = ?{kind_param}
                     AND a.{value_col} = ?{value_param})"
        ));
    }
    if let Some(want_contested) = filter.contested {
        // Contested is resolved INSIDE the bounded query (PR #58 review):
        // post-filtering a fetch window silently drops matches past the
        // window, so `limit` could under-fill despite more matches
        // existing. The predicate is the SQL expression of
        // `active_contradicts` (an active `contradicts` edge whose far
        // endpoint is active AND in-namespace, and whose LOCAL endpoint is
        // active — an archived memory is never contested, so under
        // `contested=false` archived rows count as uncontested). Kept in
        // lockstep by the `contested_filter_agrees_with_active_contradicts`
        // drift test; change one without the other and that test fails.
        params.push(Box::new(ns.as_db_string()));
        let ns_param = params.len();
        let contested_expr = format!(
            "(m.archived_at IS NULL AND (EXISTS (
                     SELECT 1 FROM memory_links l
                       JOIN memories far ON far.memory_id = l.target_id
                     WHERE l.link_type = 'contradicts'
                       AND l.source_id = m.memory_id
                       AND far.archived_at IS NULL
                       AND far.namespace = ?{ns_param}
                 ) OR EXISTS (
                     SELECT 1 FROM memory_links l
                       JOIN memories far ON far.memory_id = l.source_id
                     WHERE l.link_type = 'contradicts'
                       AND l.target_id = m.memory_id
                       AND far.archived_at IS NULL
                       AND far.namespace = ?{ns_param}
                 )))"
        );
        if want_contested {
            sql.push_str(&format!(" AND {contested_expr}"));
        } else {
            sql.push_str(&format!(" AND NOT {contested_expr}"));
        }
    }

    params.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));
    sql.push_str(&format!(
        " ORDER BY m.created_at DESC LIMIT ?{}",
        params.len()
    ));
    Ok((sql, params))
}

impl Store for SqliteStore {
    fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()> {
        // Defense-in-depth validation before touching the DB. The SQL CHECK
        // constraints are the backstop; these give a clean early error.
        rb_types::validate_importance(note.importance)?;
        // W2.8 taxonomy: a range rejection is guidance for the caller, so it
        // must be validation-class (InvalidArgument travels verbatim over the
        // wire; Storage is replaced with an opaque "internal error").
        rb_types::validate_confidence(note.confidence)?;
        // Anchors are validated fail-closed BEFORE the transaction opens (the
        // SQL CHECKs are the backstop); the engine already validated, but the
        // store is its own boundary.
        for anchor in &note.anchors {
            anchor.validate()?;
        }

        // Take the write lock at BEGIN (IMMEDIATE) instead of deferring it to the
        // first write. This avoids a deferred-transaction upgrade racing another
        // writer mid-transaction; the busy_timeout above makes a contended BEGIN
        // wait rather than fail immediately. Atomicity is unchanged: all writes
        // commit together, and any error rolls the whole transaction back.
        immediate_tx(&self.conn, || self.insert_memory_tx_body(note, embedding))
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
        self.keyword_search_in_state(ns, query, limit, rb_types::MemoryState::Active)
    }

    fn keyword_search_in_state(
        &self,
        ns: &Namespace,
        query: &str,
        limit: usize,
        state: rb_types::MemoryState,
    ) -> Result<Vec<MemoryId>> {
        let match_expr = escape_fts5_query(query);
        // The archived predicate is one of three FIXED literals (never caller
        // data), so the composed SQL stays injection-free.
        let archived_predicate = match state {
            rb_types::MemoryState::Active => "AND m.archived_at IS NULL",
            rb_types::MemoryState::Archived => "AND m.archived_at IS NOT NULL",
            rb_types::MemoryState::All => "",
        };
        let sql = format!(
            "SELECT m.memory_id
             FROM memories_fts
             JOIN memories m ON m.rowid = memories_fts.rowid
             WHERE memories_fts MATCH ?1
               AND m.namespace = ?2
               {archived_predicate}
             ORDER BY rank
             LIMIT ?3"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
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
        let filter = rb_types::RecallFilter::default().fold_list_legacy(min_importance);
        self.list_filtered(ns, &filter, limit)
    }

    fn list_filtered(
        &self,
        ns: &Namespace,
        filter: &rb_types::RecallFilter,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        let (sql, params) = build_list_filtered_query(ns, filter, limit)?;
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(refs.as_slice())
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
        // Transaction, vector hygiene (W1.7), and the single `archive` oplog
        // row all live in the shared `archive_with_details` body (also used by
        // the retention sweep, which stamps a cause into `details`). Missing
        // or already-archived ids are Ok no-ops and log nothing.
        archive_with_details(&self.conn, &self.site_id, id, "").map(|_| ())
    }

    fn add_link(&self, link: &MemoryLink) -> Result<()> {
        // Transaction: the link INSERT and its oplog row commit (or roll back)
        // together (an FK failure on a missing endpoint rolls back both).
        //
        // memory_links carries no namespace column, so this accepts edges
        // whose endpoints live in different namespaces without complaint.
        // Isolation is enforced read-side only (e.g. `active_contradicts`
        // requires both endpoints active AND in the query namespace) — a
        // cross-namespace edge written here is silently inert until read
        // paths change. A future feature that walks the full link graph
        // without that namespace filter would inherit a cross-namespace
        // leak from this insert.
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
        // Every guard lives in the shared primitive (#501,
        // `supersede_guarded_in_tx` — the one choke point that makes the
        // pointer graph structurally acyclic); this wrapper only owns the
        // transaction and the general-path error mapping:
        // - self-supersede is a static caller bug (InvalidArgument);
        // - a missing row on either side is a precise NotFound;
        // - an already-resolved old row or a non-current new target means the
        //   caller's plan was formed against a stale view (StalePlan): the
        //   established lineage must never be rewritten, and pointing lineage
        //   at a dead end would fabricate a decision-evolution step on the
        //   W2.2 audit surface.
        let now = chrono::Utc::now().timestamp();
        immediate_tx(&self.conn, || {
            match self.supersede_guarded_in_tx(old, new, now)? {
                SupersedeGuard::Applied => Ok(()),
                SupersedeGuard::SelfSupersede => Err(Error::InvalidArgument(format!(
                    "a memory cannot supersede itself: {old}"
                ))),
                SupersedeGuard::MissingOld => Err(Error::NotFound(old.clone())),
                SupersedeGuard::OldResolved {
                    archived,
                    superseded,
                } => Err(Error::StalePlan(format!(
                    "{old} was already resolved (archived: {archived}, superseded: \
                     {superseded}); re-plan against the current row"
                ))),
                SupersedeGuard::MissingNew => Err(Error::NotFound(new.clone())),
                SupersedeGuard::NewNotCurrent {
                    archived,
                    superseded,
                } => Err(Error::StalePlan(format!(
                    "{new} is no longer current truth (archived: {archived}, superseded: \
                     {superseded}); a supersede must point at an active replacement"
                ))),
            }
        })
    }

    fn record_feedback(
        &self,
        id: &MemoryId,
        kind: rb_types::FeedbackKind,
        principal: Option<&str>,
    ) -> Result<f32> {
        let now = chrono::Utc::now().timestamp();
        // Transaction: the feedback event row, the confidence nudge, and the
        // oplog entry commit (or roll back) together — the usefulness signal,
        // the trust prior, and the durable change log must never disagree.
        immediate_tx(&self.conn, || {
            // Read the current confidence + namespace fail-closed: a missing id
            // is NotFound (the daemon already namespace-checks, but the store
            // does not depend on that). The namespace rides into the feedback
            // row and the oplog entry.
            let row = match self.conn.query_row(
                "SELECT confidence, namespace FROM memories WHERE memory_id = ?1",
                rusqlite::params![id.to_string()],
                |r| Ok((r.get::<_, f64>(0)?, r.get::<_, String>(1)?)),
            ) {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    return Err(Error::NotFound(id.clone()))
                }
                Err(e) => return Err(Error::Storage(e.to_string())),
            };
            let (current, namespace) = row;
            // Single-axis confidence coupling (W3.7): clamp to the canonical
            // 0.0..=1.0 range so one nudge can never push the prior out of band.
            let new_confidence = ((current as f32) + kind.confidence_delta()).clamp(0.0, 1.0);
            self.conn
                .execute(
                    "UPDATE memories SET confidence = ?1 WHERE memory_id = ?2",
                    rusqlite::params![new_confidence as f64, id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            self.conn
                .execute(
                    "INSERT INTO memory_feedback (memory_id, namespace, kind, principal, at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id.to_string(), namespace, kind.as_str(), principal, now],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Durable oplog entry: the kind + resulting confidence ride in
            // `details` so a replay/audit can reconstruct the nudge.
            let details = serde_json::json!({
                "kind": kind.as_str(),
                "confidence": new_confidence,
            })
            .to_string();
            append_oplog(&self.conn, &self.site_id, "feedback", id, &details)?;
            Ok(new_confidence)
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

/// Outcome of the guarded supersede pointer-take (#501). The guards are
/// shared by every supersede path; each caller owns the error mapping (the
/// review-merge path wants its keyed StalePlan message, the general path
/// disambiguates NotFound), so the guard reports data, not errors. One
/// distinct variant per guard: callers match exhaustively, so adding a guard
/// forces every caller to reconsider its mapping.
pub(crate) enum SupersedeGuard {
    /// Pointer taken, old row archived, vector pruned, oplog row appended.
    Applied,
    /// `old == new`: statically invalid, nothing was written.
    SelfSupersede,
    /// The old row does not exist.
    MissingOld,
    /// The old row was already resolved (archived and/or superseded): the
    /// guarded UPDATE refused and nothing was written.
    OldResolved { archived: bool, superseded: bool },
    /// The new target does not exist.
    MissingNew,
    /// The new target is not current truth (archived and/or superseded):
    /// refused before the pointer UPDATE, nothing was written.
    NewNotCurrent { archived: bool, superseded: bool },
}

impl SqliteStore {
    /// The guarded supersede transaction body (#501, generalizing the PR #63
    /// review-merge guard) — the ONE choke point that structurally enforces
    /// acyclicity, with no reliance on caller invariants:
    ///
    /// 1. `old != new` (a self-loop is never a replacement);
    /// 2. `new` is CURRENT TRUTH — present, active, not itself superseded —
    ///    so the last edge of any would-be cycle, which necessarily targets
    ///    an already-superseded row, is refused here;
    /// 3. the pointer UPDATE (pointer + archive + `updated_at` in ONE
    ///    statement, `WHERE superseded_by IS NULL AND archived_at IS NULL`)
    ///    with a rows-affected check makes `superseded_by` set-once, so
    ///    lineage can never be silently rewritten.
    ///
    /// On `Applied` the vector is pruned (W1.7) and the `supersede` oplog row
    /// appended in the same transaction. MUST run inside an open transaction;
    /// callers map every non-`Applied` outcome to their own error so the
    /// whole transaction rolls back. The `superseded_by` FK stays as
    /// belt-and-suspenders behind the explicit `new`-side check.
    pub(crate) fn supersede_guarded_in_tx(
        &self,
        old: &MemoryId,
        new: &MemoryId,
        now_ts: i64,
    ) -> Result<SupersedeGuard> {
        if old == new {
            return Ok(SupersedeGuard::SelfSupersede);
        }
        let new_row: Option<(bool, bool)> = match self.conn.query_row(
            "SELECT archived_at IS NOT NULL, superseded_by IS NOT NULL
             FROM memories WHERE memory_id = ?1",
            rusqlite::params![new.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(Error::Storage(e.to_string())),
        };
        match new_row {
            None => return Ok(SupersedeGuard::MissingNew),
            Some((false, false)) => {}
            Some((archived, superseded)) => {
                return Ok(SupersedeGuard::NewNotCurrent {
                    archived,
                    superseded,
                })
            }
        }
        let affected = self
            .conn
            .execute(
                "UPDATE memories
                 SET superseded_by = ?1, archived_at = ?2, updated_at = ?2
                 WHERE memory_id = ?3 AND superseded_by IS NULL AND archived_at IS NULL",
                rusqlite::params![new.to_string(), now_ts, old.to_string()],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        if affected == 0 {
            // Disambiguate the guard miss: a missing row vs one already
            // resolved by a concurrent/prior action.
            let row: Option<(bool, bool)> = match self.conn.query_row(
                "SELECT archived_at IS NOT NULL, superseded_by IS NOT NULL
                 FROM memories WHERE memory_id = ?1",
                rusqlite::params![old.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ) {
                Ok(v) => Some(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(Error::Storage(e.to_string())),
            };
            return Ok(match row {
                None => SupersedeGuard::MissingOld,
                Some((archived, superseded)) => SupersedeGuard::OldResolved {
                    archived,
                    superseded,
                },
            });
        }
        // Vector hygiene (W1.7): the superseded (now archived) memory's
        // vector leaves the KNN index in the same transaction. Idempotent:
        // a vectorless row deletes nothing.
        self.conn
            .execute(
                "DELETE FROM memory_vectors WHERE memory_id = ?1",
                rusqlite::params![old.to_string()],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        // One `supersede` oplog row covers the whole compound mutation
        // (pointer + archive); the replacement id rides in `details`.
        let details = serde_json::json!({ "new": new.to_string() }).to_string();
        append_oplog(&self.conn, &self.site_id, "supersede", old, &details)?;
        Ok(SupersedeGuard::Applied)
    }
}
#[cfg(test)]
mod delete_link_tests {
    use super::*;

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

    #[cfg(feature = "bench-utils")]
    #[test]
    fn benchmark_batch_loader_commits_memory_and_vectors_together() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let namespace = Namespace::Project("bench-batch".into());
        let rows: Vec<_> = (0..3)
            .map(|index| {
                (
                    MemoryNote::new(
                        namespace.clone(),
                        format!("fixture {index}"),
                        MemoryType::Insight,
                        5,
                    ),
                    vec8(index as f32),
                )
            })
            .collect();

        store.insert_memory_batch_for_benchmark(&rows).unwrap();

        assert_eq!(store.list(&namespace, None, 10).unwrap().len(), 3);
        assert_eq!(
            store
                .vector_search(&namespace, rows[0].1.as_slice(), 10)
                .unwrap()
                .len(),
            3
        );
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
mod list_filtered_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryState, MemoryType, Namespace, RecallFilter};

    fn ns() -> Namespace {
        Namespace::Project("filter".into())
    }

    fn insert(store: &SqliteStore, f: impl FnOnce(&mut MemoryNote)) -> rb_types::MemoryId {
        let mut m = MemoryNote::new(ns(), "filterable content".into(), MemoryType::Insight, 5);
        f(&mut m);
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
        id
    }

    fn ids(notes: &[MemoryNote]) -> Vec<rb_types::MemoryId> {
        notes.iter().map(|m| m.id.clone()).collect()
    }

    #[test]
    fn state_scopes_archived_rows() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let active = insert(&store, |_| {});
        let archived = insert(&store, |m| {
            m.created_at -= chrono::Duration::seconds(10);
        });
        store.archive_memory(&archived).unwrap();

        let default_scope = store
            .list_filtered(&ns(), &RecallFilter::default(), 10)
            .unwrap();
        assert_eq!(ids(&default_scope), vec![active.clone()]);

        let archived_scope = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    state: MemoryState::Archived,
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&archived_scope), vec![archived.clone()]);

        let all_scope = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    state: MemoryState::All,
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&all_scope), vec![active, archived]);
    }

    #[test]
    fn filters_by_importance_and_confidence_ranges() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let _low = insert(&store, |m| {
            m.importance = 2;
            m.confidence = 0.2;
        });
        let mid = insert(&store, |m| {
            m.importance = 5;
            m.confidence = 0.6;
        });
        let _high = insert(&store, |m| {
            m.importance = 9;
            m.confidence = 0.95;
        });

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    min_importance: Some(4),
                    max_importance: Some(6),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![mid.clone()]);

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    min_confidence: Some(0.5),
                    max_confidence: Some(0.8),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![mid]);
    }

    #[test]
    fn filters_by_created_at_window() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let t0 = chrono::Utc::now();
        let _old = insert(&store, |m| m.created_at = t0 - chrono::Duration::days(10));
        let recent = insert(&store, |m| m.created_at = t0 - chrono::Duration::days(2));
        let _newest = insert(&store, |m| m.created_at = t0);

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    since: Some(t0 - chrono::Duration::days(5)),
                    until: Some(t0 - chrono::Duration::days(1)),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![recent]);
    }

    #[test]
    fn filters_by_types_and_sources() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let bug = insert(&store, |m| {
            m.memory_type = MemoryType::BugFix;
            m.origin_source = Some("hook".into());
        });
        let _insight = insert(&store, |m| {
            m.origin_source = Some("cli".into());
            m.created_at -= chrono::Duration::seconds(5);
        });
        let constraint = insert(&store, |m| {
            m.memory_type = MemoryType::Constraint;
            // No provenance: must never match a source constraint.
            m.origin_source = None;
            m.created_at -= chrono::Duration::seconds(10);
        });

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    types: vec![MemoryType::BugFix, MemoryType::Constraint],
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![bug.clone(), constraint]);

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    sources: vec!["hook".into(), "mcp".into()],
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![bug]);
    }

    #[test]
    fn requires_every_tag() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let both = insert(&store, |m| m.tags = vec!["a".into(), "b".into()]);
        let _only_a = insert(&store, |m| {
            m.tags = vec!["a".into()];
            m.created_at -= chrono::Duration::seconds(5);
        });
        let _untagged = insert(&store, |m| {
            m.created_at -= chrono::Duration::seconds(10);
        });

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    tags: vec!["a".into(), "b".into()],
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![both]);
    }

    #[test]
    fn composes_dimensions_and_respects_limit_and_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let hit = insert(&store, |m| {
            m.importance = 8;
            m.origin_source = Some("hook".into());
            m.tags = vec!["x".into()];
        });
        // Fails the importance leg.
        let _low = insert(&store, |m| {
            m.importance = 3;
            m.origin_source = Some("hook".into());
            m.tags = vec!["x".into()];
            m.created_at -= chrono::Duration::seconds(5);
        });
        // Fails the source leg.
        let _cli = insert(&store, |m| {
            m.importance = 8;
            m.origin_source = Some("cli".into());
            m.tags = vec!["x".into()];
            m.created_at -= chrono::Duration::seconds(10);
        });
        // Out-of-namespace row never surfaces.
        let mut foreign =
            MemoryNote::new(Namespace::Global, "foreign".into(), MemoryType::Insight, 8);
        foreign.origin_source = Some("hook".into());
        foreign.tags = vec!["x".into()];
        store.insert_memory(&foreign, None).unwrap();

        let filter = RecallFilter {
            min_importance: Some(7),
            sources: vec!["hook".into()],
            tags: vec!["x".into()],
            ..Default::default()
        };
        let got = store.list_filtered(&ns(), &filter, 10).unwrap();
        assert_eq!(ids(&got), vec![hit.clone()]);

        // LIMIT still applies under a filter.
        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    tags: vec!["x".into()],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        assert_eq!(ids(&got), vec![hit]);
    }

    #[test]
    fn unfiltered_list_filtered_matches_legacy_list() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = insert(&store, |_| {});
        let b = insert(&store, |m| {
            m.importance = 8;
            m.created_at -= chrono::Duration::seconds(5);
        });

        let legacy = store.list(&ns(), None, 10).unwrap();
        let unified = store
            .list_filtered(&ns(), &RecallFilter::default(), 10)
            .unwrap();
        assert_eq!(ids(&legacy), ids(&unified));
        assert_eq!(ids(&unified), vec![a, b.clone()]);

        let legacy_min = store.list(&ns(), Some(7), 10).unwrap();
        let unified_min = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    min_importance: Some(7),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(ids(&legacy_min), ids(&unified_min));
        assert_eq!(ids(&unified_min), vec![b]);
    }

    fn contradict(store: &SqliteStore, a: &rb_types::MemoryId, b: &rb_types::MemoryId) {
        store
            .add_link(&rb_types::MemoryLink {
                source_id: a.clone(),
                target_id: b.clone(),
                link_type: rb_types::LinkType::Contradicts,
                strength: 1.0,
                reason: "test contradiction".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
    }

    #[test]
    fn contested_filter_fills_limit_beyond_any_bounded_window() {
        // Regression (PR #58 review): contested filtering must be resolved
        // INSIDE the bounded query, not by post-filtering a fetch window — 20
        // uncontested rows are newer than the 4 contested ones, so any
        // "over-fetch 4x limit then retain" scheme would return NOTHING here.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut uncontested = Vec::new();
        for i in 0..20 {
            uncontested.push(insert(&store, |m| {
                m.created_at -= chrono::Duration::seconds(i);
            }));
        }
        let mut contested = Vec::new();
        for i in 0..4 {
            contested.push(insert(&store, |m| {
                m.created_at -= chrono::Duration::seconds(100 + i);
            }));
        }
        contradict(&store, &contested[0], &contested[1]);
        contradict(&store, &contested[2], &contested[3]);

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    contested: Some(true),
                    ..Default::default()
                },
                2,
            )
            .unwrap();
        assert_eq!(
            ids(&got),
            vec![contested[0].clone(), contested[1].clone()],
            "limit must fill with contested matches even past a 4x window"
        );

        let got = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    contested: Some(false),
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        assert_eq!(ids(&got), uncontested, "contested=false keeps the others");
    }

    #[test]
    fn contested_filter_agrees_with_active_contradicts() {
        // Drift guard: the SQL contested predicate and `active_contradicts`
        // are two expressions of ONE semantics (active contradicts edge, both
        // endpoints active + in-namespace). If either changes alone, this
        // fails. The archived pair exercises the far-endpoint-active rule; the
        // cross-namespace pair exercises namespace purity.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = insert(&store, |_| {});
        let b = insert(&store, |m| m.created_at -= chrono::Duration::seconds(1));
        let c = insert(&store, |m| m.created_at -= chrono::Duration::seconds(2));
        let gone = insert(&store, |m| m.created_at -= chrono::Duration::seconds(3));
        let mut foreign =
            MemoryNote::new(Namespace::Global, "foreign".into(), MemoryType::Insight, 5);
        let foreign_id = foreign.id.clone();
        foreign.created_at -= chrono::Duration::seconds(4);
        store.insert_memory(&foreign, None).unwrap();

        contradict(&store, &a, &b); // both active, in-ns -> contested
        contradict(&store, &c, &gone); // far endpoint archived -> NOT contested
        contradict(&store, &c, &foreign_id); // far endpoint out-of-ns -> NOT contested
        store.archive_memory(&gone).unwrap();

        let all_ids = vec![a.clone(), b.clone(), c.clone(), gone.clone()];
        let expected = store.active_contradicts(&ns(), &all_ids).unwrap();
        assert_eq!(
            expected,
            [a.clone(), b.clone()].into_iter().collect(),
            "precondition: active_contradicts flags exactly a and b"
        );

        let contested_rows = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    contested: Some(true),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        let got: std::collections::HashSet<_> = ids(&contested_rows).into_iter().collect();
        assert_eq!(
            got, expected,
            "SQL predicate must agree with active_contradicts"
        );

        let uncontested_rows = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    contested: Some(false),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(
            ids(&uncontested_rows),
            vec![c.clone()],
            "uncontested = active rows minus the contested set"
        );
    }

    #[test]
    fn fractional_since_bound_excludes_the_floored_second() {
        // Regression (PR #58 review): created_at is stored whole-second, so a
        // fractional `since` must round UP — flooring 12:00:00.5 to 12:00:00
        // would wrongly admit a row created at 12:00:00 (disagreeing with
        // RecallFilter::matches). A fractional `until` floors EXACTLY right
        // for whole-second storage, pinned here too.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let whole = chrono::DateTime::from_timestamp(1_780_000_000, 0).unwrap();
        let row = insert(&store, |m| m.created_at = whole);
        let fractional = whole + chrono::Duration::milliseconds(500);

        let since_hits = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    since: Some(fractional),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert!(
            since_hits.is_empty(),
            "a row at 12:00:00 is BEFORE since=12:00:00.5 and must not match"
        );

        let until_hits = store
            .list_filtered(
                &ns(),
                &RecallFilter {
                    until: Some(fractional),
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        assert_eq!(
            ids(&until_hits),
            vec![row],
            "a row at 12:00:00 is BEFORE until=12:00:00.5 and must match"
        );
    }

    fn anchor_only(kind: rb_types::AnchorKind, value: &str) -> RecallFilter {
        RecallFilter {
            anchors: vec![rb_types::AnchorFilter {
                kind,
                value: value.to_string(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn anchors_round_trip_through_insert_get_and_get_many() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let anchors = vec![
            rb_types::MemoryAnchor::parse_file_spec("src/server.rs:12-40").unwrap(),
            rb_types::MemoryAnchor::parse_file_spec("src/lib.rs").unwrap(),
            rb_types::MemoryAnchor::new(rb_types::AnchorKind::Commit, "abc123").unwrap(),
            rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Engine::recall").unwrap(),
        ];
        let with = insert(&store, |m| m.anchors = anchors.clone());
        let without = insert(&store, |m| m.created_at -= chrono::Duration::seconds(1));

        let got = store.get_memory(&with).unwrap().unwrap();
        assert_eq!(got.anchors, anchors, "get must load anchors with the row");
        let bare = store.get_memory(&without).unwrap().unwrap();
        assert!(
            bare.anchors.is_empty(),
            "pre-anchor rows load an empty list"
        );

        let many = store
            .get_many(&ns(), &[with.clone(), without.clone()])
            .unwrap();
        assert_eq!(many[0].anchors, anchors, "get_many loads anchors too");
        assert!(many[1].anchors.is_empty());
    }

    #[test]
    fn insert_rejects_invalid_anchors_fail_closed() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(ns(), "bad anchor".into(), MemoryType::Insight, 5);
        m.anchors = vec![rb_types::MemoryAnchor {
            kind: rb_types::AnchorKind::File,
            value: String::new(),
            start_line: None,
            end_line: None,
        }];
        let err = store.insert_memory(&m, None).unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
        assert!(
            store.get_memory(&m.id).unwrap().is_none(),
            "a rejected insert must write nothing"
        );
    }

    #[test]
    fn anchor_filters_scope_by_kind_and_value() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let on_server = insert(&store, |m| {
            m.anchors = vec![rb_types::MemoryAnchor::parse_file_spec("src/server.rs").unwrap()];
        });
        let on_commit = insert(&store, |m| {
            m.created_at -= chrono::Duration::seconds(1);
            m.anchors =
                vec![rb_types::MemoryAnchor::new(rb_types::AnchorKind::Commit, "abc123").unwrap()];
        });
        let on_symbol = insert(&store, |m| {
            m.created_at -= chrono::Duration::seconds(2);
            m.anchors =
                vec![
                    rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Engine::recall")
                        .unwrap(),
                ];
        });
        let _bare = insert(&store, |m| m.created_at -= chrono::Duration::seconds(3));

        // The PRD acceptance criterion: present under its own file, absent
        // under a different file.
        let hits = store
            .list_filtered(
                &ns(),
                &anchor_only(rb_types::AnchorKind::File, "src/server.rs"),
                10,
            )
            .unwrap();
        assert_eq!(ids(&hits), vec![on_server.clone()]);
        let miss = store
            .list_filtered(
                &ns(),
                &anchor_only(rb_types::AnchorKind::File, "src/other.rs"),
                10,
            )
            .unwrap();
        assert!(miss.is_empty());

        let hits = store
            .list_filtered(
                &ns(),
                &anchor_only(rb_types::AnchorKind::Commit, "abc123"),
                10,
            )
            .unwrap();
        assert_eq!(ids(&hits), vec![on_commit.clone()]);

        let hits = store
            .list_filtered(
                &ns(),
                &anchor_only(rb_types::AnchorKind::Symbol, "Engine::recall"),
                10,
            )
            .unwrap();
        assert_eq!(ids(&hits), vec![on_symbol.clone()]);

        // Kinds never cross-match on an equal value.
        let cross = store
            .list_filtered(
                &ns(),
                &anchor_only(rb_types::AnchorKind::Symbol, "abc123"),
                10,
            )
            .unwrap();
        assert!(cross.is_empty(), "commit value must not match as a symbol");
    }

    #[test]
    fn anchor_filter_normalizes_values_and_ignores_line_ranges() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // Stored WITH a line range and a ./ prefix in the filter: still one
        // path-level anchor (normalization on both sides; v1 matches by path).
        let ranged = insert(&store, |m| {
            m.anchors = vec![rb_types::MemoryAnchor::parse_file_spec("src/a.rs:12-40").unwrap()];
        });
        let hits = store
            .list_filtered(
                &ns(),
                &anchor_only(rb_types::AnchorKind::File, "./src/a.rs"),
                10,
            )
            .unwrap();
        assert_eq!(ids(&hits), vec![ranged]);
    }

    #[test]
    fn anchor_filters_compose_all_of_and_with_metadata() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let both = insert(&store, |m| {
            m.importance = 8;
            m.tags = vec!["infra".to_string()];
            m.anchors = vec![
                rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap(),
                rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Foo::bar").unwrap(),
            ];
        });
        let file_only = insert(&store, |m| {
            m.created_at -= chrono::Duration::seconds(1);
            m.importance = 8;
            m.tags = vec!["infra".to_string()];
            m.anchors = vec![rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap()];
        });
        let _low = insert(&store, |m| {
            m.created_at -= chrono::Duration::seconds(2);
            m.importance = 2;
            m.anchors = vec![
                rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap(),
                rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Foo::bar").unwrap(),
            ];
        });

        // All-of over multiple anchor filters.
        let filter = RecallFilter {
            anchors: vec![
                rb_types::AnchorFilter {
                    kind: rb_types::AnchorKind::File,
                    value: "src/a.rs".to_string(),
                },
                rb_types::AnchorFilter {
                    kind: rb_types::AnchorKind::Symbol,
                    value: "Foo::bar".to_string(),
                },
            ],
            ..Default::default()
        };
        let hits = store.list_filtered(&ns(), &filter, 10).unwrap();
        assert_eq!(
            ids(&hits),
            vec![both.clone(), _low.clone()],
            "every anchor filter must match (all-of)"
        );

        // Anchors compose with --type/--tags/--min-importance (PRD acceptance).
        let composed = RecallFilter {
            types: vec![MemoryType::Insight],
            tags: vec!["infra".to_string()],
            min_importance: Some(7),
            anchors: vec![rb_types::AnchorFilter {
                kind: rb_types::AnchorKind::File,
                value: "src/a.rs".to_string(),
            }],
            ..Default::default()
        };
        let hits = store.list_filtered(&ns(), &composed, 10).unwrap();
        assert_eq!(ids(&hits), vec![both, file_only]);
    }

    #[test]
    fn anchor_filter_rejects_empty_values_fail_closed() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let err = store
            .list_filtered(&ns(), &anchor_only(rb_types::AnchorKind::File, "  "), 10)
            .unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
    }

    #[test]
    fn anchor_filters_probe_the_wide_anchor_indexes() {
        // Reviewer finding (PR #59, verified with EXPLAIN QUERY PLAN at 100k
        // rows): the original correlated-EXISTS anchor probe was ALWAYS
        // planned as a full `memories` SCAN plus a per-row
        // idx_memory_anchors_memory lookup — the wide (namespace, kind,
        // path|ref) indexes were never chosen, so a selective anchor filter
        // cost O(active memories), not O(matching anchors). The filter is a
        // namespace-scoped IN semi-join precisely so those indexes drive it;
        // this test EXPLAINs the EXACT query `list_filtered` executes and
        // pins the index choice for every anchor kind.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let _ = insert(&store, |m| {
            m.anchors = vec![rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap()];
        });
        for (filter, index) in [
            (
                anchor_only(rb_types::AnchorKind::File, "src/a.rs"),
                "idx_memory_anchors_path",
            ),
            (
                anchor_only(rb_types::AnchorKind::Commit, "abc123"),
                "idx_memory_anchors_ref",
            ),
            (
                anchor_only(rb_types::AnchorKind::Symbol, "Foo::bar"),
                "idx_memory_anchors_ref",
            ),
        ] {
            let (sql, params) = build_list_filtered_query(&ns(), &filter, 10).unwrap();
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let mut stmt = store
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let plan: Vec<String> = stmt
                .query_map(refs.as_slice(), |row| row.get::<_, String>(3))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            let plan = plan.join("\n");
            assert!(
                plan.contains(index),
                "the anchor subquery must be served by {index}; plan:\n{plan}"
            );
        }
    }

    #[test]
    fn duplicate_anchors_collapse_to_one_row() {
        // Repeated identical anchors (repeated CLI flags, `--batch` fan-out)
        // must not accumulate duplicate rows. Dedup is normalization-aware
        // (`./src/a.rs` == `src/a.rs`) and exact otherwise: a different line
        // range is a DISTINCT anchor and is kept.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ranged = rb_types::MemoryAnchor::parse_file_spec("src/a.rs:3-9").unwrap();
        // Same anchor spelled with a `./` prefix, bypassing parse-time
        // normalization (a raw wire payload could carry this).
        let ranged_dotted = rb_types::MemoryAnchor {
            kind: rb_types::AnchorKind::File,
            value: "./src/a.rs".to_string(),
            start_line: Some(3),
            end_line: Some(9),
        };
        let rangeless = rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap();
        let id = insert(&store, |m| {
            m.anchors = vec![
                ranged.clone(),
                ranged_dotted,
                ranged.clone(),
                rangeless.clone(),
            ];
        });
        let got = store.get_memory(&id).unwrap().unwrap();
        assert_eq!(
            got.anchors,
            vec![ranged, rangeless],
            "exact duplicates (post-normalization) collapse; distinct ranges stay"
        );
    }

    #[test]
    fn anchor_filter_agrees_with_recall_filter_matches() {
        // Drift guard (the `contested_filter_agrees_with_active_contradicts`
        // pattern): the SQL anchor predicate in `list_filtered` and
        // `RecallFilter::matches` are two expressions of ONE semantics
        // (all-of, kind-scoped, normalized-value equality, path-level for
        // files). If either changes alone, this fails.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut inserted = Vec::new();
        let variants: Vec<Vec<rb_types::MemoryAnchor>> = vec![
            vec![],
            vec![rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap()],
            vec![rb_types::MemoryAnchor::parse_file_spec("./src/a.rs:3-9").unwrap()],
            vec![rb_types::MemoryAnchor::parse_file_spec("src/b.rs").unwrap()],
            vec![
                rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap(),
                rb_types::MemoryAnchor::new(rb_types::AnchorKind::Commit, "abc123").unwrap(),
            ],
            vec![rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "abc123").unwrap()],
        ];
        for (i, anchors) in variants.into_iter().enumerate() {
            let mut m = MemoryNote::new(ns(), format!("variant {i}"), MemoryType::Insight, 5);
            m.created_at -= chrono::Duration::seconds(i as i64);
            m.anchors = anchors;
            store.insert_memory(&m, None).unwrap();
            inserted.push(m);
        }

        let filters = [
            anchor_only(rb_types::AnchorKind::File, "src/a.rs"),
            anchor_only(rb_types::AnchorKind::File, "./src/a.rs"),
            anchor_only(rb_types::AnchorKind::Commit, "abc123"),
            anchor_only(rb_types::AnchorKind::Symbol, "abc123"),
            RecallFilter {
                anchors: vec![
                    rb_types::AnchorFilter {
                        kind: rb_types::AnchorKind::File,
                        value: "src/a.rs".to_string(),
                    },
                    rb_types::AnchorFilter {
                        kind: rb_types::AnchorKind::Commit,
                        value: "abc123".to_string(),
                    },
                ],
                ..Default::default()
            },
        ];
        for filter in filters {
            let sql_ids: std::collections::HashSet<_> =
                ids(&store.list_filtered(&ns(), &filter, 100).unwrap())
                    .into_iter()
                    .collect();
            let matches_ids: std::collections::HashSet<_> = inserted
                .iter()
                .filter(|m| filter.matches(m))
                .map(|m| m.id.clone())
                .collect();
            assert_eq!(
                sql_ids, matches_ids,
                "SQL anchor predicate must agree with RecallFilter::matches for {filter:?}"
            );
        }
    }

    #[test]
    fn keyword_search_in_state_scopes_archived_rows() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let active = insert(&store, |m| m.content = "tokio runtime decision".into());
        let archived = insert(&store, |m| m.content = "tokio executor decision".into());
        store.archive_memory(&archived).unwrap();

        let default_scope = store
            .keyword_search_in_state(&ns(), "tokio", 10, MemoryState::Active)
            .unwrap();
        assert_eq!(default_scope, vec![active.clone()]);
        // The legacy method stays active-only.
        assert_eq!(
            store.keyword_search(&ns(), "tokio", 10).unwrap(),
            default_scope
        );

        let archived_scope = store
            .keyword_search_in_state(&ns(), "tokio", 10, MemoryState::Archived)
            .unwrap();
        assert_eq!(archived_scope, vec![archived.clone()]);

        let mut all_scope = store
            .keyword_search_in_state(&ns(), "tokio", 10, MemoryState::All)
            .unwrap();
        all_scope.sort_by_key(std::string::ToString::to_string);
        let mut expected = vec![active, archived];
        expected.sort_by_key(std::string::ToString::to_string);
        assert_eq!(all_scope, expected);
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
    fn record_feedback_logs_event_moves_confidence_and_appends_oplog() {
        use rb_types::FeedbackKind;
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(ns.clone(), "body".into(), MemoryType::Insight, 5);
        m.confidence = 0.5;
        store.insert_memory(&m, None).unwrap();

        // `wrong` lowers confidence by the bounded delta and returns the result.
        let after = store
            .record_feedback(&m.id, FeedbackKind::Wrong, Some("alice"))
            .unwrap();
        assert!((after - 0.2).abs() < 1e-6, "0.5 - 0.30 = 0.20, got {after}");
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert!(
            (got.confidence - 0.2).abs() < 1e-6,
            "row reflects the nudge"
        );

        // `helpful` raises it back up by its (smaller) delta.
        let after2 = store
            .record_feedback(&m.id, FeedbackKind::Helpful, None)
            .unwrap();
        assert!(
            (after2 - 0.25).abs() < 1e-6,
            "0.20 + 0.05 = 0.25, got {after2}"
        );

        // Two event rows recorded, with kind + principal preserved (NULL when none).
        let rows: Vec<(String, Option<String>, String)> = {
            let mut stmt = store
                .conn
                .prepare(
                    "SELECT kind, principal, namespace FROM memory_feedback
                     WHERE memory_id = ?1 ORDER BY id",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![m.id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            ("wrong".into(), Some("alice".into()), ns.as_db_string())
        );
        assert_eq!(rows[1], ("helpful".into(), None, ns.as_db_string()));

        // Each feedback event appended exactly one `feedback` oplog row.
        let oplog_feedback: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'feedback' AND memory_id = ?1",
                rusqlite::params![m.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(oplog_feedback, 2, "one oplog row per feedback event");
    }

    #[test]
    fn record_feedback_clamps_confidence_to_the_canonical_range() {
        use rb_types::FeedbackKind;
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(ns, "body".into(), MemoryType::Insight, 5);
        m.confidence = 1.0;
        store.insert_memory(&m, None).unwrap();

        // Many `wrong` reports floor at 0.0, never below.
        let mut last = 1.0;
        for _ in 0..10 {
            last = store
                .record_feedback(&m.id, FeedbackKind::Wrong, None)
                .unwrap();
        }
        assert!(
            (last - 0.0).abs() < 1e-6,
            "wrong feedback floors at 0.0, got {last}"
        );

        // Many `helpful` reports cap at 1.0, never above.
        let mut hi = 0.0;
        for _ in 0..40 {
            hi = store
                .record_feedback(&m.id, FeedbackKind::Helpful, None)
                .unwrap();
        }
        assert!(
            (hi - 1.0).abs() < 1e-6,
            "helpful feedback caps at 1.0, got {hi}"
        );
    }

    #[test]
    fn record_feedback_missing_id_is_not_found_and_writes_nothing() {
        use rb_types::FeedbackKind;
        let store = SqliteStore::open_in_memory(8).unwrap();
        let missing = MemoryId::new();
        let err = store
            .record_feedback(&missing, FeedbackKind::Helpful, None)
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");

        // Fail-closed: the rolled-back transaction left no event row behind.
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_feedback", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a NotFound feedback inserts no row");
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
    fn supersede_missing_new_target_is_not_found() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old");
        // The current-truth check on `new` loads the row first, so a missing
        // target fails closed as NotFound (client-safe and precise) BEFORE any
        // mutation; the FK on superseded_by stays as belt-and-suspenders. The
        // old note is unchanged (nothing was written).
        let missing_new = MemoryId::new();
        let err = store.supersede(&old.id, &missing_new).unwrap_err();
        assert!(
            matches!(err, Error::NotFound(ref id) if *id == missing_new),
            "got {err:?}"
        );
        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert!(got.superseded_by.is_none(), "rolled back: no superseded_by");
        assert!(got.archived_at.is_none(), "rolled back: not archived");
    }

    #[test]
    fn supersede_rejects_self_supersede() {
        // #501 guard 1: old == new is statically invalid (a self-loop can
        // never be a "replacement as current truth") and must be refused
        // before any SQL runs — a distinct validation-class error.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "self target");

        let err = store.supersede(&a.id, &a.id).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)), "got {err:?}");

        let got = store.get_memory(&a.id).unwrap().unwrap();
        assert!(got.superseded_by.is_none(), "row untouched: no pointer");
        assert!(got.archived_at.is_none(), "row untouched: not archived");
        let oplog: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'supersede'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(oplog, 0, "a refused supersede leaves no oplog row");
    }

    #[test]
    fn supersede_rejects_already_superseded_old_and_keeps_the_pointer() {
        // #501 guard 2 (the review-finding regression): the pointer UPDATE is
        // guarded (`WHERE superseded_by IS NULL AND archived_at IS NULL`), so
        // a second supersede of the same old row must FAIL with the distinct
        // stale-plan error instead of silently REWRITING lineage.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old truth");
        let first = node(&store, "first replacement");
        let second = node(&store, "second replacement");

        store.supersede(&old.id, &first.id).unwrap();
        let err = store.supersede(&old.id, &second.id).unwrap_err();
        assert!(matches!(err, Error::StalePlan(_)), "got {err:?}");

        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert_eq!(
            got.superseded_by.as_ref(),
            Some(&first.id),
            "the original pointer must never be overwritten"
        );
        // Exactly ONE supersede oplog row: the refused attempt wrote nothing.
        let oplog: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'supersede'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(oplog, 1);
    }

    #[test]
    fn supersede_rejects_archived_old() {
        // #501 guard 2b: an archived old row was already retired (explicit
        // forget or retention) — a supersede plan against it was formed on a
        // stale view. Previously this silently set the pointer; now it is the
        // distinct stale-plan error and the row keeps a NULL pointer.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "retired row");
        let new = node(&store, "replacement");
        store.archive_memory(&old.id).unwrap();

        let err = store.supersede(&old.id, &new.id).unwrap_err();
        assert!(matches!(err, Error::StalePlan(_)), "got {err:?}");

        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert!(
            got.superseded_by.is_none(),
            "no lineage pointer is invented on a retired row"
        );
        assert!(got.archived_at.is_some(), "still archived");
    }

    #[test]
    fn supersede_guarded_in_tx_owns_every_guard_including_new_side() {
        // #501 review follow-up (HIGH): the acyclicity induction must be
        // STRUCTURAL in the one shared primitive, not an undocumented caller
        // invariant -- an in-tx call passing a non-fresh `new` (which the
        // review-merge path never does today) must be refused by the
        // primitive itself, with a distinct variant per guard so each caller
        // can own its error mapping.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let now = chrono::Utc::now().timestamp();

        // Guard 1 lives in the primitive: self-supersede.
        let a = node(&store, "self");
        assert!(matches!(
            store.supersede_guarded_in_tx(&a.id, &a.id, now).unwrap(),
            SupersedeGuard::SelfSupersede
        ));

        // Guard 3 lives in the primitive: missing new target.
        assert!(matches!(
            store
                .supersede_guarded_in_tx(&a.id, &MemoryId::new(), now)
                .unwrap(),
            SupersedeGuard::MissingNew
        ));

        // Guard 3: an archived (non-current) new target.
        let live = node(&store, "live old");
        let dead = node(&store, "archived new");
        store.archive_memory(&dead.id).unwrap();
        assert!(matches!(
            store
                .supersede_guarded_in_tx(&live.id, &dead.id, now)
                .unwrap(),
            SupersedeGuard::NewNotCurrent {
                archived: true,
                superseded: false
            }
        ));
        let got_live = store.get_memory(&live.id).unwrap().unwrap();
        assert!(
            got_live.superseded_by.is_none() && got_live.archived_at.is_none(),
            "a refused new-side guard writes nothing"
        );

        // Guard 3: a superseded new target -- the cycle-closing edge.
        let x = node(&store, "x");
        let y = node(&store, "y");
        store.supersede(&x.id, &y.id).unwrap();
        assert!(matches!(
            store.supersede_guarded_in_tx(&y.id, &x.id, now).unwrap(),
            SupersedeGuard::NewNotCurrent {
                archived: true,
                superseded: true
            }
        ));
        let got_y = store.get_memory(&y.id).unwrap().unwrap();
        assert!(got_y.superseded_by.is_none() && got_y.archived_at.is_none());

        // Guard 2 arms are unchanged: missing old / already-resolved old.
        assert!(matches!(
            store
                .supersede_guarded_in_tx(&MemoryId::new(), &a.id, now)
                .unwrap(),
            SupersedeGuard::MissingOld
        ));
        assert!(matches!(
            store.supersede_guarded_in_tx(&x.id, &a.id, now).unwrap(),
            SupersedeGuard::OldResolved {
                archived: true,
                superseded: true
            }
        ));

        // The happy path through the bare primitive still applies.
        let b = node(&store, "fresh new");
        assert!(matches!(
            store.supersede_guarded_in_tx(&a.id, &b.id, now).unwrap(),
            SupersedeGuard::Applied
        ));
        let got_a = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(got_a.superseded_by.as_ref(), Some(&b.id));
    }

    #[test]
    fn supersede_rejects_non_current_new_target() {
        // #501 guard 3 (the write-side cycle defense): `new` must itself be
        // current truth (active, not superseded). This is what actually
        // blocks A->B then B->A — when superseding B, the OLD row B is still
        // active and unclaimed, so the old-row guard passes; the cycle is
        // refused because the NEW target A is archived+superseded. Together
        // with set-once pointers and old != new this makes the pointer graph
        // acyclic at the write side.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "cycle a");
        let b = node(&store, "cycle b");

        store.supersede(&a.id, &b.id).unwrap();
        let err = store.supersede(&b.id, &a.id).unwrap_err();
        assert!(matches!(err, Error::StalePlan(_)), "got {err:?}");

        // B is fully untouched: still active, still unclaimed.
        let got_b = store.get_memory(&b.id).unwrap().unwrap();
        assert!(got_b.superseded_by.is_none(), "no cycle pointer");
        assert!(got_b.archived_at.is_none(), "B stays active");

        // An archived-but-unsuperseded target is refused the same way.
        let c = node(&store, "live row");
        let d = node(&store, "archived target");
        store.archive_memory(&d.id).unwrap();
        let err = store.supersede(&c.id, &d.id).unwrap_err();
        assert!(matches!(err, Error::StalePlan(_)), "got {err:?}");
        let got_c = store.get_memory(&c.id).unwrap().unwrap();
        assert!(got_c.superseded_by.is_none() && got_c.archived_at.is_none());
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
}
#[cfg(test)]
mod vector_write_hygiene_tests {
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
}
