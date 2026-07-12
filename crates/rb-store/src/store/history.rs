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

impl SqliteStore {
    /// Derive the decision-history timeline for `id` in `ns`: the supersede
    /// chain in BOTH directions (each direction bounded by `depth` hops) plus
    /// the active `contradicts`/`extends`/`references` edges touching chain
    /// members (bounded by `edge_limit`). Pure reads — safe on a read-pool
    /// connection.
    ///
    /// Semantics guarantees:
    /// - Namespace purity: every chain member and BOTH endpoints of every edge
    ///   live in `ns` (`memory_links` carries no namespace and cross-namespace
    ///   pointers/edges exist, so each hop and both edge endpoints are scoped
    ///   explicitly — the `active_contradicts` rigor). A target id living in
    ///   another namespace is indistinguishable from a missing one: `NotFound`.
    /// - Cycle defense: `superseded_by` is user-influencable data; a visited
    ///   set plus the depth cap make the walk terminate on any input.
    /// - `truncated` reports a depth- or edge-cap stop with rows remaining.
    pub fn memory_history(
        &self,
        ns: &Namespace,
        id: &MemoryId,
        depth: u32,
        edge_limit: usize,
    ) -> Result<MemoryHistory> {
        let ns_str = ns.as_db_string();
        let target = self
            .history_row(&ns_str, id)?
            .ok_or_else(|| Error::NotFound(id.clone()))?;

        let mut truncated = false;
        let mut visited: HashSet<MemoryId> = HashSet::new();
        visited.insert(id.clone());

        // Forward walk: follow the superseded_by pointer (old -> new). A
        // pointer that leaves the namespace, dangles, or closes a cycle ends
        // the walk; only a depth stop with a live pointer marks truncation.
        let mut successors: Vec<ChainRow> = Vec::new();
        let mut cursor = target.entry.superseded_by.clone();
        while let Some(next_id) = cursor {
            if successors.len() as u32 >= depth {
                truncated = true;
                break;
            }
            if !visited.insert(next_id.clone()) {
                break; // cycle: already on the chain
            }
            match self.history_row(&ns_str, &next_id)? {
                Some(row) => {
                    cursor = row.entry.superseded_by.clone();
                    successors.push(row);
                }
                None => break, // cross-namespace or dangling pointer: never leak
            }
        }

        // Backward walk: BFS over predecessors (rows whose superseded_by
        // points INTO the frontier). Multiple predecessors per hop are
        // possible (near-dup absorption fans in), so this is a level walk,
        // not a pointer chase.
        let mut ancestors: Vec<(u32, ChainRow)> = Vec::new();
        let mut frontier: Vec<MemoryId> = vec![id.clone()];
        for hop in 0..=depth {
            if frontier.is_empty() {
                break;
            }
            let rows = self.predecessor_rows(&ns_str, &frontier)?;
            let mut next_frontier = Vec::new();
            for row in rows {
                if !visited.insert(row.entry.id.clone()) {
                    continue; // cycle or already-collected member
                }
                if hop >= depth {
                    // A probe level past the bound: rows remain, do not emit.
                    truncated = true;
                    continue;
                }
                next_frontier.push(row.entry.id.clone());
                ancestors.push((hop, row));
            }
            frontier = next_frontier;
        }
        // Timeline order (HIST-1: "ordered by time"): ancestors oldest first.
        // The hop distance is the primary key — `created_at` has one-second
        // resolution, so a fast A->B->C chain would tie on time and shuffle;
        // deeper ancestors are by construction older. Time then id break ties
        // WITHIN a fan-in level (near-dup absorption).
        ancestors.sort_by(|(ah, a), (bh, b)| {
            bh.cmp(ah)
                .then_with(|| a.entry.created_at.cmp(&b.entry.created_at))
                .then_with(|| a.entry.id.to_string().cmp(&b.entry.id.to_string()))
        });

        let mut chain: Vec<HistoryEntry> = ancestors
            .into_iter()
            .map(|(_, r)| r.entry)
            .chain(std::iter::once(target.entry))
            .chain(successors.into_iter().map(|r| r.entry))
            .collect();

        // Edges: active links touching any chain member, both endpoints
        // scoped to `ns` and non-archived (the `active_contradicts` rigor).
        let chain_ids: Vec<MemoryId> = chain.iter().map(|e| e.id.clone()).collect();
        let (mut edges, edges_truncated) = self.chain_edges(&ns_str, &chain_ids, edge_limit)?;
        truncated |= edges_truncated;

        // Contested flags for chain members AND far endpoints in one batch.
        let mut flag_ids = chain_ids.clone();
        flag_ids.extend(edges.iter().map(|e| e.other.clone()));
        flag_ids.sort_by_key(|i| i.to_string());
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

    /// Rows whose `superseded_by` points at any id in `frontier`, scoped to
    /// `ns`, ordered deterministically.
    fn predecessor_rows(&self, ns_str: &str, frontier: &[MemoryId]) -> Result<Vec<ChainRow>> {
        if frontier.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> =
            (0..frontier.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "SELECT memory_id, summary, content, importance, confidence, created_at,
                    archived_at, superseded_by, origin_user, origin_host, origin_agent,
                    origin_source
             FROM memories
             WHERE namespace = ?1 AND superseded_by IN ({})
             ORDER BY created_at ASC, memory_id ASC",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(frontier.len() + 1);
        params.push(Box::new(ns_str.to_string()));
        for id in frontier {
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
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            out.push(decode_chain_row(row)?);
        }
        Ok(out)
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
    let importance = u8::try_from(row.get::<_, i64>(3).map_err(storage)?).unwrap_or(0);
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

        let history = store.memory_history(&ns, &b, 100, 200).unwrap();
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

        let history = store.memory_history(&ns, &c, 100, 200).unwrap();
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

        let history = store.memory_history(&ns, &b, 100, 200).unwrap();
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
        store.supersede(&b, &a).unwrap(); // closes the cycle A -> B -> A

        for anchor in [&a, &b] {
            let history = store.memory_history(&ns, anchor, 100, 200).unwrap();
            let mut ids: Vec<String> = history.chain.iter().map(|e| e.id.to_string()).collect();
            let len_before = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), len_before, "no duplicate chain members");
            assert_eq!(history.chain.len(), 2, "both cycle members, exactly once");
        }
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

        let history = store.memory_history(&ns, &ids[2], 1, 200).unwrap();
        let got: Vec<String> = history.chain.iter().map(|e| e.id.to_string()).collect();
        assert_eq!(
            got,
            vec![ids[1].to_string(), ids[2].to_string(), ids[3].to_string()],
            "depth 1 keeps one hop in each direction"
        );
        assert!(history.truncated, "rows remain past the bound");

        // A bound that covers everything reports no truncation.
        let full = store.memory_history(&ns, &ids[2], 10, 200).unwrap();
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

        let history = store.memory_history(&ns, &a, 100, 200).unwrap();
        assert_eq!(history.chain.len(), 1, "the cross-ns successor never leaks");
        assert_eq!(history.chain[0].id, a);
        // The backward direction is scoped too: history of the foreign row in
        // ITS namespace must not pull in the `ns` ancestor.
        let foreign_history = store.memory_history(&other, &foreign, 100, 200).unwrap();
        assert_eq!(foreign_history.chain.len(), 1);

        // A target id from another namespace is NotFound, not a leak.
        let err = store.memory_history(&ns, &foreign, 100, 200).unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err:?}");
        // A missing id errors identically.
        let err = store
            .memory_history(&ns, &MemoryId::new(), 100, 200)
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

        let history = store.memory_history(&ns, &b, 100, 200).unwrap();
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

        let history = store.memory_history(&ns, &hub, 100, 2).unwrap();
        assert_eq!(history.edges.len(), 2, "edge cap applied");
        assert!(history.truncated, "capped edge list reports truncation");

        let full = store.memory_history(&ns, &hub, 100, 200).unwrap();
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

        let history = store.memory_history(&ns, &a, 100, 200).unwrap();
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

        let history = store.memory_history(&ns, &a, 100, 200).unwrap();
        assert!(
            history.chain.iter().all(|e| !e.current),
            "no member of an all-archived chain may claim current truth"
        );
    }
}
