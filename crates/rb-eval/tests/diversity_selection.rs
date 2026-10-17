//! Aggregate-only #86 bounded diversity (MMR) experiment, preregistered in
//! `docs/eval/2026-10-17-diversity-selection-preregistration.md`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_eval::metrics;
use rb_eval::{
    build_engine_with_at, detailed_rankings, document_vectors, evaluate_retrieval_arm,
    load_semantic_gate, mmr_rankings, run_corpus_with, silent_losses, Corpus, DetailedRun,
    FusionMode, RetrievalArm, RetrievalArmReport, SilentLosses, MMR_LAMBDA, STREAM_SEEDS,
};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;

fn seed_time(seed: u64) -> chrono::DateTime<chrono::Utc> {
    // Same registered instants as the controlled arms: seed i -> 2026-(i+1)-01.
    let month = STREAM_SEEDS
        .iter()
        .position(|registered| *registered == seed)
        .expect("seed is registered")
        + 1;
    chrono::DateTime::parse_from_rfc3339(&format!("2026-{month:02}-01T00:00:00Z"))
        .unwrap()
        .to_utc()
}

async fn linear_run(corpus: &Corpus, seed: u64) -> DetailedRun {
    let inputs = load_semantic_gate().expect("frozen inputs validate");
    let engine = build_engine_with_at(inputs.replay, seed_time(seed))
        .expect("engine builds")
        .with_fusion_mode(FusionMode::Linear);
    run_corpus_with(&engine, corpus)
        .await
        .expect("strict replay covers the corpus")
}

async fn vectors(corpus: &Corpus) -> HashMap<String, Vec<f32>> {
    let inputs = load_semantic_gate().expect("frozen inputs validate");
    let engine = build_engine_with_at(inputs.replay, seed_time(STREAM_SEEDS[0])).unwrap();
    document_vectors(&engine, corpus)
        .await
        .expect("strict replay has every stored document vector")
}

struct SeedResult {
    linear: RetrievalArmReport,
    mmr: RetrievalArmReport,
    losses: SilentLosses,
    step_p50_us: u64,
    step_p99_us: u64,
}

