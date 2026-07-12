//! Read-only observability aggregation (doctor/stats PRD): the queries behind
//! `rusty-brain stats`/`status`. Inherent-impl module like `oplog.rs` — these
//! run on the daemon's READ pool, never through the `Store` trait the engine
//! writes with, so the stats path issues zero writer ops (W1.8) by
//! construction. Counts and ids only — never memory content.

use super::internal::*;
use super::*;
use rb_types::{FeedbackTotals, GrowthBucket, MemoryStats, TopRecalled};

/// Read `meta.embedding_model` from `db_path` over a plain READ-ONLY SQLite
/// open — no migrations, no writer, no file creation — so `rusty-brain doctor`
/// can compare the DB's recorded model identity against the configured
/// provider even while a daemon owns the DB, or when the daemon refuses to
/// start on a model mismatch (the W0.2 fail-closed contract this check
/// diagnoses). A missing or unreadable database is an error; a database
/// without the stamp (legacy) is `Ok(None)`.
pub fn read_meta_embedding_model(db_path: &std::path::Path) -> Result<Option<String>> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| Error::Storage(format!("read-only open {}: {e}", db_path.display())))?;
    meta_value(&conn, "embedding_model")
}

/// Decode one `(count_a, count_b)` aggregate row.
fn two_counts(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, u64)> {
    Ok((
        row.get::<_, i64>(0)?.max(0) as u64,
        row.get::<_, i64>(1)?.max(0) as u64,
    ))
}

