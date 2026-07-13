//! Capture the pre-Phase-1 retrieval baseline artifact (W1.0).
//!
//! Runs the committed corpus (golden + held-out query sets) through the
//! committed REAL-vector replay fixture and the deterministic provider, and
//! emits one JSON document with overall metrics AND per-channel contribution
//! stats. The committed artifact (`docs/eval/2026-06-12-pre-phase1-baseline.json`)
//! is FROZEN: later Phase 1 workstreams compare against it; it remains a
//! measurement artifact, separate from the W4.1 gate in `semantic_gate.json`.
//!
//! ```text
//! cargo run -p rb-eval --example capture_baseline > docs/eval/<date>-pre-phase1-baseline.json
//! ```

use rb_embed::EmbeddingProvider;
use rb_eval::{
    build_engine_with, load_committed_corpus, load_committed_holdout_queries, run_corpus_detailed,
    run_corpus_with, Corpus, DetailedRun, ReplayProvider,
};
use serde_json::json;

/// Run `corpus` through a fresh replay-provider engine.
async fn replay_run(corpus: &Corpus) -> Result<DetailedRun, Box<dyn std::error::Error>> {
    let replay = ReplayProvider::committed()?;
    let engine = build_engine_with(replay)?;
    Ok(run_corpus_with(&engine, corpus).await?)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load_committed_corpus()?;
    let holdout_queries = load_committed_holdout_queries()?;
    let mut holdout_corpus = corpus.clone();
    holdout_corpus.golden_queries = holdout_queries;

    let replay = ReplayProvider::committed()?;
    let (model_id, dim, recorded_vectors) =
        (replay.model_id().to_string(), replay.dim(), replay.len());

    // Real-vector replay runs: goldens (with per-query detail) + holdout
    // (aggregates ONLY — the held-out set must never feed a tuning loop, so
    // the frozen artifact does not lay out its per-query behavior).
    let golden = replay_run(&corpus).await?;
    let holdout = replay_run(&holdout_corpus).await?;

    // Deterministic reference (the committed baselines.json gate context).
    let deterministic_golden = run_corpus_detailed(&corpus).await?;

    let artifact = json!({
        "_comment": "Pre-Phase-1 retrieval baseline (W1.0). FROZEN measurement artifact: later \
                     Phase 1 workstreams report before/after against these numbers. It is NOT a \
                     CI gate (W4.1 now lives in semantic_gate.json) and must never be edited; re-capture under \
                     a new dated filename only if the corpus or fixture is re-recorded. \
                     Regenerate: cargo run -p rb-eval --example capture_baseline",
        "captured_at": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "fusion_mode": "Linear",
        "embedding": {
            "mode": "replay (committed real-model vectors, zero network)",
            "model_id": model_id,
            "dim": dim,
            "recorded_vectors": recorded_vectors,
            "fixture": "crates/rb-eval/fixtures/embeddings/all-MiniLM-L6-v2.json",
        },
        "corpus": {
            "memories": corpus.memories.len(),
            "golden_queries": corpus.golden_queries.len(),
            "holdout_queries": holdout_corpus.golden_queries.len(),
            "dedup_clusters": corpus.dedup_clusters.len(),
        },
        "golden": {
            "report": golden.report,
            "per_query": golden.per_query,
        },
        "holdout": {
            "_comment": "Aggregates only by design: the held-out set is reserved for the W4.1 \
                         CI gate and must never be used for tuning.",
            "report": holdout.report,
        },
        "deterministic_reference": {
            "_comment": "Same goldens under DeterministicProvider — the regression-gate context \
                         that crates/rb-eval/baselines.json tracks.",
            "report": deterministic_golden.report,
        },
    });

    println!("{}", serde_json::to_string_pretty(&artifact)?);
    Ok(())
}
