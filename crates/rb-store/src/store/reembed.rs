//! Re-embedding: accepting a configured embedding-model/dimension change.

use super::internal::*;
use super::*;
use crate::error::storage_err;
use std::path::Path;

impl SqliteStore {
    /// Explicitly accept a provider embedding-space change. The caller must
    /// hold daemon database admission for the migration and subsequent opens.
    pub fn accept_embedding_space_change(
        path: &Path,
        embedding_dim: usize,
        new_model: &str,
    ) -> Result<bool> {
        if embedding_dim == 0 {
            return Err(Error::Storage(
                "embedding_dim must be greater than 0".to_string(),
            ));
        }
        let conn = Self::open_for_model_acceptance(path)?;
        accept_embedding_space_change_on_connection(&conn, embedding_dim, new_model)
    }

    /// Explicit opt-in for an embedding-space swap: atomically update both
    /// global markers, replace only the derived vector index, and stale every
    /// row's embedding stamp so reembed converges under the new provider.
    pub fn accept_model_change(&self, new_model: &str) -> Result<bool> {
        accept_embedding_space_change_on_connection(&self.conn, self.embedding_dim, new_model)
    }
}

fn accept_embedding_space_change_on_connection(
    conn: &rusqlite::Connection,
    embedding_dim: usize,
    new_model: &str,
) -> Result<bool> {
    immediate_tx(conn, || {
        let stored_model = meta_value(conn, "embedding_model")?;
        let stored_dim = meta_value(conn, "embedding_dim")?;
        let target_dim = embedding_dim.to_string();
        if stored_model.as_deref() == Some(new_model)
            && stored_dim.as_deref() == Some(target_dim.as_str())
        {
            return Ok(false);
        }

        SqliteStore::replace_vector_index_for_model_acceptance(conn, embedding_dim)?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embedding_dim', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![target_dim],
        )
        .map_err(storage_err)?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embedding_model', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![new_model],
        )
        .map_err(storage_err)?;
        conn.execute("UPDATE memories SET embedding_input_version = ''", [])
            .map_err(storage_err)?;
        Ok(true)
    })
}
#[cfg(test)]
mod accept_model_change_tests {
    use super::*;

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

    fn note(ns: &Namespace, content: &str, model: &str) -> MemoryNote {
        let mut note = MemoryNote::new(ns.clone(), content.to_string(), MemoryType::Insight, 7);
        note.embedding_model = model.to_string();
        note.embedding_input_version = "v2-composite".to_string();
        note
    }

