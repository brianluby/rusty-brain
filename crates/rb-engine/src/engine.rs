use crate::backend::MemoryBackend;
use crate::enrich::{default_summary, derive_keywords};
use crate::enricher::Enricher;
use crate::linker::{Linker, SimilarityLinker};
use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_search::{FusionMode, RrfConfig, Weights};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};
use std::sync::Arc;

/// Input to `remember`. Mirrors the proto `Request::Remember` payload, plus
/// the connection-scoped provenance the daemon resolves at handshake (W0.5).
pub struct RememberInput {
    pub content: String,
    pub context: Option<String>,
    pub memory_type: MemoryType,
    pub importance: u8,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub related_files: Vec<String>,
    /// Explicit trust prior in `0.0..=1.0` (validated fail-closed), or `None`
    /// when the caller expressed no prior. `None` keeps the full-trust 1.0
    /// baseline AND lets an enricher fill it; `Some(x)` is an explicit prior an
    /// enricher must not override (fix #4). Hook captures send `Some(0.7)`.
    pub confidence: Option<f32>,
    pub provenance: Provenance,
}

/// What `recall_with_status` returns (W1.6d): the ranked results plus whether
/// retrieval DEGRADED to keyword + graph because the embedder errored. The
/// daemon forwards `degraded` on the wire so clients can warn instead of
/// silently serving vector-blind results.
#[derive(Debug, Clone)]
pub struct RecallOutcome {
    pub results: Vec<rb_types::SearchResult>,
    pub degraded: bool,
}