async fn run_seed(
    corpus: &Corpus,
    vectors: &HashMap<String, Vec<f32>>,
    set: &str,
    seed: u64,
) -> SeedResult {
    let run = linear_run(corpus, seed).await;
    let latencies = [run.report.p50_latency_us, run.report.p99_latency_us];
    let linear_rankings = detailed_rankings(corpus, &run);

    // Time the MMR step per query on the same candidates.
    let mut step_us = Vec::with_capacity(run.per_query.len());
    let mut mmr = Vec::with_capacity(run.per_query.len());
    for detail in &run.per_query {
        let single = DetailedRun {
            report: run.report.clone(),
            per_query: vec![detail.clone()],
        };
        let started = Instant::now();
        let mut ranked = mmr_rankings(corpus, &single, vectors, MMR_LAMBDA).unwrap();
        step_us.push(started.elapsed().as_micros() as u64);
        mmr.push(ranked.pop().unwrap());
    }
    // Pair each query's recall time with its own MMR step before taking
    // percentiles; summing independent percentiles is not a valid bound.
    let combined_us: Vec<u64> = run
        .per_query
        .iter()
        .zip(&step_us)
        .map(|(detail, step)| detail.recall_latency_us + step)
        .collect();
    let step_p50_us = metrics::p50(&step_us);
    let step_p99_us = metrics::p99(&step_us);

    let mut linear = evaluate_retrieval_arm(
        corpus,
        &linear_rankings,
        RetrievalArm::Linear,
        set,
        seed,
        &latencies,
    )
    .unwrap();
    linear.metrics.p50_latency_us = run.report.p50_latency_us;
    linear.metrics.p99_latency_us = run.report.p99_latency_us;
    let mut mmr_report =
        evaluate_retrieval_arm(corpus, &mmr, RetrievalArm::Mmr, set, seed, &latencies).unwrap();
    mmr_report.metrics.p50_latency_us = metrics::p50(&combined_us);
    mmr_report.metrics.p99_latency_us = metrics::p99(&combined_us);

    for (detail, shadow) in run.per_query.iter().zip(&mmr) {
        assert!(shadow.len() <= 5, "MMR rows stay within the row budget");
        assert!(
            shadow.iter().all(|key| detail.ranked_keys.contains(key)),
            "MMR only reselects Linear's own candidates"
        );
    }
    SeedResult {
        losses: silent_losses(corpus, &linear_rankings, &mmr).unwrap(),
        linear,
        mmr: mmr_report,
        step_p50_us,
        step_p99_us,
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<f64> = values.collect();
    values.iter().sum::<f64>() / values.len() as f64
}

fn summary(
    results: &[SeedResult],
    pick: impl Fn(&SeedResult) -> &RetrievalArmReport,
) -> serde_json::Value {
    let metric = |f: &dyn Fn(&RetrievalArmReport) -> f64| mean(results.iter().map(|r| f(pick(r))));
    json!({
        "recall_at_5": metric(&|r| r.metrics.recall_at_5),
        "mrr": metric(&|r| r.metrics.mrr),
        "ndcg_at_5": metric(&|r| r.metrics.ndcg_at_5),
        "dedup_precision_at_5": metric(&|r| r.metrics.dedup_precision_at_5),
        "injected_rows": metric(&|r| r.metrics.injected_rows as f64),
        "injected_bytes": metric(&|r| r.metrics.injected_bytes as f64),
        "injected_tokens": metric(&|r| r.metrics.injected_tokens as f64),
        "p50_latency_us": metric(&|r| r.metrics.p50_latency_us as f64),
        "p99_latency_us": metric(&|r| r.metrics.p99_latency_us as f64),
    })
}

#[tokio::test]
#[ignore = "manual aggregate-only #86 experiment; never mutates production behavior"]
async fn bounded_mmr_diversity_against_linear_on_the_same_candidates() {
    let frozen = load_semantic_gate().expect("gate inputs validate");
    let golden = frozen.corpus.clone();
    let mut holdout = frozen.corpus;
    holdout.golden_queries = frozen.holdout_queries;
    let vectors = vectors(&golden).await;

    let mut sets = serde_json::Map::new();
    let mut holdout_decision = None;
    for (set, corpus) in [("golden", &golden), ("holdout", &holdout)] {
        let mut results = Vec::new();
        for seed in STREAM_SEEDS {
            results.push(run_seed(corpus, &vectors, set, seed).await);
        }
        let linear = summary(&results, |r| &r.linear);
        let mmr = summary(&results, |r| &r.mmr);
        let relevant_drops: usize = results.iter().map(|r| r.losses.relevant_drops).sum();
        let contradiction_drops: usize = results.iter().map(|r| r.losses.contradiction_drops).sum();
        let step_p99_us = results.iter().map(|r| r.step_p99_us).max().unwrap();
        let step_p50_us = results.iter().map(|r| r.step_p50_us).max().unwrap();
        let delta = |key: &str| mmr[key].as_f64().unwrap() - linear[key].as_f64().unwrap();
        let checks = json!({
            "dedup_precision_gain_ge_0_02": delta("dedup_precision_at_5") >= 0.02,
            "recall_drop_le_0_01": delta("recall_at_5") >= -0.01,
            "ndcg_drop_le_0_01": delta("ndcg_at_5") >= -0.01,
            "no_relevant_drops": relevant_drops == 0,
            "no_contradiction_drops": contradiction_drops == 0,
            "step_p99_le_1ms": step_p99_us <= 1_000,
        });
        if set == "holdout" {
            let adopt = checks.as_object().unwrap().values().all(|v| v == true);
            holdout_decision = Some(if adopt { "adopt" } else { "defer" });
        }
        sets.insert(
            set.into(),
            json!({
                "queries": corpus.golden_queries.len(),
                "seeds": STREAM_SEEDS.len(),
                "linear": linear,
                "mmr": mmr,
                "delta": {
                    "recall_at_5": delta("recall_at_5"),
                    "ndcg_at_5": delta("ndcg_at_5"),
                    "mrr": delta("mrr"),
                    "dedup_precision_at_5": delta("dedup_precision_at_5"),
                    "injected_tokens": delta("injected_tokens"),
                },
                "silent_losses_summed_over_seeds": {
                    "relevant_drops": relevant_drops,
                    "contradiction_drops": contradiction_drops,
                },
                "mmr_step_us_worst_seed": {"p50": step_p50_us, "p99": step_p99_us},
                "decision_checks": checks,
            }),
        );
    }
    let report = json!({
        "schema": "rusty-brain-diversity-selection-v1",
        "preregistration": "docs/eval/2026-10-17-diversity-selection-preregistration.md",
        "lambda": MMR_LAMBDA,
        "candidate_pool": "Linear top-10 (RECALL_LIMIT) per query",
        "relevance": "(n - rank) / n over Linear order",
        "similarity": "cosine over stored composite document vectors (strict replay)",
        "budget": "5 rows / 800 tokens, whole rows",
        "latency_note": "in-process, shared machine; MMR arm latency = percentiles of per-query (recall + MMR step)",
        "sets": sets,
        "decision_rule_set": "holdout",
        "decision": holdout_decision,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
