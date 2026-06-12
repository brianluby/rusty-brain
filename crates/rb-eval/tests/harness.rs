//! The harness gate: `cargo test -p rb-eval` runs this.
//!
//! It ingests the committed fixture corpus through `rb-engine` with the
//! deterministic provider, runs every golden query through recall, computes
//! the aggregate metrics, and asserts each quality metric meets its committed
//! baseline. A regression (any metric below baseline) fails with a readable diff.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_eval::corpus::{Corpus, DedupCluster, FixtureMemory, GoldenQuery};
use rb_eval::{
    check_against_baselines, compare_modes_committed, run_corpus, run_corpus_detailed, run_eval,
    run_eval_mode, Baselines, FusionMode,
};
use rb_types::MemoryType;

#[tokio::test]
async fn confidence_poison_low_confidence_wrong_memory_ranks_last() {
    // Feature C "poison" scenario (spec §9): two equally-matching memories with
    // IDENTICAL content (so the deterministic vector + FTS signals tie). One is
    // the "correct" full-confidence answer; the other is a low-confidence
    // "poison". The confidence dampener must rank the high-confidence memory
    // ABOVE the low-confidence one — a wrong, high-matching memory cannot
    // dominate recall.
    let corpus = Corpus::from_json(
        r#"{
            "memories": [
                {"key": "correct", "content": "the deploy rollback procedure resets the feature flag",
                 "keywords": ["rollback", "deploy"], "memory_type": "Insight", "importance": 5,
                 "confidence": 1.0},
                {"key": "poison", "content": "the deploy rollback procedure resets the feature flag",
                 "keywords": ["rollback", "deploy"], "memory_type": "Insight", "importance": 5,
                 "confidence": 0.05}
            ],
            "golden_queries": [
                {"query": "rollback procedure", "expected": ["correct"], "k": 1}
            ]
        }"#,
    )
    .expect("poison corpus valid");

    let run = run_corpus_detailed(&corpus).await.expect("run");
    let detail = &run.per_query[0];
    let correct_pos = detail
        .ranked_keys
        .iter()
        .position(|k| k == "correct")
        .expect("correct present");
    let poison_pos = detail
        .ranked_keys
        .iter()
        .position(|k| k == "poison")
        .expect("poison present");
    assert!(
        correct_pos < poison_pos,
        "high-confidence correct memory must rank above the low-confidence poison: {:?}",
        detail.ranked_keys
    );
    // And recall@1 lands on the correct one.
    assert!(
        (detail.recall_at_k - 1.0).abs() < 1e-9,
        "the correct memory must be the top hit, got recall {}",
        detail.recall_at_k
    );
}

#[tokio::test]
async fn committed_fixtures_meet_baselines() {
    let report = run_eval().await.expect("eval run must succeed");
    let baselines = Baselines::committed().expect("baselines.json must parse");

    // Print the report so a maintainer updating baselines sees the live numbers.
    println!(
        "rb-eval report: recall@k={:.4} mrr={:.4} dedup={:.4} p50={}us p99={}us",
        report.mean_recall_at_k,
        report.mrr,
        report.dedup_precision,
        report.p50_latency_us,
        report.p99_latency_us
    );

    if let Err(diff) = check_against_baselines(&report, &baselines) {
        panic!("{diff}");
    }
}

#[tokio::test]
async fn fixtures_run_under_both_modes_and_report_metric_delta() {
    // Feature B acceptance: the committed fixture set runs under BOTH Linear and
    // Rrf, and the harness reports the per-metric delta. This is observability
    // only — it does NOT gate (the eval-gated default flip is a later commit).
    let cmp = compare_modes_committed()
        .await
        .expect("mode comparison must run");

    println!(
        "rb-eval mode comparison (Rrf - Linear):\n  \
         recall@k: linear={:.4} rrf={:.4} delta={:+.4}\n  \
         mrr:      linear={:.4} rrf={:.4} delta={:+.4}\n  \
         dedup:    linear={:.4} rrf={:.4} delta={:+.4}",
        cmp.linear.mean_recall_at_k,
        cmp.rrf.mean_recall_at_k,
        cmp.recall_at_k_delta,
        cmp.linear.mrr,
        cmp.rrf.mrr,
        cmp.mrr_delta,
        cmp.linear.dedup_precision,
        cmp.rrf.dedup_precision,
        cmp.dedup_precision_delta,
    );

    // Both modes produce valid metric fractions; deltas are finite.
    for r in [&cmp.linear, &cmp.rrf] {
        assert!((0.0..=1.0).contains(&r.mean_recall_at_k));
        assert!((0.0..=1.0).contains(&r.mrr));
        assert!((0.0..=1.0).contains(&r.dedup_precision));
    }
    assert!(cmp.recall_at_k_delta.is_finite());
    assert!(cmp.mrr_delta.is_finite());
    assert!(cmp.dedup_precision_delta.is_finite());
}

#[tokio::test]
async fn linear_mode_matches_default_eval_quality_metrics() {
    // The default `run_eval` is `Linear`; running explicitly in `Linear` mode
    // must reproduce its quality metrics exactly (the default did not change).
    // Latency fields are machine/timing-dependent and intentionally not compared.
    let default = run_eval().await.expect("default eval");
    let linear = run_eval_mode(FusionMode::Linear)
        .await
        .expect("explicit linear");
    assert!(
        (default.mean_recall_at_k - linear.mean_recall_at_k).abs() < 1e-12,
        "explicit Linear recall@k must equal the default eval"
    );
    assert!(
        (default.mrr - linear.mrr).abs() < 1e-12,
        "explicit Linear mrr must equal the default eval"
    );
    assert!(
        (default.dedup_precision - linear.dedup_precision).abs() < 1e-12,
        "explicit Linear dedup must equal the default eval"
    );
}

