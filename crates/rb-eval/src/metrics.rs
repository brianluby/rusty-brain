//! Pure ranking-quality metrics.
//!
//! Every function here is deterministic and side-effect-free so the harness's
//! pass/fail decision is reproducible. The inputs are *ranked id lists* (best
//! first) paired with sets of expected-relevant ids, plus raw latency samples.
//!
//! These metrics intentionally measure *relative ordering and regression*, not
//! absolute semantic quality (see the crate README): with `DeterministicProvider`
//! the vectors are stable but not semantic, so the value of a metric is only
//! meaningful as a baseline to detect change, not as a ground-truth score.

/// Fraction of expected-relevant ids that appear in the top `k` of `ranked`.
///
/// `ranked` is best-first. `expected` is the set that *should* be retrieved.
/// Returns a value in `[0.0, 1.0]`. An empty `expected` set is treated as a
/// perfect match (`1.0`) — there is nothing to miss. `k == 0` yields `0.0`.
pub fn recall_at_k(ranked: &[String], expected: &[String], k: usize) -> f64 {
    if expected.is_empty() {
        return 1.0;
    }
    if k == 0 {
        return 0.0;
    }
    let top: std::collections::HashSet<&String> = ranked.iter().take(k).collect();
    let hits = expected.iter().filter(|e| top.contains(e)).count();
    hits as f64 / expected.len() as f64
}

/// Mean Reciprocal Rank over a set of queries.
///
/// For each query, the reciprocal rank is `1 / position` of the first
/// expected-relevant id in `ranked` (1-based), or `0.0` if none of the expected
/// ids appear. MRR is the mean of those per-query reciprocal ranks. An empty
/// query set returns `0.0`.
pub fn mrr(per_query: &[(Vec<String>, Vec<String>)]) -> f64 {
    if per_query.is_empty() {
        return 0.0;
    }
    let total: f64 = per_query
        .iter()
        .map(|(ranked, expected)| reciprocal_rank(ranked, expected))
        .sum();
    total / per_query.len() as f64
}

/// Reciprocal rank of the first expected-relevant id in a single ranked list.
///
/// `1 / position` (1-based) of the earliest hit, or `0.0` if no expected id is
/// present. Exposed for unit testing and reuse by `mrr`.
pub fn reciprocal_rank(ranked: &[String], expected: &[String]) -> f64 {
    let want: std::collections::HashSet<&String> = expected.iter().collect();
    for (idx, id) in ranked.iter().enumerate() {
        if want.contains(id) {
            return 1.0 / (idx as f64 + 1.0);
        }
    }
    0.0
}

/// Deduplication precision over near-duplicate clusters in a ranked result list.
///
/// Given near-duplicate `clusters` (each a group of ids that are mutually
/// redundant), this measures how well the ranking avoids surfacing *multiple*
/// members of the same cluster inside the top `k`. A clean ranking returns at
/// most one representative per cluster; surfacing extra cluster members crowds
/// out genuinely distinct results (the dedup-quality failure mode).
///
/// Definition: of the `n` ids in the top `k`, count `r` redundant ids — the
/// 2nd-and-later members of any cluster that appear. Precision = `(n - r) / n`,
/// the fraction of top-`k` slots holding distinct (non-redundant) information.
/// Ids not in any cluster are always distinct. An empty top-`k` returns `1.0`
/// (vacuously precise).
pub fn dedup_precision(ranked: &[String], clusters: &[Vec<String>], k: usize) -> f64 {
    let top: Vec<&String> = ranked.iter().take(k).collect();
    if top.is_empty() {
        return 1.0;
    }
    // Map each id that belongs to a cluster to a stable cluster index.
    let mut cluster_of: std::collections::HashMap<&String, usize> =
        std::collections::HashMap::new();
    for (ci, cluster) in clusters.iter().enumerate() {
        for id in cluster {
            cluster_of.insert(id, ci);
        }
    }
    let mut seen_cluster: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut redundant = 0usize;
    for id in &top {
        if let Some(&ci) = cluster_of.get(*id) {
            if !seen_cluster.insert(ci) {
                // A second-or-later member of an already-seen cluster.
                redundant += 1;
            }
        }
    }
    let n = top.len();
    (n - redundant) as f64 / n as f64
}

