//! Real-vector REPLAY mode (W1.0): runs the committed corpus through the
//! committed real-model embedding fixture with ZERO network and ZERO keys.
//! This is the semantic-measurement path CI and later Phase 1 workstreams use;
//! the deterministic harness (`harness.rs`) remains the regression gate.
//!
//! These compatibility/diagnostic tests retain their historical sanity floors.
//! The strict W4.1 production gate lives in `semantic_gate.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_eval::{
    build_engine_with, load_committed_corpus, load_committed_holdout_queries, run_corpus_with,
    ReplayProvider,
};

#[tokio::test]
async fn committed_fixture_parses_and_covers_corpus_and_holdout_texts() {
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
    // COVERAGE-ONLY holdout check: every held-out query text must have a
    // recorded vector (embed fails closed on a miss), WITHOUT running recall
    // or computing any holdout metric — holdout aggregates are measured only
    // at frozen-artifact captures and the W4.1 gate (see the #[ignore]d
    // holdout replay test below).
    for q in &holdout {
        replay
            .embed(std::slice::from_ref(&q.query), EmbedKind::Query)
            .await
            .expect("every held-out query text must be covered by the fixture");
    }
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

    // Compatibility sanity floors only; semantic_gate.rs owns the strict
    // preregistered quality thresholds.
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
async fn readme_quickstart_query_returns_its_target_memory() {
    // Phase 1 gate (plan section 4): "the README quickstart query returns its
    // target memory". The README's recall example is committed verbatim as a
    // golden (`how is writing serialized?` -> graded answers
    // `readme_quickstart` (3), `single_writer` (2), `writer_thread_dup` (2),
    // W1.0a — the three are a committed near-duplicate cluster restating the
    // same fact). This asserts the gate under the committed real-vector replay
    // engine (W1.2): the TOP result must be one of the graded answers, and the
    // verbatim README target must be among the returned results.
    //
    // Rank history of the verbatim target, recorded honestly: 5 in the frozen
    // pre-Phase-1 artifact (pure vector signal), 7 under W1.2's revived
    // keyword leg — the target's authored text ("single-writer daemon") has no
    // inflection of "serialize" and porter keeps writer != write, so it is
    // keyword-invisible for its own query while its cluster siblings gained
    // keyword signal. W1.4 (query-kind embeddings) landed with NO effect here:
    // the committed fixture's model is kind-blind, so replayed query vectors
    // are unchanged — tightening this to verbatim-target-in-top-5 now waits on
    // a Voyage fixture (true input_type=query vectors) or W4.1 weight tuning;
    // see the W1.2 commit body.
    const QUICKSTART_QUERY: &str = "how is writing serialized?";
    const TARGET_KEY: &str = "readme_quickstart";

    let replay = ReplayProvider::committed().expect("committed embedding fixture parses");
    let engine = build_engine_with(replay).expect("engine builds");
    let corpus = load_committed_corpus().expect("corpus loads");
    let run = run_corpus_with(&engine, &corpus)
        .await
        .expect("replay run succeeds");

    let golden = corpus
        .golden_queries
        .iter()
        .find(|q| q.query == QUICKSTART_QUERY)
        .expect("README quickstart query must be a committed golden");
    let detail = run
        .per_query
        .iter()
        .find(|d| d.query == QUICKSTART_QUERY)
        .expect("README quickstart query must have been run");

    let top = detail
        .ranked_keys
        .first()
        .expect("quickstart query must return results");
    assert!(
        golden.expected.contains(top),
        "quickstart query's top result must be a graded answer, got {top:?} \
         (expected one of {:?})",
        golden.expected
    );
    assert!(
        detail.ranked_keys.iter().any(|k| k == TARGET_KEY),
        "quickstart query must return the verbatim README target {TARGET_KEY}, \
         got {:?}",
        detail.ranked_keys
    );
}

#[tokio::test]
#[ignore = "holdout aggregates are measured ONLY at frozen-artifact captures \
            (examples/capture_baseline.rs) and by the W4.1 semantic gate. \
            Running this in the default suite surfaced holdout metrics to \
            every local/CI iteration on ranking changes, eroding the set's \
            held-out status (Phase-1 review). Run explicitly with \
            `cargo test -p rb-eval --test replay_model -- --ignored` only \
            when capturing a frozen artifact or debugging a W4.1 gate failure."]
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
