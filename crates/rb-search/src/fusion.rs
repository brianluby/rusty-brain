//! RRF (Reciprocal Rank Fusion) two-stage hybrid ranking.
//!
//! `Linear` (the existing `rank`) stays the default; this module adds the
//! opt-in `Rrf` path (spec §7). It is pure and deterministic — no IO, no async —
//! mirroring `rank`'s non-finite sanitization and stable-sort discipline.
//!
//! - **Stage 1 (rank fusion):** each retrieval path (keyword / vector / graph)
//!   contributes a ranked candidate list. The fused score is
//!   `Σ_paths 1 / (k + rank_path)` with a documented default `k = 60`. A
//!   candidate missing from a path contributes nothing (no penalty), matching the
//!   "missing signal = 0" rule of `rank`.
//! - **Stage 2 (priors):** `importance`, `recency` (the 30-day-half-life
//!   exponential), and `confidence` are applied as multiplicative priors on the
//!   fused score, each behind a documented, configurable weight.

use crate::rank::{confidence_dampener, recency_decay, Signals};
use rb_types::MemoryId;

/// Which fusion strategy `recall` uses. `Linear` is the default and is the
/// existing `rank`; `Rrf` is the two-stage hybrid in this module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FusionMode {
    /// Weighted linear sum of normalized signals (`rank`). Default.
    #[default]
    Linear,
    /// Reciprocal Rank Fusion of the path rank lists, then metadata priors.
    Rrf,
}

/// Documented, configurable knobs for the `Rrf` path.
///
/// `k` is the RRF constant (de-facto default 60: larger `k` flattens the
/// contribution of rank position). The three prior weights are each in `[0, 1]`
/// and blend their prior toward neutral: a weight of `0.0` makes the prior a
/// no-op, `1.0` applies it fully (`effective = (1 - w) + w * prior`). The
/// `confidence_floor` bounds how far a low-confidence memory is suppressed
/// (never to zero), matching Feature C's dampener contract.
#[derive(Clone, Copy, Debug)]
pub struct RrfConfig {
    /// RRF constant `k`. Documented default `60.0`. Clamped at use to a small
    /// positive floor (`MIN_K`) so a degenerate `k <= 0` cannot divide by zero.
    pub k: f32,
    /// Weight of the importance prior in `[0, 1]`.
    pub importance: f32,
    /// Weight of the recency prior in `[0, 1]`.
    pub recency: f32,
    /// Weight of the confidence prior in `[0, 1]`.
    pub confidence: f32,
    /// Floor for the confidence prior in `[0, 1]` (e.g. `0.5`).
    pub confidence_floor: f32,
}

impl Default for RrfConfig {
    fn default() -> Self {
        Self {
            k: 60.0,
            importance: 0.5,
            recency: 0.25,
            confidence: 1.0,
            confidence_floor: 0.5,
        }
    }
}

/// Sanitize a weight/parameter: non-finite -> `0.0` (mirrors `rank`).
fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Minimum effective RRF `k`. The fused term is `1 / (k + rank)`; with `k = 0`
/// the rank-0 candidate divides by zero -> non-finite, which `apply_priors` then
/// sanitizes to `0.0` and so *inverts* the best candidate below rank-1. A tiny
/// positive floor keeps every term finite and preserves correct best-first
/// ordering for any in-range `k`; it only clamps the degenerate `k <= 0` case.
const MIN_K: f32 = 1e-6;

/// Derive ascending rank positions for the candidates that carry `key`.
///
/// Candidates with `None` for the path key get no entry (they contribute
/// nothing to that path). Ties are broken by input order (the index), so the
/// derivation is stable and deterministic. Returns `id -> rank` where rank `0`
/// is the best (smallest key).
fn derive_ranks<K, F>(signals: &[Signals], key: F) -> std::collections::HashMap<MemoryId, usize>
where
    K: PartialOrd + Copy,
    F: Fn(&Signals) -> Option<K>,
{
    // (index, id, key) for every candidate that has this path's signal.
    let mut keyed: Vec<(usize, &MemoryId, K)> = signals
        .iter()
        .enumerate()
        .filter_map(|(i, s)| key(s).map(|k| (i, &s.id, k)))
        .collect();
    // Ascending by key; ties keep input order (stable on the original index).
    keyed.sort_by(|a, b| match a.2.partial_cmp(&b.2) {
        Some(ord) => ord,
        // Non-comparable (NaN) sinks to the end deterministically.
        None => std::cmp::Ordering::Greater,
    });
    keyed
        .into_iter()
        .enumerate()
        .map(|(rank, (_, id, _))| (id.clone(), rank))
        .collect()
}

