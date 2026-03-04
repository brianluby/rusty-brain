//! Sidecar module unit tests (T004, T017).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use opencode::sidecar;
use opencode::types::{MAX_DEDUP_ENTRIES, SidecarState};

/// Backdate a file's mtime by `seconds_ago` using `touch -t`.
fn backdate_file(path: &Path, seconds_ago: u64) {
    use chrono::{Duration as ChronoDuration, Utc};
    let past = Utc::now() - ChronoDuration::seconds(seconds_ago as i64);
    let stamp = past.format("%Y%m%d%H%M.%S").to_string();
    std::process::Command::new("touch")
        .args(["-t", &stamp, &path.to_string_lossy()])
        .status()
        .expect("touch command failed");
}

// ---------------------------------------------------------------------------
// T004: Core sidecar tests
// ---------------------------------------------------------------------------

#[test]
fn load_save_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session-test.json");

    let state = SidecarState::new("test-session".to_string());
    sidecar::save(&path, &state).unwrap();

    let loaded = sidecar::load(&path).unwrap();
    assert_eq!(loaded.session_id, "test-session");
    assert_eq!(loaded.observation_count, 0);
    assert!(loaded.dedup_hashes.is_empty());
}

#[test]
fn lru_eviction_at_max_boundary() {
    let mut state = SidecarState::new("eviction-test".to_string());

    // Fill to capacity
    for i in 0..MAX_DEDUP_ENTRIES {
        state = sidecar::with_hash(&state, format!("{i:016x}"));
    }
    assert_eq!(state.dedup_hashes.len(), MAX_DEDUP_ENTRIES);

    // One more should evict the oldest (0000000000000000)
    let state = sidecar::with_hash(&state, "new_hash_value_x".to_string());
    assert_eq!(state.dedup_hashes.len(), MAX_DEDUP_ENTRIES);
    assert!(!sidecar::is_duplicate(&state, "0000000000000000"));
    assert!(sidecar::is_duplicate(&state, "new_hash_value_x"));
}

#[test]
fn hash_computation_determinism() {
    let h1 = sidecar::compute_dedup_hash("read", "file contents summary");
    let h2 = sidecar::compute_dedup_hash("read", "file contents summary");
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 16, "hash should be 16-char hex string");

    // Different inputs produce different hashes
    let h3 = sidecar::compute_dedup_hash("write", "file contents summary");
    assert_ne!(h1, h3);
}

#[test]
fn sidecar_path_sanitization() {
    let cwd = Path::new("/tmp/project");

    // Normal session ID
    let p = sidecar::sidecar_path(cwd, "abc-123");
    assert_eq!(p, Path::new("/tmp/project/.opencode/session-abc-123.json"));

    // Path traversal attempt
    let p = sidecar::sidecar_path(cwd, "../../../etc/passwd");
    let name = p.file_name().unwrap().to_string_lossy();
    assert!(
        !name.contains(".."),
        "path traversal should be sanitized: {name}"
    );

    // Special characters replaced
    let p = sidecar::sidecar_path(cwd, "test/id with spaces");
    let name = p.file_name().unwrap().to_string_lossy();
    assert!(!name.contains('/'), "slash should be sanitized: {name}");
    assert!(!name.contains(' '), "spaces should be sanitized: {name}");
}

#[test]
fn atomic_write_creates_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("subdir").join("session-test.json");

    let state = SidecarState::new("nested-test".to_string());
    sidecar::save(&nested, &state).unwrap();

    assert!(nested.exists());
}

#[test]
fn corrupt_file_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session-corrupt.json");

    std::fs::write(&path, "not valid json{{{").unwrap();
    let result = sidecar::load(&path);
    assert!(result.is_err());
}

#[test]
fn is_duplicate_true_false() {
    let state = SidecarState::new("dedup-test".to_string());

    let hash = sidecar::compute_dedup_hash("read", "some content");
    assert!(!sidecar::is_duplicate(&state, &hash));

    let state = sidecar::with_hash(&state, hash.clone());
    assert!(sidecar::is_duplicate(&state, &hash));
}

#[test]
fn with_hash_lru_refresh() {
    let state = SidecarState::new("lru-refresh".to_string());

    let state = sidecar::with_hash(&state, "aaa".to_string());
    let state = sidecar::with_hash(&state, "bbb".to_string());
    let state = sidecar::with_hash(&state, "ccc".to_string());

    // Refresh "aaa" — should move to end
    let state = sidecar::with_hash(&state, "aaa".to_string());

    assert_eq!(state.dedup_hashes, vec!["bbb", "ccc", "aaa"]);
}

#[test]
fn file_permissions_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session-perms.json");

    let state = SidecarState::new("perms-test".to_string());
    sidecar::save(&path, &state).unwrap();

    let metadata = std::fs::metadata(&path).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "sidecar file should have 0600 permissions, got {mode:o}"
    );
}

// ---------------------------------------------------------------------------
// T017: Orphan cleanup tests
// ---------------------------------------------------------------------------

#[test]
fn cleanup_stale_deletes_old_files() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar_dir = dir.path();

    // Create a "stale" file and backdate its mtime via touch command
    let stale_path = sidecar_dir.join("session-old.json");
    std::fs::write(&stale_path, "{}").unwrap();
    backdate_file(&stale_path, 48 * 3600);

    // Create a fresh file
    let fresh_path = sidecar_dir.join("session-fresh.json");
    std::fs::write(&fresh_path, "{}").unwrap();

    sidecar::cleanup_stale(sidecar_dir, Duration::from_secs(24 * 3600));

    assert!(!stale_path.exists(), "stale file should be deleted");
    assert!(fresh_path.exists(), "fresh file should be preserved");
}

#[test]
fn cleanup_only_matches_session_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar_dir = dir.path();

    // Non-matching file, backdated
    let other_path = sidecar_dir.join("config.json");
    std::fs::write(&other_path, "{}").unwrap();
    backdate_file(&other_path, 48 * 3600);

    sidecar::cleanup_stale(sidecar_dir, Duration::from_secs(24 * 3600));

    assert!(
        other_path.exists(),
        "non-session file should not be deleted"
    );
}

#[test]
fn cleanup_no_recursive_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar_dir = dir.path();

    // Create a subdirectory with a matching-name file
    let subdir = sidecar_dir.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let nested = subdir.join("session-nested.json");
    std::fs::write(&nested, "{}").unwrap();

    sidecar::cleanup_stale(sidecar_dir, Duration::from_secs(24 * 3600));

    assert!(
        nested.exists(),
        "files in subdirectories should not be touched"
    );
}

#[test]
fn cleanup_empty_directory_handled() {
    let dir = tempfile::tempdir().unwrap();
    // Should not panic on empty directory
    sidecar::cleanup_stale(dir.path(), Duration::from_secs(24 * 3600));
}

#[test]
fn cleanup_nonexistent_directory_handled() {
    let path = Path::new("/tmp/nonexistent-sidecar-cleanup-test");
    // Should not panic on nonexistent directory
    sidecar::cleanup_stale(path, Duration::from_secs(24 * 3600));
}