/// Latency percentile (`p` in `[0.0, 100.0]`) over `samples` in microseconds.
///
/// Uses the nearest-rank method on a sorted copy: deterministic, no
/// interpolation, no panics. Returns `0` for an empty sample set. `p` is
/// clamped to `[0.0, 100.0]`.
pub fn latency_percentile(samples: &[u64], p: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let p = p.clamp(0.0, 100.0);
    // Nearest-rank: rank = ceil(p/100 * N), 1-based, clamped to [1, N].
    let n = sorted.len();
    let rank = (p / 100.0 * n as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

/// Convenience: the 50th percentile (median by nearest-rank).
pub fn p50(samples: &[u64]) -> u64 {
    latency_percentile(samples, 50.0)
}

/// Convenience: the 99th percentile.
pub fn p99(samples: &[u64]) -> u64 {
    latency_percentile(samples, 99.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    // ---- recall_at_k ---------------------------------------------------------

    #[test]
    fn recall_at_k_all_relevant_in_top_k() {
        let ranked = ids(&["a", "b", "c", "d"]);
        let expected = ids(&["a", "c"]);
        // both expected appear in top 3 -> 2/2 = 1.0
        assert!((recall_at_k(&ranked, &expected, 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_partial_hit() {
        let ranked = ids(&["a", "b", "c", "d"]);
        let expected = ids(&["a", "d"]);
        // only "a" is in top 2 -> 1/2 = 0.5
        assert!((recall_at_k(&ranked, &expected, 2) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_no_hit() {
        let ranked = ids(&["a", "b"]);
        let expected = ids(&["x", "y"]);
        assert!((recall_at_k(&ranked, &expected, 5) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_empty_expected_is_perfect() {
        let ranked = ids(&["a", "b"]);
        assert!((recall_at_k(&ranked, &[], 2) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_zero_k_is_zero() {
        let ranked = ids(&["a", "b"]);
        let expected = ids(&["a"]);
        assert!((recall_at_k(&ranked, &expected, 0) - 0.0).abs() < 1e-9);
    }

    // ---- reciprocal_rank / mrr ----------------------------------------------

    #[test]
    fn reciprocal_rank_first_position() {
        let ranked = ids(&["a", "b", "c"]);
        let expected = ids(&["a"]);
        assert!((reciprocal_rank(&ranked, &expected) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reciprocal_rank_third_position() {
        let ranked = ids(&["x", "y", "a"]);
        let expected = ids(&["a"]);
        // 1/3
        assert!((reciprocal_rank(&ranked, &expected) - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn reciprocal_rank_uses_earliest_hit() {
        let ranked = ids(&["x", "b", "a"]);
        let expected = ids(&["a", "b"]);
        // "b" is at position 2 -> 1/2 (earliest of the two expected)
        assert!((reciprocal_rank(&ranked, &expected) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn reciprocal_rank_no_hit_is_zero() {
        let ranked = ids(&["x", "y"]);
        let expected = ids(&["a"]);
        assert!((reciprocal_rank(&ranked, &expected) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn mrr_mean_of_reciprocal_ranks() {
        let per_query = vec![
            (ids(&["a", "b"]), ids(&["a"])),      // rr = 1.0
            (ids(&["x", "a"]), ids(&["a"])),      // rr = 0.5
            (ids(&["x", "y", "z"]), ids(&["a"])), // rr = 0.0
        ];
        // (1.0 + 0.5 + 0.0) / 3 = 0.5
        assert!((mrr(&per_query) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn mrr_empty_is_zero() {
        assert!((mrr(&[]) - 0.0).abs() < 1e-9);
    }

    // ---- dedup_precision -----------------------------------------------------

    #[test]
    fn dedup_precision_no_clusters_is_perfect() {
        let ranked = ids(&["a", "b", "c"]);
        assert!((dedup_precision(&ranked, &[], 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn dedup_precision_one_redundant_member() {
        let ranked = ids(&["a", "a2", "b"]);
        let clusters = vec![ids(&["a", "a2"])];
        // top 3 = [a, a2, b]; a2 is a 2nd cluster member -> 1 redundant.
        // (3 - 1) / 3 = 0.6667
        assert!((dedup_precision(&ranked, &clusters, 3) - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn dedup_precision_distinct_representatives_are_clean() {
        let ranked = ids(&["a", "b", "c"]);
        let clusters = vec![ids(&["a", "a2"]), ids(&["b", "b2"])];
        // top 3 = [a, b, c]; one rep per cluster + a non-cluster id -> no redundancy.
        assert!((dedup_precision(&ranked, &clusters, 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn dedup_precision_two_redundant_in_same_cluster() {
        let ranked = ids(&["a", "a2", "a3", "b"]);
        let clusters = vec![ids(&["a", "a2", "a3"])];
        // top 4 = [a, a2, a3, b]; a2 and a3 redundant -> 2 redundant.
        // (4 - 2) / 4 = 0.5
        assert!((dedup_precision(&ranked, &clusters, 4) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn dedup_precision_empty_top_is_vacuously_precise() {
        assert!((dedup_precision(&[], &[], 0) - 1.0).abs() < 1e-9);
    }

    // ---- latency percentiles -------------------------------------------------

    #[test]
    fn latency_percentile_empty_is_zero() {
        assert_eq!(latency_percentile(&[], 50.0), 0);
        assert_eq!(p50(&[]), 0);
        assert_eq!(p99(&[]), 0);
    }

    #[test]
    fn latency_percentile_nearest_rank() {
        // sorted: [10, 20, 30, 40, 50], N=5
        let samples = vec![50u64, 10, 40, 20, 30];
        // p50: ceil(0.5*5)=3 -> idx 2 -> 30
        assert_eq!(latency_percentile(&samples, 50.0), 30);
        // p99: ceil(0.99*5)=5 -> idx 4 -> 50
        assert_eq!(latency_percentile(&samples, 99.0), 50);
        // p0 clamps to rank 1 -> idx 0 -> 10
        assert_eq!(latency_percentile(&samples, 0.0), 10);
        // p100 -> rank 5 -> 50
        assert_eq!(latency_percentile(&samples, 100.0), 50);
    }

    #[test]
    fn latency_percentile_single_sample() {
        assert_eq!(latency_percentile(&[42], 50.0), 42);
        assert_eq!(latency_percentile(&[42], 99.0), 42);
    }

    #[test]
    fn latency_percentile_clamps_out_of_range_p() {
        let samples = vec![1u64, 2, 3];
        // p > 100 clamps to 100 -> max; p < 0 clamps to 0 -> rank 1 -> min
        assert_eq!(latency_percentile(&samples, 250.0), 3);
        assert_eq!(latency_percentile(&samples, -5.0), 1);
    }
}
