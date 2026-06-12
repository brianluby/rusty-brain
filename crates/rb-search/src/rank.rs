use crate::weights::Weights;
use rb_types::MemoryId;

/// Recency half-life in days: a memory `HALF_LIFE` days old scores `e^-1 ~= 0.368`
/// on the recency term. Documented, fixed constant for deterministic ranking.
pub const HALF_LIFE: f32 = 30.0;

/// Minimum `Linear` score a recall result must reach to be returned (W1.3,
/// F30 "junk default recall"). Results scoring below this are dropped so
/// recall may return fewer than `limit` — or nothing — instead of padding
/// with junk; callers render an explicit empty state.
///
/// Derivation (recorded per the W1.3 spec; recalibrate if `Weights::default`
/// or the W1.1 cosine scale changes):
/// 1. Prior-only ceiling: with default weights a candidate carrying ZERO
///    retrieval signal (no keyword hit, no graph hit, vector cosine sim <= 0)
///    scores at most `importance (0.10) + recency (0.05) = 0.15`. The floor
///    must exceed 0.15 so priors alone can never surface a memory.
/// 2. Graph-channel preservation: a fresh default-importance (5) graph-only
///    neighbor scores `0.10 + 0.05 + ~0.05 ~= 0.1999`; the floor must stay
///    below that or the graph channel goes dark for default-importance
///    memories (fights W1.5).
/// 3. Harness upper bound: the weakest golden counted by recall@5 scores
///    0.2724 (real-vector replay) / 0.2800 (deterministic gate); the floor
///    sweep showed recall@5/MRR bit-identical through 0.25 and degrading at
///    0.30, and the floor needs >= 0.05 headroom below the weakest golden
///    (the full recency-aging swing) so aged true hits do not fall through.
/// 4. 0.18 is the midpoint of the admissible (0.15, 0.1999) band: every
///    returned result must carry genuine retrieval signal (vector sim >
///    ~0.067 at default importance, any top-3 keyword rank, or a graph hop),
///    while staying 0.09+ under the weakest golden.
///
/// Applies to `Linear` scores only — `Rrf` scores live on a different scale
/// (`~1/(k + rank)` sums) and stay unfloored until RRF is reachable and
/// calibrated (W2.2/W4.1).
pub const SCORE_FLOOR: f32 = 0.18;

/// Floor for the confidence dampener (Feature C, spec §9). The final score is
/// multiplied by `CONFIDENCE_FLOOR + (1 - CONFIDENCE_FLOOR) * confidence`, so a
/// confidence-1.0 memory is unchanged (no-op) and a confidence-0.0 memory keeps
/// `CONFIDENCE_FLOOR` of its score: meaningfully suppressed, never fully zeroed
/// (the context-poisoning mitigation). Documented, configurable constant — the
/// same floor is applied to the `Rrf` confidence prior (`RrfConfig::confidence_floor`).
pub const CONFIDENCE_FLOOR: f32 = 0.5;

