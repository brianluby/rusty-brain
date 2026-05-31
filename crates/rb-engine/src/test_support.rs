#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::backend::MemoryBackend;
use rb_types::{MemoryId, MemoryNote, MemoryUpdates, Namespace};
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

    async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        Ok(self.notes.lock().unwrap().get(&id).cloned())
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
        ns: Namespace,
        _embedding: Vec<f32>,
        _limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .cloned()
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        Ok(v.into_iter().map(|n| (n.id, 0.0)).collect())
    }

    async fn graph(&self, _id: MemoryId, _depth: u8) -> rb_types::Result<Vec<MemoryId>> {
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

    async fn update(&self, id: MemoryId, updates: MemoryUpdates) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
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
        Ok(())
    }

    async fn archive(&self, id: MemoryId) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        note.archived_at = Some(chrono::Utc::now());
        Ok(())
    }
}