/// Stage 1: fused RRF score for one candidate over the three path-rank maps.
fn rrf_fuse(
    s: &Signals,
    k: f32,
    vector_ranks: &std::collections::HashMap<MemoryId, usize>,
    graph_ranks: &std::collections::HashMap<MemoryId, usize>,
) -> f32 {
    let term = |rank: usize| 1.0 / (k + rank as f32);
    let mut fused = 0.0;
    // Keyword path: the existing position index is the rank directly.
    if let Some(r) = s.keyword_rank {
        fused += term(r);
    }
    if let Some(&r) = vector_ranks.get(&s.id) {
        fused += term(r);
    }
    if let Some(&r) = graph_ranks.get(&s.id) {
        fused += term(r);
    }
    fused
}

/// Stage 2: multiplicative metadata priors on the fused score.
fn apply_priors(
    fused: f32,
    s: &Signals,
    cfg: &RrfConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> f32 {
    // Each prior blends toward neutral by its (sanitized, clamped) weight.
    let blend = |weight: f32, prior: f32| {
        let w = finite_or_zero(weight).clamp(0.0, 1.0);
        let p = finite_or_zero(prior).clamp(0.0, 1.0);
        (1.0 - w) + w * p
    };

    let importance_prior = (s.importance as f32 / 10.0).clamp(0.0, 1.0);
    let recency_prior = recency_decay(s.created_at, now);
    // Confidence dampener with a floor: a low-confidence memory is suppressed but
    // never zeroed. Neutral at confidence 1.0. Shared with the `Linear` dampener
    // (`confidence_dampener`) so both fusion modes suppress identically.
    let confidence_prior = confidence_dampener(s.confidence, cfg.confidence_floor);

    let scored = fused
        * blend(cfg.importance, importance_prior)
        * blend(cfg.recency, recency_prior)
        * blend(cfg.confidence, confidence_prior);

    if scored.is_finite() {
        scored.max(0.0)
    } else {
        0.0
    }
}

/// Rank candidates by RRF (Stage 1) with metadata priors (Stage 2).
///
/// Pure and deterministic: returns `(id, score)` pairs sorted by score
/// descending, truncated to `limit`. Equal scores preserve input order (stable
/// sort with `total_cmp`). Missing path signals contribute nothing; non-finite
/// inputs are sanitized exactly as in `rank`.
pub fn rank_rrf(
    signals: Vec<Signals>,
    cfg: RrfConfig,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> Vec<(MemoryId, f32)> {
    let k = finite_or_zero(cfg.k).max(MIN_K);
    // Stage 1 inputs: vector rank from ascending distance, graph rank from
    // ascending hops. Keyword already carries its position in `keyword_rank`.
    let vector_ranks = derive_ranks(&signals, |s| s.vector_distance.filter(|d| d.is_finite()));
    let graph_ranks = derive_ranks(&signals, |s| s.graph_hops);

    let mut scored: Vec<(MemoryId, f32)> = signals
        .iter()
        .map(|s| {
            let fused = rrf_fuse(s, k, &vector_ranks, &graph_ranks);
            (s.id.clone(), apply_priors(fused, s, &cfg, now))
        })
        .collect();
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

    fn now() -> chrono::DateTime<chrono::Utc> {
        Utc::now()
    }

    /// A neutral config: priors fully off (weights 0), so we test Stage 1 alone.
    fn stage1_only() -> RrfConfig {
        RrfConfig {
            k: 60.0,
            importance: 0.0,
            recency: 0.0,
            confidence: 0.0,
            confidence_floor: 0.5,
        }
    }

    fn sig(
        id: MemoryId,
        keyword_rank: Option<usize>,
        vector_distance: Option<f32>,
        graph_hops: Option<u8>,
        importance: u8,
        created_at: chrono::DateTime<chrono::Utc>,
        confidence: f32,
    ) -> Signals {
        Signals {
            id,
            keyword_rank,
            vector_distance,
            graph_hops,
            importance,
            created_at,
            confidence,
        }
    }

    #[test]
    fn fusion_mode_default_is_linear() {
        assert_eq!(FusionMode::default(), FusionMode::Linear);
    }

    #[test]
    fn rrf_config_default_k_is_sixty() {
        assert!((RrfConfig::default().k - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rrf_arithmetic_on_known_single_path_lists() {
        // Three candidates, keyword-only, at positions 0,1,2. With priors off,
        // the score is exactly 1/(k+rank).
        let n = now();
        let a = MemoryId::new();
        let b = MemoryId::new();
        let c = MemoryId::new();
        let signals = vec![
            sig(a.clone(), Some(0), None, None, 5, n, 1.0),
            sig(b.clone(), Some(1), None, None, 5, n, 1.0),
            sig(c.clone(), Some(2), None, None, 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, stage1_only(), n, 10);
        let k = 60.0_f32;
        assert_eq!(ranked[0].0, a);
        assert!((ranked[0].1 - 1.0 / (k + 0.0)).abs() < 1e-6);
        assert!((ranked[1].1 - 1.0 / (k + 1.0)).abs() < 1e-6);
        assert!((ranked[2].1 - 1.0 / (k + 2.0)).abs() < 1e-6);
    }

    #[test]
    fn rrf_sums_contributions_across_paths() {
        // One candidate present in all three paths at rank 0 in each ⇒ score 3/k.
        // Another present only in keyword at rank 0 ⇒ score 1/k. The multi-path
        // candidate must outrank, and its score is exactly the sum.
        let n = now();
        let multi = MemoryId::new();
        let single = MemoryId::new();
        let signals = vec![
            sig(multi.clone(), Some(0), Some(0.1), Some(0), 5, n, 1.0),
            sig(single.clone(), Some(0), None, None, 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, stage1_only(), n, 10);
        let k = 60.0_f32;
        assert_eq!(ranked[0].0, multi, "multi-path candidate fuses higher");
        assert!((ranked[0].1 - 3.0 / k).abs() < 1e-6);
        assert!((ranked[1].1 - 1.0 / k).abs() < 1e-6);
    }

    #[test]
    fn vector_rank_derives_from_ascending_distance() {
        // Two vector-only candidates: the closer (smaller distance) must get the
        // better (rank 0) RRF contribution regardless of input order.
        let n = now();
        let far = MemoryId::new();
        let close = MemoryId::new();
        let signals = vec![
            sig(far.clone(), None, Some(1.8), None, 5, n, 1.0),
            sig(close.clone(), None, Some(0.05), None, 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, stage1_only(), n, 10);
        let k = 60.0_f32;
        assert_eq!(ranked[0].0, close, "smaller distance ⇒ rank 0");
        assert!((ranked[0].1 - 1.0 / k).abs() < 1e-6);
        assert!((ranked[1].1 - 1.0 / (k + 1.0)).abs() < 1e-6);
    }

    #[test]
    fn graph_rank_derives_from_ascending_hops() {
        let n = now();
        let near = MemoryId::new();
        let far = MemoryId::new();
        // Input order deliberately far-then-near; hops decide the rank.
        let signals = vec![
            sig(far.clone(), None, None, Some(3), 5, n, 1.0),
            sig(near.clone(), None, None, Some(0), 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, stage1_only(), n, 10);
        assert_eq!(ranked[0].0, near, "fewer hops ⇒ better rank");
    }

    #[test]
    fn missing_from_a_list_contributes_nothing() {
        // A candidate absent from every path scores 0 (no penalty, no credit) and
        // sinks below any candidate with a single path hit.
        let n = now();
        let hit = MemoryId::new();
        let ghost = MemoryId::new();
        let signals = vec![
            sig(ghost.clone(), None, None, None, 10, n, 1.0),
            sig(hit.clone(), Some(0), None, None, 0, n, 1.0),
        ];
        let ranked = rank_rrf(signals, stage1_only(), n, 10);
        assert_eq!(ranked[0].0, hit);
        assert_eq!(ranked[1].0, ghost);
        assert_eq!(ranked[1].1, 0.0, "no path hit ⇒ exactly zero, no penalty");
    }

    #[test]
    fn importance_prior_breaks_ties_when_enabled() {
        // Identical Stage-1 score (both keyword rank 0); importance prior (weight
        // on) lifts the higher-importance candidate.
        let n = now();
        let low = MemoryId::new();
        let high = MemoryId::new();
        let cfg = RrfConfig {
            importance: 1.0,
            recency: 0.0,
            confidence: 0.0,
            ..RrfConfig::default()
        };
        let signals = vec![
            sig(low.clone(), Some(0), None, None, 1, n, 1.0),
            sig(high.clone(), Some(0), None, None, 10, n, 1.0),
        ];
        let ranked = rank_rrf(signals, cfg, n, 10);
        assert_eq!(
            ranked[0].0, high,
            "higher importance wins via Stage-2 prior"
        );
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn recency_prior_breaks_ties_when_enabled() {
        let n = now();
        let old = MemoryId::new();
        let fresh = MemoryId::new();
        let cfg = RrfConfig {
            importance: 0.0,
            recency: 1.0,
            confidence: 0.0,
            ..RrfConfig::default()
        };
        let signals = vec![
            sig(
                old.clone(),
                Some(0),
                None,
                None,
                5,
                n - Duration::days(300),
                1.0,
            ),
            sig(fresh.clone(), Some(0), None, None, 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, cfg, n, 10);
        assert_eq!(ranked[0].0, fresh, "more recent wins via recency prior");
    }

    #[test]
    fn confidence_prior_suppresses_low_confidence_but_never_zeroes() {
        // Equal Stage-1 score; the low-confidence memory must rank below the
        // high-confidence one, but keep a strictly positive score (floor honored).
        let n = now();
        let low = MemoryId::new();
        let high = MemoryId::new();
        let cfg = RrfConfig {
            importance: 0.0,
            recency: 0.0,
            confidence: 1.0,
            confidence_floor: 0.5,
            ..RrfConfig::default()
        };
        let signals = vec![
            sig(low.clone(), Some(0), None, None, 5, n, 0.0),
            sig(high.clone(), Some(0), None, None, 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, cfg, n, 10);
        assert_eq!(ranked[0].0, high, "high confidence ranks first");
        assert_eq!(ranked[1].0, low);
        assert!(ranked[1].1 > 0.0, "floor keeps low-confidence score > 0");
        // With floor 0.5 and confidence 0.0, the low score is exactly half the high.
        assert!((ranked[1].1 - ranked[0].1 * 0.5).abs() < 1e-6);
    }

    #[test]
    fn confidence_neutral_at_one_is_a_no_op() {
        // Default-confidence (1.0) candidates score identically whether or not the
        // confidence prior is enabled — proving the neutral default.
        let n = now();
        let id = MemoryId::new();
        let with = RrfConfig {
            importance: 0.0,
            recency: 0.0,
            confidence: 1.0,
            ..RrfConfig::default()
        };
        let without = RrfConfig {
            confidence: 0.0,
            ..with
        };
        let a = rank_rrf(
            vec![sig(id.clone(), Some(0), None, None, 5, n, 1.0)],
            with,
            n,
            10,
        );
        let b = rank_rrf(
            vec![sig(id.clone(), Some(0), None, None, 5, n, 1.0)],
            without,
            n,
            10,
        );
        assert!(
            (a[0].1 - b[0].1).abs() < 1e-9,
            "confidence 1.0 ⇒ prior is no-op"
        );
    }

    #[test]
    fn determinism_same_input_identical_order_and_scores() {
        let n = now();
        let signals: Vec<Signals> = (0..20)
            .map(|i| {
                sig(
                    MemoryId::new(),
                    if i % 2 == 0 { Some(i) } else { None },
                    if i % 3 == 0 {
                        Some(0.05 * i as f32)
                    } else {
                        None
                    },
                    if i % 4 == 0 {
                        Some((i % 5) as u8)
                    } else {
                        None
                    },
                    (i % 11) as u8,
                    n - Duration::days(i as i64),
                    1.0,
                )
            })
            .collect();
        let first = rank_rrf(signals.clone(), RrfConfig::default(), n, 20);
        let second = rank_rrf(signals, RrfConfig::default(), n, 20);
        let ids_a: Vec<_> = first.iter().map(|(id, _)| id.clone()).collect();
        let ids_b: Vec<_> = second.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids_a, ids_b, "ordering must be stable across runs");
        for (x, y) in first.iter().zip(second.iter()) {
            assert!((x.1 - y.1).abs() < f32::EPSILON, "scores reproducible");
        }
        // sorted descending, every score finite.
        for w in first.windows(2) {
            assert!(w[0].1 >= w[1].1, "descending");
        }
        assert!(first.iter().all(|(_, s)| s.is_finite()));
    }

    #[test]
    fn equal_scores_preserve_input_order() {
        // Two candidates with identical signals ⇒ identical scores; stable sort
        // keeps input order.
        let n = now();
        let first_in = MemoryId::new();
        let second_in = MemoryId::new();
        let signals = vec![
            sig(first_in.clone(), Some(0), None, None, 5, n, 1.0),
            sig(second_in.clone(), Some(0), None, None, 5, n, 1.0),
        ];
        let ranked = rank_rrf(signals, RrfConfig::default(), n, 10);
        assert_eq!(ranked[0].0, first_in);
        assert_eq!(ranked[1].0, second_in);
        assert!((ranked[0].1 - ranked[1].1).abs() < f32::EPSILON);
    }

    #[test]
    fn non_finite_inputs_are_sanitized_like_rank() {
        // NaN distance ⇒ not a vector hit (no contribution). NaN confidence ⇒
        // neutral. Non-finite k ⇒ sanitized to 0. Scores stay finite and >= 0.
        let n = now();
        let nan_dist = MemoryId::new();
        let nan_conf = MemoryId::new();
        let clean = MemoryId::new();
        let signals = vec![
            sig(nan_dist.clone(), Some(0), Some(f32::NAN), None, 5, n, 1.0),
            sig(nan_conf.clone(), Some(0), None, None, 5, n, f32::NAN),
            sig(clean.clone(), Some(0), Some(0.1), None, 5, n, 1.0),
        ];
        let cfg = RrfConfig {
            k: f32::NAN,
            importance: f32::INFINITY,
            recency: f32::NAN,
            confidence: f32::NEG_INFINITY,
            confidence_floor: f32::NAN,
        };
        let ranked = rank_rrf(signals, cfg, n, 10);
        assert_eq!(ranked.len(), 3);
        assert!(
            ranked.iter().all(|(_, s)| s.is_finite() && *s >= 0.0),
            "all scores finite and non-negative after sanitization"
        );
    }

    #[test]
    fn k_zero_does_not_invert_or_produce_non_finite_scores() {
        // A degenerate k = 0.0 must NOT send the rank-0 term to infinity (which the
        // sanitizer would flip to 0.0 and push the best candidate below rank-1).
        // The clamp to MIN_K keeps scores finite and preserves best-first ordering.
        let n = now();
        let best = MemoryId::new();
        let next = MemoryId::new();
        let signals = vec![
            sig(best.clone(), Some(0), None, None, 5, n, 1.0),
            sig(next.clone(), Some(1), None, None, 5, n, 1.0),
        ];
        let cfg = RrfConfig {
            k: 0.0,
            ..stage1_only()
        };
        let ranked = rank_rrf(signals, cfg, n, 10);
        assert!(
            ranked.iter().all(|(_, s)| s.is_finite()),
            "k=0 must not yield non-finite scores"
        );
        assert_eq!(
            ranked[0].0, best,
            "rank-0 candidate must stay first, not invert"
        );
        assert!(
            ranked[0].1 > ranked[1].1,
            "best candidate keeps the strictly higher score"
        );
    }

    #[test]
    fn limit_truncates_to_top_n() {
        let n = now();
        let signals: Vec<Signals> = (0..5)
            .map(|i| sig(MemoryId::new(), Some(i), None, None, 5, n, 1.0))
            .collect();
        let ranked = rank_rrf(signals, RrfConfig::default(), n, 2);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].1 >= ranked[1].1);
    }
}
