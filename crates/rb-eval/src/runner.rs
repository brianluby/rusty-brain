//! The harness runner: ingest fixtures, run golden queries, compute metrics,
//! and compare against committed baselines.
//!
//! Flow (spec §6): load fixtures -> `engine.remember` each (deterministic
//! vectors) -> for each golden query, `engine.recall` -> compute metrics vs
//! expected -> assert each metric >= its committed baseline.

use crate::backend::{eval_namespace, SqliteBackend, EVAL_DIM};
use crate::corpus::Corpus;
use crate::metrics;
use rb_embed::{DeterministicProvider, EmbeddingProvider};
use rb_engine::{MemoryEngine, RememberInput};
use rb_search::FusionMode;
use rb_types::MemoryId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

/// `recall` over-fetch + metric horizon. Golden queries may override per-query.
const DEFAULT_K: usize = 5;
/// How many candidates to ask recall for (>= max metric k so top-k is filled).
const RECALL_LIMIT: usize = 10;

/// Aggregate metrics over the whole fixture run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalReport {
    /// Mean recall@k across golden queries (each query's own k, or `DEFAULT_K`).
    pub mean_recall_at_k: f64,
    /// Mean reciprocal rank across golden queries.
    pub mrr: f64,
    /// Mean dedup precision across golden queries (top-`DEFAULT_K`).
    pub dedup_precision: f64,
    /// p50 recall latency in microseconds.
    pub p50_latency_us: u64,
    /// p99 recall latency in microseconds.
    pub p99_latency_us: u64,
}

/// Committed baselines. The harness fails if a *quality* metric drops below its
/// baseline. Latency baselines are advisory (machine-dependent) and are not
/// asserted by default; they are recorded for observability.
#[derive(Debug, Clone, Deserialize)]
pub struct Baselines {
    pub mean_recall_at_k: f64,
    pub mrr: f64,
    pub dedup_precision: f64,
}

impl Baselines {
    /// Parse committed baselines bundled at compile time.
    pub fn committed() -> Result<Self, String> {
        const RAW: &str = include_str!("../baselines.json");
        serde_json::from_str(RAW).map_err(|e| format!("baselines.json parse error: {e}"))
    }
}

/// Build a fresh in-memory engine over the deterministic provider, using the
/// given fusion mode (`Linear` is the committed-baseline default; `Rrf` is the
/// opt-in comparison path — spec §7).
fn build_engine(
    mode: FusionMode,
) -> rb_types::Result<MemoryEngine<SqliteBackend, DeterministicProvider>> {
    let backend = SqliteBackend::in_memory(EVAL_DIM)?;
    let embedder = DeterministicProvider::new(EVAL_DIM);
    Ok(MemoryEngine::new(backend, embedder, eval_namespace()).with_fusion_mode(mode))
}

/// Build a fresh in-memory engine over an arbitrary provider (real-model mode).
///
/// The store dimension is taken from the provider so the dim contract holds.
pub fn build_engine_with<P: EmbeddingProvider>(
    embedder: P,
) -> rb_types::Result<MemoryEngine<SqliteBackend, P>> {
    let backend = SqliteBackend::in_memory(embedder.dim())?;
    Ok(MemoryEngine::new(backend, embedder, eval_namespace()))
}

/// Ingest the corpus through the engine, returning a fixture-key -> MemoryId map.
///
/// Each fixture is remembered with its authored enrichment fields supplied, so
/// the engine stores them verbatim (the `remember` path keeps caller-supplied
/// keywords/tags/context, falling back to heuristics only for empties).
///
/// A fixture's `confidence` (default `1.0`) is stamped on the stored row after
/// remember (the `remember` path always writes full trust), so the confidence
/// dampener (Feature C) is exercised by fixtures like the "poison" scenario.
async fn ingest<P: EmbeddingProvider>(
    engine: &MemoryEngine<SqliteBackend, P>,
    corpus: &Corpus,
) -> rb_types::Result<HashMap<String, MemoryId>> {
    let mut key_to_id: HashMap<String, MemoryId> = HashMap::new();
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
                confidence: 1.0,
                provenance: Default::default(),
            })
            .await?;
        // Apply the authored confidence (no-op at the 1.0 default).
        if (m.confidence - 1.0).abs() > f32::EPSILON {
            engine.backend().set_confidence(&id, m.confidence)?;
        }
        key_to_id.insert(m.key.clone(), id);
    }
    Ok(key_to_id)
}

