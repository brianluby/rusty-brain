//! Aggregate-only controlled W4.1 retrieval and shadow-admission report.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_eval::{
    build_engine_with_at, detailed_rankings, evaluate_admission_arm, evaluate_retrieval_arm,
    exact_evidence_rankings, importance_rankings, load_semantic_gate, recency_rankings,
    run_corpus_with, AdmissionArm, AdmissionArmReport, Corpus, DetailedRun, FusionMode,
    RetrievalArm, RetrievalArmReport, STREAM_SEEDS,
};
use serde_json::json;
use std::time::Instant;

fn seed_time(seed: u64) -> chrono::DateTime<chrono::Utc> {
    let value = [
        (20_260_101, "2026-01-01T00:00:00Z"),
        (20_260_201, "2026-02-01T00:00:00Z"),
        (20_260_301, "2026-03-01T00:00:00Z"),
        (20_260_401, "2026-04-01T00:00:00Z"),
        (20_260_501, "2026-05-01T00:00:00Z"),
    ]
    .into_iter()
    .find_map(|(registered, value)| (registered == seed).then_some(value))
    .expect("controlled-arm seed is registered");
    chrono::DateTime::parse_from_rfc3339(value)
        .unwrap()
        .to_utc()
}

async fn production_run(corpus: &Corpus, mode: FusionMode, seed: u64) -> DetailedRun {
    let inputs = load_semantic_gate().expect("frozen controlled inputs validate");
    let engine = build_engine_with_at(inputs.replay, seed_time(seed))
        .expect("engine builds")
        .with_fusion_mode(mode);
    run_corpus_with(&engine, corpus)
        .await
        .expect("strict replay covers controlled corpus")
}

fn production_report(
    corpus: &Corpus,
    run: &DetailedRun,
    arm: RetrievalArm,
    set: &str,
    seed: u64,
) -> RetrievalArmReport {
    let rankings = detailed_rankings(corpus, run);
    let mut report = evaluate_retrieval_arm(
        corpus,
        &rankings,
        arm,
        set,
        seed,
        &[run.report.p50_latency_us, run.report.p99_latency_us],
    )
    .unwrap();
    report.metrics.p50_latency_us = run.report.p50_latency_us;
    report.metrics.p99_latency_us = run.report.p99_latency_us;
    report
}

fn generated_report(
    corpus: &Corpus,
    arm: RetrievalArm,
    set: &str,
    seed: u64,
    build: impl FnOnce() -> Vec<Vec<String>>,
) -> RetrievalArmReport {
    let started = Instant::now();
    let rankings = build();
    let per_query_us =
        (started.elapsed().as_micros() as u64).div_ceil(corpus.golden_queries.len() as u64);
    let latencies = vec![per_query_us; corpus.golden_queries.len()];
    evaluate_retrieval_arm(corpus, &rankings, arm, set, seed, &latencies).unwrap()
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<f64> = values.collect();
    values.iter().sum::<f64>() / values.len() as f64
}

fn retrieval_average(
    reports: &[RetrievalArmReport],
    arm: RetrievalArm,
    metric: impl Fn(&RetrievalArmReport) -> f64,
) -> f64 {
    mean(
        reports
            .iter()
            .filter(|report| report.arm == arm)
            .map(metric),
    )
}

fn admission_average(
    reports: &[AdmissionArmReport],
    arm: AdmissionArm,
    metric: impl Fn(&AdmissionArmReport) -> f64,
) -> f64 {
    mean(
        reports
            .iter()
            .filter(|report| report.arm == arm)
            .map(metric),
    )
}

