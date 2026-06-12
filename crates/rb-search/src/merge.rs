use crate::rank::Signals;
use rb_types::MemoryId;
use std::collections::HashMap;

/// Fold the three retrieval result sets into one `Signals` per distinct candidate.
///
/// - `keyword`: ids in rank order (index 0 = best keyword hit).
/// - `vector`: `(id, cosine_distance)` pairs (smaller distance = closer).
/// - `graph`: `(id, hops)` pairs with REAL minimum hop distances from the walk
///   anchor (1 = direct neighbor; the anchor is excluded upstream). W1.5: the
///   hop value is carried verbatim into `Signals.graph_hops` — previously the
///   list INDEX stood in for hops, so the first-listed neighbor scored as if it
///   were the anchor itself and every later one decayed by list position.
/// - `meta`: per-id `(importance, confidence, created_at)`, the source of truth
///   for scoring/fetch. `confidence` (Feature C) flows into `Signals.confidence`
///   and is applied as a multiplicative dampener at rank time.
///
/// Chosen graph decay (W1.5): reciprocal of the real hop distance,
/// `1 / hops` in `rank::score_one` — a direct neighbor (1 hop) earns the FULL
/// graph weight (preserving the SCORE_FLOOR clause-2 invariant that a fresh
/// default-importance direct neighbor clears the floor), 2 hops 1/2, 3 hops
/// 1/3. RRF (`rank_rrf`) derives its graph rank from ascending hops, so
/// closer-in-the-graph beats farther in both fusion modes.
///
/// A candidate may appear in any subset of the three paths; each path fills only its
/// own field, leaving the rest `None` so `rank` contributes 0 for absent signals.
/// Candidates with no `meta` entry are dropped (fail closed - they cannot be scored
/// or later fetched). Output order follows first appearance across keyword, then
/// vector, then graph, which `rank` re-sorts deterministically.
pub fn build_signals(
    keyword: &[MemoryId],
    vector: &[(MemoryId, f32)],
    graph: &[(MemoryId, u8)],
    meta: &HashMap<MemoryId, (u8, f32, chrono::DateTime<chrono::Utc>)>,
) -> Vec<Signals> {
    // Preserve first-seen order with a parallel index map into `out`.
    let mut index: HashMap<MemoryId, usize> = HashMap::new();
    let mut out: Vec<Signals> = Vec::new();

    // The closure captures only `meta` (by shared ref) and takes `index`/`out` as
    // `&mut` parameters per call, so it does NOT need to be `mut` itself. Marking it
    // `mut` would trip the `unused_mut` warning, which is denied under -D warnings.
    let slot = |id: &MemoryId,
                index: &mut HashMap<MemoryId, usize>,
                out: &mut Vec<Signals>|
     -> Option<usize> {
        if let Some(&i) = index.get(id) {
            return Some(i);
        }
        // Only materialize a candidate we have metadata for.
        let (importance, confidence, created_at) = *meta.get(id)?;
        let i = out.len();
        out.push(Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: None,
            graph_hops: None,
            importance,
            // Confidence flows from `meta` (Feature C); applied as a multiplicative
            // dampener at rank time. A confidence of 1.0 is the neutral no-op.
            confidence,
            created_at,
        });
        index.insert(id.clone(), i);
        Some(i)
    };

    for (rank_idx, id) in keyword.iter().enumerate() {
        if let Some(i) = slot(id, &mut index, &mut out) {
            out[i].keyword_rank = Some(out[i].keyword_rank.map_or(rank_idx, |r| r.min(rank_idx)));
        }
    }
    for (id, distance) in vector {
        if let Some(i) = slot(id, &mut index, &mut out) {
            out[i].vector_distance = Some(
                out[i]
                    .vector_distance
                    .map_or(*distance, |d| d.min(*distance)),
            );
        }
    }
    for (id, hops) in graph {
        if let Some(i) = slot(id, &mut index, &mut out) {
            // Real hop distance from the walk anchor (W1.5). A duplicate entry
            // keeps the MINIMUM — closest appearance wins, like the other paths.
            out[i].graph_hops = Some(out[i].graph_hops.map_or(*hops, |h| h.min(*hops)));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::Utc;
    use rb_types::MemoryId;
    use std::collections::HashMap;

    #[test]
    fn merges_overlapping_paths_into_one_signal_per_id() {
        let now = Utc::now();
        let shared = MemoryId::new();
        let kw_only = MemoryId::new();
        let vec_only = MemoryId::new();
        let graph_only = MemoryId::new();

        let mut meta: HashMap<MemoryId, (u8, f32, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for id in [&shared, &kw_only, &vec_only, &graph_only] {
            meta.insert(id.clone(), (5, 1.0, now));
        }

        let keyword = vec![shared.clone(), kw_only.clone()];
        let vector = vec![(shared.clone(), 0.2), (vec_only.clone(), 0.9)];
        let graph = vec![(shared.clone(), 1), (graph_only.clone(), 2)];

        let signals = build_signals(&keyword, &vector, &graph, &meta);
        // one Signals per distinct id (4 total).
        assert_eq!(signals.len(), 4);

        let by_id: HashMap<MemoryId, &Signals> =
            signals.iter().map(|s| (s.id.clone(), s)).collect();

        // shared appears in all three paths: keyword_rank 0 (first), vector dist 0.2, graph hops 1.
        let sh = by_id.get(&shared).unwrap();
        assert_eq!(sh.keyword_rank, Some(0));
        assert!((sh.vector_distance.unwrap() - 0.2).abs() < f32::EPSILON);
        assert_eq!(sh.graph_hops, Some(1));

        // kw_only: keyword_rank 1, no vector, no graph.
        let k = by_id.get(&kw_only).unwrap();
        assert_eq!(k.keyword_rank, Some(1));
        assert!(k.vector_distance.is_none());
        assert!(k.graph_hops.is_none());

        // vec_only: only vector distance 0.9.
        let v = by_id.get(&vec_only).unwrap();
        assert!(v.keyword_rank.is_none());
        assert!((v.vector_distance.unwrap() - 0.9).abs() < f32::EPSILON);
        assert!(v.graph_hops.is_none());

        // graph_only: graph hops 2 (its real walk distance, not list position).
        let g = by_id.get(&graph_only).unwrap();
        assert!(g.keyword_rank.is_none());
        assert!(g.vector_distance.is_none());
        assert_eq!(g.graph_hops, Some(2));
    }

    #[test]
    fn keyword_rank_follows_order_and_graph_hops_carry_real_distances() {
        let now = Utc::now();
        let a = MemoryId::new();
        let b = MemoryId::new();
        let c = MemoryId::new();
        let mut meta = HashMap::new();
        for id in [&a, &b, &c] {
            meta.insert(id.clone(), (5, 1.0, now));
        }
        let keyword = vec![a.clone(), b.clone(), c.clone()];
        // A 3-deep chain seed -> c -> b -> a: hops are 1, 2, 3 (W1.5) and are
        // taken from the PAIR, independent of list position.
        let graph = vec![(c.clone(), 1), (b.clone(), 2), (a.clone(), 3)];
        let signals = build_signals(&keyword, &[], &graph, &meta);
        let by_id: HashMap<MemoryId, &Signals> =
            signals.iter().map(|s| (s.id.clone(), s)).collect();
        // keyword rank = index in keyword vec.
        assert_eq!(by_id.get(&a).unwrap().keyword_rank, Some(0));
        assert_eq!(by_id.get(&b).unwrap().keyword_rank, Some(1));
        assert_eq!(by_id.get(&c).unwrap().keyword_rank, Some(2));
        // graph hops = the real distance carried in the pair.
        assert_eq!(by_id.get(&c).unwrap().graph_hops, Some(1));
        assert_eq!(by_id.get(&b).unwrap().graph_hops, Some(2));
        assert_eq!(by_id.get(&a).unwrap().graph_hops, Some(3));
    }

    #[test]
    fn duplicate_keyword_ids_keep_earliest_rank() {
        let now = Utc::now();
        let duplicate = MemoryId::new();
        let earlier = MemoryId::new();
        let mut meta = HashMap::new();
        for id in [&duplicate, &earlier] {
            meta.insert(id.clone(), (5, 1.0, now));
        }

        let keyword = vec![duplicate.clone(), earlier, duplicate.clone()];
        let signals = build_signals(&keyword, &[], &[], &meta);
        let by_id: HashMap<MemoryId, &Signals> =
            signals.iter().map(|s| (s.id.clone(), s)).collect();

        assert_eq!(by_id.get(&duplicate).unwrap().keyword_rank, Some(0));
    }

    #[test]
    fn duplicate_vector_ids_keep_smallest_distance() {
        let now = Utc::now();
        let duplicate = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert(duplicate.clone(), (5, 1.0, now));

        let vector = vec![(duplicate.clone(), 0.8), (duplicate.clone(), 0.2)];
        let signals = build_signals(&[], &vector, &[], &meta);

        assert!((signals[0].vector_distance.unwrap() - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn duplicate_graph_ids_keep_minimum_hops() {
        // Diamond shape: two paths reach the same node at different depths; the
        // SHORTER distance must win regardless of list order (W1.5).
        let now = Utc::now();
        let duplicate = MemoryId::new();
        let other = MemoryId::new();
        let mut meta = HashMap::new();
        for id in [&duplicate, &other] {
            meta.insert(id.clone(), (5, 1.0, now));
        }

        let graph = vec![(duplicate.clone(), 3), (other, 1), (duplicate.clone(), 2)];
        let signals = build_signals(&[], &[], &graph, &meta);
        let by_id: HashMap<MemoryId, &Signals> =
            signals.iter().map(|s| (s.id.clone(), s)).collect();

        assert_eq!(by_id.get(&duplicate).unwrap().graph_hops, Some(2));
    }

    #[test]
    fn carries_importance_confidence_and_created_at_from_meta() {
        let created = Utc::now();
        let id = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert(id.clone(), (8u8, 0.4f32, created));
        let signals = build_signals(std::slice::from_ref(&id), &[], &[], &meta);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].importance, 8);
        // confidence now flows from meta (Feature C), not a hardcoded neutral.
        assert!((signals[0].confidence - 0.4).abs() < f32::EPSILON);
        assert_eq!(signals[0].created_at, created);
    }

    #[test]
    fn candidates_absent_from_meta_are_dropped() {
        let now = Utc::now();
        let known = MemoryId::new();
        let unknown = MemoryId::new(); // appears in results but has no meta entry
        let mut meta = HashMap::new();
        meta.insert(known.clone(), (5, 1.0, now));
        let keyword = vec![known.clone(), unknown.clone()];
        let signals = build_signals(&keyword, &[], &[], &meta);
        // the unknown id is dropped (cannot be scored or fetched) -> fail closed.
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].id, known);
    }

    #[test]
    fn output_feeds_rank_end_to_end() {
        let now = Utc::now();
        let strong = MemoryId::new();
        let weak = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert(strong.clone(), (9u8, 1.0, now));
        meta.insert(weak.clone(), (2u8, 1.0, now));
        let keyword = vec![strong.clone(), weak.clone()];
        let vector = vec![(strong.clone(), 0.1), (weak.clone(), 1.7)];
        let signals = build_signals(&keyword, &vector, &[], &meta);
        let ranked = crate::rank::rank(signals, crate::weights::Weights::default(), now, 10);
        assert_eq!(ranked.len(), 2);
        assert_eq!(
            ranked[0].0, strong,
            "merge + rank place the strong doc first"
        );
    }
}
