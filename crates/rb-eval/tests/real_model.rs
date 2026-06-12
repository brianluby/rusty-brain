//! Optional real-model mode — `#[ignore]` by default, run manually.
//!
//! The default harness uses `DeterministicProvider`, which guards ranking
//! determinism and regression but CANNOT show absolute semantic quality (its
//! vectors are non-semantic). This module runs the same fixture corpus through a
//! real embedding model (Voyage) for manual spot checks of semantic recall.
//!
//! It is `#[ignore]`d so it never runs in CI (no live network, per spec §11).
//! Run manually with a key:
//!
//! ```text
//! VOYAGE_API_KEY=... cargo test -p rb-eval --test real_model -- --ignored --nocapture
//! ```
//!
//! It makes NO assertion against committed baselines (those are deterministic);
//! it prints the semantic metrics for a human to judge.
//!
//! Since W1.4 these runs exercise Voyage's asymmetric retrieval for real:
//! recall embeds the query with `input_type="query"` while documents embed
//! with `input_type="document"`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::VoyageProvider;
use rb_eval::{build_engine_with, run_corpus_with, Corpus};

#[tokio::test]
#[ignore = "real-model mode: requires VOYAGE_API_KEY and live network; run manually"]
async fn voyage_semantic_recall_spot_check() {
    // voyage-3-lite is 512-dim; the engine + store dim are taken from the provider.
    let provider = VoyageProvider::from_env().expect("VOYAGE_API_KEY must be set for this mode");
    let engine = build_engine_with(provider).expect("engine builds");

    let corpus =
        Corpus::from_json(include_str!("../fixtures/corpus.json")).expect("fixtures parse");
    let run = run_corpus_with(&engine, &corpus)
        .await
        .expect("real-model run succeeds");

    println!(
        "real-model (voyage) report: recall@k={:.4} mrr={:.4} dedup={:.4}",
        run.report.mean_recall_at_k, run.report.mrr, run.report.dedup_precision
    );
    for d in &run.per_query {
        println!(
            "  Q='{}' recall={:.3} rr={:.3} top={:?}",
            d.query,
            d.recall_at_k,
            d.reciprocal_rank,
            d.ranked_keys.iter().take(5).collect::<Vec<_>>()
        );
    }
    // Sanity floor only: a real semantic model should not be worse than random
    // on this hand-authored corpus. This is a spot check, not a CI gate.
    assert!(
        run.report.mrr > 0.0,
        "real model returned zero MRR; check API/key/fixtures"
    );
}

/// Content-only vs composite (P5 Feature A) semantic comparison.
///
/// `DeterministicProvider` vectors are non-semantic, so they CANNOT show the
/// recall lift composite embeddings give (spec §8, §12 risk). The lift can only
/// be observed with a real model, so this comparison is real-model-only and
/// `#[ignore]`d (no network in CI). The current engine always embeds the
/// composite representation; this run reports its metrics for a human to compare
/// against the pre-P5 content-only numbers recorded in the README. It is a spot
/// check that prints, it asserts only a non-zero sanity floor — never a CI gate.
///
/// Run manually:
/// ```text
/// VOYAGE_API_KEY=... cargo test -p rb-eval --test real_model \
///   -- --ignored composite_embedding_semantic_recall --nocapture
/// ```
#[tokio::test]
#[ignore = "real-model mode: composite-embedding semantic lift; requires VOYAGE_API_KEY; run manually"]
async fn composite_embedding_semantic_recall() {
    let provider = VoyageProvider::from_env().expect("VOYAGE_API_KEY must be set for this mode");
    let engine = build_engine_with(provider).expect("engine builds");

    let corpus =
        Corpus::from_json(include_str!("../fixtures/corpus.json")).expect("fixtures parse");
    let run = run_corpus_with(&engine, &corpus)
        .await
        .expect("real-model composite run succeeds");

    println!(
        "real-model (voyage) COMPOSITE embedding report: recall@k={:.4} mrr={:.4} dedup={:.4}",
        run.report.mean_recall_at_k, run.report.mrr, run.report.dedup_precision
    );
    println!(
        "compare against the content-only baseline documented in rb-eval/README.md \
         to judge composite-embedding semantic lift"
    );
    assert!(
        run.report.mrr > 0.0,
        "composite real-model run returned zero MRR; check API/key/fixtures"
    );
}
