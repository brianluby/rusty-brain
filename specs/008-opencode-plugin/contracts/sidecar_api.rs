// Contract: Sidecar Module API
// Feature: 008-opencode-plugin
// Date: 2026-03-03
//
// Public API for the sidecar module (crates/opencode/src/sidecar.rs).
// Handles session state persistence, LRU dedup cache management,
// hash computation, and orphaned file cleanup.

use std::path::Path;
use std::time::Duration;

use crate::types::SidecarState;
use types::RustyBrainError;

// ---------------------------------------------------------------------------
// File Operations (SEC-2, SEC-11)
// ---------------------------------------------------------------------------

/// Load sidecar state from a JSON file.
///
/// Returns `Ok(state)` if file exists and deserializes successfully.
/// Returns `Err` if file doesn't exist or is corrupt.
///
/// On corrupt file: caller should delete and create fresh state (WARN trace).
pub fn load(path: &Path) -> Result<SidecarState, RustyBrainError>;

/// Save sidecar state to a JSON file using atomic write.
///
/// 1. Serializes state to JSON
/// 2. Writes to a temp file in the same directory
/// 3. Renames temp file to target path (atomic on POSIX)
/// 4. Sets file permissions to 0600 (SEC-2)
///
/// Creates parent directory (`.opencode/`) if it doesn't exist.
pub fn save(path: &Path, state: &SidecarState) -> Result<(), RustyBrainError>;

/// Resolve the sidecar file path for a given session.
///
/// Returns: `<cwd>/.opencode/session-<sanitized_id>.json`
///
/// Sanitizes session_id: replaces non-alphanumeric chars (except `-`, `_`)
/// with `-` to prevent path traversal.
pub fn sidecar_path(cwd: &Path, session_id: &str) -> std::path::PathBuf;

// ---------------------------------------------------------------------------
// Dedup Cache Operations (M-4)
// ---------------------------------------------------------------------------

/// Check if a dedup hash already exists in the sidecar state.
///
/// Returns `true` if the hash is already present (duplicate observation).
pub fn is_duplicate(state: &SidecarState, hash: &str) -> bool;

/// Return a new sidecar state with the given dedup hash added (LRU eviction).
///
/// Takes `&SidecarState` and returns a new `SidecarState` (immutable API).
///
/// - If hash already exists: moves it to the end (refreshes LRU position),
///   does NOT increment `observation_count`
/// - If hash is new and cache is at capacity (1024): evicts oldest entry (front of Vec)
/// - Appends hash to the end
/// - Increments `observation_count` only for newly added hashes
/// - Always updates `last_updated`
pub fn with_hash(state: &SidecarState, hash: String) -> SidecarState;

/// Compute a dedup hash from tool name and summary.
///
/// Uses `std::collections::hash_map::DefaultHasher` with tool_name + summary
/// as input. Returns a 16-char hex string.
///
/// Deterministic within a process (sufficient for session-scoped dedup).
pub fn compute_dedup_hash(tool_name: &str, summary: &str) -> String;

// ---------------------------------------------------------------------------
// Orphan Cleanup (S-2, SEC-12)
// ---------------------------------------------------------------------------

/// Scan a directory for stale sidecar files and delete them.
///
/// Scans `sidecar_dir` for files matching pattern `session-*.json`.
/// Deletes files with mtime older than `max_age`.
///
/// - Does NOT recurse into subdirectories (SEC-12)
/// - On any individual file error: logs `tracing::warn!` and continues
/// - Never panics
pub fn cleanup_stale(sidecar_dir: &Path, max_age: Duration);
