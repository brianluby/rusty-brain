//! Decision-history timeline derivation (PRD 2026-07-02): the read-only
//! queries behind `rusty-brain history`. Inherent-impl module like `stats.rs`
//! — these run on the daemon's READ pool, never through the writer, so the
//! history path issues zero writer ops (W1.8) by construction.
//!
//! Everything is derived from EXISTING rows (HIST-2): the
//! `memories.superseded_by` pointer (the atomic supersede's artifact), the
//! `archived_at`/`confidence`/`origin_*` columns, and `memory_links`. No new
//! persistence, no schema change.

use super::internal::{from_ts, parse_id};
use super::*;
use rb_types::{HistoryEdge, HistoryEntry, MemoryHistory};
use std::collections::HashSet;

/// The link types HIST-1 surfaces on the timeline. `supersedes` rides the
/// chain itself and `implements` is out of the PRD's edge scope.
const EDGE_TYPES: [&str; 3] = ["contradicts", "extends", "references"];

/// One decoded chain-member row (pre-flag: contested/current/is_target are
/// stamped by the caller once the whole chain is known).
struct ChainRow {
    entry: HistoryEntry,
}

/// Which way one supersede-chain CTE walks from the target.
#[derive(Clone, Copy)]
enum Direction {
    /// Follow `superseded_by` pointers (old -> new): the successors.
    Forward,
    /// Follow reverse pointers (rows whose `superseded_by` reaches the
    /// target): the ancestors, including near-dup fan-in.
    Backward,
}

/// One direction's walk result: emit-eligible rows (with shortest hop
/// distance), the ids discovered at the depth+1 probe (never emitted), and
/// whether the row-budget window filled (rows beyond it exist).
struct DirectionWalk {
    rows: Vec<(u32, ChainRow)>,
    dropped: Vec<MemoryId>,
    overflow: bool,
}

