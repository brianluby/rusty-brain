// Contract: Core Memory Engine API
// Branch: 003-core-memory-engine
// Date: 2026-03-01
//
// This file defines the public and internal API contracts for crates/core.
// It is a REFERENCE — not compiled code. Implementation must match these signatures.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use chrono::{DateTime, Utc};
use rusty_brain_types::{
    InjectedContext, MindConfig, MindStats, Observation, ObservationMetadata,
    ObservationType, RustyBrainError, SessionSummary,
};

// =============================================================================
// PUBLIC API (exported from crates/core)
// =============================================================================

/// Central memory engine. One instance per `.mv2` file.
///
/// `Send + Sync` safe via internal `Mutex<Memvid>`. All mutating operations
/// (`remember`, `save_session_summary`) take `&self` — interior mutability is
/// provided by the `Mutex` wrapping the memvid backend handle. This design
/// allows `Mind` to be shared via `Arc<Mind>` without external synchronization.
pub struct Mind {
    // Internal fields — not part of contract
}

impl Mind {
    /// Open or create a memory file based on config.
    ///
    /// Flow: FileGuard validates → MemvidStore creates/opens → Mind initialized.
    /// On corruption: backup + recreate. On >100MB: reject.
    pub fn open(config: MindConfig) -> Result<Mind, RustyBrainError> { todo!() }

    /// Store an observation. Returns the observation's ULID string.
    ///
    /// Required: obs_type, summary (non-empty), tool_name (non-empty).
    /// Optional: content, metadata.
    /// Auto-generates: ULID id, UTC timestamp.
    /// Side effect: invalidates stats cache, calls memvid commit().
    ///
    /// `&self` works because `Mind` uses interior mutability via an internal
    /// `Mutex<Memvid>` — all backend mutations are serialized through the lock.
    pub fn remember(
        &self,
        obs_type: ObservationType,
        tool_name: &str,
        summary: &str,
        content: Option<&str>,
        metadata: Option<&ObservationMetadata>,
    ) -> Result<String, RustyBrainError> { todo!() }

    /// Search observations by text query. Returns results ranked by relevance.
    ///
    /// Uses memvid `find()` (lexical search). Default limit: 10.
    pub fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MemorySearchResult>, RustyBrainError> { todo!() }

    /// Ask a question against stored observations.
    ///
    /// Returns synthesized answer or "No relevant memories found."
    /// Uses memvid `ask()` (feature-gated behind "lex").
    pub fn ask(
        &self,
        question: &str,
    ) -> Result<String, RustyBrainError> { todo!() }

    /// Assemble session context for agent startup.
    ///
    /// Combines: recent observations (timeline), relevant memories (find),
    /// session summaries (find). Bounded by token budget (chars / 4).
    pub fn get_context(
        &self,
        query: Option<&str>,
    ) -> Result<InjectedContext, RustyBrainError> { todo!() }

    /// Store a session summary as a tagged, searchable observation.
    ///
    /// Returns the observation's ULID string.
    pub fn save_session_summary(
        &self,
        decisions: Vec<String>,
        files_modified: Vec<String>,
        summary: &str,
    ) -> Result<String, RustyBrainError> { todo!() }

    /// Compute memory statistics (cached; invalidated on store operations).
    pub fn stats(&self) -> Result<MindStats, RustyBrainError> { todo!() }

    /// Current session identifier (ULID, consistent across all operations).
    pub fn session_id(&self) -> &str { todo!() }

    /// Resolved path to the `.mv2` memory file.
    pub fn memory_path(&self) -> &Path { todo!() }

    /// Whether the engine has been successfully opened.
    pub fn is_initialized(&self) -> bool { todo!() }
}

/// Search result from `Mind::search`.
pub struct MemorySearchResult {
    pub obs_type: ObservationType,
    pub summary: String,
    pub content_excerpt: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub score: f64,
    pub tool_name: String,
}

/// Singleton access — get or create the shared Mind instance.
pub fn get_mind(config: MindConfig) -> Result<Arc<Mind>, RustyBrainError> { todo!() }

/// Reset singleton (test-only). Drops the existing instance.
pub fn reset_mind() { todo!() }