/// Run the full fixture suite and produce an aggregate report.
pub async fn run_eval() -> rb_types::Result<EvalReport> {
    let corpus = Corpus::from_json(include_str!("../fixtures/corpus.json"))
        .map_err(|e| rb_types::Error::Storage(format!("corpus load failed: {e}")))?;
    run_corpus(&corpus).await
}

/// Run an explicit corpus (used by `run_eval` and by integration tests that
/// inject hand-built corpora, e.g. the confidence "poison" scenario). Uses the
/// committed-default `Linear` fusion.
pub async fn run_corpus(corpus: &Corpus) -> rb_types::Result<EvalReport> {
    Ok(run_corpus_detailed(corpus).await?.report)
}

/// Run the committed fixtures under an explicit fusion mode. Lets a maintainer
/// measure `Rrf` without touching the default-`Linear` regression gate.
pub async fn run_eval_mode(mode: FusionMode) -> rb_types::Result<EvalReport> {
    let corpus = Corpus::from_json(include_str!("../fixtures/corpus.json"))
        .map_err(|e| rb_types::Error::Storage(format!("corpus load failed: {e}")))?;
    Ok(run_corpus_detailed_mode(&corpus, mode).await?.report)
}

/// Per-metric delta (`Rrf` minus `Linear`) over the same fixtures, plus the two
/// underlying reports. A positive delta means `Rrf` improved that metric. This
/// is observability only — it never flips the default; the eval-gated flip is a
/// later, intentional commit (spec §7).
#[derive(Debug, Clone)]
pub struct ModeComparison {
    pub linear: EvalReport,
    pub rrf: EvalReport,
    pub recall_at_k_delta: f64,
    pub mrr_delta: f64,
    pub dedup_precision_delta: f64,
}

/// Run a corpus under both `Linear` and `Rrf` and report the metric deltas.
pub async fn compare_modes(corpus: &Corpus) -> rb_types::Result<ModeComparison> {
    let linear = run_corpus_detailed_mode(corpus, FusionMode::Linear)
        .await?
        .report;
    let rrf = run_corpus_detailed_mode(corpus, FusionMode::Rrf)
        .await?
        .report;
    Ok(ModeComparison {
        recall_at_k_delta: rrf.mean_recall_at_k - linear.mean_recall_at_k,
        mrr_delta: rrf.mrr - linear.mrr,
        dedup_precision_delta: rrf.dedup_precision - linear.dedup_precision,
        linear,
        rrf,
    })
}

/// Run the committed fixtures under both modes and report the deltas.
pub async fn compare_modes_committed() -> rb_types::Result<ModeComparison> {
    let corpus = Corpus::from_json(include_str!("../fixtures/corpus.json"))
        .map_err(|e| rb_types::Error::Storage(format!("corpus load failed: {e}")))?;
    compare_modes(&corpus).await
}

/// Per-query detail produced by [`run_corpus_detailed`]. Lets a maintainer see
/// exactly which fixture *keys* recall surfaced, in order — the readable diff a
/// regression needs ("which result reordered?").
#[derive(Debug, Clone)]
pub struct QueryDetail {
    pub query: String,
    /// Fixture keys in recall order (ids resolved back to authored keys). Ids
    /// recall surfaces that are not in the corpus map are omitted.
    pub ranked_keys: Vec<String>,
    /// Expected fixture keys for this query.
    pub expected_keys: Vec<String>,
    pub recall_at_k: f64,
    pub reciprocal_rank: f64,
    pub dedup_precision: f64,
}

/// Full run output: the aggregate report plus per-query detail.
#[derive(Debug, Clone)]
pub struct DetailedRun {
    pub report: EvalReport,
    pub per_query: Vec<QueryDetail>,
}

/// Run a corpus through the default deterministic engine (`Linear` fusion) and
/// return both the aggregate report and per-query detail. This is the committed
/// regression gate's path; the default does not change.
pub async fn run_corpus_detailed(corpus: &Corpus) -> rb_types::Result<DetailedRun> {
    run_corpus_detailed_mode(corpus, FusionMode::Linear).await
}