/// Multiplicative confidence dampener shared by `Linear` and `Rrf`.
///
/// Returns `floor + (1 - floor) * confidence`, clamped so that a confidence of
/// `1.0` is a no-op (`1.0`) and a confidence of `0.0` floors at `floor`. A
/// non-finite confidence is sanitized to the neutral `1.0` (never poisons the
/// score). Pure and deterministic.
pub(crate) fn confidence_dampener(confidence: f32, floor: f32) -> f32 {
    let confidence = if confidence.is_finite() {
        confidence.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let floor = if floor.is_finite() {
        floor.clamp(0.0, 1.0)
    } else {
        0.0
    };
    floor + (1.0 - floor) * confidence
}

/// Exponential recency decay over age in days, shared by Linear and RRF.
///
/// Returns `e^(-age_days / HALF_LIFE)` in `(0, 1]`. Future timestamps clamp to
/// age 0 (score 1.0). Pure and deterministic.
pub(crate) fn recency_decay(
    created_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> f32 {
    let age_days = ((now - created_at).num_seconds() as f32 / 86_400.0).max(0.0);
    (-age_days / HALF_LIFE).exp()
}

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
    /// Confidence in `[0, 1]`, sourced from the note. Applied as a multiplicative
    /// dampener in both `Linear` (final score) and `Rrf` (Stage 2 prior), Feature C.
    /// Defaults to `1.0` (neutral) so adding it leaves `Linear` byte-for-byte.
    pub confidence: f32,
}

/// Normalize a single candidate's signals into a weighted score in `[0, 1]`.
fn score_one(s: &Signals, w: &Weights, now: chrono::DateTime<chrono::Utc>) -> f32 {
    // Vector similarity: RAW cosine similarity `1 - d` (cosine distance d in
    // [0, 2]), with anti-correlated candidates (d > 1, cos < 0) clamped to 0.
    // W1.1 recalibration: the store's vec0 index now really measures cosine
    // distance, and an orthogonal — semantically unrelated — candidate (d = 1)
    // must contribute 0 vector signal. The previous `1 - d/2` mapping was
    // calibrated for L2 distances; applied to cosine distances it would award
    // unrelated candidates half the vector weight, drowning keyword-only hits.
    let vector_sim = match s.vector_distance {
        Some(d) if d.is_finite() => (1.0 - d).clamp(0.0, 1.0),
        None => 0.0,
        Some(_) => 0.0,
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
    let recency = recency_decay(s.created_at, now);

    let finite_weight = |weight: f32| if weight.is_finite() { weight } else { 0.0 };
    let score = finite_weight(w.vector) * vector_sim
        + finite_weight(w.keyword) * keyword
        + finite_weight(w.graph) * graph
        + finite_weight(w.importance) * importance
        + finite_weight(w.recency) * recency;

    let score = if score.is_finite() {
        score.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Confidence dampener (Feature C): multiplicatively suppress low-confidence
    // memories on the FINAL score, with a floor so they are never fully zeroed.
    // Neutral at confidence 1.0, so the default corpus is byte-for-byte unchanged.
    score * confidence_dampener(s.confidence, CONFIDENCE_FLOOR)
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
    // Stable sort descending by score; `total_cmp` keeps ordering deterministic for
    // every public input shape, including values sanitized from non-finite signals.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
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
                confidence: 1.0,
            },
            Signals {
                id: strong.clone(),
                keyword_rank: Some(0),      // best keyword hit
                vector_distance: Some(0.1), // very close
                graph_hops: Some(1),
                importance: 9,
                created_at: n,
                confidence: 1.0,
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
            confidence: 1.0,
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
            confidence: 1.0,
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
                confidence: 1.0,
            },
            Signals {
                id: b.clone(),
                keyword_rank: None,
                vector_distance: Some(0.0),
                graph_hops: None,
                importance: 0,
                created_at: n - Duration::days(10_000),
                confidence: 1.0,
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
                confidence: 1.0,
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
                confidence: 1.0,
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

    #[test]
    fn non_finite_vector_distances_score_as_worst_vector_match() {
        let n = now();
        let finite = MemoryId::new();
        let nan = MemoryId::new();
        let infinity = MemoryId::new();
        let weights = Weights {
            vector: 1.0,
            keyword: 0.0,
            graph: 0.0,
            importance: 0.0,
            recency: 0.0,
        };
        let signals = vec![
            Signals {
                id: nan.clone(),
                keyword_rank: None,
                vector_distance: Some(f32::NAN),
                graph_hops: None,
                importance: 10,
                created_at: n,
                confidence: 1.0,
            },
            Signals {
                id: infinity.clone(),
                keyword_rank: None,
                vector_distance: Some(f32::INFINITY),
                graph_hops: None,
                importance: 10,
                created_at: n,
                confidence: 1.0,
            },
            Signals {
                id: finite.clone(),
                keyword_rank: None,
                vector_distance: Some(0.0),
                graph_hops: None,
                importance: 0,
                created_at: n,
                confidence: 1.0,
            },
        ];

        let ranked = rank(signals, weights, n, 10);

        assert_eq!(ranked[0], (finite, 1.0));
        assert_eq!(ranked[1].0, nan);
        assert_eq!(ranked[1].1, 0.0);
        assert_eq!(ranked[2].0, infinity);
        assert_eq!(ranked[2].1, 0.0);
        assert!(ranked.iter().all(|(_, score)| score.is_finite()));
    }

    #[test]
    fn non_finite_weights_are_ignored_and_custom_scores_are_clamped() {
        let n = now();
        let id = MemoryId::new();
        let weights = Weights {
            vector: f32::NAN,
            keyword: f32::INFINITY,
            graph: f32::NEG_INFINITY,
            importance: 2.0,
            recency: -1.0,
        };
        let signals = vec![Signals {
            id: id.clone(),
            keyword_rank: Some(0),
            vector_distance: Some(0.0),
            graph_hops: Some(0),
            importance: 10,
            created_at: n,
            confidence: 1.0,
        }];

        let ranked = rank(signals, weights, n, 10);

        assert_eq!(ranked, vec![(id, 1.0)]);
        assert!(ranked[0].1.is_finite());
    }

    #[test]
    fn low_confidence_ranks_below_equal_high_confidence() {
        // Two candidates identical on every signal EXCEPT confidence. The
        // confidence dampener (Feature C) must rank the low-confidence one below
        // the high-confidence one, but never zero it (floor honored).
        let n = now();
        let low = MemoryId::new();
        let high = MemoryId::new();
        let mk = |id: MemoryId, confidence| Signals {
            id,
            keyword_rank: Some(0),
            vector_distance: Some(0.0),
            graph_hops: Some(0),
            importance: 5,
            created_at: n,
            confidence,
        };
        let signals = vec![mk(low.clone(), 0.0), mk(high.clone(), 1.0)];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, high, "high-confidence ranks first");
        assert_eq!(ranked[1].0, low);
        assert!(ranked[1].1 > 0.0, "floor keeps low-confidence score > 0");
        // With CONFIDENCE_FLOOR = 0.5 and confidence 0.0, the low score is exactly
        // half the high (which is at confidence 1.0 -> dampener no-op).
        assert!((ranked[1].1 - ranked[0].1 * CONFIDENCE_FLOOR).abs() < 1e-6);
    }

    #[test]
    fn confidence_one_is_a_no_op_dampener() {
        // A default-confidence (1.0) candidate scores identically to the
        // pre-Feature-C weighted sum: dampener multiplies by exactly 1.0.
        let n = now();
        let id = MemoryId::new();
        let weights = Weights {
            vector: 1.0,
            keyword: 0.0,
            graph: 0.0,
            importance: 0.0,
            recency: 0.0,
        };
        let signals = vec![Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: Some(0.0), // sim 1.0 -> raw score 1.0
            graph_hops: None,
            importance: 0,
            created_at: n,
            confidence: 1.0,
        }];
        let ranked = rank(signals, weights, n, 10);
        assert_eq!(ranked, vec![(id, 1.0)], "confidence 1.0 => unchanged score");
    }

    #[test]
    fn non_finite_confidence_is_treated_as_neutral() {
        // A corrupt (NaN) confidence must not poison the score: sanitized to the
        // neutral 1.0 dampener, leaving the raw weighted score intact and finite.
        let n = now();
        let id = MemoryId::new();
        let weights = Weights {
            vector: 1.0,
            keyword: 0.0,
            graph: 0.0,
            importance: 0.0,
            recency: 0.0,
        };
        let signals = vec![Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: Some(0.0),
            graph_hops: None,
            importance: 0,
            created_at: n,
            confidence: f32::NAN,
        }];
        let ranked = rank(signals, weights, n, 10);
        assert_eq!(ranked, vec![(id, 1.0)]);
        assert!(ranked[0].1.is_finite());
    }

    #[test]
    fn zero_retrieval_signal_never_clears_the_score_floor() {
        // W1.3 derivation pin (clause 1 of the SCORE_FLOOR docs): the BEST
        // possible candidate with zero retrieval signal — importance 10, brand
        // new, full confidence, no keyword/graph hit, orthogonal vector — caps
        // at importance (0.10) + recency (0.05) = 0.15 < SCORE_FLOOR. Priors
        // alone can never surface a memory.
        let n = now();
        let id = MemoryId::new();
        let signals = vec![Signals {
            id,
            keyword_rank: None,
            vector_distance: Some(1.0), // orthogonal -> cosine sim 0
            graph_hops: None,
            importance: 10,
            created_at: n, // age 0 -> recency 1.0
            confidence: 1.0,
        }];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert!(
            ranked[0].1 < SCORE_FLOOR,
            "prior-only ceiling {} must sit below SCORE_FLOOR {SCORE_FLOOR}",
            ranked[0].1
        );
        // The ceiling itself is exactly 0.15 under default weights.
        assert!((ranked[0].1 - 0.15).abs() < 1e-6);
    }

    #[test]
    fn graph_only_default_importance_hit_clears_the_score_floor() {
        // W1.3 derivation pin (clause 2): a fresh, default-importance (5)
        // graph-only neighbor scores ~0.1999 and must stay ABOVE the floor —
        // otherwise the graph channel goes dark for default-importance
        // memories (fights W1.5).
        let n = now();
        let id = MemoryId::new();
        let signals = vec![Signals {
            id,
            keyword_rank: None,
            vector_distance: None,
            graph_hops: Some(0),
            importance: 5,
            created_at: n,
            confidence: 1.0,
        }];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert!(
            ranked[0].1 > SCORE_FLOOR,
            "graph-only imp-5 fresh hit {} must clear SCORE_FLOOR {SCORE_FLOOR}",
            ranked[0].1
        );
    }

    #[test]
    fn negative_custom_scores_clamp_to_zero() {
        let n = now();
        let id = MemoryId::new();
        let weights = Weights {
            vector: -1.0,
            keyword: 0.0,
            graph: 0.0,
            importance: 0.0,
            recency: 0.0,
        };
        let signals = vec![Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: Some(0.0),
            graph_hops: None,
            importance: 0,
            created_at: n,
            confidence: 1.0,
        }];

        let ranked = rank(signals, weights, n, 10);

        assert_eq!(ranked, vec![(id, 0.0)]);
    }
}
