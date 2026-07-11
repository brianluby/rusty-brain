//! Link-decay job support.

use super::internal::*;
use super::*;

impl SqliteStore {
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
}
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
#[cfg(test)]
mod link_decay_tests {
    use super::*;

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
}
