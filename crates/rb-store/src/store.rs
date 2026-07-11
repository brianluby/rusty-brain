//! `SqliteStore`: the concrete `Store` backed by SQLite + sqlite-vec.

mod core;
mod internal;
mod lifecycle;
mod link_decay;
mod oplog;
mod reembed;
mod rename;
mod scrub;
mod stats;

use rb_types::{
    Error, MemoryId, MemoryLink, MemoryNote, MemoryState, MemoryType, MemoryUpdates, Namespace,
    RecallFilter, Result,
};

pub trait Store {
    fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()>;
    fn get_memory(&self, id: &MemoryId) -> Result<Option<MemoryNote>>;
    fn keyword_search(&self, ns: &Namespace, query: &str, limit: usize) -> Result<Vec<MemoryId>>;
    /// [`Store::keyword_search`] with an explicit archived-state scope (PRD
    /// 2026-07-02 search-filter parity). `keyword_search` is the active-only
    /// convenience; recall with a `state` filter routes here so archived
    /// memories stay reachable through the FTS channel (their vectors are
    /// pruned on archive, so keyword+graph are the only archived channels).
    fn keyword_search_in_state(
        &self,
        ns: &Namespace,
        query: &str,
        limit: usize,
        state: MemoryState,
    ) -> Result<Vec<MemoryId>>;
    fn vector_search(
        &self,
        ns: &Namespace,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>>;
    /// Walk the link graph out from `id` up to `depth` hops, returning each
    /// reachable node ONCE with its MINIMUM hop distance (1 = direct neighbor;
    /// the anchor itself is excluded), ordered by hops ascending then id so the
    /// output is deterministic. (W1.5: real hop distances feed the graph
    /// ranking signal instead of incidental walk-order indices.)
    fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<(MemoryId, u8)>>;
    fn list(
        &self,
        ns: &Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>>;
    /// [`Store::list`] over the unified [`RecallFilter`] (PRD 2026-07-02
    /// search-filter parity): every METADATA dimension — types, tags,
    /// importance/confidence ranges, created-at window, sources, archived
    /// state — is honored in SQL with the same inclusive/any-of/all-of
    /// semantics as [`RecallFilter::matches`], ordered newest first, bounded
    /// by `limit`. Two dimensions are intentionally NOT evaluated here:
    /// `contested` needs the link lookup and is applied by the engine (the
    /// `active_contradicts` single source of truth), and a non-empty `anchors`
    /// filter is rejected fail-closed with `InvalidArgument` until the
    /// typed-code-anchors table lands.
    fn list_filtered(
        &self,
        ns: &Namespace,
        filter: &RecallFilter,
        limit: usize,
    ) -> Result<Vec<MemoryNote>>;
    fn update_memory(&self, id: &MemoryId, updates: &MemoryUpdates) -> Result<()>;
    fn archive_memory(&self, id: &MemoryId) -> Result<()>;
    fn add_link(&self, link: &MemoryLink) -> Result<()>;
    /// Bump `access_count` and stamp `last_accessed_at = now` for `id`.
    /// A missing id is a no-op (best-effort access tracking never errors on absence).
    fn record_access(&self, id: &MemoryId) -> Result<()>;
    /// Bump `access_count` and stamp `last_accessed_at = now` for all `ids` in a
    /// single transaction (one writer round-trip). Missing ids are silently skipped;
    /// duplicate ids are updated once per row (the UPDATE touches each row at most
    /// once regardless of how many times it appears in `ids`). Best-effort: an empty
    /// slice or all-missing ids returns `Ok(())` without touching the DB.
    fn record_accesses(&self, ids: &[MemoryId]) -> Result<()>;
    /// Apply BUFFERED access-tracking bumps in one transaction (W1.8): each
    /// entry adds `count` to `access_count` and advances `last_accessed_at`
    /// monotonically to `last_accessed_at` (an already-newer stamp is kept, so
    /// out-of-order flushes never move the clock backwards). Missing ids are
    /// silently skipped and an empty slice is a no-op (best-effort access
    /// tracking never errors on absence). Under migration 006 these updates
    /// assign no FTS-indexed column, so a flush triggers zero FTS writes.
    fn record_access_bumps(&self, bumps: &[AccessBump]) -> Result<()>;
    /// Mark `old` as superseded by `new` AND archive `old`, in one transaction.
    /// Fails closed (rolls back) if `new` does not exist (FK on `superseded_by`).
    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()>;
    /// Record one usefulness-feedback event for `id` and nudge its `confidence`
    /// (W3.7): in ONE transaction, append a `memory_feedback` row (kind +
    /// `principal` giver), clamp `confidence + kind.confidence_delta()` to
    /// `0.0..=1.0` and UPDATE the row, and append a `memory_oplog` `feedback`
    /// entry — so the event log, the trust prior, and the durable change log can
    /// never disagree. Returns the post-nudge `confidence`. A missing id is
    /// `Error::NotFound` (the daemon already verifies namespace membership, but
    /// this fails closed independently). `updated_at` is intentionally NOT
    /// bumped (like `set_confidence`): feedback is not an authorial content edit.
    fn record_feedback(
        &self,
        id: &MemoryId,
        kind: rb_types::FeedbackKind,
        principal: Option<&str>,
    ) -> Result<f32>;
    /// Fetch all of `ids` that exist AND belong to `ns`, returned in the SAME
    /// order as `ids` (missing/out-of-namespace ids skipped). One query; fixes
    /// the recall N+1. Links are loaded per returned note.
    fn get_many(&self, ns: &Namespace, ids: &[MemoryId]) -> Result<Vec<MemoryNote>>;
    /// For each id in `ids`, return whether it has at least one ACTIVE
    /// `contradicts` link — inbound OR outbound — where BOTH endpoints are active
    /// (`archived_at IS NULL`) AND live in namespace `ns`. One batched query over
    /// `memory_links` (Feature C, spec §9). Requiring the LOCAL endpoint active too
    /// means an archived memory fetched via `get` is never flagged `contested`.
    /// Scoping both endpoints to `ns` preserves namespace isolation: `memory_links` carries
    /// no namespace and `add_link` permits cross-namespace edges, so without this an
    /// out-of-namespace memory could flag a result `contested`. The returned set
    /// contains exactly the ids that are contested; ids absent from the set are not
    /// contested. Read path; used post-ranking to annotate `MemoryNote.contested`.
    fn active_contradicts(
        &self,
        ns: &Namespace,
        ids: &[MemoryId],
    ) -> Result<std::collections::HashSet<MemoryId>>;
    /// Read up to `limit` ACTIVE memories (across ALL namespaces) whose stamped
    /// `(embedding_model, embedding_input_version)` differs from the current
    /// `(model, input_version)` — the re-embed candidates. Bounded, deterministic
    /// order. Read-only (every mutation goes through the single writer).
    fn memories_for_reembed(
        &self,
        model: &str,
        input_version: &str,
        limit: usize,
    ) -> Result<Vec<MemoryNote>>;
    /// Replace `id`'s stored vector and stamp the row's
    /// `(embedding_model, embedding_input_version)` to the current pair, in one
    /// transaction. This is the ONLY vector-UPDATE path (insert is write-once).
    /// A missing OR archived memory id is `Error::NotFound` — an archived row
    /// must never get its vector resurrected (live-only vec0 partition
    /// invariant; the reembed loop treats it as a per-row skip). The embedding
    /// dimension is validated fail-closed before the write.
    fn update_vector(
        &self,
        id: &MemoryId,
        embedding: &[f32],
        model: &str,
        input_version: &str,
    ) -> Result<()>;
}

/// SQLite-backed store. Owns a single connection (write path); the daemon owns
/// the read pool separately in P1.
pub struct SqliteStore {
    // rusqlite::Connection doesn't impl Debug, so we derive it manually via a wrapper.
    // pub(crate) for intra-crate access (CRUD methods consume it).
    pub(crate) conn: rusqlite::Connection,
    /// Embedding dimension configured at open; used to fail-close dimension mismatches
    /// before touching the DB.
    pub(crate) embedding_dim: usize,
    /// This database's `meta.site_id` (uuid v4, seeded at init), stamped on every
    /// oplog row so logs from multiple machines stay distinguishable after a merge.
    pub(crate) site_id: String,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

/// One buffered access-tracking bump (W1.8): `count` recorded accesses for
/// `id`, the most recent at `last_accessed_at` (unix seconds). The daemon
/// accumulates these off the recall path and flushes them in batches through
/// [`Store::record_access_bumps`], so recall itself issues zero writer ops.
#[derive(Clone, Debug)]
pub struct AccessBump {
    pub id: MemoryId,
    pub count: u64,
    pub last_accessed_at: i64,
}

/// A minimal projection of a memory row for the consolidation scan: only the
/// fields the job and its survivor policy need. Avoids loading full notes/links.
#[derive(Clone, Debug)]
pub struct ConsolidationCandidate {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub use core::RecalRow;
pub use link_decay::LinkRow;
pub use oplog::OplogReplayPage;
pub use rename::NamespaceRenameOutcome;
pub use scrub::ScrubOutcome;
pub use stats::read_meta_embedding_model;
