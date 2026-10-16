//! Per-question retrieval shared by every external adapter (Vikunja #60):
//! a fresh in-memory store, one of two profiles, and source ids that
//! round-trip through an adapter-side map (never stored text).

use crate::backend::SqliteBackend;
use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_engine::{MemoryBackend, MemoryEngine, Provenance, RememberInput};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, RecallFilter};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// MemPalace raw-mode ingestion, ranked by rusty-brain's vector index only.
    Parity,
    /// Standard `remember` -> `recall` with heuristic enrichment and fusion.
    Native,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub session_id: String,
    /// Parity: cosine distance (lower is better). Native: fused recall score.
    pub score: f32,
    pub fts: bool,
    pub vector: bool,
    pub graph: bool,
    /// Parity records cosine similarity; native records pre-prior Linear scores.
    pub channel_scores: Option<rb_types::ChannelScores>,
}

pub struct Retrieval {
    pub ranked: Vec<Hit>,
    pub abstained: Option<String>,
    pub ingest_ms: u128,
    pub query_us: u128,
    pub logical_db_bytes: u64,
}

/// Metric outcome, not a causal ranking diagnosis. Empty gold is not a hit.
pub fn classify_failure(gold: usize, found: usize, abstained: bool) -> &'static str {
    if gold == 0 {
        "unscorable_empty_gold"
    } else if abstained {
        "abstained"
    } else if found == gold {
        "hit"
    } else if found > 0 {
        "partial_hit"
    } else {
        "miss"
    }
}

fn validate_vectors(expected: usize, dim: usize, vectors: &[Vec<f32>]) -> rb_types::Result<()> {
    if vectors.len() != expected
        || vectors
            .iter()
            .any(|v| v.len() != dim || v.iter().any(|x| !x.is_finite()))
    {
        return Err(rb_types::Error::Embedding(
            "external benchmark requires complete finite vectors of the declared dimension".into(),
        ));
    }
    Ok(())
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
        let vectors = self.0.embed(texts, kind).await?;
        validate_vectors(texts.len(), self.dim(), &vectors)?;
        Ok(vectors)
    }
}

fn namespace() -> Namespace {
    Namespace::Project("external-bench".into())
}

fn fixed_now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("fixed benchmark timestamp is valid RFC 3339")
        .to_utc()
}

/// Memoizes vectors by (kind, text) so per-question fresh stores over a
/// shared haystack embed each distinct document once.
pub struct Memo<P> {
    inner: P,
    cache: Mutex<HashMap<(EmbedKind, String), Vec<f32>>>,
}

impl<P> Memo<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl<P: EmbeddingProvider> EmbeddingProvider for Memo<P> {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        let missing: Vec<String> = {
            let cache = self.cache.lock().map_err(|_| poisoned())?;
            let mut seen = std::collections::HashSet::new();
            texts
                .iter()
                .filter(|t| !cache.contains_key(&(kind, (*t).clone())) && seen.insert(*t))
                .cloned()
                .collect()
        };
        if !missing.is_empty() {
            let vectors = self.inner.embed(&missing, kind).await?;
            validate_vectors(missing.len(), self.dim(), &vectors)?;
            let mut cache = self.cache.lock().map_err(|_| poisoned())?;
            for (text, vector) in missing.into_iter().zip(vectors) {
                cache.insert((kind, text), vector);
            }
        }
        let cache = self.cache.lock().map_err(|_| poisoned())?;
        texts
            .iter()
            .map(|t| cache.get(&(kind, t.clone())).cloned().ok_or_else(poisoned))
            .collect()
    }
}

fn poisoned() -> rb_types::Error {
    rb_types::Error::Embedding("memo cache unavailable".into())
}

