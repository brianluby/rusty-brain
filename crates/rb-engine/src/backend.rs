use rb_types::{MemoryId, MemoryNote, MemoryUpdates, Namespace};

/// Async store-access abstraction the engine is generic over. The daemon
/// implements this on top of the synchronous `rb_store::Store` using a
/// dedicated writer thread plus `spawn_blocking` readers; tests implement it
/// over an in-memory map. The engine never touches a concrete store.
#[async_trait::async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn write(&self, note: MemoryNote, embedding: Option<Vec<f32>>) -> rb_types::Result<()>;
    async fn get(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<Option<MemoryNote>>;
    async fn keyword(
        &self,
        ns: Namespace,
        query: String,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryId>>;
    async fn vector(
        &self,
        ns: Namespace,
        embedding: Vec<f32>,
        limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>>;
    /// Expand the link graph around `id` up to `depth` hops, returning each
    /// reachable id with its MINIMUM hop distance (1 = direct neighbor; the
    /// anchor is excluded), ordered by hops ascending (W1.5). The hop value
    /// feeds the graph ranking signal, so implementors must preserve it when
    /// filtering (namespace/active checks drop pairs, never renumber them).
    async fn graph(
        &self,
        ns: Namespace,
        id: MemoryId,
        depth: u8,
    ) -> rb_types::Result<Vec<(MemoryId, u8)>>;
    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>>;
    /// Apply metadata-only updates. `MemoryEngine::update` rejects content edits
    /// so the vector index cannot drift from stored note content.
    async fn update(
        &self,
        ns: Namespace,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()>;
    async fn archive(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<()>;
    /// Persist a directed link (write path).
    async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()>;
    /// Record a usefulness-feedback event for `id` and nudge its trust prior
    /// (W3.7): the explicit usefulness/correctness signal `access_count` is not
    /// (it counts "returned", not "useful"). `principal` is the giver (the
    /// connection's `origin_user`), recorded for the W5c per-author trust
    /// rollup. Returns `id`'s `confidence` after the bounded, clamped nudge. A
    /// missing or out-of-namespace id is `Error::NotFound`.
    async fn record_feedback(
        &self,
        ns: Namespace,
        id: MemoryId,
        kind: rb_types::FeedbackKind,
        principal: Option<String>,
    ) -> rb_types::Result<f32>;
    /// Bump access metadata for `id` (best-effort at call sites). W1.8:
    /// implementations MAY defer the bump — the daemon buffers it in memory
    /// and flushes batches off the read path, so recall/`get` issue zero
    /// writer-thread ops and access stats are eventually consistent.
    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()>;
    /// Bump access metadata for all `ids` (best-effort at call sites; missing
    /// ids silently skipped, duplicates within one call bump once). W1.8: same
    /// deferral contract as [`MemoryBackend::record_access`] — the daemon
    /// buffers and batch-flushes, so this never costs recall a writer op.
    async fn record_accesses(&self, ids: Vec<MemoryId>) -> rb_types::Result<()>;
    /// Batch-fetch `ids` scoped to `ns`, in request order (read path).
    async fn get_many(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<Vec<MemoryNote>>;
    /// Return the subset of `ids` that have at least one ACTIVE `contradicts`
    /// link (inbound OR outbound) whose far endpoint is active AND where both
    /// endpoints live in namespace `ns` (Feature C, spec §9). Scoping to `ns`
    /// preserves namespace isolation — `memory_links` carries no namespace, so an
    /// out-of-namespace edge must not flag a result `contested`. One batched read
    /// over `memory_links`. Used post-ranking to set `MemoryNote.contested`;
    /// callers treat errors as fail-open (unflagged).
    async fn active_contradicts(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<std::collections::HashSet<MemoryId>>;
    /// Scan up to `limit` ACTIVE memories (across ALL namespaces) whose stamped
    /// `(embedding_model, embedding_input_version)` differs from the current
    /// `(model, input_version)` pair — the re-embed candidates (read path).
    /// Bounded; deterministic order. Used by the `reembed` batch.
    async fn memories_for_reembed(
        &self,
        model: String,
        input_version: String,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>>;
    /// Replace `id`'s stored vector and stamp its row with the current
    /// `(model, input_version)` in one transaction (write path, single writer).
    /// This is the first vector-UPDATE path; `remember` only ever INSERTs.
    async fn update_vector(
        &self,
        id: MemoryId,
        embedding: Vec<f32>,
        model: String,
        input_version: String,
    ) -> rb_types::Result<()>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal in-memory backend used to unit-test the engine in isolation.
    /// NOT backed by rb-store; just a HashMap behind a Mutex.
    #[derive(Default)]
    struct MockBackend {
        notes: Mutex<HashMap<MemoryId, MemoryNote>>,
        embeddings: Mutex<HashMap<MemoryId, Vec<f32>>>,
    }

    #[async_trait::async_trait]
    impl MemoryBackend for MockBackend {
        async fn write(
            &self,
            note: MemoryNote,
            embedding: Option<Vec<f32>>,
        ) -> rb_types::Result<()> {
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
            _ns: Namespace,
            _query: String,
            _limit: usize,
        ) -> rb_types::Result<Vec<MemoryId>> {
            // Deterministic order (created_at desc) so keyword_rank is reproducible.
            let mut notes: Vec<MemoryNote> = self.notes.lock().unwrap().values().cloned().collect();
            notes.sort_by_key(|n| std::cmp::Reverse(n.created_at));
            Ok(notes.into_iter().map(|n| n.id).collect())
        }
        async fn vector(
            &self,
            _ns: Namespace,
            _embedding: Vec<f32>,
            _limit: usize,
        ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
            let mut pairs: Vec<(MemoryId, MemoryNote)> = self
                .embeddings
                .lock()
                .unwrap()
                .keys()
                .filter_map(|id| {
                    self.notes
                        .lock()
                        .unwrap()
                        .get(id)
                        .cloned()
                        .map(|n| (id.clone(), n))
                })
                .collect();
            pairs.sort_by_key(|(_, note)| std::cmp::Reverse(note.created_at));
            Ok(pairs.into_iter().map(|(id, _)| (id, 0.0)).collect())
        }
        async fn graph(
            &self,
            _ns: Namespace,
            _id: MemoryId,
            _depth: u8,
        ) -> rb_types::Result<Vec<(MemoryId, u8)>> {
            Ok(Vec::new())
        }
        async fn list(
            &self,
            _ns: Namespace,
            min_importance: Option<u8>,
            limit: usize,
        ) -> rb_types::Result<Vec<MemoryNote>> {
            let mut v: Vec<MemoryNote> = self
                .notes
                .lock()
                .unwrap()
                .values()
                .filter(|n| min_importance.map(|m| n.importance >= m).unwrap_or(true))
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
            let mut guard = self.notes.lock().unwrap();
            let note = guard
                .get_mut(&link.source_id)
                .ok_or_else(|| rb_types::Error::NotFound(link.source_id.clone()))?;
            note.links.push(link);
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
        async fn record_access(&self, id: MemoryId) -> rb_types::Result<()> {
            let mut guard = self.notes.lock().unwrap();
            if let Some(note) = guard.get_mut(&id) {
                note.access_count += 1;
                note.last_accessed_at = Some(chrono::Utc::now());
            }
            Ok(())
        }
        async fn record_accesses(&self, ids: Vec<MemoryId>) -> rb_types::Result<()> {
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
            _ns: Namespace,
            _ids: Vec<MemoryId>,
        ) -> rb_types::Result<std::collections::HashSet<MemoryId>> {
            Ok(std::collections::HashSet::new())
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
                .filter(|n| {
                    n.embedding_model != model || n.embedding_input_version != input_version
                })
                .cloned()
                .collect();
            v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
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
            // Fail closed on a missing id, like SqliteStore::update_vector.
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

    #[tokio::test]
    async fn mock_backend_round_trips_write_and_get() {
        let backend = MockBackend::default();
        let note = MemoryNote::new(
            Namespace::Global,
            "hello world".to_string(),
            MemoryType::Insight,
            5,
        );
        let id = note.id.clone();
        backend
            .write(note.clone(), Some(vec![0.1, 0.2, 0.3]))
            .await
            .unwrap();
        let got = backend.get(Namespace::Global, id).await.unwrap().unwrap();
        assert_eq!(got.content, "hello world");
    }

    #[tokio::test]
    async fn mock_backend_archive_sets_archived_at() {
        let backend = MockBackend::default();
        let note = MemoryNote::new(Namespace::Global, "x".to_string(), MemoryType::Reference, 3);
        let id = note.id.clone();
        backend.write(note, None).await.unwrap();
        backend
            .archive(Namespace::Global, id.clone())
            .await
            .unwrap();
        assert!(backend
            .get(Namespace::Global, id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_some());
    }

    #[tokio::test]
    async fn mock_backend_supports_links_access_and_batch_fetch() {
        let backend = MockBackend::default();
        let ns = Namespace::Global;
        let a = MemoryNote::new(ns.clone(), "a".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "b".to_string(), MemoryType::Insight, 5);
        let (aid, bid) = (a.id.clone(), b.id.clone());
        backend.write(a, None).await.unwrap();
        backend.write(b, None).await.unwrap();

        // add_link is accepted (stored on the source note).
        backend
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.7,
                reason: "similar".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // record_access bumps the count.
        backend.record_access(aid.clone()).await.unwrap();
        let got = backend.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(got.access_count, 1);
        assert_eq!(got.links.len(), 1);

        // get_many returns ns-scoped notes in request order.
        let many = backend
            .get_many(ns, vec![bid.clone(), aid.clone()])
            .await
            .unwrap();
        let ids: Vec<rb_types::MemoryId> = many.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![bid, aid]);
    }

    #[tokio::test]
    async fn mock_backend_record_accesses_duplicate_ids_bump_once() {
        // Trait contract (`record_accesses` doc): duplicates within one call
        // bump once. Mirrors store.rs's record_accesses_duplicate_ids_bump_once.
        let backend = MockBackend::default();
        let note = MemoryNote::new(Namespace::Global, "dup".to_string(), MemoryType::Insight, 5);
        let id = note.id.clone();
        backend.write(note, None).await.unwrap();
        backend
            .record_accesses(vec![id.clone(), id.clone()])
            .await
            .unwrap();
        let got = backend.get(Namespace::Global, id).await.unwrap().unwrap();
        assert_eq!(
            got.access_count, 1,
            "duplicate ids in one call must not double-bump"
        );
    }
}
