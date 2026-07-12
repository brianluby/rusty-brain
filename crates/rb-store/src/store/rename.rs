//! Namespace rename.

use super::internal::*;
use super::*;
use crate::error::storage_err;

impl SqliteStore {
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
    /// - `memory_anchors` — namespace mirrors memories.namespace (the 009
    ///   migration's scoping column for anchor-filter lookups), so it is
    ///   re-keyed with a plain UPDATE in the same transaction.
    /// - `review_state` — namespace scopes the review queue's
    ///   snooze-exclusion probe (migration 010), so it is re-keyed with a
    ///   plain UPDATE in the same transaction (a stranded row would let a
    ///   snoozed item resurface immediately after the rename). Item keys are
    ///   memory-id based and namespace-free, so no key rewrite is needed.
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

            // Re-key the anchor scoping column in the same transaction (it
            // mirrors memories.namespace; a stranded old-namespace anchor
            // would silently drop out of anchor-filter lookups).
            self.conn
                .execute(
                    "UPDATE memory_anchors SET namespace = ?1 WHERE namespace = ?2",
                    rusqlite::params![new_str, old_str],
                )
                .map_err(storage_err)?;

            // Re-key the review snooze/reviewed-at rows (migration 010): the
            // queue's snooze-exclusion probe scopes by namespace, so a
            // stranded row would silently un-hide every snoozed item.
            self.conn
                .execute(
                    "UPDATE review_state SET namespace = ?1 WHERE namespace = ?2",
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
    fn rename_rekeys_review_state_namespace_and_snoozes_survive() {
        // review_state.namespace scopes the review queue's snooze-exclusion
        // probe; a rename must re-key it in the same transaction or every
        // snooze silently evaporates (the item resurfaces immediately under
        // the new namespace).
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = Namespace::Project("review-old".into());
        let new = Namespace::Project("review-new".into());

        let mut low = MemoryNote::new(old.clone(), "shaky note".into(), MemoryType::Insight, 5);
        low.confidence = 0.2;
        let low_id = low.id.clone();
        store.insert_memory(&low, None).unwrap();
        let key = rb_types::review_item_key(
            rb_types::ReviewReason::LowConfidence,
            std::slice::from_ref(&low_id),
        );
        store
            .snooze_review_item(&old, &key, 7, "{}", chrono::Utc::now())
            .unwrap();

        store.rename_namespace(&old, &new, false).unwrap();

        let stranded: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM review_state WHERE namespace = ?1",
                rusqlite::params![old.as_db_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stranded, 0,
            "no review row may stay under the old namespace"
        );

        let plan = store
            .review_queue(
                &new,
                &crate::ReviewQueueParams {
                    threshold: 0.95,
                    limit: 50,
                    since: None,
                },
                chrono::Utc::now(),
            )
            .unwrap();
        assert!(
            plan.items.is_empty(),
            "the snooze must keep hiding the item after the rename: {:?}",
            plan.items
        );
        assert_eq!(plan.totals.snoozed, 1, "the snooze follows the namespace");
    }

    #[test]
    fn rename_rekeys_anchor_namespace_and_anchor_filters_follow() {
        // memory_anchors.namespace mirrors memories.namespace; a rename must
        // re-key it in the same transaction so anchors stay attached AND the
        // anchor-filter list keeps finding the memory under the new namespace.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = Namespace::Project("anchored-old".into());
        let new = Namespace::Project("anchored-new".into());

        let mut m = MemoryNote::new(old.clone(), "anchored".into(), MemoryType::Insight, 5);
        m.anchors = vec![rb_types::MemoryAnchor::parse_file_spec("src/server.rs:1-3").unwrap()];
        store.insert_memory(&m, None).unwrap();

        store.rename_namespace(&old, &new, false).unwrap();

        let stranded: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_anchors WHERE namespace = ?1",
                rusqlite::params![old.as_db_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stranded, 0, "no anchor may stay under the old namespace");

        let filter = rb_types::RecallFilter {
            anchors: vec![rb_types::AnchorFilter {
                kind: rb_types::AnchorKind::File,
                value: "src/server.rs".into(),
            }],
            ..Default::default()
        };
        let hits = store.list_filtered(&new, &filter, 10).unwrap();
        assert_eq!(
            hits.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
            vec![m.id.clone()],
            "the anchor filter must find the memory in the NEW namespace"
        );
        assert_eq!(hits[0].anchors, m.anchors, "anchors stay attached");
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
