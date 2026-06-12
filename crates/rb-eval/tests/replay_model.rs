//! Real-vector REPLAY mode (W1.0): runs the committed corpus through the
//! committed real-model embedding fixture with ZERO network and ZERO keys.
//! This is the semantic-measurement path CI and later Phase 1 workstreams use;
//! the deterministic harness (`harness.rs`) remains the regression gate.
//!
//! These tests assert sanity floors only — the pre-Phase-1 baseline artifact
//! (`docs/eval/2026-06-12-pre-phase1-baseline.json`) is measurement, and CI
//! gating thresholds on these numbers arrive in W4.1.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::EmbeddingProvider;
use rb_eval::{
    build_engine_with, load_committed_corpus, load_committed_holdout_queries, run_corpus_with,
    ReplayProvider,
};

#[test]
fn committed_fixture_parses_and_covers_corpus_and_holdout_texts() {
    // Replay fails closed on any missing text, so a successful end-to-end run
    // (below) proves coverage; this cheaper check pins the fixture's shape.
    let replay = ReplayProvider::committed().expect("committed embedding fixture parses");
    assert_eq!(replay.model_id(), "all-MiniLM-L6-v2");
    assert_eq!(replay.dim(), 384);
    let corpus = load_committed_corpus().expect("corpus loads");
    let holdout = load_committed_holdout_queries().expect("holdout loads");
    // At minimum one vector per memory and per query text (composite document
    // inputs and query strings may coincide only by exact-text collision).
    assert!(
        replay.len() >= corpus.memories.len(),
        "fixture must cover at least every corpus document: {} < {}",
        replay.len(),
        corpus.memories.len()
    );
    assert!(
        !holdout.is_empty(),
        "held-out set must be non-empty for the baseline artifact"
    );
}

#[tokio::test]
async fn replayed_real_vectors_run_the_golden_queries_offline() {
    let replay = ReplayProvider::committed().expect("committed embedding fixture parses");
    let engine = build_engine_with(replay).expect("engine builds");
    let corpus = load_committed_corpus().expect("corpus loads");

    let run = run_corpus_with(&engine, &corpus)
        .await
        .expect("replay run must succeed — a failure means the corpus drifted from the fixture");

    println!(
        "replay (all-MiniLM-L6-v2) goldens: recall@k={:.4} mrr={:.4} ndcg={:.4} dedup={:.4}",
        run.report.mean_recall_at_k, run.report.mrr, run.report.ndcg, run.report.dedup_precision
    );
    println!(
        "replay channel rates: fts={:.3} vector={:.3} graph={:.3}",
        run.report.channels.fts_query_rate,
        run.report.channels.vector_query_rate,
        run.report.channels.graph_query_rate
    );

    // Sanity floors only (gating comes in W4.1): a real semantic model must
    // find SOMETHING on a hand-authored corpus.
    assert!(run.report.mrr > 0.0, "real-vector replay returned zero MRR");
    assert!(run.report.mean_recall_at_k > 0.0);
    assert!(run.report.ndcg > 0.0);
    // Channel attribution must be populated: the vector channel surfaces
    // candidates on every query (KNN always returns neighbors), and FTS
    // contributes somewhere on a corpus full of exact terms.
    assert!(run.report.channels.vector_query_rate > 0.0);
    assert!(run.report.channels.fts_query_rate > 0.0);
}

#[tokio::test]
async fn replayed_real_vectors_run_the_holdout_queries_offline() {
    // The held-out set is measurement-only (W4.1 gates on it later). This run
    // proves the committed fixture covers its query texts and the metrics are
    // computable; it asserts only a non-zero floor, NEVER a tuned threshold.
    let replay = ReplayProvider::committed().expect("committed embedding fixture parses");
    let engine = build_engine_with(replay).expect("engine builds");
    let mut shadow = load_committed_corpus().expect("corpus loads");
    shadow.golden_queries = load_committed_holdout_queries().expect("holdout loads");

    let run = run_corpus_with(&engine, &shadow)
        .await
        .expect("holdout replay run must succeed");
    println!(
        "replay (all-MiniLM-L6-v2) holdout: recall@k={:.4} mrr={:.4} ndcg={:.4}",
        run.report.mean_recall_at_k, run.report.mrr, run.report.ndcg
    );
    assert!(run.report.mrr > 0.0, "holdout replay returned zero MRR");
}