fn count_query(
    conn: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<u64> {
    conn.query_row(sql, params, |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as u64)
        .map_err(|e| Error::Storage(e.to_string()))
}

impl SqliteStore {
    /// Compute the namespace-scoped value/health aggregate for `ns`
    /// (doctor/stats PRD). Pure reads — safe on a read-pool connection; the
    /// caller supplies the bounded `window_days`, the CURRENT
    /// `(model, input_version)` embedding stamp (for the re-embed backlog),
    /// and the `top_limit` bound on the top-recalled list.
    pub fn namespace_stats(
        &self,
        ns: &Namespace,
        window_days: u32,
        model: &str,
        input_version: &str,
        top_limit: usize,
    ) -> Result<MemoryStats> {
        let ns_str = ns.as_db_string();
        let cutoff = chrono::Utc::now().timestamp() - i64::from(window_days) * 86_400;

        let (live, archived) = self
            .conn
            .query_row(
                "SELECT COUNT(*) FILTER (WHERE archived_at IS NULL),
                        COUNT(*) FILTER (WHERE archived_at IS NOT NULL)
                 FROM memories WHERE namespace = ?1",
                rusqlite::params![ns_str],
                two_counts,
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        // The vec0 index is one table for the whole DB (DOC-1 ops number).
        let vectors = count_query(&self.conn, "SELECT COUNT(*) FROM memory_vectors", [])?;

        let (helpful, wrong, stale) = self
            .conn
            .query_row(
                "SELECT COUNT(*) FILTER (WHERE kind = 'helpful'),
                        COUNT(*) FILTER (WHERE kind = 'wrong'),
                        COUNT(*) FILTER (WHERE kind = 'stale')
                 FROM memory_feedback WHERE namespace = ?1",
                rusqlite::params![ns_str],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?.max(0) as u64,
                        row.get::<_, i64>(1)?.max(0) as u64,
                        row.get::<_, i64>(2)?.max(0) as u64,
                    ))
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let (total_accesses, accessed_in_window) = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(access_count), 0),
                        COUNT(*) FILTER (WHERE archived_at IS NULL
                                           AND last_accessed_at >= ?2)
                 FROM memories WHERE namespace = ?1",
                rusqlite::params![ns_str, cutoff],
                two_counts,
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let top_recalled = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT memory_id, access_count FROM memories
                     WHERE namespace = ?1 AND archived_at IS NULL AND access_count > 0
                     ORDER BY access_count DESC, memory_id ASC
                     LIMIT ?2",
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            let mut rows = stmt
                .query(rusqlite::params![
                    ns_str,
                    i64::try_from(top_limit).unwrap_or(i64::MAX)
                ])
                .map_err(|e| Error::Storage(e.to_string()))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
                let id: String = row.get(0).map_err(|e| Error::Storage(e.to_string()))?;
                let count: i64 = row.get(1).map_err(|e| Error::Storage(e.to_string()))?;
                out.push(TopRecalled {
                    id: parse_id(&id)?,
                    access_count: count.max(0) as u64,
                });
            }
            out
        };

        let never_recalled_live = count_query(
            &self.conn,
            "SELECT COUNT(*) FROM memories
             WHERE namespace = ?1 AND archived_at IS NULL AND access_count = 0",
            rusqlite::params![ns_str],
        )?;

        // Distinct memories with an active contradicts link whose BOTH
        // endpoints are active and in `ns` — the aggregate twin of
        // `active_contradicts` (Feature C), with the same namespace-purity
        // reasoning: `memory_links` carries no namespace and permits
        // cross-namespace edges, so both endpoints are scoped explicitly.
        let contested = count_query(
            &self.conn,
            "SELECT COUNT(*) FROM (
               SELECT l.source_id AS local FROM memory_links l
                 JOIN memories far ON far.memory_id = l.target_id
                 JOIN memories loc ON loc.memory_id = l.source_id
               WHERE l.link_type = 'contradicts'
                 AND far.archived_at IS NULL AND loc.archived_at IS NULL
                 AND far.namespace = ?1 AND loc.namespace = ?1
               UNION
               SELECT l.target_id AS local FROM memory_links l
                 JOIN memories far ON far.memory_id = l.source_id
                 JOIN memories loc ON loc.memory_id = l.target_id
               WHERE l.link_type = 'contradicts'
                 AND far.archived_at IS NULL AND loc.archived_at IS NULL
                 AND far.namespace = ?1 AND loc.namespace = ?1
             )",
            rusqlite::params![ns_str],
        )?;

        let created_per_day = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT strftime('%Y-%m-%d', created_at, 'unixepoch') AS day, COUNT(*)
                     FROM memories
                     WHERE namespace = ?1 AND created_at >= ?2
                     GROUP BY day ORDER BY day ASC",
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            let mut rows = stmt
                .query(rusqlite::params![ns_str, cutoff])
                .map_err(|e| Error::Storage(e.to_string()))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
                let day: String = row.get(0).map_err(|e| Error::Storage(e.to_string()))?;
                let count: i64 = row.get(1).map_err(|e| Error::Storage(e.to_string()))?;
                out.push(GrowthBucket {
                    day,
                    count: count.max(0) as u64,
                });
            }
            out
        };

        // Namespace-scoped re-embed backlog: same staleness predicate as the
        // cross-namespace `memories_for_reembed` scan (P5 Feature A).
        let reembed_pending = count_query(
            &self.conn,
            "SELECT COUNT(*) FROM memories
             WHERE namespace = ?1 AND archived_at IS NULL
               AND (embedding_model <> ?2 OR embedding_input_version <> ?3)",
            rusqlite::params![ns_str, model, input_version],
        )?;

        let db_path = self.conn.path().unwrap_or("").to_string();
        let wal_bytes = if db_path.is_empty() {
            0
        } else {
            std::fs::metadata(format!("{db_path}-wal"))
                .map(|m| m.len())
                .unwrap_or(0)
        };

        Ok(MemoryStats {
            namespace: ns_str,
            window_days,
            db_path,
            live,
            archived,
            vectors,
            wal_bytes,
            db_embedding_model: meta_value(&self.conn, "embedding_model")?,
            feedback: FeedbackTotals {
                helpful,
                wrong,
                stale,
            },
            total_accesses,
            accessed_in_window,
            top_recalled,
            never_recalled_live,
            contested,
            created_per_day,
            reembed_pending,
            retention_eligible: None,
            last_forget_at: None,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use crate::store::Store as _;
    use crate::SqliteStore;
    use rb_types::{FeedbackKind, LinkType, MemoryLink, MemoryNote, MemoryType, Namespace};

    const DIM: usize = 8;
    const MODEL: &str = "deterministic";
    const INPUT_VERSION: &str = "v2-composite";

    fn open(path: &std::path::Path) -> SqliteStore {
        SqliteStore::open_with_model(path, DIM, MODEL).unwrap()
    }

    /// Insert one active memory stamped with the CURRENT (model, input
    /// version) pair, returning its id.
    fn seed(store: &SqliteStore, ns: &Namespace, content: &str) -> rb_types::MemoryId {
        let mut note = MemoryNote::new(ns.clone(), content.to_string(), MemoryType::Insight, 5);
        note.embedding_model = MODEL.to_string();
        note.embedding_input_version = INPUT_VERSION.to_string();
        let id = note.id.clone();
        store.insert_memory(&note, Some(&[0.1f32; DIM])).unwrap();
        id
    }

    #[test]
    fn namespace_stats_aggregates_counts_feedback_and_recall_signals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let store = open(&path);
        let ns = Namespace::Project("stats".to_string());
        let other = Namespace::Project("other".to_string());

        // Three live rows + one archived in `ns`.
        let a = seed(&store, &ns, "a");
        let b = seed(&store, &ns, "b");
        let _c = seed(&store, &ns, "c");
        let dead = seed(&store, &ns, "dead");
        store.archive_memory(&dead).unwrap();
        // A stale-stamped live row (re-embed backlog of 1).
        let mut stale = MemoryNote::new(ns.clone(), "stale".to_string(), MemoryType::Insight, 5);
        stale.embedding_model = MODEL.to_string();
        stale.embedding_input_version = "v1-content".to_string();
        store.insert_memory(&stale, Some(&[0.2f32; DIM])).unwrap();

        // Cross-namespace noise that must NOT leak into `ns` aggregates.
        let x = seed(&store, &other, "x");
        let y = seed(&store, &other, "y");
        store
            .record_feedback(&x, FeedbackKind::Helpful, Some("mallory"))
            .unwrap();
        store
            .add_link(&MemoryLink {
                source_id: x.clone(),
                target_id: y,
                link_type: LinkType::Contradicts,
                strength: 1.0,
                reason: String::new(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        // Feedback in `ns`: 2 helpful, 1 wrong, 1 stale.
        store
            .record_feedback(&a, FeedbackKind::Helpful, Some("alice"))
            .unwrap();
        store
            .record_feedback(&a, FeedbackKind::Helpful, None)
            .unwrap();
        store
            .record_feedback(&b, FeedbackKind::Wrong, Some("bob"))
            .unwrap();
        store
            .record_feedback(&b, FeedbackKind::Stale, None)
            .unwrap();

        // Access signal: a recalled 5x, b recalled 2x; c/stale never.
        let now = chrono::Utc::now().timestamp();
        store
            .record_access_bumps(&[
                crate::AccessBump {
                    id: a.clone(),
                    count: 5,
                    last_accessed_at: now,
                },
                crate::AccessBump {
                    id: b.clone(),
                    count: 2,
                    last_accessed_at: now,
                },
            ])
            .unwrap();

        // One active contradicts pair in `ns` -> both endpoints contested.
        store
            .add_link(&MemoryLink {
                source_id: a.clone(),
                target_id: b.clone(),
                link_type: LinkType::Contradicts,
                strength: 1.0,
                reason: "team reversed".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        let stats = store
            .namespace_stats(&ns, 30, MODEL, INPUT_VERSION, 10)
            .unwrap();

        assert_eq!(stats.namespace, ns.as_db_string());
        assert_eq!(stats.window_days, 30);
        assert_eq!(stats.live, 4, "3 seeded + 1 stale-stamped live rows");
        assert_eq!(stats.archived, 1);
        // Vector index is one table: 5 ns rows + 2 other-ns rows, minus the
        // archived row's pruned vector.
        assert_eq!(stats.vectors, 6);
        // SQLite reports the resolved path (macOS: /var -> /private/var), so
        // compare canonicalized forms.
        assert_eq!(
            std::path::Path::new(&stats.db_path).canonicalize().unwrap(),
            path.canonicalize().unwrap()
        );
        assert_eq!(stats.db_embedding_model.as_deref(), Some(MODEL));
        assert!(
            stats.wal_bytes > 0,
            "writes leave a non-empty -wal sidecar under WAL mode"
        );

        assert_eq!(stats.feedback.helpful, 2);
        assert_eq!(stats.feedback.wrong, 1);
        assert_eq!(stats.feedback.stale, 1);

        assert_eq!(stats.total_accesses, 7);
        assert_eq!(stats.accessed_in_window, 2);
        assert_eq!(stats.top_recalled.len(), 2);
        assert_eq!(stats.top_recalled[0].id, a);
        assert_eq!(stats.top_recalled[0].access_count, 5);
        assert_eq!(stats.top_recalled[1].id, b);
        assert_eq!(stats.top_recalled[1].access_count, 2);
        assert_eq!(stats.never_recalled_live, 2, "c and the stale row");

        assert_eq!(stats.contested, 2, "both endpoints of the active pair");
        assert_eq!(stats.reembed_pending, 1, "the stale-stamped row only");

        // Growth: everything was created today; buckets stay in-window.
        assert_eq!(stats.created_per_day.len(), 1);
        assert_eq!(
            stats.created_per_day[0].count, 5,
            "all 5 ns rows (incl. archived) were created today"
        );

        // The other namespace sees only its own signals.
        let other_stats = store
            .namespace_stats(&other, 30, MODEL, INPUT_VERSION, 10)
            .unwrap();
        assert_eq!(other_stats.live, 2);
        assert_eq!(other_stats.feedback.helpful, 1);
        assert_eq!(other_stats.feedback.wrong, 0);
        assert_eq!(other_stats.contested, 2);
        assert_eq!(other_stats.total_accesses, 0);
    }

    #[test]
    fn namespace_stats_windows_bound_growth_and_access_recency() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let store = open(&path);
        let ns = Namespace::Project("windowed".to_string());

        // One row created now, one 10 days ago.
        let fresh = seed(&store, &ns, "fresh");
        let mut old = MemoryNote::new(ns.clone(), "old".to_string(), MemoryType::Insight, 5);
        old.embedding_model = MODEL.to_string();
        old.embedding_input_version = INPUT_VERSION.to_string();
        old.created_at = chrono::Utc::now() - chrono::Duration::days(10);
        old.updated_at = old.created_at;
        let old_id = old.id.clone();
        store.insert_memory(&old, Some(&[0.3f32; DIM])).unwrap();

        // `old` was last accessed 10 days ago, `fresh` today.
        let now = chrono::Utc::now().timestamp();
        store
            .record_access_bumps(&[
                crate::AccessBump {
                    id: fresh.clone(),
                    count: 1,
                    last_accessed_at: now,
                },
                crate::AccessBump {
                    id: old_id,
                    count: 3,
                    last_accessed_at: now - 10 * 86_400,
                },
            ])
            .unwrap();

        let stats = store
            .namespace_stats(&ns, 7, MODEL, INPUT_VERSION, 10)
            .unwrap();
        assert_eq!(stats.window_days, 7);
        // Growth buckets only cover the 7-day window: the 10-day-old row is out.
        assert_eq!(stats.created_per_day.len(), 1);
        assert_eq!(stats.created_per_day[0].count, 1);
        // Cumulative volume counts everything; the windowed count only `fresh`.
        assert_eq!(stats.total_accesses, 4);
        assert_eq!(stats.accessed_in_window, 1);

        // A wider window sees both buckets, oldest first.
        let wide = store
            .namespace_stats(&ns, 30, MODEL, INPUT_VERSION, 10)
            .unwrap();
        assert_eq!(wide.created_per_day.len(), 2);
        assert!(wide.created_per_day[0].day < wide.created_per_day[1].day);
        assert_eq!(wide.accessed_in_window, 2);
    }

    #[test]
    fn namespace_stats_top_recalled_is_bounded_by_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let store = open(&path);
        let ns = Namespace::Project("bounded".to_string());

        let now = chrono::Utc::now().timestamp();
        let bumps: Vec<crate::AccessBump> = (1..=3)
            .map(|i| crate::AccessBump {
                id: seed(&store, &ns, &format!("m{i}")),
                count: i,
                last_accessed_at: now,
            })
            .collect();
        store.record_access_bumps(&bumps).unwrap();

        let stats = store
            .namespace_stats(&ns, 30, MODEL, INPUT_VERSION, 2)
            .unwrap();
        assert_eq!(stats.top_recalled.len(), 2, "LIMIT bounds the list");
        assert_eq!(stats.top_recalled[0].access_count, 3);
        assert_eq!(stats.top_recalled[1].access_count, 2);
    }

    #[test]
    fn read_meta_embedding_model_reads_without_a_writable_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        {
            let store = open(&path);
            let ns = Namespace::Project("meta".to_string());
            seed(&store, &ns, "seed");
        }

        // Doctor's daemon-down path: a plain read-only open, no migrations, no
        // writer — usable while another process owns the DB.
        let model = crate::read_meta_embedding_model(&path).unwrap();
        assert_eq!(model.as_deref(), Some(MODEL));

        // A missing database is an error (doctor checks existence first).
        assert!(crate::read_meta_embedding_model(&dir.path().join("absent.db")).is_err());
    }

    #[test]
    fn read_meta_embedding_model_does_not_create_or_modify_the_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        {
            let store = open(&path);
            seed(&store, &Namespace::Global, "seed");
        }
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let _ = crate::read_meta_embedding_model(&path).unwrap();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "a read-only open must not touch the file");
    }
}
