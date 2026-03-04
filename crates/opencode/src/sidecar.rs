//! Sidecar file management for session state persistence.
//!
//! Handles session state persistence, LRU dedup cache management,
//! hash computation, and orphaned file cleanup.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::Utc;
use tempfile::NamedTempFile;

use crate::types::{MAX_DEDUP_ENTRIES, SidecarState};

// ---------------------------------------------------------------------------
// File Operations (SEC-2, SEC-11)
// ---------------------------------------------------------------------------

/// Load sidecar state from a JSON file.
///
/// Returns `Ok(state)` if file exists and deserializes successfully.
///
/// # Errors
///
/// Returns `RustyBrainError::FileSystem` if the file cannot be read.
/// Returns `RustyBrainError::Serialization` if deserialization fails.
pub fn load(path: &Path) -> Result<SidecarState, ::types::RustyBrainError> {
    let content =
        std::fs::read_to_string(path).map_err(|e| ::types::RustyBrainError::FileSystem {
            code: ::types::error_codes::E_FS_IO_ERROR,
            message: format!("failed to read sidecar file: {path}", path = path.display()),
            source: Some(e),
        })?;

    serde_json::from_str(&content).map_err(|e| ::types::RustyBrainError::Serialization {
        code: ::types::error_codes::E_SER_DESERIALIZE_FAILED,
        message: format!(
            "failed to deserialize sidecar file: {path}",
            path = path.display()
        ),
        source: Some(e),
    })
}

/// Save sidecar state to a JSON file using atomic write.
///
/// 1. Creates parent directory if needed
/// 2. Serializes state to JSON
/// 3. Writes to a temp file in the same directory
/// 4. Renames temp file to target path (atomic on POSIX)
/// 5. Sets file permissions to 0600 (SEC-2)
///
/// # Errors
///
/// Returns `RustyBrainError` if directory creation, serialization,
/// temp file creation, write, permission setting, or rename fails.
pub fn save(path: &Path, state: &SidecarState) -> Result<(), ::types::RustyBrainError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ::types::RustyBrainError::FileSystem {
            code: ::types::error_codes::E_FS_IO_ERROR,
            message: format!(
                "failed to create sidecar directory: {dir}",
                dir = parent.display()
            ),
            source: Some(e),
        })?;
    }

    let json = serde_json::to_string_pretty(state).map_err(|e| {
        ::types::RustyBrainError::Serialization {
            code: ::types::error_codes::E_SER_SERIALIZE_FAILED,
            message: "failed to serialize sidecar state".to_string(),
            source: Some(e),
        }
    })?;

    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = NamedTempFile::new_in(parent).map_err(|e| ::types::RustyBrainError::FileSystem {
        code: ::types::error_codes::E_FS_IO_ERROR,
        message: format!(
            "failed to create temp file in: {dir}",
            dir = parent.display()
        ),
        source: Some(e),
    })?;

    std::fs::write(temp.path(), json.as_bytes()).map_err(|e| {
        ::types::RustyBrainError::FileSystem {
            code: ::types::error_codes::E_FS_IO_ERROR,
            message: "failed to write temp file".to_string(),
            source: Some(e),
        }
    })?;

    // Set 0600 permissions before rename (SEC-2, unix only)
    #[cfg(unix)]
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        ::types::RustyBrainError::FileSystem {
            code: ::types::error_codes::E_FS_PERMISSION_DENIED,
            message: "failed to set sidecar file permissions".to_string(),
            source: Some(e),
        }
    })?;

    // Atomic rename (POSIX)
    temp.persist(path)
        .map_err(|e| ::types::RustyBrainError::FileSystem {
            code: ::types::error_codes::E_FS_IO_ERROR,
            message: format!(
                "failed to persist sidecar file: {path}",
                path = path.display()
            ),
            source: Some(e.error),
        })?;

    Ok(())
}

/// Resolve the sidecar file path for a given session.
///
/// Returns: `<cwd>/.opencode/session-<sanitized_id>.json`
///
/// Sanitizes `session_id`: replaces non-alphanumeric chars (except `-`, `_`)
/// with `-` to prevent path traversal.
#[must_use]
pub fn sidecar_path(cwd: &Path, session_id: &str) -> PathBuf {
    let sanitized: String = session_id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    cwd.join(".opencode")
        .join(format!("session-{sanitized}.json"))
}

// ---------------------------------------------------------------------------
// Dedup Cache Operations (M-4)
// ---------------------------------------------------------------------------

/// Check if a dedup hash already exists in the sidecar state.
#[must_use]
pub fn is_duplicate(state: &SidecarState, hash: &str) -> bool {
    state.dedup_hashes.iter().any(|h| h == hash)
}

/// Return a new sidecar state with the given dedup hash added (LRU eviction).
///
/// - If hash already exists: moves it to the end (refreshes LRU position),
///   does NOT increment `observation_count`
/// - If hash is new and cache is at capacity (1024): evicts oldest entry (front of Vec)
/// - Appends hash to the end
/// - Increments `observation_count` only for newly added hashes
/// - Always updates `last_updated`.
#[must_use]
pub fn with_hash(state: &SidecarState, hash: String) -> SidecarState {
    let mut new_state = state.clone();

    // If already present, remove it (will re-add at end for LRU refresh)
    let existed = if let Some(pos) = new_state.dedup_hashes.iter().position(|h| *h == hash) {
        new_state.dedup_hashes.remove(pos);
        true
    } else {
        false
    };

    // Evict oldest if at capacity
    if new_state.dedup_hashes.len() >= MAX_DEDUP_ENTRIES {
        new_state.dedup_hashes.remove(0);
    }

    new_state.dedup_hashes.push(hash);
    if !existed {
        new_state.observation_count += 1;
    }
    new_state.last_updated = Utc::now();
    new_state
}

/// Compute a dedup hash from tool name and summary.
///
/// Uses `DefaultHasher` with `tool_name + summary` as input.
/// Returns a 16-char hex string.
#[must_use]
pub fn compute_dedup_hash(tool_name: &str, summary: &str) -> String {
    let mut hasher = DefaultHasher::new();
    tool_name.hash(&mut hasher);
    summary.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

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
pub fn cleanup_stale(sidecar_dir: &Path, max_age: Duration) {
    let entries = match std::fs::read_dir(sidecar_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                error = %e,
                dir = %sidecar_dir.display(),
                "failed to read sidecar directory for cleanup"
            );
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "failed to read directory entry during cleanup");
                continue;
            }
        };

        // Skip directories (no recursion — SEC-12)
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                tracing::warn!(error = %e, "failed to get file type during cleanup");
                continue;
            }
        };
        if file_type.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Only match session-*.json pattern
        if !name_str.starts_with("session-") || !name_str.ends_with(".json") {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    file = %name_str,
                    "failed to get metadata during cleanup"
                );
                continue;
            }
        };

        let modified = match metadata.modified() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    file = %name_str,
                    "failed to get modification time during cleanup"
                );
                continue;
            }
        };

        let Ok(age) = std::time::SystemTime::now().duration_since(modified) else {
            continue; // Clock skew — skip file
        };

        if age > max_age {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::warn!(
                    error = %e,
                    file = %name_str,
                    "failed to delete stale sidecar file"
                );
            }
        }
    }
}
