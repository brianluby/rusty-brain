//! W4.1 production-embedding semantic quality gate.
//!
//! The gate manifest locks the authored corpus, untouched holdout, replay
//! fixture, input-kind contract, and preregistered floors. Loading the gate is
//! deliberately strict: drift or incomplete replay data fails before retrieval
//! runs, so CI cannot silently substitute legacy query fallbacks.

use crate::corpus::{load_committed_holdout_queries, Corpus, GoldenQuery};
use crate::replay::{
    text_sha256, EmbeddingFixture, ReplayProvider, INPUT_KIND_DOCUMENT, INPUT_KIND_QUERY,
};
use crate::runner::EvalReport;
use serde::{Deserialize, Serialize};

const MANIFEST_RAW: &str = include_str!("../semantic_gate.json");
const CORPUS_RAW: &str = include_str!("../fixtures/corpus.json");
const HOLDOUT_RAW: &str = include_str!("../fixtures/holdout_queries.json");
const FIXTURE_RAW: &str = include_str!("../fixtures/embeddings/all-MiniLM-L6-v2.json");
const PREREGISTRATION_RAW: &str =
    include_str!("../../../docs/eval/2026-07-12-w41-semantic-gate-preregistration.md");

/// Machine-readable W4.1 gate definition frozen by the preregistration.
#[derive(Debug, Clone, Deserialize)]
pub struct SemanticGateManifest {
    pub schema_version: u32,
    pub preregistration: String,
    pub corpus: CorpusLock,
    pub holdout: HoldoutLock,
    pub embedding: EmbeddingLock,
    pub floors: QualityFloors,
    pub chronological_seeds: Vec<String>,
    pub default_decision: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorpusLock {
    pub sha256: String,
    pub memories: usize,
    pub golden_queries: usize,
    pub dedup_clusters: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HoldoutLock {
    pub sha256: String,
    pub queries: usize,
    pub aggregate_only: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingLock {
    pub fixture_sha256: String,
    pub model_id: String,
    pub dim: usize,
    pub document_vectors: usize,
    pub query_vectors: usize,
    pub total_vectors: usize,
    pub require_exact_input_kinds: bool,
}

/// Preregistered quality floors applied independently to goldens and holdout.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct QualityFloors {
    pub mean_recall_at_k: f64,
    pub mrr: f64,
    pub ndcg: f64,
    pub dedup_precision: f64,
    pub fts_query_rate: f64,
    pub vector_query_rate: f64,
    pub graph_query_rate: f64,
}

/// Validated immutable inputs for one semantic-gate run.
pub struct SemanticGateInputs {
    pub manifest: SemanticGateManifest,
    pub corpus: Corpus,
    pub holdout_queries: Vec<GoldenQuery>,
    pub replay: ReplayProvider,
    pub fixture_bytes: usize,
}

/// Load and validate every frozen input before retrieval runs.
pub fn load_semantic_gate() -> Result<SemanticGateInputs, String> {
    let manifest: SemanticGateManifest = serde_json::from_str(MANIFEST_RAW)
        .map_err(|error| format!("semantic_gate.json parse error: {error}"))?;
    validate_manifest_shape(&manifest)?;

    verify_hash("corpus", CORPUS_RAW, &manifest.corpus.sha256)?;
    verify_hash("holdout", HOLDOUT_RAW, &manifest.holdout.sha256)?;
    verify_hash(
        "embedding fixture",
        FIXTURE_RAW,
        &manifest.embedding.fixture_sha256,
    )?;

    let corpus = Corpus::from_json(CORPUS_RAW).map_err(|error| error.to_string())?;
    if corpus.memories.len() != manifest.corpus.memories
        || corpus.golden_queries.len() != manifest.corpus.golden_queries
        || corpus.dedup_clusters.len() != manifest.corpus.dedup_clusters
    {
        return Err(format!(
            "frozen corpus shape drifted: got memories={} goldens={} clusters={}",
            corpus.memories.len(),
            corpus.golden_queries.len(),
            corpus.dedup_clusters.len()
        ));
    }

    let holdout_queries = load_committed_holdout_queries().map_err(|error| error.to_string())?;
    if holdout_queries.len() != manifest.holdout.queries {
        return Err(format!(
            "frozen holdout shape drifted: got {} queries, expected {}",
            holdout_queries.len(),
            manifest.holdout.queries
        ));
    }

    let fixture: EmbeddingFixture = serde_json::from_str(FIXTURE_RAW)
        .map_err(|error| format!("embedding fixture parse error: {error}"))?;
    validate_fixture_lock(&fixture, &manifest.embedding)?;
    let replay = ReplayProvider::from_fixture_strict(&fixture)?;

    Ok(SemanticGateInputs {
        manifest,
        corpus,
        holdout_queries,
        replay,
        fixture_bytes: FIXTURE_RAW.len(),
    })
}

/// Apply all preregistered floors to one aggregate report.
pub fn check_semantic_floors(
    label: &str,
    report: &EvalReport,
    floors: QualityFloors,
) -> Result<(), String> {
    let mut failures = Vec::new();
    gate(
        &mut failures,
        "mean_recall_at_k",
        report.mean_recall_at_k,
        floors.mean_recall_at_k,
    );
    gate(&mut failures, "mrr", report.mrr, floors.mrr);
    gate(&mut failures, "ndcg", report.ndcg, floors.ndcg);
    gate(
        &mut failures,
        "dedup_precision",
        report.dedup_precision,
        floors.dedup_precision,
    );
    gate(
        &mut failures,
        "fts_query_rate",
        report.channels.fts_query_rate,
        floors.fts_query_rate,
    );
    gate(
        &mut failures,
        "vector_query_rate",
        report.channels.vector_query_rate,
        floors.vector_query_rate,
    );
    gate(
        &mut failures,
        "graph_query_rate",
        report.channels.graph_query_rate,
        floors.graph_query_rate,
    );

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "W4.1 semantic gate failed for {label}:\n  {}",
            failures.join("\n  ")
        ))
    }
}