impl SqliteStore {
    /// Derive the decision-history timeline for `id` in `ns`: the supersede
    /// chain in BOTH directions (each direction bounded by `depth` hops, the
    /// whole chain by `chain_limit` members) plus the active
    /// `contradicts`/`extends`/`references` edges touching chain members
    /// (bounded by `edge_limit`). Pure reads — safe on a read-pool
    /// connection: one recursive-CTE query per direction (the
    /// `graph_neighbors` shape), one edge query, one contested batch.
    ///
    /// Semantics guarantees:
    /// - Namespace purity: every chain member and BOTH endpoints of every edge
    ///   live in `ns` (`memory_links` carries no namespace and cross-namespace
    ///   pointers/edges exist, so each hop and both edge endpoints are scoped
    ///   explicitly — the `active_contradicts` rigor). A target id living in
    ///   another namespace is indistinguishable from a missing one: `NotFound`.
    /// - Cycle defense: `superseded_by` is user-influencable data; the CTE's
    ///   hop bound terminates any cycle and `MIN(hop) GROUP BY id` collapses
    ///   revisits, so each member appears once. The two directions are walked
    ///   INDEPENDENTLY (an ancestor entering a forward cycle is still found),
    ///   then merged with successors taking precedence for cycle overlaps.
    /// - `truncated` is exact for depth stops: it is set only when a
    ///   discovered-but-dropped row is genuinely absent from the final chain
    ///   (a boundary coinciding with a dangling/cross-namespace pointer or a
    ///   cycle revisit does NOT claim truncation). Cap overflows (chain or
    ///   edge) always report it.
    pub fn memory_history(
        &self,
        ns: &Namespace,
        id: &MemoryId,
        depth: u32,
        chain_limit: usize,
        edge_limit: usize,
    ) -> Result<MemoryHistory> {
        let ns_str = ns.as_db_string();
        let target = self
            .history_row(&ns_str, id)?
            .ok_or_else(|| Error::NotFound(id.clone()))?;

        // Per-direction budget: the target always holds one chain slot.
        let budget = chain_limit.saturating_sub(1);
        let mut fwd = self.chain_direction(&ns_str, id, depth, budget, Direction::Forward)?;
        let mut bwd = self.chain_direction(&ns_str, id, depth, budget, Direction::Backward)?;

        // Cross-direction merge: a member can only appear in both directions
        // through a cycle; successors win so the timeline reads target-first
        // toward the current truth, and the ancestor copy is dropped.
        let mut member_ids: HashSet<MemoryId> = HashSet::with_capacity(fwd.rows.len() + 1);
        member_ids.insert(id.clone());
        member_ids.extend(fwd.rows.iter().map(|(_, r)| r.entry.id.clone()));
        bwd.rows.retain(|(_, r)| !member_ids.contains(&r.entry.id));
        member_ids.extend(bwd.rows.iter().map(|(_, r)| r.entry.id.clone()));

        // Total chain cap: keep the members closest to the target (smallest
        // hop; successors before ancestors on ties — the current-truth side
        // is the more valuable half of an audit view).
        let mut cap_dropped = false;
        if fwd.rows.len() + bwd.rows.len() > budget {
            cap_dropped = true;
            let mut candidates: Vec<(u32, u8, bool, ChainRow)> = fwd
                .rows
                .into_iter()
                .map(|(h, r)| (h, 0u8, true, r))
                .chain(bwd.rows.into_iter().map(|(h, r)| (h, 1u8, false, r)))
                .collect();
            candidates.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| a.3.entry.created_at.cmp(&b.3.entry.created_at))
                    .then_with(|| a.3.entry.id.as_uuid().cmp(&b.3.entry.id.as_uuid()))
            });
            candidates.truncate(budget);
            let (mut kept_fwd, mut kept_bwd) = (Vec::new(), Vec::new());
            for (h, _, is_fwd, row) in candidates {
                if is_fwd {
                    kept_fwd.push((h, row));
                } else {
                    kept_bwd.push((h, row));
                }
            }
            member_ids = kept_fwd
                .iter()
                .chain(kept_bwd.iter())
                .map(|(_, r)| r.entry.id.clone())
                .collect();
            member_ids.insert(id.clone());
            fwd.rows = kept_fwd;
            bwd.rows = kept_bwd;
        }

        // Exact depth-truncation: a row discovered at the depth+1 probe only
        // counts as truncation when it is ABSENT from the final chain — a
        // cycle revisiting an already-listed member at the boundary is
        // completeness, not loss (PR #61 review, CodeRabbit).
        let depth_dropped = fwd
            .dropped
            .iter()
            .chain(bwd.dropped.iter())
            .any(|d| !member_ids.contains(d));
        let mut truncated = depth_dropped || fwd.overflow || bwd.overflow || cap_dropped;

        // Timeline order (HIST-1: "ordered by time"): ancestors oldest first.
        // The hop distance is the primary key — `created_at` has one-second
        // resolution, so a fast A->B->C chain would tie on time and shuffle;
        // deeper ancestors are by construction older. Time then id break ties
        // WITHIN a fan-in level (near-dup absorption).
        bwd.rows.sort_by(|(ah, a), (bh, b)| {
            bh.cmp(ah)
                .then_with(|| a.entry.created_at.cmp(&b.entry.created_at))
                .then_with(|| a.entry.id.as_uuid().cmp(&b.entry.id.as_uuid()))
        });
        // Successors stay in pointer-chase order (hop ascending).
        fwd.rows.sort_by(|(ah, a), (bh, b)| {
            ah.cmp(bh)
                .then_with(|| a.entry.created_at.cmp(&b.entry.created_at))
                .then_with(|| a.entry.id.as_uuid().cmp(&b.entry.id.as_uuid()))
        });

        let mut chain: Vec<HistoryEntry> = bwd
            .rows
            .into_iter()
            .map(|(_, r)| r.entry)
            .chain(std::iter::once(target.entry))
            .chain(fwd.rows.into_iter().map(|(_, r)| r.entry))
            .collect();

        // Edges: active links touching any chain member, both endpoints
        // scoped to `ns` and non-archived (the `active_contradicts` rigor).
        let chain_ids: Vec<MemoryId> = chain.iter().map(|e| e.id.clone()).collect();
        let (mut edges, edges_truncated) = self.chain_edges(&ns_str, &chain_ids, edge_limit)?;
        truncated |= edges_truncated;

        // Contested flags for chain members AND far endpoints in one batch.
        let mut flag_ids = chain_ids.clone();
        flag_ids.extend(edges.iter().map(|e| e.other.clone()));
        flag_ids.sort_by_key(rb_types::MemoryId::as_uuid);
        flag_ids.dedup();
        let contested = self.active_contradicts(ns, &flag_ids)?;
        for entry in &mut chain {
            entry.contested = contested.contains(&entry.id);
            entry.is_target = entry.id == *id;
        }
        for edge in &mut edges {
            edge.other_contested = contested.contains(&edge.other);
        }

        // "Current truth" pointer: the newest non-archived chain member.
        if let Some(current) = chain.iter_mut().rev().find(|e| !e.archived) {
            current.current = true;
        }

        Ok(MemoryHistory {
            namespace: ns_str,
            depth,
            chain,
            edges,
            truncated,
        })
    }

    /// Fetch one chain-member row scoped to `ns`; `Ok(None)` when the id is
    /// missing OR lives in another namespace (indistinguishable on purpose).
    fn history_row(&self, ns_str: &str, id: &MemoryId) -> Result<Option<ChainRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, summary, content, importance, confidence, created_at,
                        archived_at, superseded_by, origin_user, origin_host, origin_agent,
                        origin_source
                 FROM memories WHERE memory_id = ?1 AND namespace = ?2",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(rusqlite::params![id.to_string(), ns_str])
            .map_err(|e| Error::Storage(e.to_string()))?;
        match rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            Some(row) => Ok(Some(decode_chain_row(row)?)),
            None => Ok(None),
        }
    }

    /// Walk one supersede direction with a single bounded recursive CTE (the
    /// `graph_neighbors` shape): every hop is namespace-scoped, `UNION`
    /// dedups `(id, hop)` pairs, and the `hop < depth+1` bound terminates any
    /// cycle. `MIN(hop) GROUP BY id` collapses multi-path revisits to the
    /// shortest distance; the target itself is excluded.
    ///
    /// The query probes ONE hop past `depth` and ONE row past `budget`:
    /// - rows whose shortest distance is `depth + 1` are returned as
    ///   `dropped` ids (never emitted) so the caller can decide whether a
    ///   genuinely-new row was cut (exact truncation semantics);
    /// - fetching `budget + 1` rows sets `overflow` and the extra row is
    ///   discarded (the chain cap refused to fetch further).
    fn chain_direction(
        &self,
        ns_str: &str,
        id: &MemoryId,
        depth: u32,
        budget: usize,
        direction: Direction,
    ) -> Result<DirectionWalk> {
        // Both directions share the outer projection; only the recursive
        // seed/step differ. Every fragment is a FIXED literal — caller data
        // rides numbered parameters exclusively.
        let cte = match direction {
            Direction::Forward => {
                "walk(id, hop) AS (
                     SELECT nxt.memory_id, 1
                       FROM memories cur
                       JOIN memories nxt ON nxt.memory_id = cur.superseded_by
                      WHERE cur.memory_id = ?1 AND cur.namespace = ?2
                        AND nxt.namespace = ?2
                     UNION
                     SELECT nxt.memory_id, w.hop + 1
                       FROM walk w
                       JOIN memories cur ON cur.memory_id = w.id
                       JOIN memories nxt ON nxt.memory_id = cur.superseded_by
                      WHERE nxt.namespace = ?2 AND w.hop < ?3
                 )"
            }
            Direction::Backward => {
                "walk(id, hop) AS (
                     SELECT p.memory_id, 1
                       FROM memories p
                      WHERE p.superseded_by = ?1 AND p.namespace = ?2
                     UNION
                     SELECT p.memory_id, w.hop + 1
                       FROM walk w
                       JOIN memories p ON p.superseded_by = w.id
                      WHERE p.namespace = ?2 AND w.hop < ?3
                 )"
            }
        };
        let sql = format!(
            "WITH RECURSIVE {cte}
             SELECT m.memory_id, m.summary, m.content, m.importance, m.confidence,
                    m.created_at, m.archived_at, m.superseded_by, m.origin_user,
                    m.origin_host, m.origin_agent, m.origin_source, x.hops
               FROM (SELECT id, MIN(hop) AS hops FROM walk WHERE id <> ?1 GROUP BY id) x
               JOIN memories m ON m.memory_id = x.id
              ORDER BY x.hops ASC, m.created_at ASC, m.memory_id ASC
              LIMIT ?4"
        );

        let probe_depth = i64::from(depth).saturating_add(1);
        let probe_budget = i64::try_from(budget.saturating_add(1)).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(&sql).map_err(storage)?;
        let mut db_rows = stmt
            .query(rusqlite::params![
                id.to_string(),
                ns_str,
                probe_depth,
                probe_budget
            ])
            .map_err(storage)?;

        let mut walk = DirectionWalk {
            rows: Vec::new(),
            dropped: Vec::new(),
            overflow: false,
        };
        let mut fetched: i64 = 0;
        while let Some(row) = db_rows.next().map_err(storage)? {
            fetched += 1;
            let hops_raw = row.get::<_, i64>(12).map_err(storage)?;
            let hops = u32::try_from(hops_raw).unwrap_or(u32::MAX);
            if hops > depth {
                // Depth+1 probe row: discovered but never emitted. Ordering
                // (hops ASC) guarantees these follow every emit-eligible row,
                // so the emit slots below are never displaced by a probe.
                walk.dropped
                    .push(parse_id(&row.get::<_, String>(0).map_err(storage)?)?);
            } else if walk.rows.len() < budget {
                walk.rows.push((hops, decode_chain_row(row)?));
            }
        }
        // A full window means rows beyond it exist (emit-eligible or probe —
        // unknown either way), so the cap reports conservatively. Below the
        // window, everything reachable was fetched and the `dropped` set is
        // complete, keeping the caller's depth-truncation check exact.
        walk.overflow = fetched == probe_budget;
        Ok(walk)
    }

    /// Active `contradicts`/`extends`/`references` edges touching any chain
    /// member. BOTH endpoints must be non-archived and in `ns` (the
    /// `active_contradicts` double-endpoint scoping). Returns at most
    /// `edge_limit` edges plus a truncation flag (one extra row is probed).
    fn chain_edges(
        &self,
        ns_str: &str,
        chain_ids: &[MemoryId],
        edge_limit: usize,
    ) -> Result<(Vec<HistoryEdge>, bool)> {
        if chain_ids.is_empty() || edge_limit == 0 {
            return Ok((Vec::new(), false));
        }
        // ?1 = namespace; ?2..: chain ids; trailing param = LIMIT probe.
        let placeholders: Vec<String> = (0..chain_ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let in_list = placeholders.join(", ");
        let limit_param = format!("?{}", chain_ids.len() + 2);
        let type_list = EDGE_TYPES
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT l.source_id, l.target_id, l.link_type, l.reason, l.created_at,
                    s.summary, s.content, s.confidence,
                    t.summary, t.content, t.confidence
             FROM memory_links l
               JOIN memories s ON s.memory_id = l.source_id
               JOIN memories t ON t.memory_id = l.target_id
             WHERE l.link_type IN ({type_list})
               AND s.namespace = ?1 AND t.namespace = ?1
               AND s.archived_at IS NULL AND t.archived_at IS NULL
               AND (l.source_id IN ({in_list}) OR l.target_id IN ({in_list}))
             ORDER BY l.created_at ASC, l.source_id ASC, l.target_id ASC, l.link_type ASC
             LIMIT {limit_param}"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(chain_ids.len() + 2);
        params.push(Box::new(ns_str.to_string()));
        for id in chain_ids {
            params.push(Box::new(id.to_string()));
        }
        // Probe one row past the cap so truncation is reported, not silent.
        params.push(Box::new(
            i64::try_from(edge_limit.saturating_add(1)).unwrap_or(i64::MAX),
        ));
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();

        let chain_set: HashSet<&MemoryId> = chain_ids.iter().collect();
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(refs.as_slice())
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            let source = parse_id(&row.get::<_, String>(0).map_err(storage)?)?;
            let target = parse_id(&row.get::<_, String>(1).map_err(storage)?)?;
            let link_type = rb_types::LinkType::parse(&row.get::<_, String>(2).map_err(storage)?)?;
            let reason: String = row.get(3).map_err(storage)?;
            let created_at = from_ts(row.get::<_, i64>(4).map_err(storage)?)?;
            let source_summary = summary_or_content(
                row.get::<_, String>(5).map_err(storage)?,
                row.get::<_, String>(6).map_err(storage)?,
            );
            let source_confidence = row.get::<_, f64>(7).map_err(storage)? as f32;
            let target_summary = summary_or_content(
                row.get::<_, String>(8).map_err(storage)?,
                row.get::<_, String>(9).map_err(storage)?,
            );
            let target_confidence = row.get::<_, f64>(10).map_err(storage)? as f32;

            // `local` is the chain member; when both endpoints are chain
            // members the source side wins (deterministic, matches `outbound`).
            let outbound = chain_set.contains(&source);
            let (local, other, other_summary, other_confidence) = if outbound {
                (source, target, target_summary, target_confidence)
            } else {
                (target, source, source_summary, source_confidence)
            };
            edges.push(HistoryEdge {
                link_type,
                local,
                other,
                outbound,
                reason,
                other_summary,
                other_confidence,
                other_contested: false, // stamped by the caller in one batch
                created_at,
            });
        }
        let truncated = edges.len() > edge_limit;
        edges.truncate(edge_limit);
        Ok((edges, truncated))
    }
}

