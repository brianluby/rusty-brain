//! Oplog replay and the shared `append_oplog` writer.

use super::internal::*;
use super::*;

impl SqliteStore {
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
    /// replay-on-reconnect). Bulk admin ops that do not describe a single
    /// memory's change (`namespace_rename`, `scrub`) are skipped — detected
    /// by their empty `memory_id` sentinel, not by op string, so a future
    /// bulk op is skipped automatically without needing to extend this list;
    /// `insert` maps to `Created`, `archive`/`supersede` to `Archived`, and
    /// every other mutation (`update`, `link`, `unlink`, `set_*`) to
    /// `Updated`.
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
                // `purge` is a disappearance, not an update: subscribers must
                // drop the memory exactly as they would an archived one.
                "archive" | "supersede" | "purge" => ChangeKind::Archived,
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