    #[test]
    fn populated_512_to_384_cutover_preserves_rows_links_history_and_anchors() {
        use rb_types::{AnchorKind, LinkType, MemoryAnchor, MemoryLink};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("dimension-cutover".to_string());
        let current_time =
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();

        let store = SqliteStore::open_with_model(&path, 512, "model-512").unwrap();
        let mut current = note(&ns, "current decision", "model-512");
        current.summary = "the replacement".to_string();
        current.context = "chosen after review".to_string();
        current.keywords = vec!["migration".to_string()];
        current.tags = vec!["accepted".to_string()];
        current.related_files = vec!["src/store.rs".to_string()];
        current.origin_user = Some("author".to_string());
        current.origin_host = Some("workstation".to_string());
        current.origin_agent = Some("cli".to_string());
        current.origin_source = Some("cli".to_string());
        current.session_id = Some("session-1".to_string());
        current.anchors = vec![MemoryAnchor {
            kind: AnchorKind::File,
            value: "src/store.rs".to_string(),
            start_line: Some(10),
            end_line: Some(12),
        }];
        current.created_at = current_time;
        current.updated_at = current_time;
        store
            .insert_memory(&current, Some(&vec![0.125f32; 512]))
            .unwrap();

        let mut predecessor = note(&ns, "superseded decision", "model-512");
        predecessor.archived_at = Some(current_time);
        predecessor.updated_at = current_time;
        predecessor.superseded_by = Some(current.id.clone());
        store.insert_memory(&predecessor, None).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: predecessor.id.clone(),
                target_id: current.id.clone(),
                link_type: LinkType::Supersedes,
                strength: 1.0,
                reason: "explicit history".to_string(),
                created_at: current_time,
            })
            .unwrap();

        let before_current = store.get_memory(&current.id).unwrap().unwrap();
        let before_predecessor = store.get_memory(&predecessor.id).unwrap().unwrap();
        let before_oplog: i64 = store
            .conn
            .query_row("SELECT count(*) FROM memory_oplog", [], |row| row.get(0))
            .unwrap();
        assert!(matches!(
            SqliteStore::open_with_model(&path, 384, "model-384"),
            Err(Error::DimensionMismatch { .. })
        ));
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='embedding_dim'",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            "512",
            "a no-consent open leaves the old dimension marker intact"
        );
        drop(store);

        assert!(SqliteStore::accept_embedding_space_change(&path, 384, "model-384").unwrap());

        let store = SqliteStore::open_with_model(&path, 384, "model-384").unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='embedding_dim'",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            "384"
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='embedding_model'",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            "model-384"
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_vectors", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "the incompatible derived index is replaced empty"
        );
        let mut expected_current = before_current;
        expected_current.embedding_input_version.clear();
        let mut expected_predecessor = before_predecessor;
        expected_predecessor.embedding_input_version.clear();
        assert_eq!(
            store.get_memory(&current.id).unwrap().unwrap(),
            expected_current
        );
        assert_eq!(
            store.get_memory(&predecessor.id).unwrap().unwrap(),
            expected_predecessor
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_links", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_anchors", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_oplog", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            before_oplog,
            "acceptance is not a memory mutation"
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM memories WHERE archived_at IS NULL",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1,
            "the superseded predecessor stays archived"
        );
    }

    #[test]
    fn dimension_change_same_model_and_same_pair_retry_preserve_partial_reembed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("same-model-dimension".to_string());
        let old = SqliteStore::open_with_model(&path, 8, "same-model").unwrap();
        let first = note(&ns, "first", "same-model");
        let second = note(&ns, "second", "same-model");
        old.insert_memory(&first, Some(&[0.1f32; 8])).unwrap();
        old.insert_memory(&second, Some(&[0.2f32; 8])).unwrap();
        drop(old);

        assert!(SqliteStore::accept_embedding_space_change(&path, 4, "same-model").unwrap());
        let target = SqliteStore::open_with_model(&path, 4, "same-model").unwrap();
        let queued = target
            .memories_for_reembed("same-model", "v2-composite", 10)
            .unwrap();
        assert_eq!(
            queued.len(),
            2,
            "a dimension change stales target-stamped rows"
        );
        let progressed = queued[0].clone();
        target
            .update_vector(
                &progressed.id,
                &[0.75f32; 4],
                "same-model",
                "v2-composite",
                rb_types::EmbeddingInputFingerprint::from(&progressed),
            )
            .unwrap();
        drop(target);

        assert!(!SqliteStore::accept_embedding_space_change(&path, 4, "same-model").unwrap());
        let restarted = SqliteStore::open_with_model(&path, 4, "same-model").unwrap();
        assert_eq!(
            restarted
                .conn
                .query_row("SELECT count(*) FROM memory_vectors", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1,
            "same-pair retry must not discard a converged vector"
        );
        assert_eq!(
            restarted
                .get_memory(&progressed.id)
                .unwrap()
                .unwrap()
                .embedding_input_version,
            "v2-composite"
        );
        assert_eq!(
            restarted
                .memories_for_reembed("same-model", "v2-composite", 10)
                .unwrap()
                .len(),
            1,
            "the unprocessed active row remains pending after restart"
        );
    }

    #[test]
    fn missing_model_marker_stales_rows_even_when_their_pair_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("missing-model-marker".to_string());
        let store = SqliteStore::open_with_model(&path, 8, "target-model").unwrap();
        let matching = note(&ns, "already stamped for target", "target-model");
        store.insert_memory(&matching, Some(&[0.1f32; 8])).unwrap();
        drop(store);

        let conn = SqliteStore::open_for_model_acceptance(&path).unwrap();
        conn.execute("DELETE FROM meta WHERE key='embedding_model'", [])
            .unwrap();
        drop(conn);

        assert!(
            SqliteStore::accept_embedding_space_change(&path, 8, "target-model").unwrap(),
            "a missing global marker is not an idempotent target pair"
        );
        let store = SqliteStore::open_with_model(&path, 8, "target-model").unwrap();
        assert_eq!(
            store
                .get_memory(&matching.id)
                .unwrap()
                .unwrap()
                .embedding_input_version,
            "",
            "matching per-row stamps must not skip vector staling"
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_vectors", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn acceptance_failure_before_commit_restores_old_index_and_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("accept-rollback".to_string());
        let store = SqliteStore::open_with_model(&path, 8, "model-old").unwrap();
        let row = note(&ns, "rollback row", "model-old");
        store.insert_memory(&row, Some(&[0.1f32; 8])).unwrap();
        drop(store);

        let conn = SqliteStore::open_for_model_acceptance(&path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_accept_staling
             BEFORE UPDATE OF embedding_input_version ON memories
             BEGIN SELECT RAISE(ABORT, 'injected acceptance failure'); END;",
        )
        .unwrap();
        drop(conn);

        assert!(SqliteStore::accept_embedding_space_change(&path, 4, "model-new").is_err());
        let store = SqliteStore::open_with_model(&path, 8, "model-old").unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='embedding_dim'",
                    [],
                    |r| { r.get::<_, String>(0) }
                )
                .unwrap(),
            "8"
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='embedding_model'",
                    [],
                    |r| { r.get::<_, String>(0) }
                )
                .unwrap(),
            "model-old"
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_vectors", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1,
            "the old-dimensional vector index rolls back with both markers"
        );
        assert_eq!(
            store
                .get_memory(&row.id)
                .unwrap()
                .unwrap()
                .embedding_input_version,
            "v2-composite"
        );
    }

    #[test]
    fn legacy_vector_layout_cutover_keeps_links_without_copying_old_bytes() {
        use rb_types::{LinkType, MemoryLink};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rb.db");
        let ns = Namespace::Project("legacy-layout".to_string());
        let store = SqliteStore::open_with_model(&path, 8, "legacy-model").unwrap();
        let left = note(&ns, "legacy left", "legacy-model");
        let right = note(&ns, "legacy right", "legacy-model");
        store.insert_memory(&left, Some(&[1.0f32; 8])).unwrap();
        store
            .insert_memory(&right, Some(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .unwrap();
        store
            .add_link(&MemoryLink {
                source_id: left.id.clone(),
                target_id: right.id.clone(),
                link_type: LinkType::References,
                strength: 0.5,
                reason: "similar".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        drop(store);

        let conn = SqliteStore::open_for_model_acceptance(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE memory_vectors;
             CREATE VIRTUAL TABLE memory_vectors USING vec0(
               memory_id TEXT PRIMARY KEY,
               embedding float[8]
             );
             DELETE FROM meta WHERE key IN
               ('vector_schema_version', 'vector_metric');",
        )
        .unwrap();
        for id in [&left.id, &right.id] {
            conn.execute(
                "INSERT INTO memory_vectors (memory_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![id.to_string(), embedding_bytes(&[0.5f32; 8])],
            )
            .unwrap();
        }
        drop(conn);

        assert!(SqliteStore::accept_embedding_space_change(&path, 4, "new-model").unwrap());
        let store = SqliteStore::open_with_model(&path, 4, "new-model").unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM memory_vectors", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM memory_links WHERE reason='similar'",
                    [],
                    |r| { r.get::<_, i64>(0) }
                )
                .unwrap(),
            1,
            "acceptance must not run the link-pruning legacy vector rebuild"
        );
    }
}