fn gate(failures: &mut Vec<String>, name: &str, actual: f64, floor: f64) {
    if !actual.is_finite() || !(0.0..=1.0).contains(&actual) {
        failures.push(format!(
            "{name}: actual {actual:.4} is outside the valid [0, 1] range"
        ));
    } else if actual + 1e-9 < floor {
        failures.push(format!(
            "{name}: actual {actual:.4} < preregistered floor {floor:.4}"
        ));
    }
}

fn verify_hash(label: &str, raw: &str, expected: &str) -> Result<(), String> {
    let actual = text_sha256(raw);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} SHA-256 drifted: actual {actual}, expected {expected}"
        ))
    }
}

fn validate_manifest_shape(manifest: &SemanticGateManifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "unsupported semantic gate schema_version {}",
            manifest.schema_version
        ));
    }
    if manifest.preregistration != "docs/eval/2026-07-12-w41-semantic-gate-preregistration.md"
        || PREREGISTRATION_RAW.trim().is_empty()
    {
        return Err("semantic gate preregistration is missing or unexpected".to_string());
    }
    if !manifest.holdout.aggregate_only {
        return Err("holdout must remain aggregate-only".to_string());
    }
    if !manifest.embedding.require_exact_input_kinds {
        return Err("semantic gate must require exact embedding input kinds".to_string());
    }
    if manifest.default_decision != "keep_linear" {
        return Err("the preregistered default decision must remain keep_linear".to_string());
    }
    if manifest.chronological_seeds.len() != 5 {
        return Err("semantic gate requires exactly five chronological seeds".to_string());
    }
    let parsed = manifest
        .chronological_seeds
        .iter()
        .map(|seed| {
            chrono::DateTime::parse_from_rfc3339(seed)
                .map_err(|error| format!("invalid chronological seed {seed:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parsed.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("semantic gate seeds must be strictly chronological".to_string());
    }
    for (name, floor) in [
        ("mean_recall_at_k", manifest.floors.mean_recall_at_k),
        ("mrr", manifest.floors.mrr),
        ("ndcg", manifest.floors.ndcg),
        ("dedup_precision", manifest.floors.dedup_precision),
        ("fts_query_rate", manifest.floors.fts_query_rate),
        ("vector_query_rate", manifest.floors.vector_query_rate),
        ("graph_query_rate", manifest.floors.graph_query_rate),
    ] {
        if !floor.is_finite() || !(0.0..=1.0).contains(&floor) {
            return Err(format!("semantic gate floor {name} is invalid: {floor}"));
        }
    }
    Ok(())
}

fn validate_fixture_lock(fixture: &EmbeddingFixture, lock: &EmbeddingLock) -> Result<(), String> {
    let documents = fixture
        .vectors
        .iter()
        .filter(|vector| vector.input_kind == INPUT_KIND_DOCUMENT)
        .count();
    let queries = fixture
        .vectors
        .iter()
        .filter(|vector| vector.input_kind == INPUT_KIND_QUERY)
        .count();
    if fixture.model_id != lock.model_id
        || fixture.dim != lock.dim
        || documents != lock.document_vectors
        || queries != lock.query_vectors
        || fixture.vectors.len() != lock.total_vectors
    {
        return Err(format!(
            "embedding fixture lock drifted: model={:?} dim={} documents={} queries={} total={}",
            fixture.model_id,
            fixture.dim,
            documents,
            queries,
            fixture.vectors.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_gate_inputs_match_every_frozen_lock() {
        let inputs = load_semantic_gate().unwrap();
        assert_eq!(inputs.corpus.memories.len(), 205);
        assert_eq!(inputs.holdout_queries.len(), 20);
        assert_eq!(inputs.replay.query_fallbacks(), 0);
    }

    #[test]
    fn floor_check_reports_every_regression() {
        let floors = QualityFloors {
            mean_recall_at_k: 0.8,
            mrr: 0.7,
            ndcg: 0.75,
            dedup_precision: 0.9,
            fts_query_rate: 0.8,
            vector_query_rate: 0.95,
            graph_query_rate: 0.0,
        };
        let report = EvalReport {
            mean_recall_at_k: 0.1,
            mrr: 0.1,
            dedup_precision: 0.1,
            ndcg: 0.1,
            channels: crate::runner::ChannelContribution::default(),
            p50_latency_us: 0,
            p99_latency_us: 0,
        };

        let error = check_semantic_floors("test", &report, floors).unwrap_err();
        for metric in [
            "mean_recall_at_k",
            "mrr",
            "ndcg",
            "dedup_precision",
            "fts_query_rate",
            "vector_query_rate",
        ] {
            assert!(error.contains(metric), "missing regression for {metric}");
        }
        assert!(!error.contains("graph_query_rate"));
    }

    #[test]
    fn floor_check_rejects_metrics_outside_probability_range() {
        let mut failures = Vec::new();
        gate(&mut failures, "too_high", 1.01, 0.5);
        gate(&mut failures, "too_low", -0.01, 0.0);
        gate(&mut failures, "nan", f64::NAN, 0.0);

        assert_eq!(failures.len(), 3);
        assert!(failures
            .iter()
            .all(|failure| failure.contains("valid [0, 1]")));
    }
}