/// Run a corpus through the deterministic engine under an explicit fusion mode.
/// Used by the mode-comparison harness to evaluate `Linear` vs `Rrf` over the
/// same fixtures without flipping the default.
pub async fn run_corpus_detailed_mode(
    corpus: &Corpus,
    mode: FusionMode,
) -> rb_types::Result<DetailedRun> {
    let engine = build_engine(mode)?;
    run_corpus_with(&engine, corpus).await
}

/// Run a corpus through an explicit engine (any provider). The deterministic
/// path delegates here; the real-model `#[ignore]` mode calls it with a Voyage
/// or local engine built via [`build_engine_with`].
pub async fn run_corpus_with<P: EmbeddingProvider>(
    engine: &MemoryEngine<SqliteBackend, P>,
    corpus: &Corpus,
) -> rb_types::Result<DetailedRun> {
    let key_to_id = ingest(engine, corpus).await?;
    // Reverse map so recall ids can be reported as authored keys.
    let id_to_key: HashMap<String, String> = key_to_id
        .iter()
        .map(|(k, id)| (id.to_string(), k.clone()))
        .collect();

    // Resolve dedup clusters to id strings once.
    let id_clusters: Vec<Vec<String>> = corpus
        .dedup_clusters
        .iter()
        .map(|c| {
            c.members
                .iter()
                .filter_map(|k| key_to_id.get(k))
                .map(|id| id.to_string())
                .collect()
        })
        .collect();

    let mut per_query: Vec<QueryDetail> = Vec::new();
    let mut rr_inputs: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    let mut latencies_us: Vec<u64> = Vec::new();

    for q in &corpus.golden_queries {
        let started = Instant::now();
        let results = engine.recall(&q.query, RECALL_LIMIT, None, &[]).await?;
        latencies_us.push(started.elapsed().as_micros() as u64);

        let ranked: Vec<String> = results.iter().map(|r| r.memory.id.to_string()).collect();
        let expected: Vec<String> = q
            .expected
            .iter()
            .filter_map(|k| key_to_id.get(k))
            .map(|id| id.to_string())
            .collect();

        let k = q.k.unwrap_or(DEFAULT_K);
        let recall = metrics::recall_at_k(&ranked, &expected, k);
        let rr = metrics::reciprocal_rank(&ranked, &expected);
        let dedup = metrics::dedup_precision(&ranked, &id_clusters, DEFAULT_K);
        rr_inputs.push((ranked.clone(), expected));

        per_query.push(QueryDetail {
            query: q.query.clone(),
            ranked_keys: ranked
                .iter()
                .filter_map(|id| id_to_key.get(id).cloned())
                .collect(),
            expected_keys: q.expected.clone(),
            recall_at_k: recall,
            reciprocal_rank: rr,
            dedup_precision: dedup,
        });
    }

    let mean_recall_at_k = mean(&per_query.iter().map(|d| d.recall_at_k).collect::<Vec<_>>());
    let mrr = metrics::mrr(&rr_inputs);
    let dedup_precision = mean(
        &per_query
            .iter()
            .map(|d| d.dedup_precision)
            .collect::<Vec<_>>(),
    );

    let report = EvalReport {
        mean_recall_at_k,
        mrr,
        dedup_precision,
        p50_latency_us: metrics::p50(&latencies_us),
        p99_latency_us: metrics::p99(&latencies_us),
    };
    Ok(DetailedRun { report, per_query })
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Assert that a report meets the committed baselines, returning a readable
/// diff string on the first regression. Quality metrics only; latency is
/// advisory and not gated (machine-dependent).
pub fn check_against_baselines(report: &EvalReport, baselines: &Baselines) -> Result<(), String> {
    let tol = 1e-9;
    let mut failures: Vec<String> = Vec::new();
    let mut gate = |name: &str, actual: f64, baseline: f64| {
        if actual + tol < baseline {
            failures.push(format!(
                "{name}: actual {actual:.4} < baseline {baseline:.4}"
            ));
        }
    };
    gate(
        "mean_recall_at_k",
        report.mean_recall_at_k,
        baselines.mean_recall_at_k,
    );
    gate("mrr", report.mrr, baselines.mrr);
    gate(
        "dedup_precision",
        report.dedup_precision,
        baselines.dedup_precision,
    );

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "rb-eval regression(s):\n  {}",
            failures.join("\n  ")
        ))
    }
}
