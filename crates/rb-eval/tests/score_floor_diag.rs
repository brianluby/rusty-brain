//! W1.3 score-floor recalibration tool (`#[ignore]`d — run on demand): sweeps
//! candidate floors over the committed corpus under both the real-vector
//! replay engine and the deterministic gate engine, reporting what each floor
//! does to recall@k / MRR, how much junk it drops, and the score distribution
//! of expected-vs-junk returned results. The numbers behind
//! `rb_search::SCORE_FLOOR`'s derivation came from this sweep; re-run it when
//! `Weights::default`, the distance scale (W1.1), or the embedding input
//! shape (W1.4) changes:
//!
//! ```sh
//! cargo test -p rb-eval --test score_floor_diag -- --ignored --nocapture
//! ```
//!
//! The sweep engines disable the production floor (`with_score_floor(0.0)`)
//! so every candidate's unfloored score is observable.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_embed::{DeterministicProvider, EmbeddingProvider};
use rb_engine::{MemoryEngine, RememberInput};
use rb_eval::backend::{eval_namespace, SqliteBackend, EVAL_DIM};
use rb_eval::{build_engine_with, load_committed_corpus, metrics, Corpus, ReplayProvider};
use std::collections::HashMap;

async fn ingest<P: EmbeddingProvider>(
    engine: &MemoryEngine<SqliteBackend, P>,
    corpus: &Corpus,
) -> HashMap<String, String> {
    let mut key_to_id = HashMap::new();
    for m in &corpus.memories {
        let context = if m.context.trim().is_empty() {
            None
        } else {
            Some(m.context.clone())
        };
        let id = engine
            .remember(RememberInput {
                content: m.content.clone(),
                context,
                memory_type: m.memory_type,
                importance: m.importance,
                keywords: m.keywords.clone(),
                tags: m.tags.clone(),
                related_files: Vec::new(),
                confidence: None,
                provenance: Default::default(),
            })
            .await
            .expect("remember");
        if (m.confidence - 1.0).abs() > f32::EPSILON {
            engine.backend().set_confidence(&id, m.confidence).unwrap();
        }
        key_to_id.insert(m.key.clone(), id.to_string());
    }
    key_to_id
}

/// One golden query's run: ranked `(id, score)` pairs, expected ids, and k.
type RankedRun = (Vec<(String, f64)>, Vec<String>, usize);

async fn sweep<P: EmbeddingProvider>(label: &str, engine: &MemoryEngine<SqliteBackend, P>) {
    let corpus = load_committed_corpus().expect("corpus");
    let key_to_id = ingest(engine, &corpus).await;

    let mut runs: Vec<RankedRun> = Vec::new();
    let mut expected_scores: Vec<f64> = Vec::new(); // expected results inside top-k
    let mut junk_scores: Vec<f64> = Vec::new(); // non-expected returned results
    for q in &corpus.golden_queries {
        let results = engine
            .recall(&q.query, 10, &rb_types::RecallFilter::default())
            .await
            .expect("recall");
        let expected: Vec<String> = q
            .expected
            .iter()
            .filter_map(|k| key_to_id.get(k))
            .cloned()
            .collect();
        let k = q.k.unwrap_or(5);
        let pairs: Vec<(String, f64)> = results
            .iter()
            .map(|r| (r.memory.id.to_string(), r.score as f64))
            .collect();
        for (rank, (id, score)) in pairs.iter().enumerate() {
            if expected.contains(id) {
                if rank < k {
                    expected_scores.push(*score);
                }
            } else {
                junk_scores.push(*score);
            }
        }
        runs.push((pairs, expected, k));
    }
    expected_scores.sort_by(|a, b| a.total_cmp(b));
    junk_scores.sort_by(|a, b| a.total_cmp(b));
    let pct = |v: &[f64], p: f64| v[((v.len() - 1) as f64 * p) as usize];
    println!("== {label} ==");
    if expected_scores.is_empty() {
        println!("expected-in-top-k scores: n=0");
    } else {
        println!(
            "expected-in-top-k scores: n={} min={:.4} p05={:.4} p25={:.4} med={:.4}",
            expected_scores.len(),
            expected_scores.first().unwrap(),
            pct(&expected_scores, 0.05),
            pct(&expected_scores, 0.25),
            pct(&expected_scores, 0.50),
        );
    }
    if junk_scores.is_empty() {
        println!("non-expected returned scores: n=0");
    } else {
        println!(
            "non-expected returned scores: n={} min={:.4} med={:.4} p75={:.4} p95={:.4} max={:.4}",
            junk_scores.len(),
            junk_scores.first().unwrap(),
            pct(&junk_scores, 0.50),
            pct(&junk_scores, 0.75),
            pct(&junk_scores, 0.95),
            junk_scores.last().unwrap(),
        );
    }

    for floor in [
        0.0, 0.05, 0.075, 0.10, 0.125, 0.15, 0.175, 0.20, 0.25, 0.30, 0.35, 0.40,
    ] {
        let mut recalls = Vec::new();
        let mut rrs = Vec::new();
        let mut kept = 0usize;
        let mut total = 0usize;
        for (pairs, expected, k) in &runs {
            let ranked: Vec<String> = pairs
                .iter()
                .filter(|(_, s)| *s >= floor)
                .map(|(id, _)| id.clone())
                .collect();
            kept += ranked.len();
            total += pairs.len();
            recalls.push(metrics::recall_at_k(&ranked, expected, *k));
            rrs.push(metrics::reciprocal_rank(&ranked, expected));
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        println!(
            "floor={floor:.3} recall@k={:.4} mrr={:.4} kept={kept}/{total}",
            mean(&recalls),
            mean(&rrs),
        );
    }

    // F30 scenario: junk queries with no relation to the corpus.
    for junk_q in [
        "how do I bake sourdough bread at high altitude?",
        "best beaches in portugal for surfing",
        "quantum chromodynamics lattice simulation parameters",
    ] {
        match engine
            .recall(junk_q, 10, &rb_types::RecallFilter::default())
            .await
        {
            Ok(results) => {
                let scores: Vec<String> =
                    results.iter().map(|r| format!("{:.4}", r.score)).collect();
                println!(
                    "junk query {junk_q:?} -> {} results: {:?}",
                    results.len(),
                    scores
                );
            }
            Err(e) => println!("junk query {junk_q:?} -> error {e}"),
        }
    }
}

#[tokio::test]
#[ignore = "recalibration tool, not a gate: run with --ignored --nocapture"]
async fn score_floor_sweep_replay() {
    let replay = ReplayProvider::committed().expect("fixture");
    let engine = build_engine_with(replay)
        .expect("engine")
        .with_score_floor(0.0);
    sweep("replay all-MiniLM-L6-v2 (goldens)", &engine).await;
}

#[tokio::test]
#[ignore = "recalibration tool, not a gate: run with --ignored --nocapture"]
async fn score_floor_sweep_deterministic() {
    let backend = SqliteBackend::in_memory(EVAL_DIM).expect("backend");
    let embedder = DeterministicProvider::new(EVAL_DIM);
    let engine = MemoryEngine::new(backend, embedder, eval_namespace()).with_score_floor(0.0);
    sweep("deterministic EVAL_DIM=32 (gate context)", &engine).await;
}
