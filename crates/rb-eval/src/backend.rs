//! A `MemoryBackend` bridge over an in-memory `SqliteStore` for the harness.
//!
//! The production daemon bridges the synchronous `rb_store::Store` to the async
//! `MemoryBackend` with a dedicated writer thread + read pool. The eval harness
//! is single-threaded and offline, so it instead guards one `SqliteStore` behind
//! a `Mutex` and runs every store call inline. The *semantics* (namespace
//! scoping, active-only graph expansion) mirror the daemon's `StoreHandle`
//! exactly, so recall behaves as it does in production — only the concurrency
//! plumbing is simplified.
//!
//! This is the real FTS + sqlite-vec + graph path (not a mock map), so the
//! fixtures genuinely exercise keyword, vector, and graph retrieval.

use rb_engine::MemoryBackend;
use rb_store::{SqliteStore, Store};
use rb_types::{MemoryId, MemoryLink, MemoryNote, MemoryUpdates, Namespace};
use std::sync::Mutex;

/// In-memory store wrapped for the engine. Single-threaded by construction.
pub struct SqliteBackend {
    store: Mutex<SqliteStore>,
}

impl SqliteBackend {
    /// Build an ephemeral in-memory store with the given embedding dimension.
    pub fn in_memory(dim: usize) -> rb_types::Result<Self> {
        let store = SqliteStore::open_in_memory(dim)?;
        Ok(Self {
            store: Mutex::new(store),
        })
    }

    fn lock(&self) -> rb_types::Result<std::sync::MutexGuard<'_, SqliteStore>> {
        self.store
            .lock()
            .map_err(|_| rb_types::Error::Storage("rb-eval store mutex poisoned".into()))
    }

    /// Test-harness helper: stamp a memory's `confidence` directly in the store.
    ///
    /// `engine.remember` does not expose a confidence input (it defaults to full
    /// trust), but the confidence DAMPENER is what Feature C's "poison" scenario
    /// exercises. This lets a fixture author a low-confidence wrong memory so the
    /// harness can prove it ranks below an equal-match high-confidence one. Only
    /// used by the offline harness; never on the shipped path.
    pub fn set_confidence(&self, id: &MemoryId, confidence: f32) -> rb_types::Result<()> {
        let store = self.lock()?;
        store.set_confidence(id, confidence)
    }
}

#[async_trait::async_trait]
impl MemoryBackend for SqliteBackend {
    async fn write(&self, note: MemoryNote, embedding: Option<Vec<f32>>) -> rb_types::Result<()> {
        let store = self.lock()?;
        store.insert_memory(&note, embedding.as_deref())
    }

    async fn get(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        let store = self.lock()?;
        Ok(store.get_memory(&id)?.filter(|note| note.namespace == ns))
    }

    async fn keyword(
        &self,
        ns: Namespace,
        query: String,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryId>> {
        let store = self.lock()?;
        store.keyword_search(&ns, &query, limit)
    }

    async fn vector(
        &self,
        ns: Namespace,
        embedding: Vec<f32>,
        limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
        let store = self.lock()?;
        store.vector_search(&ns, &embedding, limit)
    }

    async fn graph(
        &self,
        ns: Namespace,
        id: MemoryId,
        depth: u8,
    ) -> rb_types::Result<Vec<(MemoryId, u8)>> {
        let store = self.lock()?;
        // Mirror StoreHandle::graph: anchor must be active + in namespace; each
        // neighbor is re-checked for namespace + active status, preserving its
        // real minimum hop distance (W1.5) through the filter.
        let Some(anchor) = store.get_memory(&id)? else {
            return Ok(Vec::new());
        };
        if anchor.namespace != ns || anchor.archived_at.is_some() {
            return Ok(Vec::new());
        }
        let pairs = store.graph_neighbors(&id, depth)?;
        let mut filtered = Vec::with_capacity(pairs.len());
        for (graph_id, hops) in pairs {
            let Some(note) = store.get_memory(&graph_id)? else {
                continue;
            };
            if note.namespace == ns && note.archived_at.is_none() {
                filtered.push((graph_id, hops));
            }
        }
        Ok(filtered)
    }

    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let store = self.lock()?;
        store.list(&ns, min_importance, limit)
    }

    async fn update(
        &self,
        ns: Namespace,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()> {
        let store = self.lock()?;
        // Namespace-scope the update like the daemon does.
        match store.get_memory(&id)? {
            Some(note) if note.namespace == ns => store.update_memory(&id, &updates),
            _ => Err(rb_types::Error::NotFound(id)),
        }
    }

    async fn archive(&self, ns: Namespace, id: MemoryId) -> rb_types::Result<()> {
        let store = self.lock()?;
        match store.get_memory(&id)? {
            Some(note) if note.namespace == ns => store.archive_memory(&id),
            _ => Err(rb_types::Error::NotFound(id)),
        }
    }

    async fn add_link(&self, link: MemoryLink) -> rb_types::Result<()> {
        let store = self.lock()?;
        store.add_link(&link)
    }

    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()> {
        let store = self.lock()?;
        store.record_access(&id)
    }

    async fn record_accesses(&self, ids: Vec<MemoryId>) -> rb_types::Result<()> {
        let store = self.lock()?;
        store.record_accesses(&ids)
    }

    async fn get_many(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let store = self.lock()?;
        store.get_many(&ns, &ids)
    }

    async fn active_contradicts(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<std::collections::HashSet<MemoryId>> {
        let store = self.lock()?;
        store.active_contradicts(&ns, &ids)
    }

    async fn memories_for_reembed(
        &self,
        model: String,
        input_version: String,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let store = self.lock()?;
        store.memories_for_reembed(&model, &input_version, limit)
    }

    async fn update_vector(
        &self,
        id: MemoryId,
        embedding: Vec<f32>,
        model: String,
        input_version: String,
    ) -> rb_types::Result<()> {
        let store = self.lock()?;
        store.update_vector(&id, &embedding, &model, &input_version)
    }
}

/// The memory type used by all eval fixtures' namespace and the dim used by the
/// deterministic provider. Centralized so the runner and tests agree.
pub const EVAL_DIM: usize = 32;

/// Stable namespace for the whole eval corpus (single project scope).
pub fn eval_namespace() -> Namespace {
    Namespace::Project("rb-eval".into())
}
