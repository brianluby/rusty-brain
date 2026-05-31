use crate::backend::MemoryBackend;
use crate::enrich::{default_summary, derive_keywords};
use rb_embed::EmbeddingProvider;
use rb_search::Weights;
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

/// Input to `remember`. Mirrors the proto `Request::Remember` payload.
pub struct RememberInput {
    pub content: String,
    pub context: Option<String>,
    pub memory_type: MemoryType,
    pub importance: u8,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub related_files: Vec<String>,
}

/// Policy layer: orchestrates heuristic enrichment + embedding + ranking over a
/// `MemoryBackend`. Generic so it is unit-tested without a DB or network.
pub struct MemoryEngine<B: MemoryBackend, P: EmbeddingProvider> {
    backend: B,
    embedder: P,
    weights: Weights,
    namespace: Namespace,
}

impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P> {
    /// Construct an engine bound to a single namespace (set server-side from the
    /// client handshake; clients cannot widen it).
    pub fn new(backend: B, embedder: P, namespace: Namespace) -> Self {
        Self {
            backend,
            embedder,
            weights: Weights::default(),
            namespace,
        }
    }

    /// Borrow the backend (used by daemon/tests for introspection).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Borrow the embedding provider.
    pub fn embedder(&self) -> &P {
        &self.embedder
    }

    /// Borrow the ranking weights (used by `recall`).
    pub fn weights(&self) -> Weights {
        self.weights
    }

    /// Store a new memory: heuristic-enrich, embed the content, then write.
    pub async fn remember(&self, input: RememberInput) -> rb_types::Result<MemoryId> {
        let mut note = MemoryNote::new(
            self.namespace.clone(),
            input.content,
            input.memory_type,
            input.importance,
        );
        // Heuristic enrichment (no LLM in P1).
        note.summary = default_summary(&note.content);
        note.keywords = if input.keywords.is_empty() {
            derive_keywords(&note.content)
        } else {
            input.keywords
        };
        note.tags = input.tags;
        note.related_files = input.related_files;
        if let Some(ctx) = input.context {
            note.context = ctx;
        }
        note.embedding_model = self.embedder.model_id().to_string();

        // Embed the content (single text in, single vector out).
        let mut embeddings = self.embedder.embed(&[note.content.clone()]).await?;
        let embedding = embeddings.pop();

        let id = note.id.clone();
        self.backend.write(note, embedding).await?;
        Ok(id)
    }

    /// Hybrid recall: embed the query, gather keyword + vector (+ 1-hop graph)
    /// candidates scoped to the engine namespace, rank with `rb_search`, then
    /// return ranked `SearchResult`s after applying type/tag filters.
    pub async fn recall(
        &self,
        query: &str,
        limit: usize,
        type_filter: Option<MemoryType>,
        tags: &[String],
    ) -> rb_types::Result<Vec<rb_types::SearchResult>> {
        use std::collections::HashMap;

        // Over-fetch candidates so post-filtering still has enough to fill `limit`.
        let candidate_limit = limit.saturating_mul(4).max(limit);

        let mut query_emb = self.embedder.embed(&[query.to_string()]).await?;
        let embedding = query_emb.pop().unwrap_or_default();

        let keyword = self
            .backend
            .keyword(self.namespace.clone(), query.to_string(), candidate_limit)
            .await?;
        let vector = self
            .backend
            .vector(self.namespace.clone(), embedding, candidate_limit)
            .await?;

        // Bounded 1-hop graph expansion of the top keyword hit only.
        let graph = match keyword.first() {
            Some(top) => self.backend.graph(top.clone(), 1).await?,
            None => Vec::new(),
        };

        // Collect the unique candidate id set across all three sources.
        let mut order: Vec<MemoryId> = Vec::new();
        let mut seen: std::collections::HashSet<MemoryId> = std::collections::HashSet::new();
        for id in keyword
            .iter()
            .chain(vector.iter().map(|(id, _)| id))
            .chain(graph.iter())
        {
            if seen.insert(id.clone()) {
                order.push(id.clone());
            }
        }

        // Fetch each candidate once; build the note cache + the rank meta map.
        let mut notes: HashMap<MemoryId, MemoryNote> = HashMap::new();
        let mut meta: HashMap<MemoryId, (u8, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for id in &order {
            if let Some(note) = self.backend.get(id.clone()).await? {
                meta.insert(id.clone(), (note.importance, note.created_at));
                notes.insert(id.clone(), note);
            }
        }

        let signals = rb_search::build_signals(&keyword, &vector, &graph, &meta);
        let ranked = rb_search::rank(signals, self.weights, chrono::Utc::now(), candidate_limit);

        // Assemble results in ranked order, applying filters, truncating to limit.
        let mut results: Vec<rb_types::SearchResult> = Vec::new();
        for (id, score) in ranked {
            let Some(note) = notes.get(&id) else {
                continue;
            };
            if let Some(ty) = type_filter {
                if note.memory_type != ty {
                    continue;
                }
            }
            if !tags.iter().all(|t| note.tags.contains(t)) {
                continue;
            }
            results.push(rb_types::SearchResult {
                memory: note.clone(),
                score,
            });
            if results.len() == limit {
                break;
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::test_support::MockBackend;
    use rb_embed::DeterministicProvider;
    use rb_types::{MemoryType, Namespace};

    fn engine() -> MemoryEngine<MockBackend, DeterministicProvider> {
        MemoryEngine::new(
            MockBackend::default(),
            DeterministicProvider::new(16),
            Namespace::Project("rb".into()),
        )
    }

    fn input(content: &str, importance: u8) -> RememberInput {
        RememberInput {
            content: content.to_string(),
            context: None,
            memory_type: MemoryType::Insight,
            importance,
            keywords: Vec::new(),
            tags: Vec::new(),
            related_files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn remember_stores_note_and_embedding() {
        let eng = engine();
        let id = eng
            .remember(input("single writer over sqlite wal", 7))
            .await
            .unwrap();
        // exactly one note written, with an embedding of provider dim.
        assert_eq!(eng.backend().count(), 1);
        let emb = eng.backend().embedding_of(&id).unwrap();
        assert_eq!(emb.len(), 16);
    }

    #[tokio::test]
    async fn remember_applies_heuristic_summary_and_keywords() {
        let eng = engine();
        let content = "concurrent readers never block the single dedicated writer thread";
        let id = eng.remember(input(content, 6)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // summary defaults to the (trimmed) content since it's < 150 chars.
        assert_eq!(note.summary, content);
        // keywords derived (empty input) — non-empty, lowercased, capped at 5.
        assert!(!note.keywords.is_empty());
        assert!(note.keywords.len() <= 5);
        assert!(note.keywords.iter().all(|k| k == &k.to_lowercase()));
    }

    #[tokio::test]
    async fn remember_preserves_explicit_keywords_and_namespace() {
        let eng = engine();
        let mut inp = input("body text here", 5);
        inp.keywords = vec!["explicit".to_string()];
        inp.tags = vec!["t1".to_string()];
        inp.context = Some("ctx".to_string());
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.keywords, vec!["explicit".to_string()]);
        assert_eq!(note.tags, vec!["t1".to_string()]);
        assert_eq!(note.context, "ctx");
        // engine enforces its own namespace.
        assert_eq!(note.namespace, Namespace::Project("rb".into()));
    }

    #[tokio::test]
    async fn remember_sets_embedding_model_from_provider() {
        let eng = engine();
        let id = eng.remember(input("model id check", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.embedding_model, eng.embedder().model_id());
    }

    #[tokio::test]
    async fn remember_is_deterministic_same_content_same_embedding() {
        let eng = engine();
        let id1 = eng.remember(input("identical content", 5)).await.unwrap();
        let id2 = eng.remember(input("identical content", 5)).await.unwrap();
        assert_ne!(id1, id2); // distinct notes
        assert_eq!(
            eng.backend().embedding_of(&id1),
            eng.backend().embedding_of(&id2)
        ); // deterministic provider => same vector
    }

    async fn seed(
        eng: &MemoryEngine<MockBackend, DeterministicProvider>,
        content: &str,
        ty: MemoryType,
        imp: u8,
        tags: &[&str],
    ) -> rb_types::MemoryId {
        let mut inp = input(content, imp);
        inp.memory_type = ty;
        inp.tags = tags.iter().map(|t| t.to_string()).collect();
        eng.remember(inp).await.unwrap()
    }

    #[tokio::test]
    async fn recall_returns_results_for_seeded_memories() {
        let eng = engine();
        seed(
            &eng,
            "alpha topic about sqlite",
            MemoryType::Insight,
            5,
            &[],
        )
        .await;
        seed(&eng, "beta topic about tokio", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("topic", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        // scores are finite and sorted descending.
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn recall_respects_limit() {
        let eng = engine();
        for i in 0..5 {
            seed(
                &eng,
                &format!("doc number {i}"),
                MemoryType::Insight,
                5,
                &[],
            )
            .await;
        }
        let results = eng.recall("doc", 2, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn recall_type_filter_excludes_other_types() {
        let eng = engine();
        seed(&eng, "a bug fix note", MemoryType::BugFix, 5, &[]).await;
        seed(&eng, "an insight note", MemoryType::Insight, 5, &[]).await;
        let results = eng
            .recall("note", 10, Some(MemoryType::BugFix), &[])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.memory_type, MemoryType::BugFix);
    }

    #[tokio::test]
    async fn recall_tag_filter_requires_all_tags() {
        let eng = engine();
        seed(&eng, "tagged one", MemoryType::Insight, 5, &["x", "y"]).await;
        seed(&eng, "tagged two", MemoryType::Insight, 5, &["x"]).await;
        let results = eng
            .recall("tagged", 10, None, &["x".to_string(), "y".to_string()])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].memory.tags.contains(&"y".to_string()));
    }

    #[tokio::test]
    async fn recall_ranks_all_candidates_with_finite_descending_scores() {
        // NOTE: importance does NOT decide ordering between near-identical
        // candidates (keyword-rank position dominates, see RANKING NOTE), so we
        // assert the honest invariants: every candidate is returned, scores are
        // finite, and the result is sorted descending. Importance-driven order
        // is covered by the deterministic `list` test in Task 20.
        let eng = engine();
        let _low = seed(&eng, "ranking probe content", MemoryType::Insight, 2, &[]).await;
        let _high = seed(&eng, "ranking probe content", MemoryType::Insight, 9, &[]).await;
        let results = eng.recall("ranking probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn recall_empty_store_returns_empty() {
        let eng = engine();
        let results = eng.recall("anything", 10, None, &[]).await.unwrap();
        assert!(results.is_empty());
    }
}