fn storage(e: rusqlite::Error) -> Error {
    Error::Storage(e.to_string())
}

/// The projection rule every read surface applies: the summary, falling back
/// to the content when the summary is empty.
fn summary_or_content(summary: String, content: String) -> String {
    if summary.trim().is_empty() {
        content
    } else {
        summary
    }
}

fn decode_chain_row(row: &rusqlite::Row<'_>) -> Result<ChainRow> {
    let id = parse_id(&row.get::<_, String>(0).map_err(storage)?)?;
    let summary = summary_or_content(
        row.get::<_, String>(1).map_err(storage)?,
        row.get::<_, String>(2).map_err(storage)?,
    );
    // Fail closed on an out-of-band importance: the schema CHECKs 1..=10, so
    // a value no u8 holds is corruption — surfacing 0 would mask it (PR #61
    // review, Copilot).
    let raw_importance = row.get::<_, i64>(3).map_err(storage)?;
    let importance = u8::try_from(raw_importance).map_err(|_| {
        Error::Storage(format!(
            "corrupt importance {raw_importance} on memory {id} (schema allows 1..=10)"
        ))
    })?;
    let confidence = row.get::<_, f64>(4).map_err(storage)? as f32;
    let created_at = from_ts(row.get::<_, i64>(5).map_err(storage)?)?;
    let archived = row.get::<_, Option<i64>>(6).map_err(storage)?.is_some();
    let superseded_by = row
        .get::<_, Option<String>>(7)
        .map_err(storage)?
        .map(|s| parse_id(&s))
        .transpose()?;
    Ok(ChainRow {
        entry: HistoryEntry {
            id,
            summary,
            importance,
            confidence,
            created_at,
            archived,
            contested: false,
            current: false,
            is_target: false,
            superseded_by,
            origin_user: row.get(8).map_err(storage)?,
            origin_host: row.get(9).map_err(storage)?,
            origin_agent: row.get(10).map_err(storage)?,
            origin_source: row.get(11).map_err(storage)?,
        },
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use crate::store::Store as _;
    use crate::SqliteStore;
    use rb_types::{LinkType, MemoryId, MemoryLink, MemoryNote, MemoryType, Namespace};

    const DIM: usize = 8;

    fn open(path: &std::path::Path) -> SqliteStore {
        SqliteStore::open_with_model(path, DIM, "deterministic").unwrap()
    }

    /// Insert one active memory with `summary`, returning its id.
    fn seed(store: &SqliteStore, ns: &Namespace, summary: &str) -> MemoryId {
        let mut note = MemoryNote::new(
            ns.clone(),
            format!("content for {summary}"),
            MemoryType::ArchitectureDecision,
            7,
        );
        note.summary = summary.to_string();
        note.origin_user = Some("alice".to_string());
        note.origin_source = Some("cli".to_string());
        let id = note.id.clone();
        store.insert_memory(&note, Some(&[0.1f32; DIM])).unwrap();
        id
    }

    /// Plant a supersede pointer with RAW SQL, bypassing the write-side
    /// guards (#501: `Store::supersede` now rejects self-supersede,
    /// re-supersede, and non-current targets). Cycle/self-loop fixtures model
    /// direct DB access, admin scripts, or corruption — exactly the threat
    /// the read-side cycle defense must survive, which is why these tests
    /// keep passing as belt-and-suspenders behind the write-side guards.
    fn force_supersede(store: &SqliteStore, old: &MemoryId, new: &MemoryId) {
        let now = chrono::Utc::now().timestamp();
        store
            .conn
            .execute(
                "UPDATE memories SET superseded_by = ?1, archived_at = ?2, updated_at = ?2
                 WHERE memory_id = ?3",
                rusqlite::params![new.to_string(), now, old.to_string()],
            )
            .unwrap();
    }

    fn link(store: &SqliteStore, from: &MemoryId, to: &MemoryId, lt: LinkType, reason: &str) {
        store
            .add_link(&MemoryLink {
                source_id: from.clone(),
                target_id: to.clone(),
                link_type: lt,
                strength: 1.0,
                reason: reason.to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
    }

    /// A (superseded by) B (superseded by) C: `history B` walks BOTH
    /// directions, orders oldest first, flags C current and A/B archived, and
    /// carries the HIST-1 metadata.
    #[test]
    fn history_walks_the_supersede_chain_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist".to_string());

        let a = seed(&store, &ns, "use rabbitmq");
        let b = seed(&store, &ns, "use kafka");
        let c = seed(&store, &ns, "use kafka with kraft");
        store.supersede(&a, &b).unwrap();
        store.supersede(&b, &c).unwrap();

        let history = store.memory_history(&ns, &b, 100, 200, 200).unwrap();
        assert_eq!(history.namespace, ns.as_db_string());
        assert_eq!(history.depth, 100);
        assert!(!history.truncated);

        let ids: Vec<&MemoryId> = history.chain.iter().map(|e| &e.id).collect();
        assert_eq!(ids, vec![&a, &b, &c], "oldest first: A -> B -> C");

        let [ea, eb, ec] = &history.chain[..] else {
            panic!("expected 3 chain entries, got {}", history.chain.len());
        };
        assert!(
            ea.archived && eb.archived,
            "superseded members are archived"
        );
        assert!(!ec.archived);
        assert!(ec.current, "C is the current truth");
        assert!(!ea.current && !eb.current);
        assert!(eb.is_target, "the requested member is flagged");
        assert!(!ea.is_target && !ec.is_target);
        assert_eq!(ea.superseded_by.as_ref(), Some(&b));
        assert_eq!(eb.superseded_by.as_ref(), Some(&c));
        assert!(ec.superseded_by.is_none());
        // HIST-1 metadata rides each hop.
        assert_eq!(ea.summary, "use rabbitmq");
        assert_eq!(ea.importance, 7);
        assert!((ea.confidence - 1.0).abs() < 1e-6);
        assert_eq!(ea.origin_user.as_deref(), Some("alice"));
        assert_eq!(ea.origin_source.as_deref(), Some("cli"));
    }

    /// `history C` (the chain head) still lists the full ancestry (the
    /// acceptance-criteria direction).
    #[test]
    fn history_of_the_current_head_lists_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-head".to_string());

        let a = seed(&store, &ns, "v1");
        let b = seed(&store, &ns, "v2");
        let c = seed(&store, &ns, "v3");
        store.supersede(&a, &b).unwrap();
        store.supersede(&b, &c).unwrap();

        let history = store.memory_history(&ns, &c, 100, 200, 200).unwrap();
        let ids: Vec<&MemoryId> = history.chain.iter().map(|e| &e.id).collect();
        assert_eq!(ids, vec![&a, &b, &c]);
        assert!(history.chain[2].current && history.chain[2].is_target);
        assert!(
            history.chain[0].archived && history.chain[1].archived,
            "A and B are flagged superseded/archived"
        );
    }

    /// Near-dup absorption fans several predecessors into one successor; all
    /// of them appear, ordered by time.
    #[test]
    fn history_includes_multiple_predecessors_per_hop() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-fanin".to_string());

        let a1 = seed(&store, &ns, "dup one");
        let a2 = seed(&store, &ns, "dup two");
        let b = seed(&store, &ns, "absorbed");
        store.supersede(&a1, &b).unwrap();
        store.supersede(&a2, &b).unwrap();

        let history = store.memory_history(&ns, &b, 100, 200, 200).unwrap();
        let ids: std::collections::HashSet<String> =
            history.chain.iter().map(|e| e.id.to_string()).collect();
        assert_eq!(history.chain.len(), 3);
        assert!(ids.contains(&a1.to_string()) && ids.contains(&a2.to_string()));
        assert!(history.chain[2].is_target);
    }

    /// A supersede cycle (user-influencable data) must not hang the walk: the
    /// visited set ends it and every member appears once.
    #[test]
    fn history_terminates_on_a_supersede_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-cycle".to_string());

        let a = seed(&store, &ns, "a");
        let b = seed(&store, &ns, "b");
        store.supersede(&a, &b).unwrap();
        force_supersede(&store, &b, &a); // closes the cycle A -> B -> A (raw SQL: the write path refuses it)

        for anchor in [&a, &b] {
            let history = store.memory_history(&ns, anchor, 100, 200, 200).unwrap();
            let mut ids: Vec<String> = history.chain.iter().map(|e| e.id.to_string()).collect();
            let len_before = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), len_before, "no duplicate chain members");
            assert_eq!(history.chain.len(), 2, "both cycle members, exactly once");
            assert!(
                !history.truncated,
                "a cycle stop is completeness, not truncation"
            );
        }
    }

    /// PR #61 review (HIGH): an external ancestor that enters a supersede
    /// cycle must still appear in the chain. With a shared cross-direction
    /// visited set, the forward walk (B, C) poisoned the backward BFS: it hit
    /// forward-visited C, skipped without enqueueing, and A -> C's ancestor A
    /// silently vanished while `truncated` stayed false — an integrity defect
    /// for an audit surface.
    #[test]
    fn history_backward_reaches_ancestors_that_enter_a_forward_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-cycle-entry".to_string());

        let a = seed(&store, &ns, "external ancestor");
        let b = seed(&store, &ns, "cycle b");
        let c = seed(&store, &ns, "cycle c");
        let d = seed(&store, &ns, "cycle d");
        store.supersede(&b, &c).unwrap();
        store.supersede(&c, &d).unwrap();
        force_supersede(&store, &d, &b); // closes the cycle B -> C -> D -> B (raw SQL)
        force_supersede(&store, &a, &c); // the external entry A -> C (raw SQL: C is superseded)

        // From D (inside the cycle) and from A (outside, feeding in): every
        // member appears exactly once and nothing is silently dropped.
        for anchor in [&d, &a] {
            let history = store.memory_history(&ns, anchor, 100, 200, 200).unwrap();
            let mut got: Vec<String> = history.chain.iter().map(|e| e.id.to_string()).collect();
            let len_before = got.len();
            got.sort();
            got.dedup();
            assert_eq!(got.len(), len_before, "no duplicates from {anchor}");
            for member in [&a, &b, &c, &d] {
                assert!(
                    history.chain.iter().any(|e| &e.id == member),
                    "member {member} missing from the chain anchored at {anchor}: {got:?}"
                );
            }
            assert_eq!(history.chain.len(), 4);
            assert!(
                !history.truncated,
                "nothing was dropped, so truncated must be false (anchor {anchor})"
            );
        }
    }

    /// Diamond fan-in (a1 -> b1 -> d, a2 -> b2 -> d): every branch member
    /// appears exactly once, ordered by hop distance (deepest ancestors
    /// first), with no cross-path duplicates.
    #[test]
    fn history_diamond_fan_in_lists_every_branch_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-diamond".to_string());

        let a1 = seed(&store, &ns, "a1");
        let a2 = seed(&store, &ns, "a2");
        let b1 = seed(&store, &ns, "b1");
        let b2 = seed(&store, &ns, "b2");
        let d = seed(&store, &ns, "d");
        store.supersede(&a1, &b1).unwrap();
        store.supersede(&a2, &b2).unwrap();
        store.supersede(&b1, &d).unwrap();
        store.supersede(&b2, &d).unwrap();

        let history = store.memory_history(&ns, &d, 100, 200, 200).unwrap();
        assert_eq!(history.chain.len(), 5, "all branch members, exactly once");
        let hop2: Vec<&MemoryId> = history.chain[..2].iter().map(|e| &e.id).collect();
        let hop1: Vec<&MemoryId> = history.chain[2..4].iter().map(|e| &e.id).collect();
        assert!(hop2.contains(&&a1) && hop2.contains(&&a2), "deepest first");
        assert!(hop1.contains(&&b1) && hop1.contains(&&b2));
        assert_eq!(history.chain[4].id, d);
        assert!(history.chain[4].is_target);
        assert!(!history.truncated);
    }

    /// A self-supersede (A -> A) terminates and yields a one-entry chain.
    #[test]
    fn history_self_supersede_terminates() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-self".to_string());
        let a = seed(&store, &ns, "self-superseded");
        force_supersede(&store, &a, &a); // raw SQL: the write path rejects self-supersede (#501)

        let history = store.memory_history(&ns, &a, 100, 200, 200).unwrap();
        assert_eq!(history.chain.len(), 1, "the self-loop adds no members");
        assert_eq!(history.chain[0].id, a);
        assert!(history.chain[0].archived, "supersede archived it");
        assert!(
            !history.chain[0].current,
            "an archived row is never current"
        );
        assert!(!history.truncated);
    }

    /// PR #61 review (Copilot): depth bounds hops, not width — near-dup
    /// fan-in can point many predecessors at one id. The total chain-member
    /// cap bounds the response and reports the drop via `truncated`.
    #[test]
    fn history_chain_cap_bounds_members_and_reports_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-chain-cap".to_string());

        let hub = seed(&store, &ns, "absorbing hub");
        for i in 0..6 {
            let dup = seed(&store, &ns, &format!("near-dup {i}"));
            store.supersede(&dup, &hub).unwrap();
        }

        let history = store.memory_history(&ns, &hub, 100, 3, 200).unwrap();
        assert_eq!(history.chain.len(), 3, "total chain members are capped");
        assert!(
            history.chain.iter().any(|e| e.id == hub),
            "the target always survives the cap"
        );
        assert!(history.truncated, "the cap dropped rows: report it");

        let full = store.memory_history(&ns, &hub, 100, 200, 200).unwrap();
        assert_eq!(full.chain.len(), 7);
        assert!(!full.truncated);
    }

    /// PR #61 review (CodeRabbit): a depth-boundary stop where the next
    /// pointer is cross-namespace/dangling or cyclic has NO further row to
    /// fetch — it must not claim truncation.
    #[test]
    fn history_depth_boundary_without_further_rows_is_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-boundary".to_string());
        let other = Namespace::Project("hist-boundary-other".to_string());

        // A -> B -> X where X is foreign: at depth 1 from A the boundary
        // coincides with the cross-namespace pointer — nothing was dropped.
        let a = seed(&store, &ns, "a");
        let b = seed(&store, &ns, "b");
        let x = seed(&store, &other, "foreign x");
        store.supersede(&a, &b).unwrap();
        store.supersede(&b, &x).unwrap();
        let history = store.memory_history(&ns, &a, 1, 200, 200).unwrap();
        assert_eq!(history.chain.len(), 2, "A and B; the foreign X never leaks");
        assert!(
            !history.truncated,
            "a cross-namespace pointer at the boundary is not truncation"
        );

        // C -> D -> C: at depth 1 from C the boundary coincides with the
        // cycle closing back onto the target — nothing was dropped either.
        let c = seed(&store, &ns, "c");
        let d = seed(&store, &ns, "d");
        store.supersede(&c, &d).unwrap();
        force_supersede(&store, &d, &c); // raw SQL: closes the cycle past the write guard
        let history = store.memory_history(&ns, &c, 1, 200, 200).unwrap();
        assert_eq!(history.chain.len(), 2);
        assert!(
            !history.truncated,
            "a cycle closing at the boundary is not truncation"
        );
    }

    /// PR #61 review (Copilot): a corrupt importance outside the schema's
    /// CHECK range must fail closed with a storage error, not silently decode
    /// as the impossible value 0.
    #[test]
    fn history_corrupt_importance_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-corrupt".to_string());
        let a = seed(&store, &ns, "will be corrupted");

        // Simulate on-disk corruption: bypass the CHECK constraint and plant
        // an importance no u8 (or the schema) allows.
        store
            .conn
            .execute_batch(&format!(
                "PRAGMA ignore_check_constraints = ON;
                 UPDATE memories SET importance = 300 WHERE memory_id = '{a}';
                 PRAGMA ignore_check_constraints = OFF;"
            ))
            .unwrap();

        let err = store.memory_history(&ns, &a, 100, 200, 200).unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Storage(_)),
            "corruption must surface as a storage error, got {err:?}"
        );
    }

    /// The depth bound clamps BOTH directions and reports truncation.
    #[test]
    fn history_depth_bounds_the_walk_and_reports_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-depth".to_string());

        // Chain of five: m0 -> m1 -> m2 -> m3 -> m4.
        let ids: Vec<MemoryId> = (0..5)
            .map(|i| seed(&store, &ns, &format!("m{i}")))
            .collect();
        for w in ids.windows(2) {
            store.supersede(&w[0], &w[1]).unwrap();
        }

        let history = store.memory_history(&ns, &ids[2], 1, 200, 200).unwrap();
        let got: Vec<String> = history.chain.iter().map(|e| e.id.to_string()).collect();
        assert_eq!(
            got,
            vec![ids[1].to_string(), ids[2].to_string(), ids[3].to_string()],
            "depth 1 keeps one hop in each direction"
        );
        assert!(history.truncated, "rows remain past the bound");

        // A bound that covers everything reports no truncation.
        let full = store.memory_history(&ns, &ids[2], 10, 200, 200).unwrap();
        assert_eq!(full.chain.len(), 5);
        assert!(!full.truncated);
    }

    /// Namespace purity: a raw cross-namespace supersede pointer is never
    /// followed, and a foreign target id is NotFound (indistinguishable from
    /// missing).
    #[test]
    fn history_never_crosses_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-ns".to_string());
        let other = Namespace::Project("hist-other".to_string());

        let a = seed(&store, &ns, "in-ns decision");
        let foreign = seed(&store, &other, "foreign successor");
        // The raw Store::supersede has no namespace check (the daemon's
        // StoreHandle enforces it); simulate the hostile/corrupt edge.
        store.supersede(&a, &foreign).unwrap();

        let history = store.memory_history(&ns, &a, 100, 200, 200).unwrap();
        assert_eq!(history.chain.len(), 1, "the cross-ns successor never leaks");
        assert_eq!(history.chain[0].id, a);
        // The backward direction is scoped too: history of the foreign row in
        // ITS namespace must not pull in the `ns` ancestor.
        let foreign_history = store
            .memory_history(&other, &foreign, 100, 200, 200)
            .unwrap();
        assert_eq!(foreign_history.chain.len(), 1);

        // A target id from another namespace is NotFound, not a leak.
        let err = store
            .memory_history(&ns, &foreign, 100, 200, 200)
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err:?}");
        // A missing id errors identically.
        let err = store
            .memory_history(&ns, &MemoryId::new(), 100, 200, 200)
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err:?}");
    }

    /// Active contradicts/extends/references edges surface with the far
    /// memory's summary/confidence/contested state; cross-namespace edges and
    /// archived far endpoints never appear (the `active_contradicts` scoping).
    #[test]
    fn history_surfaces_active_edges_with_far_endpoint_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-edges".to_string());
        let other = Namespace::Project("hist-edges-other".to_string());

        let a = seed(&store, &ns, "old decision");
        let b = seed(&store, &ns, "new decision");
        store.supersede(&a, &b).unwrap();

        let rival = seed(&store, &ns, "rival claim");
        let extension = seed(&store, &ns, "extension note");
        let archived_far = seed(&store, &ns, "archived far");
        let foreign = seed(&store, &other, "foreign contradiction");

        // Inbound contradicts + outbound extends on the live head B.
        link(
            &store,
            &rival,
            &b,
            LinkType::Contradicts,
            "benchmarks disagree",
        );
        link(&store, &b, &extension, LinkType::Extends, "adds detail");
        // Noise that must NOT appear: an archived far endpoint, a
        // cross-namespace edge, and an edge on the ARCHIVED chain member A.
        link(&store, &archived_far, &b, LinkType::References, "");
        store.archive_memory(&archived_far).unwrap();
        link(&store, &foreign, &b, LinkType::Contradicts, "cross-ns");
        link(
            &store,
            &rival,
            &a,
            LinkType::Contradicts,
            "on archived member",
        );

        let history = store.memory_history(&ns, &b, 100, 200, 200).unwrap();
        assert_eq!(history.edges.len(), 2, "{:?}", history.edges);

        let contra = history
            .edges
            .iter()
            .find(|e| e.link_type == LinkType::Contradicts)
            .expect("contradicts edge present");
        assert_eq!(contra.local, b);
        assert_eq!(contra.other, rival);
        assert!(
            !contra.outbound,
            "rival -> b is inbound for the chain member"
        );
        assert_eq!(contra.reason, "benchmarks disagree");
        assert_eq!(contra.other_summary, "rival claim");
        assert!((contra.other_confidence - 1.0).abs() < 1e-6);
        assert!(
            contra.other_contested,
            "the rival is contested by the same active pair"
        );

        let ext = history
            .edges
            .iter()
            .find(|e| e.link_type == LinkType::Extends)
            .expect("extends edge present");
        assert_eq!(ext.local, b);
        assert_eq!(ext.other, extension);
        assert!(ext.outbound);
        assert!(!ext.other_contested);

        // The chain member itself carries the contested read flag (W2.2).
        let head = history.chain.iter().find(|e| e.id == b).unwrap();
        assert!(
            head.contested,
            "an active contradicts pair contests the head"
        );
    }

    /// The edge cap bounds the list and reports truncation.
    #[test]
    fn history_edge_cap_bounds_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-edge-cap".to_string());

        let hub = seed(&store, &ns, "hub");
        for i in 0..4 {
            let spoke = seed(&store, &ns, &format!("spoke {i}"));
            link(&store, &spoke, &hub, LinkType::References, "");
        }

        let history = store.memory_history(&ns, &hub, 100, 200, 2).unwrap();
        assert_eq!(history.edges.len(), 2, "edge cap applied");
        assert!(history.truncated, "capped edge list reports truncation");

        let full = store.memory_history(&ns, &hub, 100, 200, 200).unwrap();
        assert_eq!(full.edges.len(), 4);
        assert!(!full.truncated);
    }

    /// A single memory with no supersede history and no links is a one-entry
    /// timeline: itself, current, target.
    #[test]
    fn history_of_an_unversioned_memory_is_itself() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-single".to_string());
        let a = seed(&store, &ns, "lone decision");

        let history = store.memory_history(&ns, &a, 100, 200, 200).unwrap();
        assert_eq!(history.chain.len(), 1);
        let entry = &history.chain[0];
        assert!(entry.current && entry.is_target && !entry.archived);
        assert!(history.edges.is_empty());
        assert!(!history.truncated);
    }

    /// A fully archived chain has NO current-truth member (no false pointer).
    #[test]
    fn history_of_a_fully_archived_chain_has_no_current() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir.path().join("rb.db"));
        let ns = Namespace::Project("hist-dead".to_string());
        let a = seed(&store, &ns, "old");
        let b = seed(&store, &ns, "newer but deleted");
        store.supersede(&a, &b).unwrap();
        store.archive_memory(&b).unwrap();

        let history = store.memory_history(&ns, &a, 100, 200, 200).unwrap();
        assert!(
            history.chain.iter().all(|e| !e.current),
            "no member of an all-archived chain may claim current truth"
        );
    }
}