/// Who/where/what produced a write (W0.5): the connection's handshake identity
/// after the daemon's whoami fallback for user/host. `Default` (all `None`)
/// matches pre-W0.5 behavior and is what direct engine callers (tests, eval)
/// use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    pub origin_user: Option<String>,
    pub origin_host: Option<String>,
    pub origin_agent: Option<String>,
    /// Producer surface: `hook` | `mcp` | `cli` | `job`.
    pub origin_source: Option<String>,
    pub session_id: Option<String>,
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
    /// Minimum `Linear` recall score a result must reach to be returned (W1.3).
    /// Below-floor results are dropped, so recall may return fewer than `limit`
    /// — or nothing. Applies to `Linear` only: `Rrf` scores live on a different
    /// scale and stay unfloored until RRF is reachable and calibrated
    /// (W2.2/W4.1). See `rb_search::SCORE_FLOOR` for the derivation.
    score_floor: f32,
    /// Determinism hook for eval/tests: when set, `remember` stamps
    /// created/updated at this instant and ranking computes recency against
    /// it, so (now - created_at) deltas are reproducible across runs instead
    /// of riding the wall clock. `None` (production) uses `Utc::now()`.
    fixed_now: Option<chrono::DateTime<chrono::Utc>>,
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
            score_floor: rb_search::SCORE_FLOOR,
            fixed_now: None,
        }
    }

    /// Pin the engine's clock (eval/test determinism): `remember` stamps
    /// created/updated at `now` and ranking computes recency against it, so
    /// `(now - created_at)` deltas reproduce bit-for-bit across runs instead
    /// of drifting with insert/query wall-clock timing. Never set in the
    /// daemon.
    pub fn with_fixed_now(mut self, now: chrono::DateTime<chrono::Utc>) -> Self {
        self.fixed_now = Some(now);
        self
    }

    /// The instant "now" for write stamps and recency ranking: the pinned
    /// eval/test clock when set, else the wall clock.
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.fixed_now.unwrap_or_else(chrono::Utc::now)
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

    /// The recall score floor applied under `Linear` fusion (W1.3).
    pub fn score_floor(&self) -> f32 {
        self.score_floor
    }

    /// Override the recall score floor (tests and eval recalibration only —
    /// production keeps the derived `rb_search::SCORE_FLOOR` default). A
    /// non-finite floor is sanitized to 0.0 (floor disabled, fail-open).
    pub fn with_score_floor(mut self, floor: f32) -> Self {
        self.score_floor = if floor.is_finite() { floor } else { 0.0 };
        self
    }

    fn in_namespace(&self, note: &MemoryNote) -> bool {
        note.namespace == self.namespace
    }

    fn active_in_namespace(&self, note: &MemoryNote) -> bool {
        self.in_namespace(note) && note.archived_at.is_none()
    }

    /// Fail-fast filter gate shared by `recall` and `list`: range/bound
    /// validation plus the anchor rejection — the anchors TABLE ships with the
    /// typed-code-anchors PRD, so until then a non-empty anchor filter is an
    /// explicit error, never a silently ignored constraint.
    fn ensure_filter_supported(filter: &rb_types::RecallFilter) -> rb_types::Result<()> {
        filter.validate()?;
        if !filter.anchors.is_empty() {
            return Err(rb_types::Error::InvalidArgument(
                "anchor filters are not supported yet (the memory_anchors table ships with \
                 typed code anchors)"
                    .to_string(),
            ));
        }
        Ok(())
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
        // Fail-closed confidence range check on an EXPLICIT caller prior
        // (mirrors the storage CHECK, but surfaces as a clean validation error
        // rather than a Storage one). `None` carries no prior — valid.
        if let Some(c) = input.confidence {
            rb_types::validate_confidence(c)?;
        }

        let mut note = MemoryNote::new(
            self.namespace.clone(),
            input.content,
            input.memory_type,
            input.importance,
        );
        // Determinism hook: under a pinned clock the write stamps are the
        // pinned instant, keeping recency deltas reproducible (eval only).
        if let Some(now) = self.fixed_now {
            note.created_at = now;
            note.updated_at = now;
        }
        // `MemoryNote::new` defaults confidence to the full-trust 1.0 baseline;
        // an explicit caller prior overrides it (an enricher will not — below).
        if let Some(c) = input.confidence {
            note.confidence = c;
        }
        note.origin_user = input.provenance.origin_user;
        note.origin_host = input.provenance.origin_host;
        note.origin_agent = input.provenance.origin_agent;
        note.origin_source = input.provenance.origin_source;
        note.session_id = input.provenance.session_id;
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
        // Enricher-declared confidence (W2.2: the enrichment trust producer).
        // Applied only when the caller expressed NO prior (`input.confidence`
        // is None) — an explicit caller prior always wins, even an explicit
        // 1.0 (fix #4: a bare-f32 sentinel could not distinguish an explicit
        // 1.0 from the default and would wrongly downgrade it). An out-of-range
        // enricher value is ignored, not an error: enrichment is advisory and
        // never fails a remember.
        if input.confidence.is_none() {
            if let Some(conf) = enrichment.as_ref().and_then(|e| e.confidence) {
                if rb_types::validate_confidence(conf).is_ok() {
                    note.confidence = conf;
                } else {
                    tracing::warn!(
                        confidence = conf,
                        "enricher returned out-of-range confidence; ignoring"
                    );
                }
            }
        }
        note.embedding_model = self.embedder.model_id().to_string();
        // Stamp the composition version alongside the model so the `reembed`
        // batch can detect rows built from an older representation (Feature A).
        note.embedding_input_version = crate::embed_input::EMBEDDING_INPUT_VERSION.to_string();

        // Embed the COMPOSITE document representation (content + keywords + tags
        // + context), not raw content (spec §8). The query stays embedded raw.
        // Write path => EmbedKind::Document (W1.4).
        let input = crate::embed_input::embedding_input(&note);
        let mut embeddings = self.embedder.embed(&[input], EmbedKind::Document).await?;
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
    /// return ranked `SearchResult`s after applying the unified
    /// [`rb_types::RecallFilter`] (PRD 2026-07-02 search-filter parity).
    ///
    /// Thin wrapper over [`MemoryEngine::recall_with_status`] that drops the
    /// degraded flag; callers that surface degradation (the daemon's Recall
    /// dispatch) use the `_with_status` form.
    pub async fn recall(
        &self,
        query: &str,
        limit: usize,
        filter: &rb_types::RecallFilter,
    ) -> rb_types::Result<Vec<rb_types::SearchResult>> {
        Ok(self.recall_with_status(query, limit, filter).await?.results)
    }

    /// [`MemoryEngine::recall`] plus a degradation flag (W1.6d / F19): when the
    /// embedder errors, recall DEGRADES to the keyword + graph channels instead
    /// of failing outright — an embedding-API outage must not take retrieval
    /// down with it — and `degraded` is set so the response can carry a
    /// warning. The vector channel is skipped entirely in that case; ranking
    /// proceeds on the surviving signals.
    ///
    /// Filter semantics: metadata dimensions apply per candidate BEFORE
    /// ranking ([`rb_types::RecallFilter::matches`]); `contested` is resolved
    /// through one batched `active_contradicts` lookup and FAILS CLOSED (a
    /// filter must never silently return unfiltered results — unlike the
    /// best-effort contested ANNOTATION, which stays fail-open); a non-default
    /// `state` widens the keyword channel to archived rows (their vectors are
    /// pruned on archive, so archived recall rides keyword+graph only).
    pub async fn recall_with_status(
        &self,
        query: &str,
        limit: usize,
        filter: &rb_types::RecallFilter,
    ) -> rb_types::Result<RecallOutcome> {
        use std::collections::HashMap;

        Self::ensure_filter_supported(filter)?;

        // Over-fetch candidates so post-filtering still has enough to fill `limit`.
        let candidate_limit = limit.saturating_mul(4).max(limit);

        // Recall path => EmbedKind::Query (W1.4): asymmetric providers (Voyage
        // input_type) condition the query vector for retrieval.
        let (embedding, degraded) = match self
            .embedder
            .embed(&[query.to_string()], EmbedKind::Query)
            .await
        {
            Ok(mut query_emb) => (query_emb.pop().unwrap_or_default(), false),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "query embedding failed; recall degrades to keyword+graph"
                );
                (Vec::new(), true)
            }
        };

        let keyword = self
            .backend
            .keyword(
                self.namespace.clone(),
                query.to_string(),
                candidate_limit,
                filter.state,
            )
            .await?;
        // Archived rows have no vector (pruned on archive: the live-only vec0
        // partition invariant), so an archived-only recall skips the channel.
        let vector = if degraded || filter.state == rb_types::MemoryState::Archived {
            Vec::new()
        } else {
            self.backend
                .vector(self.namespace.clone(), embedding, candidate_limit)
                .await?
        };

        // Bounded 1-hop graph expansion of the top filter-matching in-namespace
        // keyword hit only.
        let mut graph_seed = None;
        for id in &keyword {
            if self
                .get_scoped(id.clone())
                .await?
                .as_ref()
                .is_some_and(|note| filter.matches(note))
            {
                graph_seed = Some(id.clone());
                break;
            }
        }
        // `(id, hops)` pairs with REAL minimum hop distances (W1.5); hops feed
        // the graph ranking signal through `build_signals`.
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
            .chain(graph.iter().map(|(id, _)| id))
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
            // `get_many` is already namespace-scoped; `filter.matches` covers
            // every metadata dimension including the archived-state scope
            // (default: active-only, the historical behavior).
            if !self.in_namespace(&note) || !filter.matches(&note) {
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

        // Contested filter (tri-state): resolve via ONE batched lookup over the
        // metadata-surviving candidates and retain the requested side. FAILS
        // CLOSED — an error here fails the recall rather than silently
        // returning unfiltered results (the fail-open path below is only the
        // best-effort annotation). The resolved set also stamps the returned
        // notes' `contested` flag, so filter and flag can never disagree.
        let mut contested_for_annotation: Option<std::collections::HashSet<MemoryId>> = None;
        if let Some(want_contested) = filter.contested {
            let ids: Vec<MemoryId> = notes.keys().cloned().collect();
            let contested = self
                .backend
                .active_contradicts(self.namespace.clone(), ids)
                .await?;
            notes.retain(|id, _| contested.contains(id) == want_contested);
            meta.retain(|id, _| notes.contains_key(id));
            contested_for_annotation = Some(contested);
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
        let filtered_graph: Vec<(MemoryId, u8)> = graph
            .iter()
            .filter(|(id, _)| notes.contains_key(id))
            .cloned()
            .collect();

        let signals =
            rb_search::build_signals(&filtered_keyword, &filtered_vector, &filtered_graph, &meta);
        // Per-channel hit attribution (W1.0): which channels surfaced each
        // candidate, read off the merged signals BEFORE ranking consumes them.
        // A `Some` signal field means that channel's candidate set contained
        // the id, so the flags record contribution, not rank.
        let channel_map: HashMap<MemoryId, rb_types::ChannelHits> = signals
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    rb_types::ChannelHits {
                        fts: s.keyword_rank.is_some(),
                        vector: s.vector_distance.is_some(),
                        graph: s.graph_hops.is_some(),
                    },
                )
            })
            .collect();
        // Dispatch on the configured fusion mode. `Linear` is the default and
        // preserves prior behavior byte-for-byte; `Rrf` is the opt-in two-stage
        // hybrid (spec §7).
        let now = self.now();
        let ranked = match self.fusion_mode {
            FusionMode::Linear => rb_search::rank(signals, self.weights, now, candidate_limit),
            FusionMode::Rrf => rb_search::rank_rrf(signals, self.rrf_config, now, candidate_limit),
        };

        // Assemble results in ranked order, truncating to limit. Under `Linear`
        // the score floor (W1.3) drops below-floor candidates: `ranked` is
        // sorted descending, so the first below-floor score ends assembly and
        // recall may return fewer than `limit` — or nothing — instead of
        // padding with junk the KNN leg surfaced by construction (F30). `Rrf`
        // scores live on a different scale and stay unfloored until calibrated
        // (W2.2/W4.1).
        let floor = match self.fusion_mode {
            FusionMode::Linear => self.score_floor,
            FusionMode::Rrf => f32::NEG_INFINITY,
        };
        let mut results: Vec<rb_types::SearchResult> = Vec::new();
        for (id, score) in ranked {
            if score < floor {
                break;
            }
            let Some(note) = notes.get(&id) else {
                continue;
            };
            results.push(rb_types::SearchResult {
                memory: note.clone(),
                score,
                channels: channel_map.get(&id).copied().unwrap_or_default(),
            });
            if results.len() == limit {
                break;
            }
        }

        let returned_ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();

        // Contradiction surfacing (Feature C): for the returned (post-ranking,
        // truncated) set only, batch-load active contradicts links and flag each
        // contested memory. FAIL-OPEN: a lookup error leaves results unflagged
        // rather than failing recall. When a contested FILTER already resolved
        // the set (fail-closed above), reuse it instead of a second lookup.
        let contested = match contested_for_annotation {
            Some(set) => set,
            None => self.contested_set(&returned_ids).await,
        };
        for r in &mut results {
            r.memory.contested = contested.contains(&r.memory.id);
        }

        // Best-effort batch access tracking. W1.8: the daemon backend BUFFERS
        // these bumps and flushes them off the recall path, so this call costs
        // recall zero writer-thread ops (and, under migration 006, the eventual
        // flush costs zero FTS writes).
        if !returned_ids.is_empty() {
            if let Err(e) = self.backend.record_accesses(returned_ids).await {
                tracing::debug!(error = %e, "record_accesses failed; ignoring");
            }
        }
        Ok(RecallOutcome { results, degraded })
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
            // Reembed converges stored vectors => EmbedKind::Document (W1.4).
            let input = crate::embed_input::embedding_input(&note);
            let embedding = match self.embedder.embed(&[input], EmbedKind::Document).await {
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

    /// Namespace-scoped read that does NOT record an access, unlike [`get`](Self::get).
    /// Used by internal maintenance (W3.1 write-time near-dup suppression) that
    /// must inspect a memory's provenance without polluting its access /
    /// usefulness signal — `access_count` is the "returned by recall" signal
    /// (W3.7), so an automatic scan must never inflate it.
    pub async fn peek(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        self.get_scoped(id).await
    }

    /// List memories in the engine namespace, most-recent first, filtered by
    /// the unified [`rb_types::RecallFilter`] (PRD 2026-07-02 search-filter
    /// parity). EVERY dimension — metadata AND the contested tri-state — is
    /// honored by the backend's bounded query (SQL on the real store), so
    /// `limit` fills with actual matches no matter how deep they sit; a
    /// backend error under a contested filter fails the list (fail-closed —
    /// never silently unfiltered results). Anchors are rejected until typed
    /// code anchors land.
    pub async fn list(
        &self,
        filter: &rb_types::RecallFilter,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        Self::ensure_filter_supported(filter)?;
        let mut notes = self
            .backend
            .list(self.namespace.clone(), filter.clone(), limit)
            .await?;
        if let Some(want_contested) = filter.contested {
            // The backend already guaranteed every returned row satisfies the
            // contested filter; stamp the flag from that same guarantee so
            // filter and flag can never disagree.
            for n in &mut notes {
                n.contested = want_contested;
            }
            return Ok(notes);
        }
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
        for (nid, _hops) in ids {
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
        if let Some(confidence) = updates.confidence {
            rb_types::validate_confidence(confidence)?;
        }
        if self.get_scoped(id.clone()).await?.is_none() {
            return Err(rb_types::Error::NotFound(id));
        }
        self.backend
            .update(self.namespace.clone(), id, updates)
            .await
    }

    /// Create an explicit link between two memories in this namespace (W2.2:
    /// the user-facing producer for `contradicts` — the read side already
    /// surfaces `contested` from active contradicts edges). Fail-closed
    /// validation: self-links and duplicate `(from, to, type)` edges are
    /// validation-class errors; both endpoints must exist in the caller's
    /// namespace. A user-asserted link carries full strength (1.0).
    pub async fn link(
        &self,
        from: MemoryId,
        to: MemoryId,
        link_type: rb_types::LinkType,
        reason: Option<String>,
    ) -> rb_types::Result<()> {
        if from == to {
            return Err(rb_types::Error::InvalidArgument(
                "cannot link a memory to itself".to_string(),
            ));
        }
        if link_type == rb_types::LinkType::Supersedes {
            // A bare supersedes edge would bypass the atomic supersede
            // machinery (which also stamps `superseded_by` on the old row).
            return Err(rb_types::Error::InvalidArgument(
                "supersedes links are created by storing a replacement memory, \
                 not by linking; use a new memory that supersedes the old one"
                    .to_string(),
            ));
        }
        let source = self
            .get_scoped(from.clone())
            .await?
            .ok_or_else(|| rb_types::Error::NotFound(from.clone()))?;
        if self.get_scoped(to.clone()).await?.is_none() {
            return Err(rb_types::Error::NotFound(to));
        }
        if source
            .links
            .iter()
            .any(|l| l.target_id == to && l.link_type == link_type)
        {
            return Err(rb_types::Error::InvalidArgument(format!(
                "a {} link from {from} to {to} already exists",
                link_type.as_str()
            )));
        }
        self.backend
            .add_link(rb_types::MemoryLink {
                source_id: from,
                target_id: to,
                link_type,
                strength: 1.0,
                reason: reason.unwrap_or_else(|| "user-asserted".to_string()),
                created_at: chrono::Utc::now(),
            })
            .await
    }

    /// Record a usefulness-feedback event for a memory in this namespace (W3.7
    /// / F37): the explicit usefulness/correctness signal `access_count` is not
    /// (it counts "returned", not "useful"). Verifies `id` lives in the engine's
    /// namespace (fail-closed `NotFound`), then records the event and nudges the
    /// trust prior through the backend. `provenance` supplies the giver
    /// (`origin_user`) for the W5c per-author trust rollup. Returns the memory's
    /// `confidence` after the bounded nudge.
    pub async fn feedback(
        &self,
        id: MemoryId,
        kind: rb_types::FeedbackKind,
        provenance: &Provenance,
    ) -> rb_types::Result<f32> {
        // Feedback targets a LIVE, recallable memory: recall never returns
        // archived rows, so an archived target is a caller error. Reject it
        // (NotFound) via the `active_in_namespace` check `graph` uses — distinct
        // from `update`/`link`, whose metadata edits remain valid on archived
        // rows. This also avoids a pointless confidence nudge + `Updated` event
        // on a soft-deleted memory.
        let active = self
            .get_scoped(id.clone())
            .await?
            .as_ref()
            .is_some_and(|note| self.active_in_namespace(note));
        if !active {
            return Err(rb_types::Error::NotFound(id));
        }
        self.backend
            .record_feedback(
                self.namespace.clone(),
                id,
                kind,
                provenance.origin_user.clone(),
            )
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
            .list(
                self.namespace.clone(),
                rb_types::RecallFilter::default(),
                CONTEXT_LIMIT,
            )
            .await?;
        let mut important = self
            .backend
            .list(
                self.namespace.clone(),
                rb_types::RecallFilter {
                    min_importance: Some(IMPORTANT_FLOOR),
                    ..Default::default()
                },
                CONTEXT_LIMIT,
            )
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
            // No explicit prior: the engine applies the 1.0 baseline and an
            // enricher (when present) may fill it.
            confidence: None,
            provenance: Provenance::default(),
        }
    }

    /// Store `n` notes shaped by `shape(i, note)` through the backend directly
    /// (bypassing `remember` so tests control confidence/provenance/timestamps).
    async fn seed_notes(
        eng: &MemoryEngine<MockBackend, DeterministicProvider>,
        n: usize,
        shape: impl Fn(usize, &mut rb_types::MemoryNote),
    ) -> Vec<MemoryId> {
        let mut ids = Vec::new();
        for i in 0..n {
            let mut note = rb_types::MemoryNote::new(
                Namespace::Project("rb".into()),
                format!("seeded searchable content {i}"),
                MemoryType::Insight,
                5,
            );
            shape(i, &mut note);
            ids.push(note.id.clone());
            eng.backend().insert_note(note);
        }
        ids
    }

    fn only(filter: rb_types::RecallFilter) -> rb_types::RecallFilter {
        filter
    }

    #[tokio::test]
    async fn recall_filters_by_confidence_range() {
        let eng = engine();
        let ids = seed_notes(&eng, 3, |i, note| {
            note.confidence = [0.2f32, 0.6, 0.95][i];
        })
        .await;
        let results = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    min_confidence: Some(0.5),
                    max_confidence: Some(0.8),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[1].clone()]);
    }

    #[tokio::test]
    async fn recall_filters_by_created_at_window() {
        let eng = engine();
        let t0 = chrono::Utc::now();
        let ids = seed_notes(&eng, 3, |i, note| {
            note.created_at = t0 - chrono::Duration::days([10, 3, 0][i]);
        })
        .await;
        let results = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    since: Some(t0 - chrono::Duration::days(5)),
                    until: Some(t0 - chrono::Duration::days(1)),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[1].clone()]);
    }

    #[tokio::test]
    async fn recall_filters_by_source() {
        let eng = engine();
        let ids = seed_notes(&eng, 3, |i, note| {
            note.origin_source =
                [Some("hook".to_string()), Some("cli".to_string()), None][i].clone();
        })
        .await;
        let results = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    sources: vec!["hook".to_string(), "mcp".to_string()],
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[0].clone()]);
    }

    #[tokio::test]
    async fn recall_filters_by_importance_range() {
        let eng = engine();
        let ids = seed_notes(&eng, 3, |i, note| {
            note.importance = [2, 5, 9][i];
        })
        .await;
        let results = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    min_importance: Some(4),
                    max_importance: Some(6),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[1].clone()]);
    }

    #[tokio::test]
    async fn recall_contested_filter_selects_by_contested_state() {
        let eng = engine();
        let ids = seed_notes(&eng, 3, |_, _| {}).await;
        // ids[0] <-> ids[1] contradict each other; ids[2] is uncontested.
        eng.backend().link_contradicts(&ids[0], &ids[1]);

        let contested_only = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    contested: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let mut got: Vec<MemoryId> = contested_only.iter().map(|r| r.memory.id.clone()).collect();
        got.sort_by_key(std::string::ToString::to_string);
        let mut expected = vec![ids[0].clone(), ids[1].clone()];
        expected.sort_by_key(std::string::ToString::to_string);
        assert_eq!(got, expected);
        assert!(
            contested_only.iter().all(|r| r.memory.contested),
            "filtered-to-contested results must carry the contested flag"
        );

        let uncontested_only = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    contested: Some(false),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = uncontested_only
            .iter()
            .map(|r| r.memory.id.clone())
            .collect();
        assert_eq!(got, vec![ids[2].clone()]);
        assert!(uncontested_only.iter().all(|r| !r.memory.contested));
    }

    #[tokio::test]
    async fn recall_contested_filter_fails_closed_on_lookup_error() {
        // The contested ANNOTATION is fail-open (best-effort enrichment), but a
        // contested FILTER must fail closed: silently returning unfiltered
        // results would violate the query.
        let eng = engine();
        seed_notes(&eng, 1, |_, _| {}).await;
        eng.backend().set_fail_contradicts(true);
        let err = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    contested: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::Storage(_)));
    }

    #[tokio::test]
    async fn recall_state_filter_reaches_archived_memories() {
        let eng = engine();
        let t0 = chrono::Utc::now();
        let ids = seed_notes(&eng, 2, |i, note| {
            if i == 1 {
                note.archived_at = Some(t0);
            }
        })
        .await;

        // Default scope stays active-only.
        let active = eng
            .recall("seeded", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        let got: Vec<MemoryId> = active.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[0].clone()]);

        // state=archived surfaces ONLY the archived memory (keyword channel).
        let archived = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    state: rb_types::MemoryState::Archived,
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = archived.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[1].clone()]);

        // state=all surfaces both.
        let all = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    state: rb_types::MemoryState::All,
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn recall_rejects_anchor_filters_until_typed_anchors_land() {
        let eng = engine();
        let err = eng
            .recall(
                "q",
                10,
                &only(rb_types::RecallFilter {
                    anchors: vec![rb_types::AnchorFilter {
                        kind: rb_types::AnchorKind::File,
                        value: "src/lib.rs".into(),
                    }],
                    ..Default::default()
                }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "expected InvalidArgument, got {err:?}"
        );
    }

    #[tokio::test]
    async fn recall_rejects_invalid_filter_bounds() {
        let eng = engine();
        let err = eng
            .recall(
                "q",
                10,
                &only(rb_types::RecallFilter {
                    min_confidence: Some(0.9),
                    max_confidence: Some(0.1),
                    ..Default::default()
                }),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn recall_filters_compose_across_dimensions() {
        let eng = engine();
        let t0 = chrono::Utc::now();
        let ids = seed_notes(&eng, 4, |i, note| {
            // Only note 0 satisfies ALL three legs.
            note.origin_source = Some(if i == 2 { "cli" } else { "hook" }.to_string());
            note.importance = if i == 3 { 3 } else { 8 };
            note.created_at = if i == 1 {
                t0 - chrono::Duration::days(30)
            } else {
                t0
            };
        })
        .await;
        let results = eng
            .recall(
                "seeded",
                10,
                &only(rb_types::RecallFilter {
                    sources: vec!["hook".to_string()],
                    min_importance: Some(7),
                    since: Some(t0 - chrono::Duration::days(7)),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        assert_eq!(got, vec![ids[0].clone()]);
    }

    #[tokio::test]
    async fn list_honors_the_unified_filter() {
        let eng = engine();
        let ids = seed_notes(&eng, 3, |i, note| {
            note.origin_source = Some(if i == 0 { "hook" } else { "cli" }.to_string());
            note.confidence = [0.9f32, 0.9, 0.2][i];
        })
        .await;
        let listed = eng
            .list(
                &only(rb_types::RecallFilter {
                    sources: vec!["hook".to_string()],
                    min_confidence: Some(0.5),
                    ..Default::default()
                }),
                10,
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = listed.iter().map(|n| n.id.clone()).collect();
        assert_eq!(got, vec![ids[0].clone()]);
    }

    #[tokio::test]
    async fn list_contested_filter_selects_by_contested_state() {
        let eng = engine();
        let ids = seed_notes(&eng, 3, |_, _| {}).await;
        eng.backend().link_contradicts(&ids[0], &ids[1]);

        let contested = eng
            .list(
                &only(rb_types::RecallFilter {
                    contested: Some(true),
                    ..Default::default()
                }),
                10,
            )
            .await
            .unwrap();
        let mut got: Vec<MemoryId> = contested.iter().map(|n| n.id.clone()).collect();
        got.sort_by_key(std::string::ToString::to_string);
        let mut expected = vec![ids[0].clone(), ids[1].clone()];
        expected.sort_by_key(std::string::ToString::to_string);
        assert_eq!(got, expected);
        assert!(contested.iter().all(|n| n.contested));

        let uncontested = eng
            .list(
                &only(rb_types::RecallFilter {
                    contested: Some(false),
                    ..Default::default()
                }),
                10,
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = uncontested.iter().map(|n| n.id.clone()).collect();
        assert_eq!(got, vec![ids[2].clone()]);
    }

    #[tokio::test]
    async fn list_contested_filter_fills_limit_beyond_any_bounded_window() {
        // Regression (PR #58 review): 20 uncontested rows are NEWER than the 2
        // contested ones, so an "over-fetch 4x limit then retain" scheme would
        // return nothing for limit=2. Contested filtering belongs to the
        // backend's bounded query; the engine must not re-window it.
        let eng = engine();
        let t0 = chrono::Utc::now();
        let _noise = seed_notes(&eng, 20, |i, note| {
            note.created_at = t0 - chrono::Duration::seconds(i as i64);
        })
        .await;
        let old = seed_notes(&eng, 2, |i, note| {
            note.created_at = t0 - chrono::Duration::seconds(100 + i as i64);
        })
        .await;
        eng.backend().link_contradicts(&old[0], &old[1]);

        let listed = eng
            .list(
                &only(rb_types::RecallFilter {
                    contested: Some(true),
                    ..Default::default()
                }),
                2,
            )
            .await
            .unwrap();
        let got: Vec<MemoryId> = listed.iter().map(|n| n.id.clone()).collect();
        assert_eq!(
            got,
            vec![old[0].clone(), old[1].clone()],
            "limit must fill with contested matches even past a 4x window"
        );
        assert!(listed.iter().all(|n| n.contested));
    }

    #[tokio::test]
    async fn list_rejects_anchor_filters_and_invalid_bounds() {
        let eng = engine();
        let err = eng
            .list(
                &only(rb_types::RecallFilter {
                    anchors: vec![rb_types::AnchorFilter {
                        kind: rb_types::AnchorKind::Symbol,
                        value: "Engine::recall".into(),
                    }],
                    ..Default::default()
                }),
                10,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)));

        let err = eng
            .list(
                &only(rb_types::RecallFilter {
                    min_importance: Some(9),
                    max_importance: Some(2),
                    ..Default::default()
                }),
                10,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)));
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
    async fn remember_stores_confidence_and_provenance() {
        let eng = engine();
        let mut inp = input("hook capture with provenance", 5);
        inp.confidence = Some(0.7);
        inp.provenance = Provenance {
            origin_user: Some("alice".into()),
            origin_host: Some("devbox".into()),
            origin_agent: Some("claude-code".into()),
            origin_source: Some("hook".into()),
            session_id: Some("s-1".into()),
        };
        let id = eng.remember(inp).await.unwrap();
        let note = eng.get(id).await.unwrap().unwrap();
        assert!((note.confidence - 0.7).abs() < f32::EPSILON);
        assert_eq!(note.origin_user.as_deref(), Some("alice"));
        assert_eq!(note.origin_host.as_deref(), Some("devbox"));
        assert_eq!(note.origin_agent.as_deref(), Some("claude-code"));
        assert_eq!(note.origin_source.as_deref(), Some("hook"));
        assert_eq!(note.session_id.as_deref(), Some("s-1"));
    }

    #[tokio::test]
    async fn remember_rejects_out_of_range_confidence() {
        let eng = engine();
        for bad in [-0.1f32, 1.1, f32::NAN, f32::INFINITY] {
            let mut inp = input("bad confidence", 5);
            inp.confidence = Some(bad);
            let err = eng.remember(inp).await.unwrap_err();
            assert!(
                matches!(err, rb_types::Error::InvalidArgument(_)),
                "confidence {bad} must be InvalidArgument, got {err:?}"
            );
        }
        assert_eq!(eng.backend().count(), 0, "nothing may be written");
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
            .embed(&[composite], EmbedKind::Document)
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
            .embed(std::slice::from_ref(&note.content), EmbedKind::Document)
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

        // Counts calls only; the kind is irrelevant to this stub.
        async fn embed(
            &self,
            texts: &[String],
            _kind: EmbedKind,
        ) -> rb_types::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![vec![0.0; 16]; texts.len()])
        }
    }

    /// Records every kind passed to `embed`, in call order, behind an `Arc`
    /// handle so the engine can own the provider while the test reads the log.
    struct KindLoggingProvider {
        kinds: Arc<std::sync::Mutex<Vec<EmbedKind>>>,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for KindLoggingProvider {
        fn model_id(&self) -> &str {
            "kind-logging"
        }

        fn dim(&self) -> usize {
            16
        }

        async fn embed(
            &self,
            texts: &[String],
            kind: EmbedKind,
        ) -> rb_types::Result<Vec<Vec<f32>>> {
            self.kinds
                .lock()
                .map_err(|_| rb_types::Error::Embedding("kind log poisoned".to_string()))?
                .push(kind);
            // Distinct unit vectors keep the vector leg functional.
            Ok(vec![vec![1.0; 16]; texts.len()])
        }
    }

    #[tokio::test]
    async fn recall_embeds_as_query_and_write_paths_embed_as_document() {
        // W1.4 kind routing: remember and reembed are write paths (Document);
        // recall embeds the user query (Query).
        let kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
        let eng = MemoryEngine::new(
            MockBackend::default(),
            KindLoggingProvider {
                kinds: Arc::clone(&kinds),
            },
            Namespace::Project("rb".into()),
        );

        eng.remember(input("kind routing note", 5)).await.unwrap();
        eng.recall("kind routing", 5, &rb_types::RecallFilter::default())
            .await
            .unwrap();

        // A stale row forces reembed to issue one Document-kind embed.
        let mut stale = note(
            Namespace::Project("rb".into()),
            "stale row for kind routing",
            MemoryType::Insight,
            5,
            &[],
        );
        stale.embedding_model = "old-model".to_string();
        stale.embedding_input_version = "v1-content-only".to_string();
        eng.backend().insert_note(stale);
        let (scanned, _, _) = eng.reembed(10).await.unwrap();
        assert_eq!(scanned, 1, "the stale row must be scanned");

        let seen = kinds.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![EmbedKind::Document, EmbedKind::Query, EmbedKind::Document],
            "remember=Document, recall=Query, reembed=Document"
        );
    }

    /// Document embeds succeed (so seeding via `remember` works); Query embeds
    /// fail, simulating an embedding-API outage at recall time (W1.6d).
    struct QueryFailingProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for QueryFailingProvider {
        fn model_id(&self) -> &str {
            "query-failing"
        }

        fn dim(&self) -> usize {
            16
        }

        async fn embed(
            &self,
            texts: &[String],
            kind: EmbedKind,
        ) -> rb_types::Result<Vec<Vec<f32>>> {
            match kind {
                EmbedKind::Document => Ok(vec![vec![1.0; 16]; texts.len()]),
                EmbedKind::Query => {
                    Err(rb_types::Error::Embedding("embedding API down".to_string()))
                }
            }
        }
    }

    #[tokio::test]
    async fn recall_degrades_to_keyword_and_graph_when_the_embedder_errors() {
        // W1.6d / F19: an embedder error at recall time must DEGRADE retrieval
        // to the keyword + graph channels (flagged), not fail the request.
        let eng = MemoryEngine::new(
            MockBackend::default(),
            QueryFailingProvider,
            Namespace::Project("rb".into()),
        );
        eng.remember(input("sqlite wal checkpoint decision", 5))
            .await
            .unwrap();

        let outcome = eng
            .recall_with_status("sqlite checkpoint", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert!(outcome.degraded, "embedder error must flag degradation");
        assert!(
            !outcome.results.is_empty(),
            "the keyword channel must still serve the stored memory"
        );
        assert!(
            outcome.results.iter().all(|r| !r.channels.vector),
            "no result may claim a vector contribution while degraded"
        );
        assert!(
            outcome.results.iter().any(|r| r.channels.fts),
            "the surviving keyword channel attributes the hits"
        );

        // The thin recall() wrapper serves the same degraded results.
        let results = eng
            .recall("sqlite checkpoint", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), outcome.results.len());
    }

    #[tokio::test]
    async fn recall_with_status_is_not_degraded_under_a_healthy_embedder() {
        let eng = engine();
        eng.remember(input("healthy embedder note", 5))
            .await
            .unwrap();
        let outcome = eng
            .recall_with_status("healthy embedder", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert!(!outcome.degraded, "a healthy embedder never degrades");
        assert!(!outcome.results.is_empty());
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
        let results = eng
            .recall("topic", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        // scores are finite and sorted descending.
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn recall_attributes_each_result_to_its_contributing_channels() {
        // W1.0 per-channel hit attribution: pin each candidate to exactly one
        // channel via the mock's overrides, then assert the returned flags.
        let eng = engine();
        let kw = seed(&eng, "keyword-only candidate", MemoryType::Insight, 5, &[]).await;
        let vec_only = seed(&eng, "vector-only candidate", MemoryType::Insight, 5, &[]).await;
        let graph_only = seed(&eng, "graph-only candidate", MemoryType::Insight, 5, &[]).await;

        // fts surfaces only `kw`; vector surfaces only `vec_only`; the 1-hop
        // graph expansion of the top keyword hit surfaces only `graph_only`.
        eng.backend().set_keyword_results(vec![kw.clone()]);
        eng.backend()
            .set_vector_results(vec![(vec_only.clone(), 0.1)]);
        eng.backend()
            .set_graph_neighbors(kw.clone(), vec![(graph_only.clone(), 1)]);

        let results = eng
            .recall("candidate", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        let channels_of = |id: &rb_types::MemoryId| {
            results
                .iter()
                .find(|r| &r.memory.id == id)
                .map(|r| r.channels)
                .unwrap()
        };

        let kw_channels = channels_of(&kw);
        assert!(kw_channels.fts, "keyword hit must be fts-attributed");
        assert!(!kw_channels.vector);
        // The graph walk is seeded at `kw` and the mock returns only its
        // neighbors, so `kw` itself is not graph-attributed here.
        assert!(!kw_channels.graph);

        let vec_channels = channels_of(&vec_only);
        assert!(vec_channels.vector, "vector hit must be vector-attributed");
        assert!(!vec_channels.fts && !vec_channels.graph);

        let graph_channels = channels_of(&graph_only);
        assert!(
            graph_channels.graph,
            "graph-expanded hit must be graph-attributed"
        );
        assert!(!graph_channels.fts && !graph_channels.vector);
    }

    #[tokio::test]
    async fn recall_attributes_multi_channel_hits_to_every_contributing_channel() {
        // A candidate surfaced by several channels carries every flag —
        // contribution, not exclusivity.
        let eng = engine();
        let id = seed(&eng, "everywhere candidate", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_keyword_results(vec![id.clone()]);
        eng.backend().set_vector_results(vec![(id.clone(), 0.0)]);
        // Graph expansion seeds at the top keyword hit (`id`) and the mock
        // returns its neighbor list, which includes the seed here.
        eng.backend()
            .set_graph_neighbors(id.clone(), vec![(id.clone(), 1)]);

        let results = eng
            .recall("everywhere", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        let channels = results[0].channels;
        assert!(channels.fts && channels.vector && channels.graph);
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
        let results = eng
            .recall("doc", 2, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn recall_returns_empty_when_every_candidate_is_below_the_floor() {
        // W1.3 / F30: the KNN leg surfaces *something* by construction even for
        // an unrelated query. Candidates whose only signal is priors (orthogonal
        // vector, no keyword/graph hit) score <= 0.15 < SCORE_FLOOR and must be
        // dropped — recall returns NOTHING instead of padding with junk.
        let eng = engine();
        let a = seed(&eng, "alpha unrelated note", MemoryType::Insight, 5, &[]).await;
        let b = seed(&eng, "beta unrelated note", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_keyword_results(vec![]);
        // Cosine distance 1.0 = orthogonal = zero vector signal (W1.1 scale).
        eng.backend()
            .set_vector_results(vec![(a.clone(), 1.0), (b.clone(), 1.0)]);
        let results = eng
            .recall("unrelated query", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert!(
            results.is_empty(),
            "prior-only candidates must not be returned, got {} results",
            results.len()
        );
    }

    #[tokio::test]
    async fn recall_returns_fewer_than_limit_keeping_only_above_floor_results() {
        let eng = engine();
        let strong = seed(&eng, "strong vector match", MemoryType::Insight, 5, &[]).await;
        let junk = seed(&eng, "junk far candidate", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_keyword_results(vec![]);
        eng.backend()
            .set_vector_results(vec![(strong.clone(), 0.0), (junk.clone(), 1.0)]);
        let results = eng
            .recall("query", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "only the above-floor result is returned (fewer than limit)"
        );
        assert_eq!(results[0].memory.id, strong);
        assert!(results[0].score >= eng.score_floor());
    }

    #[tokio::test]
    async fn recall_score_floor_override_disables_the_floor() {
        // with_score_floor(0.0) restores pad-to-limit behavior: the same
        // prior-only candidates ARE returned, proving the default floor (and
        // nothing else) is what drops them.
        let eng = engine().with_score_floor(0.0);
        let a = seed(&eng, "alpha unrelated note", MemoryType::Insight, 5, &[]).await;
        let b = seed(&eng, "beta unrelated note", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_keyword_results(vec![]);
        eng.backend()
            .set_vector_results(vec![(a.clone(), 1.0), (b.clone(), 1.0)]);
        let results = eng
            .recall("unrelated query", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2, "floor disabled -> junk returned again");
    }

    #[tokio::test]
    async fn recall_type_filter_excludes_other_types() {
        let eng = engine();
        seed(&eng, "a bug fix note", MemoryType::BugFix, 5, &[]).await;
        seed(&eng, "an insight note", MemoryType::Insight, 5, &[]).await;
        let results = eng
            .recall(
                "note",
                10,
                &rb_types::RecallFilter {
                    types: vec![MemoryType::BugFix],
                    ..Default::default()
                },
            )
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
            .recall(
                "tagged",
                10,
                &rb_types::RecallFilter {
                    tags: vec!["x".to_string(), "y".to_string()],
                    ..Default::default()
                },
            )
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
        // Distance 0.5 (cosine sim 0.5): real-but-modest vector signal, so the
        // matching candidates clear the W1.3 score floor while still carrying
        // no keyword/graph signal — the test exercises filter-before-rank, not
        // prior-only junk (which the floor now drops by design).
        eng.backend()
            .set_vector_results(matching_ids.iter().cloned().map(|id| (id, 0.5)).collect());

        let results = eng
            .recall(
                "candidate",
                3,
                &rb_types::RecallFilter {
                    types: vec![MemoryType::BugFix],
                    tags: vec!["keep".to_string()],
                    ..Default::default()
                },
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
        let results = eng
            .recall("ranking probe", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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
        let results = eng
            .recall("topic", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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

        let results = eng
            .recall("alpha", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        // both endpoints of the active contradicts link are contested.
        assert!(results.iter().all(|r| r.memory.contested));
    }

    #[tokio::test]
    async fn recall_does_not_flag_uncontested_memories() {
        let eng = engine();
        seed(&eng, "uncontested note one", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "uncontested note two", MemoryType::Insight, 5, &[]).await;
        let results = eng
            .recall("note", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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

        let results = eng
            .recall("claim", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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

        let results = eng
            .recall("probe", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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
        let notes = eng
            .list(&rb_types::RecallFilter::default(), 10)
            .await
            .unwrap();
        assert!(notes.iter().find(|n| n.id == a).unwrap().contested);
        assert!(notes.iter().find(|n| n.id == b).unwrap().contested);
    }

    #[tokio::test]
    async fn recall_empty_store_returns_empty() {
        let eng = engine();
        let results = eng
            .recall("anything", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn recall_bumps_access_count_on_returned_results() {
        let eng = engine();
        seed(&eng, "alpha sqlite topic", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "beta tokio topic", MemoryType::Insight, 5, &[]).await;
        let results = eng
            .recall("topic", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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
        let results = eng
            .recall("probe", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();
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
        let all = eng
            .list(&rb_types::RecallFilter::default(), 10)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        // most recent first (second was inserted last).
        assert_eq!(all[0].content, "second");
        let important = eng
            .list(
                &rb_types::RecallFilter {
                    min_importance: Some(8),
                    ..Default::default()
                },
                10,
            )
            .await
            .unwrap();
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
    async fn update_sets_confidence_then_get_reflects_change() {
        // W2.2: confidence is settable through the user-facing update path.
        let eng = engine();
        let id = eng.remember(input("trusted at first", 5)).await.unwrap();
        eng.update(
            id.clone(),
            rb_types::MemoryUpdates {
                confidence: Some(0.3),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let note = eng.get(id).await.unwrap().unwrap();
        assert!((note.confidence - 0.3).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn update_rejects_out_of_range_confidence() {
        let eng = engine();
        let id = eng.remember(input("valid", 5)).await.unwrap();
        for bad in [-0.1f32, 1.5, f32::NAN] {
            let err = eng
                .update(
                    id.clone(),
                    rb_types::MemoryUpdates {
                        confidence: Some(bad),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, rb_types::Error::InvalidArgument(_)),
                "confidence {bad} must be InvalidArgument, got {err:?}"
            );
            assert!(err.to_string().contains("confidence"), "got {err}");
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
    async fn link_contradicts_makes_both_sides_contested_on_get() {
        // W2.2 e2e: the user-facing link op is the contradicts producer; the
        // existing read-side annotation must then surface contested on BOTH
        // endpoints.
        let eng = engine();
        let a = eng.remember(input("use tabs", 5)).await.unwrap();
        let b = eng.remember(input("use spaces", 5)).await.unwrap();
        eng.link(
            a.clone(),
            b.clone(),
            rb_types::LinkType::Contradicts,
            Some("team flip-flopped".to_string()),
        )
        .await
        .unwrap();
        let got_a = eng.get(a).await.unwrap().unwrap();
        let got_b = eng.get(b).await.unwrap().unwrap();
        assert!(got_a.contested, "source of a contradicts link is contested");
        assert!(got_b.contested, "target of a contradicts link is contested");
    }

    #[tokio::test]
    async fn link_rejects_self_supersedes_missing_and_duplicate() {
        let eng = engine();
        let a = eng.remember(input("a", 5)).await.unwrap();
        let b = eng.remember(input("b", 5)).await.unwrap();

        let err = eng
            .link(a.clone(), a.clone(), rb_types::LinkType::References, None)
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)), "{err}");

        let err = eng
            .link(a.clone(), b.clone(), rb_types::LinkType::Supersedes, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("supersedes"),
            "supersedes must be rejected with guidance: {err}"
        );

        let missing = MemoryId::new();
        let err = eng
            .link(
                a.clone(),
                missing.clone(),
                rb_types::LinkType::References,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err}");

        eng.link(a.clone(), b.clone(), rb_types::LinkType::References, None)
            .await
            .unwrap();
        let err = eng
            .link(a, b, rb_types::LinkType::References, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already exists"),
            "duplicate edge must be a clean validation error: {err}"
        );
    }

    #[tokio::test]
    async fn link_does_not_cross_namespaces() {
        // Both endpoints must be visible in the caller's namespace; a foreign
        // memory is NotFound, not linkable.
        let eng = engine();
        let local = eng.remember(input("local", 5)).await.unwrap();
        let foreign = note(
            Namespace::Project("other".into()),
            "foreign",
            MemoryType::Insight,
            5,
            &[],
        );
        let foreign_id = foreign.id.clone();
        eng.backend().insert_note(foreign);
        let err = eng
            .link(local, foreign_id, rb_types::LinkType::Contradicts, None)
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn feedback_nudges_confidence_for_a_known_memory() {
        let eng = engine();
        let id = eng.remember(input("decision", 5)).await.unwrap();
        // A fresh MCP/CLI write starts at the full-trust 1.0 baseline.
        let before = eng.get(id.clone()).await.unwrap().unwrap().confidence;
        assert!((before - 1.0).abs() < 1e-6);

        let after = eng
            .feedback(
                id.clone(),
                rb_types::FeedbackKind::Wrong,
                &Provenance::default(),
            )
            .await
            .unwrap();
        assert!(
            (after - 0.70).abs() < 1e-6,
            "1.0 - 0.30 = 0.70, got {after}"
        );
        let stored = eng.get(id.clone()).await.unwrap().unwrap().confidence;
        assert!((stored - 0.70).abs() < 1e-6, "get reflects the nudge");

        let after2 = eng
            .feedback(id, rb_types::FeedbackKind::Helpful, &Provenance::default())
            .await
            .unwrap();
        assert!(
            (after2 - 0.75).abs() < 1e-6,
            "0.70 + 0.05 = 0.75, got {after2}"
        );
    }

    #[tokio::test]
    async fn feedback_rejects_unknown_and_cross_namespace() {
        let eng = engine();
        // Missing id => NotFound.
        let err = eng
            .feedback(
                MemoryId::new(),
                rb_types::FeedbackKind::Helpful,
                &Provenance::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err}");

        // A memory in another namespace is invisible to this engine => NotFound.
        let foreign = note(
            Namespace::Project("other".into()),
            "foreign",
            MemoryType::Insight,
            5,
            &[],
        );
        let foreign_id = foreign.id.clone();
        eng.backend().insert_note(foreign);
        let err = eng
            .feedback(
                foreign_id,
                rb_types::FeedbackKind::Stale,
                &Provenance::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn feedback_rejects_an_archived_memory() {
        // Feedback is about live, recallable memories; an archived (soft-deleted)
        // target is a caller error => NotFound (the `graph` active-scope rule).
        let eng = engine();
        let id = eng.remember(input("doomed but graded", 5)).await.unwrap();
        eng.delete(id.clone()).await.unwrap();
        let err = eng
            .feedback(id, rb_types::FeedbackKind::Wrong, &Provenance::default())
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "{err}");
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
            vec![
                (cross_id.clone(), 1),
                (archived_id.clone(), 1),
                (active_id.clone(), 2),
            ],
        );

        let neighbors = eng.graph(id, 2).await.unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].id, active_id);

        eng.backend()
            .set_graph_neighbors(cross_id.clone(), vec![(active_id, 1)]);
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
            vec![
                (cross_id.clone(), 1),
                (archived_id.clone(), 1),
                (active_id, 2),
            ],
        );

        let results = eng
            .recall("topic", 10, &rb_types::RecallFilter::default())
            .await
            .unwrap();

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
    async fn remember_applies_enricher_confidence_when_caller_sent_no_prior() {
        // W2.2: the enricher is a trust producer — its declared confidence
        // lands on the note when the caller expressed no prior (confidence
        // None), overriding the 1.0 baseline.
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        let id = eng.remember(input("body", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert!(
            (note.confidence - 0.6).abs() < f32::EPSILON,
            "FixedEnricher declares 0.6, got {}",
            note.confidence
        );
    }

    #[tokio::test]
    async fn remember_keeps_explicit_caller_confidence_over_the_enricher() {
        // An explicit caller prior (hook captures write 0.7) always wins.
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        let mut inp = input("body", 5);
        inp.confidence = Some(0.7);
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert!(
            (note.confidence - 0.7).abs() < f32::EPSILON,
            "caller prior must win, got {}",
            note.confidence
        );
    }

    #[tokio::test]
    async fn remember_keeps_explicit_full_trust_over_the_enricher() {
        // Fix #4: an EXPLICIT 1.0 must NOT be downgraded by the enricher. The
        // old sentinel `(confidence - 1.0).abs() < EPSILON` could not tell an
        // explicit 1.0 from the default and would wrongly apply the enricher's
        // 0.6; with Option, Some(1.0) is explicit and is preserved.
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        let mut inp = input("fully trusted fact", 5);
        inp.confidence = Some(1.0);
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert!(
            (note.confidence - 1.0).abs() < f32::EPSILON,
            "explicit 1.0 must survive enrichment, got {}",
            note.confidence
        );
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
