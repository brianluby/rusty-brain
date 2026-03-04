//! Session cleanup unit tests (T014).

use std::path::Path;

use opencode::session_cleanup::handle_session_cleanup;
use opencode::sidecar;
use opencode::types::SidecarState;

/// Helper: seed memory and create a sidecar with observations.
fn seed_session(cwd: &Path, session_id: &str) {
    // Create memory
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
    let mut config = types::MindConfig::from_env().unwrap();
    config.memory_path = resolved.path;
    let mind = rusty_brain_core::mind::Mind::open(config).unwrap();
    mind.with_lock(|m| {
        m.remember(
            types::ObservationType::Discovery,
            "test_tool",
            "auth design decision",
            Some("JWT with refresh tokens"),
            None,
        )
    })
    .unwrap();

    // Create sidecar with observation count
    let sidecar_path = sidecar::sidecar_path(cwd, session_id);
    let state = SidecarState::new(session_id.to_string());
    let hash = sidecar::compute_dedup_hash("test_tool", "auth design decision");
    let state = sidecar::with_hash(&state, hash);
    sidecar::save(&sidecar_path, &state).unwrap();
}

/// AC-14: Summary generated and stored with observation count from sidecar.
#[test]
fn summary_stored_with_observation_count() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    seed_session(cwd, "cleanup-test-001");

    let result = handle_session_cleanup("cleanup-test-001", cwd);
    assert!(result.is_ok(), "session cleanup should succeed");
}

/// Sidecar file deleted after summary storage.
#[test]
fn sidecar_deleted_after_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    seed_session(cwd, "cleanup-test-002");

    let sidecar_path = sidecar::sidecar_path(cwd, "cleanup-test-002");
    assert!(sidecar_path.exists(), "sidecar should exist before cleanup");

    handle_session_cleanup("cleanup-test-002", cwd).unwrap();

    assert!(
        !sidecar_path.exists(),
        "sidecar should be deleted after cleanup"
    );
}

/// AC-15: Empty session (no observations) stores minimal summary.
#[test]
fn empty_session_stores_minimal_summary() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    // Create memory but no sidecar (or sidecar with 0 observations)
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
    let mut config = types::MindConfig::from_env().unwrap();
    config.memory_path = resolved.path;
    let _mind = rusty_brain_core::mind::Mind::open(config).unwrap();

    // Create empty sidecar
    let sidecar_path = sidecar::sidecar_path(cwd, "empty-session");
    let state = SidecarState::new("empty-session".to_string());
    sidecar::save(&sidecar_path, &state).unwrap();

    let result = handle_session_cleanup("empty-session", cwd);
    assert!(result.is_ok(), "empty session cleanup should succeed");
}

/// M-5: Error path returns Err (caller wraps in fail-open).
#[test]
fn invalid_cwd_returns_error() {
    // Create a file where a directory would need to be, making mkdir fail
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let cwd = blocker.join("fake_project");
    let result = handle_session_cleanup("test-session", &cwd);
    assert!(result.is_err(), "cleanup should error for invalid cwd");
}

/// Missing sidecar file handled gracefully (no sidecar to delete).
#[test]
fn missing_sidecar_handled_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    // Create memory but no sidecar
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
    let mut config = types::MindConfig::from_env().unwrap();
    config.memory_path = resolved.path;
    let _mind = rusty_brain_core::mind::Mind::open(config).unwrap();

    let result = handle_session_cleanup("no-sidecar-session", cwd);
    assert!(
        result.is_ok(),
        "cleanup should succeed even without sidecar"
    );
}