#[tokio::test]
#[ignore = "scheduled/manual aggregate-only controlled arms; never mutates production behavior"]
async fn controlled_retrieval_and_admission_arms_report_every_seed() {
    let frozen = load_semantic_gate().expect("gate inputs validate");
    let golden = frozen.corpus.clone();
    let mut holdout = frozen.corpus;
    holdout.golden_queries = frozen.holdout_queries;
    let mut retrieval = Vec::new();
    let mut admission = Vec::new();

    for seed in STREAM_SEEDS {
        for (set, corpus) in [("golden", &golden), ("holdout", &holdout)] {
            let linear = production_run(corpus, FusionMode::Linear, seed).await;
            let rrf = production_run(corpus, FusionMode::Rrf, seed).await;
            retrieval.push(production_report(
                corpus,
                &linear,
                RetrievalArm::Linear,
                set,
                seed,
            ));
            retrieval.push(production_report(
                corpus,
                &rrf,
                RetrievalArm::Rrf,
                set,
                seed,
            ));
            retrieval.push(generated_report(
                corpus,
                RetrievalArm::ExactEvidence,
                set,
                seed,
                || exact_evidence_rankings(corpus, &linear),
            ));
            retrieval.push(generated_report(
                corpus,
                RetrievalArm::RecencyOnly,
                set,
                seed,
                || recency_rankings(corpus, seed).unwrap(),
            ));
            retrieval.push(generated_report(
                corpus,
                RetrievalArm::ImportanceOnly,
                set,
                seed,
                || importance_rankings(corpus),
            ));

            for arm in [
                AdmissionArm::NoveltyOnly,
                AdmissionArm::ImportanceConfidence,
                AdmissionArm::Combined,
            ] {
                admission.push(evaluate_admission_arm(corpus, arm, set, seed).unwrap());
            }
        }
    }

    let holdout_retrieval: Vec<_> = retrieval
        .iter()
        .filter(|report| report.set == "holdout")
        .cloned()
        .collect();
    let linear_exact = retrieval_average(&holdout_retrieval, RetrievalArm::Linear, |r| {
        r.metrics.exact_span_coverage
    });
    let exact_exact = retrieval_average(&holdout_retrieval, RetrievalArm::ExactEvidence, |r| {
        r.metrics.exact_span_coverage
    });
    let linear_answer = retrieval_average(&holdout_retrieval, RetrievalArm::Linear, |r| {
        r.metrics.answer_accuracy
    });
    let exact_answer = retrieval_average(&holdout_retrieval, RetrievalArm::ExactEvidence, |r| {
        r.metrics.answer_accuracy
    });
    let exact_quality_ok = [
        |r: &RetrievalArmReport| r.metrics.recall_at_5,
        |r: &RetrievalArmReport| r.metrics.mrr,
        |r: &RetrievalArmReport| r.metrics.ndcg_at_5,
        |r: &RetrievalArmReport| r.metrics.dedup_precision_at_5,
    ]
    .into_iter()
    .all(|metric| {
        retrieval_average(&holdout_retrieval, RetrievalArm::ExactEvidence, metric) + 0.01
            >= retrieval_average(&holdout_retrieval, RetrievalArm::Linear, metric)
    });
    let exact_go = (exact_exact >= linear_exact + 0.01 || exact_answer >= linear_answer + 0.01)
        && exact_quality_ok
        && retrieval_average(&holdout_retrieval, RetrievalArm::ExactEvidence, |r| {
            r.metrics.stale_exposure_rate
        }) <= retrieval_average(&holdout_retrieval, RetrievalArm::Linear, |r| {
            r.metrics.stale_exposure_rate
        })
        && retrieval_average(&holdout_retrieval, RetrievalArm::ExactEvidence, |r| {
            r.metrics.poison_exposure_rate
        }) <= retrieval_average(&holdout_retrieval, RetrievalArm::Linear, |r| {
            r.metrics.poison_exposure_rate
        });

    let holdout_admission: Vec<_> = admission
        .iter()
        .filter(|report| report.set == "holdout")
        .cloned()
        .collect();
    let combined_quality_ok = [
        |r: &AdmissionArmReport| r.metrics.recall_at_5,
        |r: &AdmissionArmReport| r.metrics.mrr,
        |r: &AdmissionArmReport| r.metrics.ndcg_at_5,
    ]
    .into_iter()
    .all(|metric| {
        let combined = admission_average(&holdout_admission, AdmissionArm::Combined, metric);
        let best_component =
            admission_average(&holdout_admission, AdmissionArm::NoveltyOnly, metric).max(
                admission_average(
                    &holdout_admission,
                    AdmissionArm::ImportanceConfidence,
                    metric,
                ),
            );
        combined + 0.01 >= best_component
    });
    let combined_reports = admission
        .iter()
        .filter(|report| report.arm == AdmissionArm::Combined);
    let combined_safety_ok = combined_reports.clone().all(|report| {
        report.poison_rows_retained == 0
            && report.metrics.poison_exposure_rate == 0.0
            && report.metrics.stale_exposure_rate == 0.0
            && report.metrics.contested_disclosure_rate == 1.0
            && report.retained_rows <= 128
            && report.retained_bytes <= 32_768
    });
    let full_corpus_bytes: usize = golden
        .memories
        .iter()
        .map(|memory| memory.content.len())
        .sum();
    let combined_reduction_ok = combined_reports.clone().all(|report| {
        report.retained_rows * 4 <= golden.memories.len() * 3
            || report.retained_bytes * 4 <= full_corpus_bytes * 3
    });
    let admission_go = combined_quality_ok && combined_safety_ok && combined_reduction_ok;

    let retrieval_summary: Vec<_> = [
        RetrievalArm::Linear,
        RetrievalArm::Rrf,
        RetrievalArm::ExactEvidence,
        RetrievalArm::RecencyOnly,
        RetrievalArm::ImportanceOnly,
    ]
    .into_iter()
    .map(|arm| {
        json!({
            "arm": arm,
            "recall_at_5": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.recall_at_5),
            "mrr": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.mrr),
            "ndcg_at_5": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.ndcg_at_5),
            "dedup_precision_at_5": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.dedup_precision_at_5),
            "exact_span_coverage": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.exact_span_coverage),
            "answer_accuracy": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.answer_accuracy),
            "stale_exposure_rate": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.stale_exposure_rate),
            "wrong_answer_rate": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.wrong_answer_rate),
            "poison_exposure_rate": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.poison_exposure_rate),
            "contested_disclosure_rate": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.contested_disclosure_rate),
            "injected_tokens": retrieval_average(&holdout_retrieval, arm, |r| r.metrics.injected_tokens as f64),
        })
    })
    .collect();
    let admission_summary: Vec<_> = [
        AdmissionArm::NoveltyOnly,
        AdmissionArm::ImportanceConfidence,
        AdmissionArm::Combined,
    ]
    .into_iter()
    .map(|arm| {
        json!({
            "arm": arm,
            "recall_at_5": admission_average(&holdout_admission, arm, |r| r.metrics.recall_at_5),
            "mrr": admission_average(&holdout_admission, arm, |r| r.metrics.mrr),
            "ndcg_at_5": admission_average(&holdout_admission, arm, |r| r.metrics.ndcg_at_5),
            "dedup_precision_at_5": admission_average(&holdout_admission, arm, |r| r.metrics.dedup_precision_at_5),
            "exact_span_coverage": admission_average(&holdout_admission, arm, |r| r.metrics.exact_span_coverage),
            "answer_accuracy": admission_average(&holdout_admission, arm, |r| r.metrics.answer_accuracy),
            "retained_relevant_rate": admission_average(&holdout_admission, arm, |r| r.retained_relevant_rate),
            "retained_rows": admission_average(&holdout_admission, arm, |r| r.retained_rows as f64),
            "retained_bytes": admission_average(&holdout_admission, arm, |r| r.retained_bytes as f64),
            "stale_rows_retained": admission_average(&holdout_admission, arm, |r| r.stale_rows_retained as f64),
            "poison_rows_retained": admission_average(&holdout_admission, arm, |r| r.poison_rows_retained as f64),
            "stale_exposure_rate": admission_average(&holdout_admission, arm, |r| r.metrics.stale_exposure_rate),
            "poison_exposure_rate": admission_average(&holdout_admission, arm, |r| r.metrics.poison_exposure_rate),
            "contested_disclosure_rate": admission_average(&holdout_admission, arm, |r| r.metrics.contested_disclosure_rate),
            "injected_tokens": admission_average(&holdout_admission, arm, |r| r.metrics.injected_tokens as f64),
        })
    })
    .collect();
    let summary = json!({
        "retrieval_holdout_five_seed_mean": retrieval_summary,
        "admission_holdout_five_seed_mean": admission_summary,
        "decision": {
            "exact_evidence_lane_go": exact_go,
            "combined_admission_go": admission_go,
        }
    });

    assert_eq!(retrieval.len(), STREAM_SEEDS.len() * 2 * 5);
    assert_eq!(admission.len(), STREAM_SEEDS.len() * 2 * 3);
    assert!(
        !exact_go,
        "exact lane must retain its frozen NO-GO decision"
    );
    assert!(
        admission_go,
        "combined admission must retain its frozen pilot GO decision"
    );
    println!(
        "CONTROLLED_SUMMARY={}",
        serde_json::to_string(&summary).unwrap()
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "gate": "W4.1-controlled-offline-arms",
            "holdout_policy": "aggregate-only",
            "voyage": "not_run_missing_existing_credential",
            "retrieval": retrieval,
            "admission": admission,
            "decision": {
                "exact_evidence_lane_go": exact_go,
                "combined_admission_go": admission_go,
                "production_changes": "none"
            }
        }))
        .unwrap()
    );
}
