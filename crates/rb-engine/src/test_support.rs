#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use crate::backend::MemoryBackend;
use crate::enricher::{Enricher, Enrichment};
use rb_types::{MemoryId, MemoryNote, MemoryState, MemoryUpdates, Namespace, RecallFilter};
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory `MemoryBackend` for engine unit tests. Records writes (note +
/// embedding) so tests can assert what `remember` produced. Reads return the
/// stored notes. Keyword/vector return ALL stored ids in a DETERMINISTIC order
/// (created_at desc) so the ranker, not HashMap iteration order, decides
/// result ordering; `graph` returns nothing (graph paths tested separately).
#[derive(Default)]
pub(crate) struct MockBackend {
    pub notes: Mutex<HashMap<MemoryId, MemoryNote>>,
    pub embeddings: Mutex<HashMap<MemoryId, Vec<f32>>>,
    graph: Mutex<HashMap<MemoryId, Vec<(MemoryId, u8)>>>,
    keyword_results: Mutex<Option<Vec<MemoryId>>>,
    vector_results: Mutex<Option<Vec<(MemoryId, f32)>>>,
    record_access_calls: std::sync::atomic::AtomicUsize,
    fail_record_access: std::sync::atomic::AtomicBool,
    fail_add_link: std::sync::atomic::AtomicBool,
    update_vector_calls: std::sync::atomic::AtomicUsize,
    fail_update_vector: std::sync::atomic::AtomicBool,
    fail_contradicts: std::sync::atomic::AtomicBool,
}

impl MockBackend {
    pub fn count(&self) -> usize {
        self.notes.lock().unwrap().len()
    }

    pub fn embedding_of(&self, id: &MemoryId) -> Option<Vec<f32>> {
        self.embeddings.lock().unwrap().get(id).cloned()
    }

    pub fn note_of(&self, id: &MemoryId) -> Option<MemoryNote> {
        self.notes.lock().unwrap().get(id).cloned()
    }

    pub fn insert_note(&self, note: MemoryNote) {
        self.notes.lock().unwrap().insert(note.id.clone(), note);
    }

    /// Stub the graph expansion for `id` as `(neighbor, hops)` pairs, mirroring
    /// the store's real-hop-distance contract (W1.5).
    pub fn set_graph_neighbors(&self, id: MemoryId, neighbors: Vec<(MemoryId, u8)>) {
        self.graph.lock().unwrap().insert(id, neighbors);
    }

    pub fn set_keyword_results(&self, ids: Vec<MemoryId>) {
        *self.keyword_results.lock().unwrap() = Some(ids);
    }

    pub fn set_vector_results(&self, ids: Vec<(MemoryId, f32)>) {
        *self.vector_results.lock().unwrap() = Some(ids);
    }

    pub fn record_access_count(&self) -> usize {
        self.record_access_calls
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn set_fail_record_access(&self, fail: bool) {
        self.fail_record_access
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn set_fail_add_link(&self, fail: bool) {
        self.fail_add_link
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn links_of(&self, id: &MemoryId) -> Vec<rb_types::MemoryLink> {
        self.notes
            .lock()
            .unwrap()
            .get(id)
            .map(|n| n.links.clone())
            .unwrap_or_default()
    }
    pub fn update_vector_count(&self) -> usize {
        self.update_vector_calls
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn set_fail_update_vector(&self, fail: bool) {
        self.fail_update_vector
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn set_fail_contradicts(&self, fail: bool) {
        self.fail_contradicts
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    /// Record a directed contradicts link on the source note (test helper).
    pub fn link_contradicts(&self, source: &MemoryId, target: &MemoryId) {
        let mut guard = self.notes.lock().unwrap();
        if let Some(note) = guard.get_mut(source) {
            note.links.push(rb_types::MemoryLink {
                source_id: source.clone(),
                target_id: target.clone(),
                link_type: rb_types::LinkType::Contradicts,
                strength: 1.0,
                reason: "test contradiction".to_string(),
                created_at: chrono::Utc::now(),
            });
        }
    }
}

#[async_trait::async_trait]
impl MemoryBackend for MockBackend {
    async fn write(&self, note: MemoryNote, embedding: Option<Vec<f32>>) -> rb_types::Result<()> {
        if let Some(emb) = embedding {
            self.embeddings.lock().unwrap().insert(note.id.clone(), emb);
        }
        self.notes.lock().unwrap().insert(note.id.clone(), note);
        Ok(())
    }

    async fn get(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        Ok(self
            .notes
            .lock()
            .unwrap()
            .get(&id)
            .filter(|note| note.namespace == ns)
            .cloned())
    }

    async fn keyword(
        &self,
        ns: Namespace,
        _query: String,
        limit: usize,
        state: MemoryState,
    ) -> rb_types::Result<Vec<MemoryId>> {
        if let Some(ids) = self.keyword_results.lock().unwrap().clone() {
            return Ok(ids.into_iter().take(limit).collect());
        }
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            // Mirror the store: the FTS channel scopes on archived state.
            .filter(|n| state.admits_archived(n.archived_at.is_some()))
            .cloned()
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        Ok(v.into_iter().take(limit).map(|n| n.id).collect())
    }

    async fn vector(
        &self,
        ns: Namespace,
        _embedding: Vec<f32>,
        limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
        if let Some(ids) = self.vector_results.lock().unwrap().clone() {
            return Ok(ids.into_iter().take(limit).collect());
        }
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .cloned()
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        Ok(v.into_iter().take(limit).map(|n| (n.id, 0.0)).collect())
    }

    async fn graph(
        &self,
        ns: Namespace,
        id: MemoryId,
        _depth: u8,
    ) -> rb_types::Result<Vec<(MemoryId, u8)>> {
        let has_anchor = self
            .notes
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|note| note.namespace == ns);
        if !has_anchor {
            return Ok(Vec::new());
        }
        Ok(self
            .graph
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default())
    }

    async fn list(
        &self,
        ns: Namespace,
        filter: RecallFilter,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            // Mirror the store: every metadata dimension via the ONE predicate.
            .filter(|n| filter.matches(n))
            .cloned()
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        v.truncate(limit);
        Ok(v)
    }

    async fn update(
        &self,
        ns: Namespace,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .filter(|note| note.namespace == ns)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        if let Some(c) = updates.content {
            note.content = c;
        }
        if let Some(s) = updates.summary {
            note.summary = s;
        }
        if let Some(i) = updates.importance {
            note.importance = i;
        }
        if let Some(t) = updates.tags {
            note.tags = t;
        }
        if let Some(ctx) = updates.context {
            note.context = ctx;
        }
        if let Some(conf) = updates.confidence {
            note.confidence = conf;
        }
        Ok(())
    }

    async fn archive(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .filter(|note| note.namespace == ns)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        note.archived_at = Some(chrono::Utc::now());
        Ok(())
    }

    async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()> {
        if self.fail_add_link.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(rb_types::Error::Storage(
                "add_link forced failure".to_string(),
            ));
        }
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&link.source_id)
            .ok_or_else(|| rb_types::Error::NotFound(link.source_id.clone()))?;
        note.links.push(link);
        Ok(())
    }

    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()> {
        use std::sync::atomic::Ordering;
        self.record_access_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_record_access.load(Ordering::SeqCst) {
            return Err(rb_types::Error::Storage(
                "record_access forced failure".to_string(),
            ));
        }
        if let Some(note) = self.notes.lock().unwrap().get_mut(&id) {
            note.access_count += 1;
            note.last_accessed_at = Some(chrono::Utc::now());
        }
        Ok(())
    }

    async fn record_feedback(
        &self,
        ns: Namespace,
        id: MemoryId,
        kind: rb_types::FeedbackKind,
        _principal: Option<String>,
    ) -> rb_types::Result<f32> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .filter(|note| note.namespace == ns)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        note.confidence = (note.confidence + kind.confidence_delta()).clamp(0.0, 1.0);
        Ok(note.confidence)
    }

    async fn record_accesses(&self, ids: Vec<MemoryId>) -> rb_types::Result<()> {
        use std::sync::atomic::Ordering;
        self.record_access_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_record_access.load(Ordering::SeqCst) {
            return Err(rb_types::Error::Storage(
                "record_accesses forced failure".to_string(),
            ));
        }
        let mut guard = self.notes.lock().unwrap();
        // Trait contract: duplicates within one call bump once (mirrors
        // StoreHandle::buffer_accesses and the store's SQL IN-list dedup).
        let mut seen = std::collections::HashSet::with_capacity(ids.len());
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(note) = guard.get_mut(&id) {
                note.access_count += 1;
                note.last_accessed_at = Some(chrono::Utc::now());
            }
        }
        Ok(())
    }

    async fn get_many(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let guard = self.notes.lock().unwrap();
        Ok(ids
            .iter()
            .filter_map(|id| guard.get(id).filter(|n| n.namespace == ns).cloned())
            .collect())
    }

    async fn active_contradicts(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<std::collections::HashSet<MemoryId>> {
        use std::collections::HashSet;
        if self
            .fail_contradicts
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(rb_types::Error::Storage(
                "active_contradicts forced failure".to_string(),
            ));
        }
        let guard = self.notes.lock().unwrap();
        // Mirror the store contract: flag an endpoint only when BOTH endpoints are
        // active AND in the queried namespace. (SqliteStore::active_contradicts
        // requires `archived_at IS NULL` and `namespace = ns` on both the local and
        // far endpoints.) Without gating the local endpoint the mock would flag
        // archived or out-of-namespace ids the store never returns.
        let active = |id: &MemoryId| {
            guard
                .get(id)
                .is_some_and(|n| n.archived_at.is_none() && n.namespace == ns)
        };
        let requested: HashSet<&MemoryId> = ids.iter().collect();
        let mut contested: HashSet<MemoryId> = HashSet::new();
        // Scan every stored note's outbound contradicts links; flag BOTH endpoints
        // when each is requested and both endpoints are active + in namespace
        // (mirrors the store's inbound-OR-outbound, both-active, both-in-ns semantics).
        for note in guard.values() {
            for link in &note.links {
                if link.link_type != rb_types::LinkType::Contradicts {
                    continue;
                }
                let (src, tgt) = (&link.source_id, &link.target_id);
                if requested.contains(src) && active(src) && active(tgt) {
                    contested.insert(src.clone());
                }
                if requested.contains(tgt) && active(tgt) && active(src) {
                    contested.insert(tgt.clone());
                }
            }
        }
        Ok(contested)
    }

    async fn memories_for_reembed(
        &self,
        model: String,
        input_version: String,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.archived_at.is_none())
            .filter(|n| n.embedding_model != model || n.embedding_input_version != input_version)
            .cloned()
            .collect();
        // Mirror the store's scan order: oldest first, then memory_id (the TEXT
        // column) ascending — bounded + deterministic, matching the contract
        // (HashMap iteration order must never decide which rows a `limit` selects).
        v.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });
        v.truncate(limit);
        Ok(v)
    }

    async fn update_vector(
        &self,
        id: MemoryId,
        embedding: Vec<f32>,
        model: String,
        input_version: String,
    ) -> rb_types::Result<()> {
        use std::sync::atomic::Ordering;
        self.update_vector_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_update_vector.load(Ordering::SeqCst) {
            return Err(rb_types::Error::Storage(
                "update_vector forced failure".to_string(),
            ));
        }
        // Fail closed on a missing id, like SqliteStore::update_vector (NotFound),
        // so engine tests cannot pass on a vector-update path production rejects.
        if !self.notes.lock().unwrap().contains_key(&id) {
            return Err(rb_types::Error::NotFound(id));
        }
        self.embeddings
            .lock()
            .unwrap()
            .insert(id.clone(), embedding);
        if let Some(note) = self.notes.lock().unwrap().get_mut(&id) {
            note.embedding_model = model;
            note.embedding_input_version = input_version;
        }
        Ok(())
    }
}

/// In-test enricher returning fixed values so `remember` wiring is assertable
/// without any network. Used to prove the engine fills empty fields from it.
pub(crate) struct FixedEnricher;

#[async_trait::async_trait]
impl Enricher for FixedEnricher {
    async fn enrich(&self, _content: &str, _context: Option<&str>) -> rb_types::Result<Enrichment> {
        Ok(Enrichment {
            summary: Some("enriched summary".to_string()),
            keywords: vec!["enrkw".to_string()],
            tags: vec!["enrtag".to_string()],
            memory_type: None,
            importance: None,
            confidence: Some(0.6),
        })
    }
}

/// In-test enricher that always fails, to prove `remember` falls back cleanly.
pub(crate) struct FailingEnricher;

#[async_trait::async_trait]
impl Enricher for FailingEnricher {
    async fn enrich(&self, _content: &str, _context: Option<&str>) -> rb_types::Result<Enrichment> {
        Err(rb_types::Error::Enrichment("boom".to_string()))
    }
}
