#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::DeterministicProvider;
use rb_engine::{MemoryBackend, MemoryEngine, RememberInput};
use rb_types::{
    MemoryId, MemoryNote, MemoryState, MemoryType, MemoryUpdates, Namespace, RecallFilter,
};
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
        limit: usize,
        state: MemoryState,
    ) -> rb_types::Result<Vec<MemoryId>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .filter(|n| state.admits_archived(n.archived_at.is_some()))
            .cloned()
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.created_at));
        Ok(v.into_iter().take(limit).map(|n| n.id).collect())
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
    ) -> rb_types::Result<Vec<(MemoryId, u8)>> {
        Ok(Vec::new())
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
            .filter(|n| filter.matches(n))
            .cloned()
            .collect();
        // Trait contract: contested resolves before the limit via this
        // backend's own active_contradicts (which returns the empty set here,
        // so contested=true yields nothing and contested=false everything).
        if let Some(want) = filter.contested {
            let ids: Vec<MemoryId> = v.iter().map(|n| n.id.clone()).collect();
            let contested = self.active_contradicts(ns, ids).await?;
            v.retain(|n| contested.contains(&n.id) == want);
        }
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

    async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&link.source_id)
            .ok_or_else(|| rb_types::Error::NotFound(link.source_id.clone()))?;
        note.links.push(link);
        Ok(())
    }

    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()> {
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
            .filter(|n| n.embedding_model != model || n.embedding_input_version != input_version)
            .cloned()
            .collect();
        // Mirror the store's scan order: oldest first, then memory_id ascending —
        // bounded + deterministic (created_at ASC, memory_id ASC), as the contract
        // requires, so a `limit` here selects the same batch as production.
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
        _embedding: Vec<f32>,
        model: String,
        input_version: String,
    ) -> rb_types::Result<()> {
        // Fail closed on a missing id, like SqliteStore::update_vector, so the
        // public-API backend matches the store-backed behavior.
        let mut guard = self.notes.lock().unwrap();
        match guard.get_mut(&id) {
            Some(note) => {
                note.embedding_model = model;
                note.embedding_input_version = input_version;
                Ok(())
            }
            None => Err(rb_types::Error::NotFound(id)),
        }
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
            confidence: Some(1.0),
            provenance: Default::default(),
            anchors: Vec::new(),
        })
        .await
        .unwrap();

    // get reflects the stored, enriched note.
    let note = engine.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(note.memory_type, MemoryType::ArchitectureDecision);
    assert!(!note.keywords.is_empty());
    assert_eq!(note.context, "architecture");

    // recall finds it.
    let results = engine
        .recall("writer", 10, &RecallFilter::default())
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, id);

    // context surfaces it as both recent and important (importance 9 >= 8).
    let (recent, important, total) = engine.context().await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(recent.len(), 1);
    assert_eq!(important.len(), 1);
}