#[tokio::test]
async fn rrf_mode_is_deterministic_across_runs() {
    let a = run_eval_mode(FusionMode::Rrf).await.expect("first rrf run");
    let b = run_eval_mode(FusionMode::Rrf)
        .await
        .expect("second rrf run");
    assert!(
        (a.mean_recall_at_k - b.mean_recall_at_k).abs() < 1e-12,
        "rrf recall@k drifted between runs"
    );
    assert!(
        (a.mrr - b.mrr).abs() < 1e-12,
        "rrf mrr drifted between runs"
    );
    assert!(
        (a.dedup_precision - b.dedup_precision).abs() < 1e-12,
        "rrf dedup drifted between runs"
    );
}

#[tokio::test]
async fn report_is_deterministic_across_runs() {
    // Determinism is the core invariant: identical inputs -> identical metrics.
    let a = run_eval().await.expect("first run");
    let b = run_eval().await.expect("second run");
    assert!(
        (a.mean_recall_at_k - b.mean_recall_at_k).abs() < 1e-12,
        "recall@k drifted between runs: {} vs {}",
        a.mean_recall_at_k,
        b.mean_recall_at_k
    );
    assert!((a.mrr - b.mrr).abs() < 1e-12, "mrr drifted between runs");
    assert!(
        (a.dedup_precision - b.dedup_precision).abs() < 1e-12,
        "dedup_precision drifted between runs"
    );
}

#[tokio::test]
async fn keyword_path_surfaces_exact_term_match() {
    // A hand-built corpus that the FTS path alone must satisfy: the query term
    // appears verbatim only in the target memory's content.
    let corpus = Corpus::from_json(
        r#"{
            "memories": [
                {"key": "target", "content": "the WAL checkpoint truncates the journal",
                 "keywords": ["wal", "checkpoint"], "memory_type": "Insight", "importance": 6},
                {"key": "noise", "content": "unrelated note about color themes",
                 "keywords": ["color"], "memory_type": "Preference", "importance": 4}
            ],
            "golden_queries": [
                {"query": "checkpoint", "expected": ["target"], "k": 1}
            ]
        }"#,
    )
    .expect("corpus valid");

    let report = run_corpus(&corpus).await.expect("run");
    assert!(
        (report.mean_recall_at_k - 1.0).abs() < 1e-9,
        "FTS should put the exact-term match at rank 1, got recall {}",
        report.mean_recall_at_k
    );
}

#[tokio::test]
async fn report_carries_per_query_channel_attribution() {
    // W1.0 per-channel hit-contribution counters in the eval report: an
    // exact-term query must be FTS-attributed, the vector channel always
    // surfaces candidates (KNN returns nearest neighbors regardless of
    // semantics), and the aggregates fold the per-query flags into rates.
    let corpus = Corpus::from_json(
        r#"{
            "memories": [
                {"key": "target", "content": "the WAL checkpoint truncates the journal",
                 "keywords": ["wal", "checkpoint"], "memory_type": "Insight", "importance": 6},
                {"key": "noise", "content": "unrelated note about color themes",
                 "keywords": ["color"], "memory_type": "Preference", "importance": 4}
            ],
            "golden_queries": [
                {"query": "checkpoint", "expected": ["target"], "grades": [3], "k": 1}
            ]
        }"#,
    )
    .expect("corpus valid");

    let run = run_corpus_detailed(&corpus).await.expect("run");
    let d = &run.per_query[0];
    assert!(d.channels.returned > 0, "results returned");
    assert!(
        d.channels.fts > 0,
        "exact-term query must carry FTS attribution, got {:?}",
        d.channels
    );
    assert!(
        d.channels.vector > 0,
        "vector KNN always contributes candidates, got {:?}",
        d.channels
    );
    assert!(
        run.report.channels.fts_query_rate > 0.0 && run.report.channels.vector_query_rate > 0.0,
        "aggregate channel rates must reflect the per-query flags: {:?}",
        run.report.channels
    );
    // The graded query also exercises NDCG end-to-end.
    assert!(
        run.report.ndcg > 0.0,
        "graded golden must produce a non-zero ndcg"
    );
}

#[tokio::test]
async fn dedup_clusters_are_scored() {
    // Two near-duplicate memories form a cluster; a third is distinct. The
    // dedup metric is exercised (value asserted only to be in range — the
    // committed-corpus test gates the real threshold).
    let memories = vec![
        mem(
            "dup_a",
            "single writer thread owns the sqlite connection",
            7,
        ),
        mem(
            "dup_b",
            "one writer thread owns the sqlite connection exclusively",
            7,
        ),
        mem("other", "voyage embeddings are fetched over https", 5),
    ];
    let corpus = Corpus {
        memories,
        golden_queries: vec![GoldenQuery {
            query: "writer".into(),
            expected: vec!["dup_a".into()],
            grades: vec![3],
            k: Some(5),
        }],
        dedup_clusters: vec![DedupCluster {
            members: vec!["dup_a".into(), "dup_b".into()],
        }],
    };

    let report = run_corpus(&corpus).await.expect("run");
    assert!(
        (0.0..=1.0).contains(&report.dedup_precision),
        "dedup precision must be a valid fraction, got {}",
        report.dedup_precision
    );
}

fn mem(key: &str, content: &str, importance: u8) -> FixtureMemory {
    FixtureMemory {
        key: key.into(),
        content: content.into(),
        summary: String::new(),
        keywords: Vec::new(),
        tags: Vec::new(),
        context: String::new(),
        memory_type: MemoryType::Insight,
        importance,
        confidence: 1.0,
    }
}
