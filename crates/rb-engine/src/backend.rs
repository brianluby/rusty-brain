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
    async fn graph(
        &self,
        ns: Namespace,
        id: MemoryId,
        depth: u8,
    ) -> rb_types::Result<Vec<MemoryId>>;
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
    /// Bump access metadata for `id` (write path; best-effort at call sites).
    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()>;
    /// Bump access metadata for all `ids` in a single writer round-trip
    /// (write path; best-effort at call sites). Missing ids are silently skipped.
    async fn record_accesses(&self, ids: Vec<MemoryId>) -> rb_types::Result<()>;
    /// Batch-fetch `ids` scoped to `ns`, in request order (read path).
    async fn get_many(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<Vec<MemoryNote>>;
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
        ) -> rb_types::Result<Vec<MemoryId>> {
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
            for id in ids {
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
}