pub async fn retrieve<P: EmbeddingProvider>(
    provider: &Arc<P>,
    docs: &[(String, String)],
    query: &str,
    profile: Profile,
    top_n: usize,
) -> rb_types::Result<Retrieval> {
    let query_owned = query.to_string();
    let backend = SqliteBackend::in_memory(provider.dim())?;
    let started = Instant::now();
    let mut id_to_source: HashMap<MemoryId, String> = HashMap::new();
    let (ingest_ms, query_us, ranked, abstained, logical_db_bytes) = match profile {
        Profile::Parity => {
            let texts: Vec<String> = docs.iter().map(|(_, d)| d.clone()).collect();
            let vectors = provider.embed(&texts, EmbedKind::Document).await?;
            validate_vectors(texts.len(), provider.dim(), &vectors)?;
            for ((source, doc), vector) in docs.iter().zip(vectors) {
                let mut note = MemoryNote::new(namespace(), doc.clone(), MemoryType::Insight, 5);
                note.embedding_model = provider.model_id().to_string();
                id_to_source.insert(note.id.clone(), source.clone());
                backend.write(note, Some(vector)).await?;
            }
            let ingest_ms = started.elapsed().as_millis();
            let q = Instant::now();
            let mut queries = provider
                .embed(std::slice::from_ref(&query_owned), EmbedKind::Query)
                .await?;
            validate_vectors(1, provider.dim(), &queries)?;
            let query = queries.pop().ok_or_else(|| {
                rb_types::Error::Embedding("missing benchmark query vector".into())
            })?;
            let hits = backend
                .vector(namespace(), query, top_n, rb_types::RecallFilter::default())
                .await?;
            let query_us = q.elapsed().as_micros();
            let ranked: Vec<Hit> = hits
                .into_iter()
                .filter_map(|(id, distance)| {
                    id_to_source.get(&id).map(|s| Hit {
                        session_id: s.clone(),
                        score: distance,
                        fts: false,
                        vector: true,
                        graph: false,
                        channel_scores: Some(rb_types::ChannelScores {
                            vector: Some((1.0 - distance).clamp(0.0, 1.0)),
                            ..Default::default()
                        }),
                    })
                })
                .collect();
            (
                ingest_ms,
                query_us,
                ranked,
                None,
                backend.logical_size_bytes()?,
            )
        }
        Profile::Native => {
            let engine = MemoryEngine::new(backend, Shared(provider.clone()), namespace())
                .with_fixed_now(fixed_now());
            for (source, doc) in docs {
                let id = engine
                    .remember(RememberInput {
                        legacy_unknown: false,
                        content: doc.clone(),
                        context: None,
                        memory_type: MemoryType::Insight,
                        importance: 5,
                        keywords: Vec::new(),
                        tags: Vec::new(),
                        related_files: Vec::new(),
                        human_curated: false,
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
                id_to_source.insert(id, source.clone());
            }
            let ingest_ms = started.elapsed().as_millis();
            let q = Instant::now();
            let outcome = engine
                .recall_with_status(&query_owned, top_n, &RecallFilter::default())
                .await?;
            if outcome.degraded {
                return Err(rb_types::Error::Embedding(
                    "native benchmark recall degraded; no result artifact is admissible".into(),
                ));
            }
            let query_us = q.elapsed().as_micros();
            let ranked: Vec<Hit> = outcome
                .results
                .into_iter()
                .filter_map(|r| {
                    id_to_source.get(&r.memory.id).map(|s| Hit {
                        session_id: s.clone(),
                        score: r.score,
                        fts: r.channels.fts,
                        vector: r.channels.vector,
                        graph: r.channels.graph,
                        channel_scores: r.channel_scores,
                    })
                })
                .collect();
            let abstained = outcome.abstained.map(|a| format!("{a:?}"));
            (
                ingest_ms,
                query_us,
                ranked,
                abstained,
                engine.backend().logical_size_bytes()?,
            )
        }
    };
    Ok(Retrieval {
        ranked,
        abstained,
        ingest_ms,
        query_us,
        logical_db_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::{classify_failure, validate_vectors};

    #[test]
    fn vector_audit_rejects_missing_malformed_or_nonfinite_vectors() {
        assert!(validate_vectors(1, 2, &[vec![0.0, 1.0]]).is_ok());
        assert!(validate_vectors(1, 2, &[]).is_err());
        assert!(validate_vectors(1, 2, &[vec![0.0]]).is_err());
        assert!(validate_vectors(1, 2, &[vec![f32::NAN, 0.0]]).is_err());
    }

    #[test]
    fn failure_classes_distinguish_empty_partial_and_abstention() {
        assert_eq!(classify_failure(0, 0, false), "unscorable_empty_gold");
        assert_eq!(classify_failure(2, 0, true), "abstained");
        assert_eq!(classify_failure(2, 0, false), "miss");
        assert_eq!(classify_failure(2, 1, false), "partial_hit");
        assert_eq!(classify_failure(2, 2, false), "hit");
    }
}
