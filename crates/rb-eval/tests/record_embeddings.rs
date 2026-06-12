//! MANUAL recording of real embedding vectors into committed replay fixtures
//! (W1.0). Every test here is `#[ignore]`d: recording needs network (and, for
//! Voyage, credentials), so it never runs in CI. Replay (`replay_model.rs`)
//! is the CI path and needs neither.
//!
//! Each recording run drives the committed corpus + the held-out queries
//! through the real engine with a [`rb_eval::RecordingProvider`] wrapper, so
//! the captured text set is exactly what replay will look up: composite
//! document inputs post-heuristic-enrichment, plus every query string.
//!
//! Record with the local ONNX model (downloads ~90MB of weights on first use):
//!
//! ```text
//! cargo test -p rb-eval --features record-local --test record_embeddings \
//!   -- --ignored record_local_model_fixture --nocapture
//! ```
//!
//! Record with Voyage (preferred fixture source when a key is available):
//!
//! ```text
//! VOYAGE_API_KEY=... cargo test -p rb-eval --test record_embeddings \
//!   -- --ignored record_voyage_fixture --nocapture
//! ```
//!
//! After recording a NEW model's fixture, point
//! `rb_eval::replay::COMMITTED_FIXTURE_PATH` (and the `include_str!` in
//! `ReplayProvider::committed`) at it and re-capture the baseline artifact.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::EmbeddingProvider;
use rb_eval::{
    build_engine_with, load_committed_corpus, load_committed_holdout_queries, run_corpus_with,
    Corpus, RecordingProvider,
};

/// The committed corpus with the held-out queries APPENDED, so one engine pass
/// embeds every text either query set will ever need.
fn corpus_with_holdout() -> Corpus {
    let mut corpus = load_committed_corpus().expect("committed corpus loads");
    let holdout = load_committed_holdout_queries().expect("holdout queries load");
    corpus.golden_queries.extend(holdout);
    corpus
}

async fn record_with<P: EmbeddingProvider>(provider: P, comment: &str, file_name: &str) {
    let recorder = RecordingProvider::new(provider);
    let engine = build_engine_with(recorder).expect("engine builds");
    let corpus = corpus_with_holdout();
    let run = run_corpus_with(&engine, &corpus)
        .await
        .expect("recording run succeeds");
    println!(
        "recording run: recall@k={:.4} mrr={:.4} over {} queries",
        run.report.mean_recall_at_k,
        run.report.mrr,
        run.per_query.len()
    );

    let fixture = engine
        .embedder()
        .fixture(comment)
        .expect("fixture snapshot");
    println!(
        "recorded {} vectors for model '{}' (dim {})",
        fixture.vectors.len(),
        fixture.model_id,
        fixture.dim
    );
    let path = format!(
        "{}/fixtures/embeddings/{file_name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let json = serde_json::to_string_pretty(&fixture).expect("fixture serializes");
    std::fs::write(&path, json + "\n").expect("fixture written");
    println!("wrote {path}");
}

/// Record the committed default fixture with the local ONNX model.
#[cfg(feature = "record-local")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "recording mode: downloads the all-MiniLM-L6-v2 weights and runs ONNX inference"]
async fn record_local_model_fixture() {
    let provider = rb_embed::LocalProvider::load("all-MiniLM-L6-v2").expect("local model loads");
    record_with(
        provider,
        "Real all-MiniLM-L6-v2 (384-dim) vectors for the committed corpus + held-out queries \
         (W1.0). Keyed on (model_id, input_kind, sha256(text)); input_kind is 'document' for \
         every entry until W1.4 adds query-kind embeddings. Regenerate: cargo test -p rb-eval \
         --features record-local --test record_embeddings -- --ignored \
         record_local_model_fixture --nocapture",
        "all-MiniLM-L6-v2.json",
    )
    .await;
}

/// Record a Voyage fixture (preferred source; requires `VOYAGE_API_KEY`).
#[tokio::test]
#[ignore = "recording mode: requires VOYAGE_API_KEY and live network"]
async fn record_voyage_fixture() {
    let provider =
        rb_embed::VoyageProvider::from_env().expect("VOYAGE_API_KEY must be set for this mode");
    let file_name = format!("{}.json", provider.model_id());
    record_with(
        provider,
        "Real Voyage vectors for the committed corpus + held-out queries (W1.0). Keyed on \
         (model_id, input_kind, sha256(text)); input_kind is 'document' for every entry until \
         W1.4 adds query-kind embeddings. Regenerate: VOYAGE_API_KEY=... cargo test -p rb-eval \
         --test record_embeddings -- --ignored record_voyage_fixture --nocapture",
        &file_name,
    )
    .await;
}
