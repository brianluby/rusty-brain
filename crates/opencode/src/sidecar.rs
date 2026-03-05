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
    let content = std::fs::read_to_string(path).map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            ::types::error_codes::E_FS_NOT_FOUND
        } else {
            ::types::error_codes::E_FS_IO_ERROR
        };
        ::types::RustyBrainError::FileSystem {
            code,
            message: format!("failed to read sidecar file: {path}", path = path.display()),
            source: Some(e),
        }
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
        let code = match e.kind() {
            std::io::ErrorKind::PermissionDenied => ::types::error_codes::E_FS_PERMISSION_DENIED,
            _ => ::types::error_codes::E_FS_IO_ERROR,
        };
        ::types::RustyBrainError::FileSystem {
            code,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SidecarState;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // sidecar_path
    // -----------------------------------------------------------------------

    #[test]
    fn sidecar_path_produces_expected_format() {
        let cwd = Path::new("/tmp/project");
        let path = sidecar_path(cwd, "abc-123");
        assert_eq!(
            path,
            PathBuf::from("/tmp/project/.opencode/session-abc-123.json")
        );
    }

    #[test]
    fn sidecar_path_sanitizes_special_characters() {
        let cwd = Path::new("/tmp/project");
        let path = sidecar_path(cwd, "../../etc/passwd");
        let filename = path.file_name().unwrap().to_string_lossy();
        assert!(!filename.contains(".."));
        assert!(!filename.contains('/'));
        assert!(filename.starts_with("session-"));
        assert!(filename.ends_with(".json"));
    }

    #[test]
    fn sidecar_path_allows_hyphens_and_underscores() {
        let cwd = Path::new("/tmp/project");
        let path = sidecar_path(cwd, "my_session-01");
        assert_eq!(
            path,
            PathBuf::from("/tmp/project/.opencode/session-my_session-01.json")
        );
    }

    // -----------------------------------------------------------------------
    // save and load round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session-test.json");
        let state = SidecarState::new("test-session".to_string());

        save(&path, &state).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded.session_id, "test-session");
        assert_eq!(loaded.observation_count, 0);
    }

    #[test]
    fn save_creates_parent_directories() {
        let tmp = TempDir::new().unwrap();
        let path = tmp
            .path()
            .join("nested")
            .join("dir")
            .join("session-test.json");
        let state = SidecarState::new("nested-test".to_string());

        save(&path, &state).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn load_returns_error_for_missing_file() {
        let path = Path::new("/tmp/nonexistent-sidecar-file-12345.json");
        let result = load(path);
        assert!(result.is_err());
        match result.unwrap_err() {
            ::types::RustyBrainError::FileSystem { code, .. } => {
                assert_eq!(code, ::types::error_codes::E_FS_NOT_FOUND);
            }
            other => panic!("expected FileSystem error, got: {other:?}"),
        }
    }

    #[test]
    fn load_returns_error_for_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session-bad.json");
        std::fs::write(&path, "not valid json").unwrap();

        let result = load(&path);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ::types::RustyBrainError::Serialization { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session-perms.json");
        let state = SidecarState::new("perms-test".to_string());

        save(&path, &state).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // -----------------------------------------------------------------------
    // compute_dedup_hash
    // -----------------------------------------------------------------------

    #[test]
    fn compute_dedup_hash_is_deterministic() {
        let h1 = compute_dedup_hash("Write", "created file test.txt");
        let h2 = compute_dedup_hash("Write", "created file test.txt");
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_dedup_hash_differs_for_different_inputs() {
        let h1 = compute_dedup_hash("Write", "file a");
        let h2 = compute_dedup_hash("Write", "file b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_dedup_hash_is_16_chars_hex() {
        let hash = compute_dedup_hash("Read", "content");
        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // -----------------------------------------------------------------------
    // is_duplicate
    // -----------------------------------------------------------------------

    #[test]
    fn is_duplicate_returns_false_for_empty_state() {
        let state = SidecarState::new("s1".to_string());
        assert!(!is_duplicate(&state, "abc123"));
    }

    #[test]
    fn is_duplicate_returns_true_for_existing_hash() {
        let mut state = SidecarState::new("s1".to_string());
        state.dedup_hashes.push("abc123".to_string());
        assert!(is_duplicate(&state, "abc123"));
    }

    // -----------------------------------------------------------------------
    // with_hash
    // -----------------------------------------------------------------------

    #[test]
    fn with_hash_adds_new_hash_and_increments_count() {
        let state = SidecarState::new("s1".to_string());
        let updated = with_hash(&state, "hash1".to_string());

        assert_eq!(updated.dedup_hashes.len(), 1);
        assert_eq!(updated.observation_count, 1);
        assert!(updated.dedup_hashes.contains(&"hash1".to_string()));
    }

    #[test]
    fn with_hash_does_not_increment_count_for_existing_hash() {
        let state = SidecarState::new("s1".to_string());
        let state = with_hash(&state, "hash1".to_string());
        assert_eq!(state.observation_count, 1);

        let state = with_hash(&state, "hash1".to_string());
        assert_eq!(state.observation_count, 1);
        assert_eq!(state.dedup_hashes.len(), 1);
    }

    #[test]
    fn with_hash_refreshes_lru_position_for_existing_hash() {
        let state = SidecarState::new("s1".to_string());
        let state = with_hash(&state, "a".to_string());
        let state = with_hash(&state, "b".to_string());
        let state = with_hash(&state, "a".to_string()); // refresh "a"

        assert_eq!(state.dedup_hashes, vec!["b", "a"]);
    }

    #[test]
    fn with_hash_evicts_oldest_when_at_capacity() {
        let mut state = SidecarState::new("s1".to_string());
        for i in 0..MAX_DEDUP_ENTRIES {
            state.dedup_hashes.push(format!("{i:016x}"));
        }
        state.observation_count = u32::try_from(MAX_DEDUP_ENTRIES).unwrap();
        assert_eq!(state.dedup_hashes.len(), MAX_DEDUP_ENTRIES);

        let updated = with_hash(&state, "new_hash".to_string());
        assert_eq!(updated.dedup_hashes.len(), MAX_DEDUP_ENTRIES);
        assert!(!updated.dedup_hashes.contains(&format!("{:016x}", 0)));
        assert!(updated.dedup_hashes.last().unwrap() == "new_hash");
    }

    #[test]
    fn with_hash_returns_new_state_without_mutating_original() {
        let original = SidecarState::new("s1".to_string());
        let _updated = with_hash(&original, "hash1".to_string());
        assert!(original.dedup_hashes.is_empty());
        assert_eq!(original.observation_count, 0);
    }

    // -----------------------------------------------------------------------
    // cleanup_stale
    // -----------------------------------------------------------------------

    #[test]
    fn cleanup_stale_removes_old_sidecar_files() {
        let tmp = TempDir::new().unwrap();
        let old_file = tmp.path().join("session-old.json");
        std::fs::write(&old_file, "{}").unwrap();

        // Ensure file age exceeds max_age (age > max_age gates deletion).
        std::thread::sleep(Duration::from_millis(2));
        cleanup_stale(tmp.path(), Duration::from_millis(1));

        assert!(!old_file.exists());
    }

    #[test]
    fn cleanup_stale_skips_non_session_files() {
        let tmp = TempDir::new().unwrap();
        let keep = tmp.path().join("config.json");
        std::fs::write(&keep, "{}").unwrap();

        cleanup_stale(tmp.path(), Duration::from_secs(0));

        assert!(keep.exists());
    }

    #[test]
    fn cleanup_stale_does_not_panic_on_missing_dir() {
        cleanup_stale(
            Path::new("/tmp/nonexistent-cleanup-dir-12345"),
            Duration::from_secs(0),
        );
    }

    #[test]
    fn cleanup_stale_preserves_recent_files() {
        let tmp = TempDir::new().unwrap();
        let recent = tmp.path().join("session-recent.json");
        std::fs::write(&recent, "{}").unwrap();

        // Max age of 1 hour -- the just-created file should survive
        cleanup_stale(tmp.path(), Duration::from_secs(3600));

        assert!(recent.exists());
    }
}
