//! LongMemEval adapter (Vikunja #60).
//!
//! Every question gets a brand-new in-memory store holding only that
//! question's haystack, mirroring MemPalace's per-question collection. Source
//! session ids never enter stored content, tags or keywords: LongMemEval names
//! gold sessions `answer_*`, so any stored id would leak the label into FTS
//! and embeddings. Ids round-trip through the adapter's `MemoryId -> session`
//! map instead. The per-turn `has_answer` flag is never deserialized.

use super::metrics::{score, QuestionMetrics};
use crate::backend::SqliteBackend;
use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_engine::{MemoryBackend, MemoryEngine, Provenance, RememberInput};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, RecallFilter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub question_id: String,
    pub question_type: String,
    pub question: String,
    pub haystack_session_ids: Vec<String>,
    pub haystack_sessions: Vec<Vec<Turn>>,
    pub answer_session_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// MemPalace raw-mode ingestion, ranked by rusty-brain's vector index only.
    Parity,
    /// Standard `remember` -> `recall` with heuristic enrichment and fusion.
    Native,
}

/// Ranking depth. Matches MemPalace's `n_results=50`; metrics use k <= 10.
pub const TOP_N: usize = 50;

/// MemPalace raw session granularity: one document per session, user turns
/// joined by newline, sessions with no user turn skipped.
pub fn session_documents(entry: &Entry) -> Vec<(String, String)> {
    entry
        .haystack_session_ids
        .iter()
        .zip(&entry.haystack_sessions)
        .filter_map(|(id, turns)| {
            let user: Vec<&str> = turns
                .iter()
                .filter(|t| t.role == "user")
                .map(|t| t.content.as_str())
                .collect();
            (!user.is_empty()).then(|| (id.clone(), user.join("\n")))
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub session_id: String,
    /// Parity: cosine distance (lower is better). Native: fused recall score.
    pub score: f32,
    pub fts: bool,
    pub vector: bool,
    pub graph: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuestionRecord {
    pub question_id: String,
    pub question_type: String,
    pub gold: Vec<String>,
    pub corpus_size: usize,
    pub ranked: Vec<Hit>,
    pub abstained: Option<String>,
    pub metrics: QuestionMetrics,
    pub failure: &'static str,
    pub ingest_ms: u128,
    pub query_us: u128,
}

fn classify(m: &QuestionMetrics, abstained: bool) -> &'static str {
    if abstained {
        "abstained"
    } else if m.at[&5].recall_any > 0.0 {
        "hit_at_5"
    } else if m.at[&10].recall_any > 0.0 {
        "hit_at_10_only"
    } else if m.mrr > 0.0 {
        "hit_below_10"
    } else {
        "miss"
    }
}

/// Lets one loaded model serve every per-question engine.
pub struct Shared<P>(pub Arc<P>);

#[async_trait::async_trait]
impl<P: EmbeddingProvider> EmbeddingProvider for Shared<P> {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }
    fn dim(&self) -> usize {
        self.0.dim()
    }
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        self.0.embed(texts, kind).await
    }
}

fn namespace() -> Namespace {
    Namespace::Project("external-bench".into())
}

fn fixed_now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .map(|t| t.to_utc())
        .unwrap_or_default()
}

