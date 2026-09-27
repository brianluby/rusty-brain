//! LongMemEval adapter (Vikunja #60).
//!
//! Every question gets a brand-new in-memory store holding only that
//! question's haystack, mirroring MemPalace's per-question collection. Source
//! session ids never enter stored content, tags or keywords: LongMemEval names
//! gold sessions `answer_*`, so any stored id would leak the label into FTS
//! and embeddings. Ids round-trip through the adapter's `MemoryId -> session`
//! map instead. The per-turn `has_answer` flag is never deserialized.

use super::core::retrieve;
pub use super::core::{Hit, Profile, Shared};
use super::metrics::{score, QuestionMetrics};
use rb_embed::EmbeddingProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

pub async fn run_question<P: EmbeddingProvider>(
    provider: &Arc<P>,
    entry: &Entry,
    profile: Profile,
) -> rb_types::Result<QuestionRecord> {
    let docs = session_documents(entry);
    let r = retrieve(provider, &docs, &entry.question, profile, TOP_N).await?;
    let (ranked, abstained, ingest_ms, query_us) = (r.ranked, r.abstained, r.ingest_ms, r.query_us);
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
