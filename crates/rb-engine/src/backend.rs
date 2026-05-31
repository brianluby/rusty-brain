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
    async fn update(
        &self,
        ns: Namespace,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()>;
    async fn archive(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<()>;
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
}