/// Estimate token count: character_count / 4.
pub fn estimate_tokens(text: &str) -> usize { todo!() }

// =============================================================================
// INTERNAL API — pub(crate) (not visible to consumers)
// =============================================================================

/// Actions recommended by FileGuard after pre-open validation.
pub(crate) enum OpenAction {
    /// No file exists; create new.
    Create,
    /// File exists and passes all guards.
    Open,
}

/// Storage abstraction hiding memvid-core behind a clean boundary.
/// All memvid types stay behind this trait — never cross into public API.
pub(crate) trait MemvidBackend: Send + Sync {
    /// Create a new .mv2 file at path.
    fn create(&self, path: &Path) -> Result<(), RustyBrainError>;

    /// Open an existing .mv2 file at path.
    fn open(&self, path: &Path) -> Result<(), RustyBrainError>;

    /// Store data as a frame. Returns frame ID.
    fn put(
        &self,
        payload: &[u8],
        labels: &[String],
        tags: &[String],
        metadata: &serde_json::Value,
    ) -> Result<u64, RustyBrainError>;

    /// Commit pending writes to disk.
    fn commit(&self) -> Result<(), RustyBrainError>;

    /// Lexical search. Returns matching hits with scores.
    fn find(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, RustyBrainError>;

    /// Question-answering against stored content.
    fn ask(
        &self,
        question: &str,
        limit: usize,
    ) -> Result<String, RustyBrainError>;

    /// Timeline query: recent frames.
    fn timeline(
        &self,
        limit: usize,
        reverse: bool,
    ) -> Result<Vec<TimelineEntry>, RustyBrainError>;

    /// Get full frame metadata by ID.
    fn frame_by_id(
        &self,
        frame_id: u64,
    ) -> Result<FrameInfo, RustyBrainError>;

    /// Get storage statistics.
    fn stats(&self) -> Result<BackendStats, RustyBrainError>;
}

/// Internal search hit from backend.
pub(crate) struct SearchHit {
    pub text: String,
    pub score: f64,
    pub metadata: serde_json::Value,
    pub labels: Vec<String>,
    pub tags: Vec<String>,
}

/// Internal timeline entry from backend.
pub(crate) struct TimelineEntry {
    pub frame_id: u64,
    pub preview: String,
    pub timestamp: Option<i64>,
}

/// Internal frame metadata from backend.
pub(crate) struct FrameInfo {
    pub labels: Vec<String>,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub timestamp: Option<i64>,
}

/// Internal storage statistics from backend.
pub(crate) struct BackendStats {
    pub frame_count: u64,
    pub file_size: u64,
}

// =============================================================================
// FileGuard — pre-open validation
// =============================================================================

pub(crate) mod file_guard {
    use super::*;

    /// Validate a memory file path before attempting to open.
    ///
    /// - Missing file → OpenAction::Create (create parent dirs)
    /// - Existing file > 100MB → Err(FileTooLarge)
    /// - Existing file within size guard → OpenAction::Open
    /// - Path resolves to system location → Err(InvalidPath)
    pub fn validate_and_open(path: &Path) -> Result<OpenAction, RustyBrainError> { todo!() }

    /// Create timestamped backup and prune old backups (max 3).
    ///
    /// Renames `path` to `{path}.backup-{YYYYMMDD-HHMMSS}`.
    /// Deletes oldest backups beyond `max_backups`.
    pub fn backup_and_prune(path: &Path, max_backups: usize) -> Result<(), RustyBrainError> { todo!() }
}

// =============================================================================
// ContextBuilder — token-budgeted assembly
// =============================================================================

pub(crate) mod context_builder {
    use super::*;

    /// Build InjectedContext from backend queries within token budget.
    ///
    /// Steps:
    /// 1. Get recent observations from timeline (max_context_observations)
    /// 2. Enrich with frame_by_id for full metadata
    /// 3. If query provided: get relevant memories from find (max_relevant_memories)
    /// 4. Get session summaries from find (max_session_summaries)
    /// 5. Apply token budget, truncating if needed
    pub fn build(
        backend: &dyn MemvidBackend,
        config: &MindConfig,
        query: Option<&str>,
    ) -> Result<InjectedContext, RustyBrainError> { todo!() }
}
