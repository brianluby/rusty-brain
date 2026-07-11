//! Re-embedding: accepting a configured embedding-model change.

use super::internal::*;
use super::*;

impl SqliteStore {
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
}
