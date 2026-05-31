use crate::weights::Weights;
use rb_types::MemoryId;

/// Recency half-life in days: a memory `HALF_LIFE` days old scores `e^-1 ~= 0.368`
/// on the recency term. Documented, fixed constant for deterministic ranking.
pub const HALF_LIFE: f32 = 30.0;

/// Per-candidate raw signals gathered from the three retrieval paths.
///
/// Any `Option` left as `None` means "this path did not surface this candidate";
/// its corresponding term contributes 0 (no penalty).
#[derive(Clone, Debug)]
pub struct Signals {
    pub id: MemoryId,
    /// Keyword rank, 0 = best. `None` if not a keyword hit.
    pub keyword_rank: Option<usize>,
    /// Cosine distance, smaller = closer. `None` if not a vector hit.
    pub vector_distance: Option<f32>,
    /// Graph hops from a seed, 0 = the seed itself. `None` if not graph-reached.
    pub graph_hops: Option<u8>,
    /// Importance 0..=10.
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Normalize a single candidate's signals into a weighted score in `[0, 1]`.
fn score_one(s: &Signals, w: &Weights, now: chrono::DateTime<chrono::Utc>) -> f32 {
    // Vector similarity: cosine distance in [0, 2] -> similarity in [0, 1].
    let vector_sim = match s.vector_distance {
        Some(d) => 1.0 - (d / 2.0).clamp(0.0, 1.0),
        None => 0.0,
    };
    // Keyword: reciprocal rank, 0 = best -> 1.0.
    let keyword = match s.keyword_rank {
        Some(r) => 1.0 / (1.0 + r as f32),
        None => 0.0,
    };
    // Graph proximity: reciprocal hops, 0 hops -> 1.0.
    let graph = match s.graph_hops {
        Some(h) => 1.0 / (1.0 + h as f32),
        None => 0.0,
    };
    // Importance normalized to [0, 1].
    let importance = (s.importance as f32 / 10.0).clamp(0.0, 1.0);
    // Recency: exponential decay over age in days. Future timestamps clamp to age 0.
    let age_days = ((now - s.created_at).num_seconds() as f32 / 86_400.0).max(0.0);
    let recency = (-age_days / HALF_LIFE).exp();

    w.vector * vector_sim
        + w.keyword * keyword
        + w.graph * graph
        + w.importance * importance
        + w.recency * recency
}

/// Rank candidates by weighted, normalized signal score.
///
/// Pure and deterministic: returns `(id, score)` pairs sorted by score descending,
/// truncated to `limit`. `slice::sort_by` is a stable sort, so equal scores preserve
/// input order and output ordering is reproducible across runs. Missing signals
/// contribute 0 (no penalty).
pub fn rank(
    signals: Vec<Signals>,
    weights: Weights,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> Vec<(MemoryId, f32)> {
    let mut scored: Vec<(MemoryId, f32)> = signals
        .iter()
        .map(|s| (s.id.clone(), score_one(s, &weights, now)))
        .collect();
    // Stable sort descending by score; partial_cmp is total here (scores are finite),
    // and we fall back to Equal so a NaN (which cannot occur with finite inputs) does
    // not panic, keeping the function panic-free on all paths.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::{Duration, Utc};
    use rb_types::MemoryId;

    /// Returns the current instant, captured once per test and reused as the single
    /// consistent reference for both `created_at` offsets and the `rank(..., n, ...)`
    /// call, so each test is internally deterministic.
    fn now() -> chrono::DateTime<chrono::Utc> {
        Utc::now()
    }

    #[test]
    fn strong_doc_outranks_weak_doc() {
        let n = now();
        let strong = MemoryId::new();
        let weak = MemoryId::new();
        let signals = vec![
            Signals {
                id: weak.clone(),
                keyword_rank: Some(9),
                vector_distance: Some(1.6), // far -> low sim
                graph_hops: None,
                importance: 2,
                created_at: n - Duration::days(120),
            },
            Signals {
                id: strong.clone(),
                keyword_rank: Some(0),      // best keyword hit
                vector_distance: Some(0.1), // very close
                graph_hops: Some(1),
                importance: 9,
                created_at: n,
            },
        ];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked.len(), 2);
        assert_eq!(
            ranked[0].0, strong,
            "strong vector+keyword doc must rank first"
        );
        assert_eq!(ranked[1].0, weak);
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn recency_breaks_ties_between_otherwise_equal_docs() {
        let n = now();
        let recent = MemoryId::new();
        let old = MemoryId::new();
        // Identical on every signal EXCEPT created_at.
        let mk = |id: MemoryId, created| Signals {
            id,
            keyword_rank: Some(1),
            vector_distance: Some(0.4),
            graph_hops: Some(2),
            importance: 5,
            created_at: created,
        };
        let signals = vec![
            mk(old.clone(), n - Duration::days(200)),
            mk(recent.clone(), n),
        ];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, recent, "more recent doc wins the tie");
        assert_eq!(ranked[1].0, old);
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn graph_only_candidate_still_ranks_above_zero() {
        let n = now();
        let id = MemoryId::new();
        let signals = vec![Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: None,
            graph_hops: Some(0), // adjacent -> graph proximity 1.0
            importance: 0,
            created_at: n - Duration::days(10_000), // ancient -> recency ~0
        }];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0, id);
        // graph weight (0.10) * proximity (1.0) dominates; score strictly > 0.
        assert!(ranked[0].1 > 0.0, "graph-only candidate must score > 0");
    }