pub async fn run_question<P: EmbeddingProvider>(
    provider: &Arc<P>,
    entry: &Entry,
    profile: Profile,
) -> rb_types::Result<QuestionRecord> {
    let docs = session_documents(entry);
    let backend = SqliteBackend::in_memory(provider.dim())?;
    let started = Instant::now();
    let mut id_to_session: HashMap<MemoryId, String> = HashMap::new();
    let (ingest_ms, query_us, ranked, abstained) = match profile {
        Profile::Parity => {
            let texts: Vec<String> = docs.iter().map(|(_, d)| d.clone()).collect();
            let vectors = provider.embed(&texts, EmbedKind::Document).await?;
            for ((session, doc), vector) in docs.iter().zip(vectors) {
                let mut note = MemoryNote::new(namespace(), doc.clone(), MemoryType::Insight, 5);
                note.embedding_model = provider.model_id().to_string();
                id_to_session.insert(note.id.clone(), session.clone());
                backend.write(note, Some(vector)).await?;
            }
            let ingest_ms = started.elapsed().as_millis();
            let q = Instant::now();
            let query = provider
                .embed(std::slice::from_ref(&entry.question), EmbedKind::Query)
                .await?
                .pop()
                .unwrap_or_default();
            let hits = backend.vector(namespace(), query, TOP_N).await?;
            let query_us = q.elapsed().as_micros();
            let ranked: Vec<Hit> = hits
                .into_iter()
                .filter_map(|(id, distance)| {
                    id_to_session.get(&id).map(|s| Hit {
                        session_id: s.clone(),
                        score: distance,
                        fts: false,
                        vector: true,
                        graph: false,
                    })
                })
                .collect();
            (ingest_ms, query_us, ranked, None)
        }
        Profile::Native => {
            let engine = MemoryEngine::new(backend, Shared(provider.clone()), namespace())
                .with_fixed_now(fixed_now());
            for (session, doc) in &docs {
                let id = engine
                    .remember(RememberInput {
                        content: doc.clone(),
                        context: None,
                        memory_type: MemoryType::Insight,
                        importance: 5,
                        keywords: Vec::new(),
                        tags: Vec::new(),
                        related_files: Vec::new(),
                        confidence: None,
                        provenance: Provenance {
                            origin_source: Some("cli".into()),
                            channel: Some(rb_types::WriteChannel::Cli),
                            ..Provenance::default()
                        },
                        anchors: Vec::new(),
                        trust_class: None,
                    })
                    .await?;
                id_to_session.insert(id, session.clone());
            }
            let ingest_ms = started.elapsed().as_millis();
            let q = Instant::now();
            let outcome = engine
                .recall_with_status(&entry.question, TOP_N, &RecallFilter::default())
                .await?;
            let query_us = q.elapsed().as_micros();
            let ranked: Vec<Hit> = outcome
                .results
                .into_iter()
                .filter_map(|r| {
                    id_to_session.get(&r.memory.id).map(|s| Hit {
                        session_id: s.clone(),
                        score: r.score,
                        fts: r.channels.fts,
                        vector: r.channels.vector,
                        graph: r.channels.graph,
                    })
                })
                .collect();
            let abstained = outcome.abstained.map(|a| format!("{a:?}"));
            (ingest_ms, query_us, ranked, abstained)
        }
    };
    let ids: Vec<String> = ranked.iter().map(|h| h.session_id.clone()).collect();
    let metrics = score(&ids, &entry.answer_session_ids);
    let failure = classify(&metrics, abstained.is_some());
    Ok(QuestionRecord {
        question_id: entry.question_id.clone(),
        question_type: entry.question_type.clone(),
        gold: entry.answer_session_ids.clone(),
        corpus_size: docs.len(),
        ranked,
        abstained,
        metrics,
        failure,
        ingest_ms,
        query_us,
    })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Aggregate {
    pub n: usize,
    /// Mean of each per-question metric, keyed `recall_any@5`, `mrr`, ...
    pub means: std::collections::BTreeMap<String, f64>,
}

pub fn aggregate<'a>(records: impl IntoIterator<Item = &'a QuestionRecord>) -> Aggregate {
    let mut sums: std::collections::BTreeMap<String, f64> = Default::default();
    let mut n = 0usize;
    for r in records {
        n += 1;
        for (k, m) in &r.metrics.at {
            *sums.entry(format!("recall_any@{k}")).or_default() += m.recall_any;
            *sums.entry(format!("recall_all@{k}")).or_default() += m.recall_all;
            *sums.entry(format!("ndcg@{k}")).or_default() += m.ndcg;
            *sums.entry(format!("ndcg_mempalace@{k}")).or_default() += m.ndcg_mempalace;
        }
        *sums.entry("mrr".into()).or_default() += r.metrics.mrr;
    }
    let means = sums
        .into_iter()
        .map(|(k, v)| (k, if n == 0 { 0.0 } else { v / n as f64 }))
        .collect();
    Aggregate { n, means }
}
