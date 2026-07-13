//! W4.1 production-embedding gate and scheduled fusion diagnostics.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_eval::{
    build_engine_with_at, check_semantic_floors, load_semantic_gate, run_corpus_with, Corpus,
    DetailedRun, FusionMode, QualityFloors,
};
use serde_json::json;

struct GateRun {
    run: DetailedRun,
    query_fallbacks: usize,
    fixture_bytes: usize,
}

async fn run_locked(
    mode: FusionMode,
    now: chrono::DateTime<chrono::Utc>,
    holdout: bool,
) -> (GateRun, QualityFloors) {
    let inputs = load_semantic_gate().expect("all frozen semantic-gate inputs validate");
    let mut corpus = inputs.corpus;
    if holdout {
        corpus.golden_queries = inputs.holdout_queries;
    }
    let floors = inputs.manifest.floors;
    let fixture_bytes = inputs.fixture_bytes;
    let engine = build_engine_with_at(inputs.replay, now)
        .expect("semantic-gate engine builds")
        .with_fusion_mode(mode);
    let run = run_corpus_with(&engine, &corpus)
        .await
        .expect("strict replay covers every frozen document and query input");
    let query_fallbacks = engine.embedder().query_fallbacks();
    (
        GateRun {
            run,
            query_fallbacks,
            fixture_bytes,
        },
        floors,
    )
}

fn diagnostics(label: &str, run: &GateRun, corpus: &Corpus) -> serde_json::Value {
    let returned_rows: usize = run
        .run
        .per_query
        .iter()
        .map(|detail| detail.channels.returned)
        .sum();
    let content_bytes: usize = run
        .run
        .per_query
        .iter()
        .flat_map(|detail| &detail.ranked_keys)
        .filter_map(|key| corpus.memories.iter().find(|memory| &memory.key == key))
        .map(|memory| memory.content.len())
        .sum();
    json!({
        "label": label,
        "report": run.run.report,
        "returned_rows": returned_rows,
        "returned_content_bytes": content_bytes,
        "approx_returned_content_tokens": content_bytes.div_ceil(4),
        "fixture_bytes": run.fixture_bytes,
        "query_fallbacks": run.query_fallbacks,
        "offline_provider_requests": 0,
        "provider_cost_usd": 0.0,
    })
}

#[tokio::test]
async fn production_embedding_linear_gate_passes_goldens_and_untouched_holdout() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .to_utc();
    let frozen = load_semantic_gate().expect("gate inputs validate");
    let golden_corpus = frozen.corpus.clone();
    let mut holdout_corpus = frozen.corpus;
    holdout_corpus.golden_queries = frozen.holdout_queries;

    let (golden, floors) = run_locked(FusionMode::Linear, now, false).await;
    let (holdout, _) = run_locked(FusionMode::Linear, now, true).await;

    check_semantic_floors("golden", &golden.run.report, floors)
        .expect("golden aggregate meets every preregistered floor");
    check_semantic_floors("holdout", &holdout.run.report, floors)
        .expect("untouched holdout aggregate meets every preregistered floor");
    assert_eq!(golden.query_fallbacks + holdout.query_fallbacks, 0);

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "gate": "W4.1-production-embedding",
            "fusion_mode": "Linear",
            "golden": diagnostics("golden", &golden, &golden_corpus),
            "holdout": diagnostics("holdout-aggregate-only", &holdout, &holdout_corpus),
            "decision": "keep_linear",
        }))
        .unwrap()
    );
}

#[tokio::test]
#[ignore = "scheduled/manual aggregate-only diagnostic: runs Linear and RRF at five \
            preregistered chronological instants; it does not change the default"]
async fn five_seed_linear_rrf_diagnostic() {
    let manifest = load_semantic_gate().expect("gate inputs validate").manifest;
    let mut rows = Vec::new();

    for seed in &manifest.chronological_seeds {
        let now = chrono::DateTime::parse_from_rfc3339(seed).unwrap().to_utc();
        for mode in [FusionMode::Linear, FusionMode::Rrf] {
            let (golden, _) = run_locked(mode, now, false).await;
            let (holdout, _) = run_locked(mode, now, true).await;
            assert_eq!(golden.query_fallbacks + holdout.query_fallbacks, 0);
            rows.push(json!({
                "seed": seed,
                "mode": format!("{mode:?}"),
                "golden": golden.run.report,
                "holdout": holdout.run.report,
            }));
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "gate": "W4.1-five-seed-fusion-diagnostic",
            "holdout_policy": "aggregate-only",
            "default_decision": manifest.default_decision,
            "runs": rows,
        }))
        .unwrap()
    );
}