    #[test]
    fn missing_signals_do_not_penalize() {
        let n = now();
        // A: only a keyword hit. B: only a vector hit at the SAME normalized strength.
        // keyword Some(0) -> 1.0 ; vector dist 0.0 -> sim 1.0. With default weights
        // keyword(0.30) vs vector(0.45), the vector-only doc should outrank.
        let a = MemoryId::new();
        let b = MemoryId::new();
        let signals = vec![
            Signals {
                id: a.clone(),
                keyword_rank: Some(0),
                vector_distance: None,
                graph_hops: None,
                importance: 0,
                created_at: n - Duration::days(10_000),
            },
            Signals {
                id: b.clone(),
                keyword_rank: None,
                vector_distance: Some(0.0),
                graph_hops: None,
                importance: 0,
                created_at: n - Duration::days(10_000),
            },
        ];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(
            ranked[0].0, b,
            "vector-only (0.45) beats keyword-only (0.30)"
        );
        // The keyword-only doc is NOT penalized to 0: it scores its full keyword term.
        assert!((ranked[1].1 - 0.30).abs() < 1e-5);
    }

    #[test]
    fn limit_truncates_to_top_n() {
        let n = now();
        let signals: Vec<Signals> = (0..5)
            .map(|i| Signals {
                id: MemoryId::new(),
                keyword_rank: Some(i),
                vector_distance: Some(0.1 * i as f32),
                graph_hops: None,
                importance: (10 - i) as u8,
                created_at: n,
            })
            .collect();
        let ranked = rank(signals, Weights::default(), n, 2);
        assert_eq!(ranked.len(), 2, "limit truncates to 2");
        // truncation keeps the two highest scores in descending order
        assert!(ranked[0].1 >= ranked[1].1);
    }

    #[test]
    fn scores_in_range_and_ordering_is_stable() {
        let n = now();
        let signals: Vec<Signals> = (0..20)
            .map(|i| Signals {
                id: MemoryId::new(),
                keyword_rank: if i % 2 == 0 { Some(i) } else { None },
                vector_distance: if i % 3 == 0 {
                    Some(0.05 * i as f32)
                } else {
                    None
                },
                graph_hops: if i % 4 == 0 {
                    Some((i % 5) as u8)
                } else {
                    None
                },
                importance: (i % 11) as u8,
                created_at: n - Duration::days(i as i64),
            })
            .collect();
        let first = rank(signals.clone(), Weights::default(), n, 20);
        // every score is a sane probability-like value in [0,1], non-increasing.
        for w in first.windows(2) {
            assert!(w[0].1 >= w[1].1, "scores must be sorted descending");
        }
        for (_, score) in &first {
            assert!(*score >= 0.0 && *score <= 1.0, "score {score} out of [0,1]");
        }
        // determinism: same inputs -> identical id ordering AND identical scores.
        let second = rank(signals, Weights::default(), n, 20);
        let ids_a: Vec<_> = first.iter().map(|(id, _)| id.clone()).collect();
        let ids_b: Vec<_> = second.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids_a, ids_b, "ordering must be stable across runs");
        for (x, y) in first.iter().zip(second.iter()) {
            assert!(
                (x.1 - y.1).abs() < f32::EPSILON,
                "scores must be reproducible"
            );
        }
    }
}
