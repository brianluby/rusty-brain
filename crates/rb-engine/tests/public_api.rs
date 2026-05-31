#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::DeterministicProvider;
use rb_engine::{MemoryBackend, MemoryEngine, RememberInput};
use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct VecBackend {
    notes: Mutex<HashMap<MemoryId, MemoryNote>>,
}

#[async_trait::async_trait]
impl MemoryBackend for VecBackend {
    async fn write(&self, note: MemoryNote, _embedding: Option<Vec<f32>>) -> rb_types::Result<()> {
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
        _limit: usize,
    ) -> rb_types::Result<Vec<MemoryId>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .cloned()
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        Ok(v.into_iter().map(|n| n.id).collect())
    }

    async fn vector(
        &self,
        _ns: Namespace,
        _embedding: Vec<f32>,
        _limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
        Ok(Vec::new())
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
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
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
async fn full_flow_through_public_api() {
    let engine = MemoryEngine::new(
        VecBackend::default(),
        DeterministicProvider::new(8),
        Namespace::Project("rb".into()),
    );

    let id = engine
        .remember(RememberInput {
            content: "single writer over sqlite wal with concurrent readers".to_string(),
            context: Some("architecture".to_string()),
            memory_type: MemoryType::ArchitectureDecision,
            importance: 9,
            keywords: Vec::new(),
            tags: vec!["concurrency".to_string()],
            related_files: Vec::new(),
        })
        .await
        .unwrap();

    // get reflects the stored, enriched note.
    let note = engine.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(note.memory_type, MemoryType::ArchitectureDecision);
    assert!(!note.keywords.is_empty());
    assert_eq!(note.context, "architecture");

    // recall finds it.
    let results = engine.recall("writer", 10, None, &[]).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, id);

    // context surfaces it as both recent and important (importance 9 >= 8).
    let (recent, important, total) = engine.context().await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(recent.len(), 1);
    assert_eq!(important.len(), 1);
}
