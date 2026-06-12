use crate::backend::MemoryBackend;
use crate::enrich::{default_summary, derive_keywords};
use crate::enricher::Enricher;
use crate::linker::{Linker, SimilarityLinker};
use rb_embed::EmbeddingProvider;
use rb_search::{FusionMode, RrfConfig, Weights};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};
use std::sync::Arc;

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
    fusion_mode: FusionMode,
    rrf_config: RrfConfig,
    namespace: Namespace,
    linker: Box<dyn Linker>,
    enricher: Option<Arc<dyn Enricher>>,
}

impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P> {
    /// Construct an engine bound to a single namespace (set server-side from the
    /// client handshake; clients cannot widen it).
    pub fn new(backend: B, embedder: P, namespace: Namespace) -> Self {
        Self {
            backend,
            embedder,
            weights: Weights::default(),
            // Default `Linear` preserves current recall behavior byte-for-byte;
            // `Rrf` is opt-in via `with_fusion_mode` (spec §7, eval-gated flip).
            fusion_mode: FusionMode::default(),
            rrf_config: RrfConfig::default(),
            namespace,
            linker: Box::new(SimilarityLinker::default()),
            enricher: None,
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

    /// The fusion mode `recall` dispatches on (`Linear` default, `Rrf` opt-in).
    pub fn fusion_mode(&self) -> FusionMode {
        self.fusion_mode
    }

    /// Select the fusion strategy for `recall`. `Linear` (default) is the
    /// existing weighted-sum ranking; `Rrf` is the two-stage hybrid (spec §7).
    /// Opt-in only — the default stays `Linear` so behavior is unchanged unless
    /// a caller flips it through the config plumbing.
    pub fn with_fusion_mode(mut self, mode: FusionMode) -> Self {
        self.fusion_mode = mode;
        self
    }

    /// Override the `Rrf` tuning (k + prior weights). No effect under `Linear`.
    pub fn with_rrf_config(mut self, config: RrfConfig) -> Self {
        self.rrf_config = config;
        self
    }

    /// Enable opt-in enrichment. When set, `remember` asks the enricher to fill
    /// fields the caller left empty; on enricher error it falls back to the
    /// heuristic path (enrichment never fails a remember).
    pub fn with_enricher(mut self, enricher: Arc<dyn Enricher>) -> Self {
        self.enricher = Some(enricher);
        self
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
        rb_types::validate_importance(input.importance)?;

        let mut note = MemoryNote::new(
            self.namespace.clone(),
            input.content,
            input.memory_type,
            input.importance,
        );
        // Enrichment: opt-in LLM, else heuristic. The enricher only fills fields
        // the caller left empty; an enricher error degrades to the heuristic.
        let enrichment = match &self.enricher {
            Some(e) => match e.enrich(&note.content, input.context.as_deref()).await {
                Ok(en) => Some(en),
                Err(err) => {
                    tracing::warn!(error = %err, "enricher failed; using heuristic enrichment");
                    None
                }
            },
            None => None,
        };

        note.summary = match enrichment.as_ref().and_then(|e| e.summary.clone()) {
            Some(s) => s,
            None => default_summary(&note.content),
        };
        note.keywords = if !input.keywords.is_empty() {
            input.keywords
        } else if let Some(en) = enrichment.as_ref().filter(|e| !e.keywords.is_empty()) {
            en.keywords.clone()
        } else {
            derive_keywords(&note.content)
        };
        note.tags = if !input.tags.is_empty() {
            input.tags
        } else {
            enrichment
                .as_ref()
                .map(|e| e.tags.clone())
                .unwrap_or_default()
        };
        note.related_files = input.related_files;
        if let Some(ctx) = input.context {
            note.context = ctx;
        }
        note.embedding_model = self.embedder.model_id().to_string();
        // Stamp the composition version alongside the model so the `reembed`
        // batch can detect rows built from an older representation (Feature A).
        note.embedding_input_version = crate::embed_input::EMBEDDING_INPUT_VERSION.to_string();

        // Embed the COMPOSITE document representation (content + keywords + tags
        // + context), not raw content (spec §8). The query stays embedded raw.
        let input = crate::embed_input::embedding_input(&note);
        let mut embeddings = self.embedder.embed(&[input]).await?;
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

        // ONE batch fetch (fixes the N+1). get_many is ns-scoped and order-preserving.
        let fetched = self
            .backend
            .get_many(self.namespace.clone(), order.clone())
            .await?;
        let mut notes: HashMap<MemoryId, MemoryNote> = HashMap::new();
        let mut meta: HashMap<MemoryId, (u8, f32, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for note in fetched {
            if !self.active_in_namespace(&note)
                || !Self::matches_recall_filters(&note, type_filter, tags)
            {
                continue;
            }
            // Carry confidence into ranking (Feature C): low-confidence memories
            // are dampened post-score so a wrong, high-matching note cannot
            // dominate recall (the context-poisoning mitigation).
            meta.insert(
                note.id.clone(),
                (note.importance, note.confidence, note.created_at),
            );
            notes.insert(note.id.clone(), note);
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
        // Dispatch on the configured fusion mode. `Linear` is the default and
        // preserves prior behavior byte-for-byte; `Rrf` is the opt-in two-stage
        // hybrid (spec §7).
        let now = chrono::Utc::now();
        let ranked = match self.fusion_mode {
            FusionMode::Linear => rb_search::rank(signals, self.weights, now, candidate_limit),
            FusionMode::Rrf => rb_search::rank_rrf(signals, self.rrf_config, now, candidate_limit),
        };

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

        let returned_ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();

        // Contradiction surfacing (Feature C): for the returned (post-ranking,
        // truncated) set only, batch-load active contradicts links and flag each
        // contested memory. FAIL-OPEN: a lookup error leaves results unflagged
        // rather than failing recall.
        let contested = self.contested_set(&returned_ids).await;
        for r in &mut results {
            r.memory.contested = contested.contains(&r.memory.id);
        }

        // Best-effort batch access tracking: single writer round-trip for all results.
        if !returned_ids.is_empty() {
            if let Err(e) = self.backend.record_accesses(returned_ids).await {
                tracing::debug!(error = %e, "record_accesses failed; ignoring");
            }
        }
        Ok(results)
    }

    /// Batch-load the subset of `ids` that have an active `contradicts` link
    /// (Feature C). FAIL-OPEN: on a lookup error this returns an empty set (so
    /// results surface unflagged), logging at debug — surfacing contested state
    /// is best-effort enrichment, never a gate on retrieval.
    async fn contested_set(&self, ids: &[MemoryId]) -> std::collections::HashSet<MemoryId> {
        if ids.is_empty() {
            return std::collections::HashSet::new();
        }
        match self
            .backend
            .active_contradicts(self.namespace.clone(), ids.to_vec())
            .await
        {
            Ok(set) => set,
            Err(e) => {
                tracing::debug!(error = %e, "contradiction lookup failed; returning unflagged");
                std::collections::HashSet::new()
            }
        }
    }

    /// Annotate each note's `contested` flag from one batched contradiction
    /// lookup over the slice (fail-open). Mirrors `recall`'s post-ranking step for
    /// the `get`/`list`/`context` surfaces.
    async fn annotate_contested(&self, notes: &mut [MemoryNote]) {
        if notes.is_empty() {
            return;
        }
        let ids: Vec<MemoryId> = notes.iter().map(|n| n.id.clone()).collect();
        let contested = self.contested_set(&ids).await;
        for n in notes.iter_mut() {
            n.contested = contested.contains(&n.id);
        }
    }

    /// Re-embed up to `limit` ACTIVE memories whose stored
    /// `(embedding_model, embedding_input_version)` stamp is stale relative to
    /// the current embedder + composition version (Feature A, spec §8).
    ///
    /// Cross-namespace maintenance (like the evolution jobs): the engine's bound
    /// namespace does NOT restrict the scan, so a single `reembed` converges the
    /// whole corpus. For each candidate it recomputes the composite
    /// [`crate::embed_input::embedding_input`], embeds it, and replaces the
    /// vector + stamp through the single writer.
    ///
    /// Bounded and idempotent: candidates are exactly the rows whose stamp
    /// differs from current, so a row already at `(model, version)` is never
    /// scanned, and a second run over unchanged data writes nothing (returns
    /// `changed == 0`). Fail-safe per row: an embedding or write failure is
    /// logged and counted as `skipped` (retried on the next run), never fatal.
    /// Returns `(scanned, changed, skipped)`.
    ///
    /// Scope: `reembed` converges *vectors* only. Similarity-derived graph links
    /// created at `remember` time are NOT regenerated here, so the graph leg of
    /// `recall` may still reflect pre-reembed neighborhoods until links are rebuilt.
    /// Link evolution on re-embed (A-MEM's "evolve-and-re-embed neighbors") is
    /// deferred to P6; the graph term is a minor 0.10-weight signal and links decay
    /// independently, so vector convergence is sufficient for P5's transition.
    pub async fn reembed(&self, limit: usize) -> rb_types::Result<(u64, u64, u64)> {
        let model = self.embedder.model_id().to_string();
        let input_version = crate::embed_input::EMBEDDING_INPUT_VERSION.to_string();

        let candidates = self
            .backend
            .memories_for_reembed(model.clone(), input_version.clone(), limit)
            .await?;

        let mut scanned: u64 = 0;
        let mut changed: u64 = 0;
        let mut skipped: u64 = 0;

        for note in candidates {
            scanned += 1;
            let input = crate::embed_input::embedding_input(&note);
            let embedding = match self.embedder.embed(&[input]).await {
                Ok(mut v) => match v.pop() {
                    Some(emb) => emb,
                    None => {
                        tracing::warn!(memory_id = %note.id, "reembed: embedder returned no vector; skipping");
                        skipped += 1;
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, memory_id = %note.id, "reembed: embed failed; will retry next run");
                    skipped += 1;
                    continue;
                }
            };
            match self
                .backend
                .update_vector(
                    note.id.clone(),
                    embedding,
                    model.clone(),
                    input_version.clone(),
                )
                .await
            {
                Ok(()) => changed += 1,
                Err(e) => {
                    tracing::warn!(error = %e, memory_id = %note.id, "reembed: vector update failed; will retry next run");
                    skipped += 1;
                }
            }
        }

        Ok((scanned, changed, skipped))
    }

    /// Fetch a single memory by id in the engine namespace.
    pub async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        let mut found = self.get_scoped(id.clone()).await?;
        if let Some(note) = found.as_mut() {
            // Surface the contested flag on the get payload (Feature C, fail-open).
            let contested = self.contested_set(std::slice::from_ref(&id)).await;
            note.contested = contested.contains(&id);
            if let Err(e) = self.backend.record_access(id.clone()).await {
                tracing::debug!(error = %e, memory_id = %id, "record_access failed; ignoring");
            }
        }
        Ok(found)
    }

    /// List memories in the engine namespace, most-recent first, optionally
    /// filtered by a minimum importance.
    pub async fn list(
        &self,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let mut notes = self
            .backend
            .list(self.namespace.clone(), min_importance, limit)
            .await?;
        // Annotate the contested flag on list result rows (Feature C, fail-open).
        self.annotate_contested(&mut notes).await;
        Ok(notes)
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
            // Validation-class error: error_map forwards the message verbatim so
            // MCP clients see actionable guidance instead of "internal error".
            return Err(rb_types::Error::InvalidArgument(
                "content updates are not supported; create a new memory so embeddings stay consistent"
                    .to_string(),
            ));
        }
        if let Some(importance) = updates.importance {
            rb_types::validate_importance(importance)?;
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
        let mut recent = self
            .backend
            .list(self.namespace.clone(), None, CONTEXT_LIMIT)
            .await?;
        let mut important = self
            .backend
            .list(self.namespace.clone(), Some(IMPORTANT_FLOOR), CONTEXT_LIMIT)
            .await?;
        let total = recent.len();
        // Annotate contested on both context halves (Feature C, fail-open).
        self.annotate_contested(&mut recent).await;
        self.annotate_contested(&mut important).await;
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
    async fn remember_stamps_current_embedding_input_version() {
        let eng = engine();
        let id = eng
            .remember(input("composite input stamp", 5))
            .await
            .unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(
            note.embedding_input_version,
            crate::embed_input::EMBEDDING_INPUT_VERSION
        );
    }

    #[tokio::test]
    async fn remember_embeds_composite_not_raw_content() {
        // The composite (content + keywords + tags + context) differs from raw
        // content, so the stored vector must equal the composite's embedding.
        let eng = engine();
        let mut inp = input("body of the note", 5);
        inp.keywords = vec!["alpha".to_string()];
        inp.tags = vec!["beta".to_string()];
        inp.context = Some("gamma".to_string());
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        let stored = eng.backend().embedding_of(&id).unwrap();

        let composite = crate::embed_input::embedding_input(&note);
        let expected = DeterministicProvider::new(16)
            .embed(&[composite])
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            stored, expected,
            "stored vector must be the composite embedding"
        );

        // And it must NOT equal the raw-content embedding (composite added signal).
        let raw = DeterministicProvider::new(16)
            .embed(std::slice::from_ref(&note.content))
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_ne!(stored, raw, "composite differs from raw-content embedding");
    }

    #[tokio::test]
    async fn reembed_on_fresh_corpus_is_a_no_op() {
        // Every note remembered through this engine is already at current stamps,
        // so a reembed scans nothing and writes nothing.
        let eng = engine();
        eng.remember(input("already current one", 5)).await.unwrap();
        eng.remember(input("already current two", 5)).await.unwrap();
        let (scanned, changed, skipped) = eng.reembed(100).await.unwrap();
        assert_eq!((scanned, changed, skipped), (0, 0, 0));
        assert_eq!(eng.backend().update_vector_count(), 0);
    }

    #[tokio::test]
    async fn reembed_updates_stale_rows_then_second_run_is_idempotent() {
        let eng = engine();
        // A stale row: stamped with an old model/version (as if written pre-P5).
        let mut stale = note(
            Namespace::Project("rb".into()),
            "stale content needing reembed",
            MemoryType::Insight,
            5,
            &[],
        );
        stale.embedding_model = "old-model".to_string();
        stale.embedding_input_version = "v1-content-only".to_string();
        let id = stale.id.clone();
        eng.backend().insert_note(stale);

        // First run re-embeds the one stale row.
        let (scanned, changed, skipped) = eng.reembed(100).await.unwrap();
        assert_eq!((scanned, changed, skipped), (1, 1, 0));
        let after = eng.backend().note_of(&id).unwrap();
        assert_eq!(after.embedding_model, eng.embedder().model_id());
        assert_eq!(
            after.embedding_input_version,
            crate::embed_input::EMBEDDING_INPUT_VERSION
        );
        // The vector now exists for the row.
        assert!(eng.backend().embedding_of(&id).is_some());

        // Second run over unchanged data writes nothing (idempotent).
        let (scanned2, changed2, skipped2) = eng.reembed(100).await.unwrap();
        assert_eq!((scanned2, changed2, skipped2), (0, 0, 0));
    }

    #[tokio::test]
    async fn reembed_skips_archived_rows() {
        let eng = engine();
        let mut archived = note(
            Namespace::Project("rb".into()),
            "archived stale row",
            MemoryType::Insight,
            5,
            &[],
        );
        archived.embedding_model = "old-model".to_string();
        archived.embedding_input_version = "v1-content-only".to_string();
        archived.archived_at = Some(chrono::Utc::now());
        eng.backend().insert_note(archived);

        let (scanned, changed, skipped) = eng.reembed(100).await.unwrap();
        assert_eq!((scanned, changed, skipped), (0, 0, 0));
    }

    #[tokio::test]
    async fn reembed_is_fail_safe_per_row_on_write_error() {
        let eng = engine();
        let mut stale = note(
            Namespace::Project("rb".into()),
            "row whose vector update fails",
            MemoryType::Insight,
            5,
            &[],
        );
        stale.embedding_model = "old-model".to_string();
        stale.embedding_input_version = "v1-content-only".to_string();
        eng.backend().insert_note(stale);

        eng.backend().set_fail_update_vector(true);
        // The write error is caught per row: scanned 1, changed 0, skipped 1,
        // and the call still succeeds (never fatal).
        let (scanned, changed, skipped) = eng.reembed(100).await.unwrap();
        assert_eq!((scanned, changed, skipped), (1, 0, 1));
    }

    #[tokio::test]
    async fn reembed_respects_limit() {
        let eng = engine();
        for i in 0..5 {
            let mut stale = note(
                Namespace::Project("rb".into()),
                &format!("stale row {i}"),
                MemoryType::Insight,
                5,
                &[],
            );
            stale.embedding_model = "old-model".to_string();
            stale.embedding_input_version = "v1-content-only".to_string();
            eng.backend().insert_note(stale);
        }
        let (scanned, changed, skipped) = eng.reembed(2).await.unwrap();
        assert_eq!((scanned, changed, skipped), (2, 2, 0));
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

    #[tokio::test]
    async fn out_of_range_importance_is_invalid_argument_on_both_paths() {
        let eng = engine();

        // remember path.
        let err = eng
            .remember(input("bad importance on remember", 0))
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "remember must reject with InvalidArgument, got {err:?}"
        );

        // update path.
        let id = eng.remember(input("valid body", 5)).await.unwrap();
        let err = eng
            .update(
                id,
                rb_types::MemoryUpdates {
                    importance: Some(11),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "update must reject with InvalidArgument, got {err:?}"
        );
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
    async fn engine_defaults_to_linear_fusion() {
        let eng = engine();
        assert_eq!(eng.fusion_mode(), rb_search::FusionMode::Linear);
    }

    #[tokio::test]
    async fn with_fusion_mode_selects_rrf_and_recall_still_returns_results() {
        let eng = engine().with_fusion_mode(rb_search::FusionMode::Rrf);
        assert_eq!(eng.fusion_mode(), rb_search::FusionMode::Rrf);
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
        // RRF path returns ranked results with finite, descending scores.
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn recall_flags_both_contradicting_memories_as_contested() {
        // Feature C: create A and B, link A contradicts B, recall -> both flagged.
        let eng = engine();
        let a = seed(
            &eng,
            "claim alpha about caching",
            MemoryType::Insight,
            5,
            &[],
        )
        .await;
        let b = seed(
            &eng,
            "claim alpha says no caching",
            MemoryType::Insight,
            5,
            &[],
        )
        .await;
        eng.backend().link_contradicts(&a, &b);

        let results = eng.recall("alpha", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        // both endpoints of the active contradicts link are contested.
        assert!(results.iter().all(|r| r.memory.contested));
    }

    #[tokio::test]
    async fn recall_does_not_flag_uncontested_memories() {
        let eng = engine();
        seed(&eng, "uncontested note one", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "uncontested note two", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("note", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.memory.contested));
    }

    #[tokio::test]
    async fn recall_is_fail_open_when_contradiction_lookup_errors() {
        // Feature C fail-open: a forced contradiction-lookup error must NOT fail
        // recall; results return UNFLAGGED.
        let eng = engine();
        let a = seed(&eng, "claim x yes", MemoryType::Insight, 5, &[]).await;
        let b = seed(&eng, "claim x no", MemoryType::Insight, 5, &[]).await;
        eng.backend().link_contradicts(&a, &b);
        eng.backend().set_fail_contradicts(true);

        let results = eng.recall("claim", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2, "recall still succeeds on lookup error");
        assert!(
            results.iter().all(|r| !r.memory.contested),
            "fail-open => unflagged"
        );
    }

    #[tokio::test]
    async fn recall_low_confidence_wrong_memory_ranks_below_high_confidence_correct() {
        // Poison scenario at the engine level: two equally-matching memories where
        // the wrong one has low confidence. The confidence dampener must rank the
        // high-confidence (correct) one first.
        let eng = engine();
        let correct = note(
            Namespace::Project("rb".into()),
            "shared probe content",
            MemoryType::Insight,
            5,
            &[],
        );
        let mut wrong = note(
            Namespace::Project("rb".into()),
            "shared probe content",
            MemoryType::Insight,
            5,
            &[],
        );
        wrong.confidence = 0.1; // low confidence "poison"
        let correct_id = correct.id.clone();
        let wrong_id = wrong.id.clone();
        // Same vector distance for both so only confidence separates them.
        eng.backend().insert_note(correct);
        eng.backend().insert_note(wrong);
        eng.backend().set_keyword_results(Vec::new());
        eng.backend()
            .set_vector_results(vec![(wrong_id.clone(), 0.2), (correct_id.clone(), 0.2)]);

        let results = eng.recall("probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].memory.id, correct_id,
            "high-confidence correct memory ranks first"
        );
        assert_eq!(results[1].memory.id, wrong_id);
        assert!(results[0].score > results[1].score);
    }

    #[tokio::test]
    async fn get_surfaces_contested_flag() {
        let eng = engine();
        let a = eng.remember(input("a contested side", 5)).await.unwrap();
        let b = eng.remember(input("b contested side", 5)).await.unwrap();
        eng.backend().link_contradicts(&a, &b);
        let got = eng.get(a.clone()).await.unwrap().unwrap();
        assert!(got.contested, "get payload carries the contested flag");
        // a memory with no contradicts link is not contested.
        let c = eng.remember(input("c lonely", 5)).await.unwrap();
        let got_c = eng.get(c).await.unwrap().unwrap();
        assert!(!got_c.contested);
    }

    #[tokio::test]
    async fn list_surfaces_contested_flag() {
        let eng = engine();
        let a = eng.remember(input("list a", 5)).await.unwrap();
        let b = eng.remember(input("list b", 5)).await.unwrap();
        eng.backend().link_contradicts(&a, &b);
        let notes = eng.list(None, 10).await.unwrap();
        assert!(notes.iter().find(|n| n.id == a).unwrap().contested);
        assert!(notes.iter().find(|n| n.id == b).unwrap().contested);
    }

    #[tokio::test]
    async fn recall_empty_store_returns_empty() {
        let eng = engine();
        let results = eng.recall("anything", 10, None, &[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn recall_bumps_access_count_on_returned_results() {
        let eng = engine();
        seed(&eng, "alpha sqlite topic", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "beta tokio topic", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("topic", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        // each returned id had its access recorded.
        for r in &results {
            let note = eng.backend().note_of(&r.memory.id).unwrap();
            assert_eq!(note.access_count, 1);
        }
    }

    #[tokio::test]
    async fn recall_record_access_failure_does_not_fail_recall() {
        let eng = engine();
        seed(&eng, "probe content", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_fail_record_access(true);
        // Recall still returns its results despite record_access failing.
        let results = eng.recall("probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 1);
        // record_access was attempted (best-effort), even though it errored.
        assert!(eng.backend().record_access_count() >= 1);
    }

    #[tokio::test]
    async fn get_bumps_access_count_when_found() {
        let eng = engine();
        let id = eng.remember(input("findable", 5)).await.unwrap();
        let before = eng.backend().note_of(&id).unwrap().access_count;
        let got = eng.get(id.clone()).await.unwrap().unwrap();
        assert_eq!(got.id, id);
        // access recorded after a successful get.
        assert_eq!(eng.backend().note_of(&id).unwrap().access_count, before + 1);
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
        // Must be the validation-class variant: error_map forwards its message
        // verbatim, so the client sees the guidance instead of "internal error".
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)));
        assert!(err
            .to_string()
            .contains("content updates are not supported"));
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

    use crate::test_support::{FailingEnricher, FixedEnricher};

    #[tokio::test]
    async fn remember_with_enricher_fills_empty_keywords_tags_and_summary() {
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        // caller leaves keywords/tags empty -> enricher fills them.
        let id = eng.remember(input("some content body", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.summary, "enriched summary");
        assert_eq!(note.keywords, vec!["enrkw".to_string()]);
        assert_eq!(note.tags, vec!["enrtag".to_string()]);
    }

    #[tokio::test]
    async fn remember_with_enricher_preserves_caller_supplied_keywords_and_tags() {
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        let mut inp = input("body", 5);
        inp.keywords = vec!["caller".to_string()];
        inp.tags = vec!["callertag".to_string()];
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // explicit caller values win over the enricher.
        assert_eq!(note.keywords, vec!["caller".to_string()]);
        assert_eq!(note.tags, vec!["callertag".to_string()]);
        // summary still comes from the enricher (caller never supplies it).
        assert_eq!(note.summary, "enriched summary");
    }

    #[tokio::test]
    async fn remember_falls_back_to_heuristic_when_enricher_errors() {
        let eng = engine().with_enricher(Arc::new(FailingEnricher));
        let content = "concurrent readers never block the single writer thread";
        let id = eng.remember(input(content, 6)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // heuristic summary == trimmed content (< 150 chars); keywords non-empty.
        assert_eq!(note.summary, content);
        assert!(!note.keywords.is_empty());
    }

    #[tokio::test]
    async fn remember_without_enricher_is_unchanged_heuristic_path() {
        let eng = engine(); // no enricher
        let content = "single writer over sqlite wal keeps things correct";
        let id = eng.remember(input(content, 7)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.summary, content);
        assert!(!note.keywords.is_empty());
    }
}
