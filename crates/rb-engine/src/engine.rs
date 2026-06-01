use crate::backend::MemoryBackend;
use crate::enrich::{default_summary, derive_keywords};
use crate::linker::{Linker, SimilarityLinker};
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
    linker: Box<dyn Linker>,
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
            linker: Box::new(SimilarityLinker::default()),
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

    fn in_namespace(&self, note: &MemoryNote) -> bool {
        note.namespace == self.namespace
    }

    fn active_in_namespace(&self, note: &MemoryNote) -> bool {
        self.in_namespace(note) && note.archived_at.is_none()
    }

    fn matches_recall_filters(
        note: &MemoryNote,
        type_filter: Option<MemoryType>,
        tags: &[String],
    ) -> bool {
        if let Some(ty) = type_filter {
            if note.memory_type != ty {
                return false;
            }
        }
        tags.iter().all(|t| note.tags.contains(t))
    }

    async fn get_scoped(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        Ok(self
            .backend
            .get(self.namespace.clone(), id)
            .await?
            .filter(|note| self.in_namespace(note)))
    }

    /// Store a new memory: heuristic-enrich, embed the content, then write.
    pub async fn remember(&self, input: RememberInput) -> rb_types::Result<MemoryId> {
        if !(1..=10).contains(&input.importance) {
            return Err(rb_types::Error::Storage(format!(
                "importance {} is out of range 1..=10",
                input.importance
            )));
        }

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
        // Keep a copy of the embedding for candidate search before the note moves.
        let embedding_for_links = embedding.clone();
        self.backend.write(note, embedding).await?;

        // Best-effort link generation: never fails the remember.
        if let Some(emb) = embedding_for_links {
            if let Err(e) = self.generate_links(&id, emb).await {
                tracing::warn!(error = %e, memory_id = %id, "link generation failed; continuing");
            }
        }
        Ok(id)
    }

    /// Vector-search for candidates similar to the just-written memory, fetch
    /// their notes, run the linker, and persist the produced links. Best-effort:
    /// callers ignore the error. `add_link` failures are logged and skipped so a
    /// single bad link never aborts the rest.
    async fn generate_links(&self, new_id: &MemoryId, embedding: Vec<f32>) -> rb_types::Result<()> {
        const CANDIDATE_LIMIT: usize = 8;
        let pairs = self
            .backend
            .vector(self.namespace.clone(), embedding, CANDIDATE_LIMIT)
            .await?;
        // Candidate ids exclude the new note itself.
        let candidate_ids: Vec<MemoryId> = pairs
            .iter()
            .filter(|(id, _)| id != new_id)
            .map(|(id, _)| id.clone())
            .collect();
        if candidate_ids.is_empty() {
            return Ok(());
        }
        let dist: std::collections::HashMap<MemoryId, f32> = pairs.into_iter().collect();
        let notes = self
            .backend
            .get_many(self.namespace.clone(), candidate_ids)
            .await?;
        let new_note = match self
            .backend
            .get(self.namespace.clone(), new_id.clone())
            .await?
        {
            Some(n) => n,
            None => return Ok(()),
        };
        let candidates: Vec<(MemoryNote, f32)> = notes
            .into_iter()
            .map(|n| {
                let d = dist.get(&n.id).copied().unwrap_or(f32::MAX);
                (n, d)
            })
            .collect();
        for link in self.linker.link(&new_note, &candidates) {
            if let Err(e) = self.backend.add_link(link).await {
                tracing::warn!(error = %e, "add_link failed; skipping one link");
            }
        }
        Ok(())
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

        // Bounded 1-hop graph expansion of the top active in-namespace keyword hit only.
        let mut graph_seed = None;
        for id in &keyword {
            if self
                .get_scoped(id.clone())
                .await?
                .as_ref()
                .is_some_and(|note| self.active_in_namespace(note))
            {
                graph_seed = Some(id.clone());
                break;
            }
        }
        let graph = match graph_seed {
            Some(top) => self.backend.graph(self.namespace.clone(), top, 1).await?,
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
            if let Some(note) = self.get_scoped(id.clone()).await? {
                if !self.active_in_namespace(&note)
                    || !Self::matches_recall_filters(&note, type_filter, tags)
                {
                    continue;
                }
                meta.insert(id.clone(), (note.importance, note.created_at));
                notes.insert(id.clone(), note);
            }
        }

        let filtered_keyword: Vec<MemoryId> = keyword
            .iter()
            .filter(|id| notes.contains_key(*id))
            .cloned()
            .collect();
        let filtered_vector: Vec<(MemoryId, f32)> = vector
            .iter()
            .filter(|(id, _)| notes.contains_key(id))
            .cloned()
            .collect();
        let filtered_graph: Vec<MemoryId> = graph
            .iter()
            .filter(|id| notes.contains_key(*id))
            .cloned()
            .collect();

        let signals =
            rb_search::build_signals(&filtered_keyword, &filtered_vector, &filtered_graph, &meta);
        let ranked = rb_search::rank(signals, self.weights, chrono::Utc::now(), candidate_limit);

        // Assemble results in ranked order, truncating to limit.
        let mut results: Vec<rb_types::SearchResult> = Vec::new();
        for (id, score) in ranked {
            let Some(note) = notes.get(&id) else {
                continue;
            };
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

    /// Fetch a single memory by id in the engine namespace.
    pub async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        self.get_scoped(id).await
    }

    /// List memories in the engine namespace, most-recent first, optionally
    /// filtered by a minimum importance.
    pub async fn list(
        &self,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        self.backend
            .list(self.namespace.clone(), min_importance, limit)
            .await
    }

    /// Expand the graph around `id` to `depth` hops and fetch the connected notes.
    pub async fn graph(&self, id: MemoryId, depth: u8) -> rb_types::Result<Vec<MemoryNote>> {
        let Some(anchor) = self.get_scoped(id.clone()).await? else {
            return Ok(Vec::new());
        };
        if !self.active_in_namespace(&anchor) {
            return Ok(Vec::new());
        }
        let ids = self
            .backend
            .graph(self.namespace.clone(), id, depth)
            .await?;
        let mut notes = Vec::with_capacity(ids.len());
        for nid in ids {
            if let Some(note) = self.get_scoped(nid).await? {
                if !self.active_in_namespace(&note) {
                    continue;
                }
                notes.push(note);
            }
        }
        Ok(notes)
    }

    /// Apply a partial update to an existing memory.
    pub async fn update(
        &self,
        id: MemoryId,
        updates: rb_types::MemoryUpdates,
    ) -> rb_types::Result<()> {
        if updates.content.is_some() {
            return Err(rb_types::Error::Storage(
                "content updates are not supported; create a new memory so embeddings stay consistent"
                    .to_string(),
            ));
        }
        if let Some(importance) = updates.importance {
            if !(1..=10).contains(&importance) {
                return Err(rb_types::Error::Storage(format!(
                    "importance {importance} is out of range 1..=10"
                )));
            }
        }
        if self.get_scoped(id.clone()).await?.is_none() {
            return Err(rb_types::Error::NotFound(id));
        }
        self.backend
            .update(self.namespace.clone(), id, updates)
            .await
    }

    /// Soft-delete (archive) a memory. Spec §12: delete == soft archive.
    pub async fn delete(&self, id: MemoryId) -> rb_types::Result<()> {
        if self.get_scoped(id.clone()).await?.is_none() {
            return Err(rb_types::Error::NotFound(id));
        }
        self.backend.archive(self.namespace.clone(), id).await
    }

    /// Project context payload: recent memories (by recency) plus important ones
    /// (importance >= 8), with a total count of the recent window.
    pub async fn context(&self) -> rb_types::Result<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        const CONTEXT_LIMIT: usize = 50;
        const IMPORTANT_FLOOR: u8 = 8;
        let recent = self
            .backend
            .list(self.namespace.clone(), None, CONTEXT_LIMIT)
            .await?;
        let important = self
            .backend
            .list(self.namespace.clone(), Some(IMPORTANT_FLOOR), CONTEXT_LIMIT)
            .await?;
        let total = recent.len();
        Ok((recent, important, total))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::test_support::MockBackend;
    use rb_embed::DeterministicProvider;
    use rb_types::{MemoryType, Namespace};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    struct CountingProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for CountingProvider {
        fn model_id(&self) -> &str {
            "counting"
        }

        fn dim(&self) -> usize {
            16
        }

        async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![vec![0.0; 16]; texts.len()])
        }
    }

    #[tokio::test]
    async fn invalid_importance_is_rejected_before_embedding() {
        let calls = Arc::new(AtomicUsize::new(0));
        let eng = MemoryEngine::new(
            MockBackend::default(),
            CountingProvider {
                calls: Arc::clone(&calls),
            },
            Namespace::Project("rb".into()),
        );

        let err = eng
            .remember(input("invalid importance should not embed", 0))
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("importance"),
            "unexpected error: {err}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(eng.backend().count(), 0);
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

    fn note(
        namespace: Namespace,
        content: &str,
        ty: MemoryType,
        importance: u8,
        tags: &[&str],
    ) -> MemoryNote {
        let mut note = MemoryNote::new(namespace, content.to_string(), ty, importance);
        note.tags = tags.iter().map(|t| t.to_string()).collect();
        note
    }

    #[tokio::test]
    async fn remember_creates_links_to_similar_existing_memories() {
        let eng = engine();
        // First memory: nothing to link to.
        let first = eng
            .remember(input("single writer over sqlite wal", 5))
            .await
            .unwrap();
        assert!(eng.backend().links_of(&first).is_empty());

        // Second memory: the deterministic mock vector() returns the first as a
        // candidate at distance 0.0 (<= threshold), so a link is created.
        let second = eng
            .remember(input("concurrent readers never block", 5))
            .await
            .unwrap();
        let links = eng.backend().links_of(&second);
        assert!(
            !links.is_empty(),
            "remember should link to the prior similar memory"
        );
        assert_eq!(links[0].source_id, second);
        assert!(
            links.iter().all(|l| l.target_id != second),
            "never links to self"
        );
        assert!(links.iter().any(|l| l.target_id == first));
        assert!(links
            .iter()
            .all(|l| l.link_type == rb_types::LinkType::References));
    }

    #[tokio::test]
    async fn remember_link_failure_does_not_fail_remember() {
        // A backend whose add_link always fails must not break remember.
        let eng = engine();
        let _first = eng.remember(input("anchor", 5)).await.unwrap();
        eng.backend().set_fail_add_link(true);
        // Should still succeed (best-effort linking).
        let id = eng.remember(input("second", 5)).await.unwrap();
        assert!(eng.backend().note_of(&id).is_some());
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
    async fn recall_filters_before_ranking_so_matching_candidates_fill_limit() {
        let eng = engine();

        let wrong_ids: Vec<MemoryId> = (0..12)
            .map(|i| {
                let ty = if i % 2 == 0 {
                    MemoryType::Insight
                } else {
                    MemoryType::BugFix
                };
                let note = note(
                    Namespace::Project("rb".into()),
                    &format!("wrong candidate {i}"),
                    ty,
                    10,
                    &["wrong"],
                );
                let id = note.id.clone();
                eng.backend().insert_note(note);
                id
            })
            .collect();
        let matching_ids: Vec<MemoryId> = (0..3)
            .map(|i| {
                let note = note(
                    Namespace::Project("rb".into()),
                    &format!("matching candidate {i}"),
                    MemoryType::BugFix,
                    1,
                    &["keep"],
                );
                let id = note.id.clone();
                eng.backend().insert_note(note);
                id
            })
            .collect();
        eng.backend().set_keyword_results(wrong_ids);
        eng.backend()
            .set_vector_results(matching_ids.iter().cloned().map(|id| (id, 2.0)).collect());

        let results = eng
            .recall(
                "candidate",
                3,
                Some(MemoryType::BugFix),
                &["keep".to_string()],
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| matching_ids.contains(&r.memory.id)));
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

    #[tokio::test]
    async fn get_returns_stored_note_or_none() {
        let eng = engine();
        let id = eng.remember(input("findable", 5)).await.unwrap();
        assert!(eng.get(id.clone()).await.unwrap().is_some());
        assert!(eng.get(rb_types::MemoryId::new()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_does_not_return_cross_namespace_note() {
        let eng = engine();
        let cross = note(
            Namespace::Project("other".into()),
            "foreign note",
            MemoryType::Insight,
            5,
            &[],
        );
        let cross_id = cross.id.clone();
        eng.backend().insert_note(cross);

        assert!(eng.get(cross_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_orders_by_recency_and_honors_min_importance() {
        let eng = engine();
        seed(&eng, "first", MemoryType::Insight, 3, &[]).await;
        seed(&eng, "second", MemoryType::Insight, 9, &[]).await;
        let all = eng.list(None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        // most recent first (second was inserted last).
        assert_eq!(all[0].content, "second");
        let important = eng.list(Some(8), 10).await.unwrap();
        assert_eq!(important.len(), 1);
        assert_eq!(important[0].importance, 9);
    }

    #[tokio::test]
    async fn update_mutates_then_get_reflects_change() {
        let eng = engine();
        let id = eng.remember(input("old body", 5)).await.unwrap();
        let updates = rb_types::MemoryUpdates {
            importance: Some(9),
            tags: Some(vec!["updated".to_string()]),
            ..Default::default()
        };
        eng.update(id.clone(), updates).await.unwrap();
        let note = eng.get(id).await.unwrap().unwrap();
        assert_eq!(note.content, "old body");
        assert_eq!(note.importance, 9);
        assert_eq!(note.tags, vec!["updated".to_string()]);
    }

    #[tokio::test]
    async fn update_rejects_content_edits_to_keep_embeddings_consistent() {
        let eng = engine();
        let id = eng.remember(input("old body", 5)).await.unwrap();
        let err = eng
            .update(
                id.clone(),
                rb_types::MemoryUpdates {
                    content: Some("new body".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("content updates"));
        let note = eng.get(id).await.unwrap().unwrap();
        assert_eq!(note.content, "old body");
    }

    #[tokio::test]
    async fn update_rejects_out_of_range_importance() {
        let eng = engine();
        let id = eng.remember(input("valid", 5)).await.unwrap();
        for bad in [0, 11] {
            let err = eng
                .update(
                    id.clone(),
                    rb_types::MemoryUpdates {
                        importance: Some(bad),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();
            assert!(err.to_string().contains("importance"), "got {err}");
        }
    }

    #[tokio::test]
    async fn update_does_not_mutate_cross_namespace_note() {
        let eng = engine();
        let cross = note(
            Namespace::Project("other".into()),
            "foreign body",
            MemoryType::Insight,
            5,
            &[],
        );
        let cross_id = cross.id.clone();
        eng.backend().insert_note(cross);

        let _ = eng
            .update(
                cross_id.clone(),
                rb_types::MemoryUpdates {
                    importance: Some(9),
                    ..Default::default()
                },
            )
            .await;

        let note = eng.backend().note_of(&cross_id).unwrap();
        assert_eq!(note.content, "foreign body");
    }

    #[tokio::test]
    async fn delete_soft_archives_the_note() {
        let eng = engine();
        let id = eng.remember(input("doomed", 5)).await.unwrap();
        eng.delete(id.clone()).await.unwrap();
        let note = eng.get(id).await.unwrap().unwrap();
        assert!(note.archived_at.is_some());
    }

    #[tokio::test]
    async fn delete_does_not_archive_cross_namespace_note() {
        let eng = engine();
        let cross = note(
            Namespace::Project("other".into()),
            "foreign body",
            MemoryType::Insight,
            5,
            &[],
        );
        let cross_id = cross.id.clone();
        eng.backend().insert_note(cross);

        let _ = eng.delete(cross_id.clone()).await;

        let note = eng.backend().note_of(&cross_id).unwrap();
        assert!(note.archived_at.is_none());
    }

    #[tokio::test]
    async fn graph_returns_connected_notes() {
        let eng = engine();
        let id = eng.remember(input("anchor", 5)).await.unwrap();
        let active = note(
            Namespace::Project("rb".into()),
            "same namespace active",
            MemoryType::Insight,
            5,
            &[],
        );
        let cross = note(
            Namespace::Project("other".into()),
            "foreign neighbor",
            MemoryType::Insight,
            5,
            &[],
        );
        let mut archived = note(
            Namespace::Project("rb".into()),
            "archived neighbor",
            MemoryType::Insight,
            5,
            &[],
        );
        archived.archived_at = Some(chrono::Utc::now());
        let active_id = active.id.clone();
        let cross_id = cross.id.clone();
        let archived_id = archived.id.clone();
        eng.backend().insert_note(active);
        eng.backend().insert_note(cross);
        eng.backend().insert_note(archived);
        eng.backend().set_graph_neighbors(
            id.clone(),
            vec![cross_id.clone(), archived_id.clone(), active_id.clone()],
        );

        let neighbors = eng.graph(id, 2).await.unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].id, active_id);

        eng.backend()
            .set_graph_neighbors(cross_id.clone(), vec![active_id]);
        let cross_anchor_neighbors = eng.graph(cross_id, 2).await.unwrap();
        assert!(cross_anchor_neighbors.is_empty());
    }

    #[tokio::test]
    async fn recall_does_not_leak_cross_namespace_or_archived_graph_neighbors() {
        let eng = engine();
        let anchor = seed(&eng, "anchor topic", MemoryType::Insight, 5, &[]).await;
        let active = note(
            Namespace::Project("rb".into()),
            "same namespace active graph result",
            MemoryType::Insight,
            5,
            &[],
        );
        let cross = note(
            Namespace::Project("other".into()),
            "foreign graph result",
            MemoryType::Insight,
            5,
            &[],
        );
        let mut archived = note(
            Namespace::Project("rb".into()),
            "archived graph result",
            MemoryType::Insight,
            5,
            &[],
        );
        archived.archived_at = Some(chrono::Utc::now());
        let active_id = active.id.clone();
        let cross_id = cross.id.clone();
        let archived_id = archived.id.clone();
        eng.backend().insert_note(active);
        eng.backend().insert_note(cross);
        eng.backend().insert_note(archived);
        eng.backend().set_keyword_results(vec![anchor.clone()]);
        eng.backend().set_vector_results(Vec::new());
        eng.backend().set_graph_neighbors(
            anchor,
            vec![cross_id.clone(), archived_id.clone(), active_id],
        );

        let results = eng.recall("topic", 10, None, &[]).await.unwrap();

        assert!(results.iter().all(|r| {
            r.memory.namespace == Namespace::Project("rb".into()) && r.memory.archived_at.is_none()
        }));
        assert!(!results.iter().any(|r| r.memory.id == cross_id));
        assert!(!results.iter().any(|r| r.memory.id == archived_id));
    }

    #[tokio::test]
    async fn context_splits_recent_and_important() {
        let eng = engine();
        seed(&eng, "low importance recent", MemoryType::Insight, 2, &[]).await;
        seed(&eng, "high importance note", MemoryType::Insight, 9, &[]).await;
        let (recent, important, total) = eng.context().await.unwrap();
        // recent includes both; important only the >= 8 one.
        assert_eq!(recent.len(), 2);
        assert_eq!(important.len(), 1);
        assert_eq!(important[0].importance, 9);
        assert_eq!(total, 2);
    }
}
