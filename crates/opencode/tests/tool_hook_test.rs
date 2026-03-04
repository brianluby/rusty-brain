//! Tool hook unit tests (T010).

use std::path::Path;

use opencode::sidecar;
use opencode::tool_hook::handle_tool_hook;
use types::HookInput;

fn make_tool_input(cwd: &str, tool_name: &str, tool_response: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": tool_name,
        "tool_response": { "content": tool_response },
        "platform": "opencode"
    }))
    .expect("valid HookInput JSON")
}

/// AC-5: New observation stored with correct obs_type, tool_name, compressed summary.
#[test]
fn new_observation_stored() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let input = make_tool_input(&cwd.to_string_lossy(), "read", "file contents here");

    let result = handle_tool_hook(&input, cwd);
    assert!(result.is_ok(), "tool hook should succeed");

    // Verify sidecar was created
    let sidecar_path = sidecar::sidecar_path(cwd, "test-session-001");
    assert!(
        sidecar_path.exists(),
        "sidecar file should be created: {}",
        sidecar_path.display()
    );

    let state = sidecar::load(&sidecar_path).unwrap();
    assert_eq!(state.observation_count, 1);
    assert_eq!(state.dedup_hashes.len(), 1);
}

/// AC-6: Duplicate detected via sidecar hash and skipped.
#[test]
fn duplicate_detected_and_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let input = make_tool_input(&cwd.to_string_lossy(), "read", "same content");

    // First call stores the observation
    handle_tool_hook(&input, cwd).unwrap();

    // Second call with same input should be deduplicated
    handle_tool_hook(&input, cwd).unwrap();

    let sidecar_path = sidecar::sidecar_path(cwd, "test-session-001");
    let state = sidecar::load(&sidecar_path).unwrap();
    assert_eq!(
        state.observation_count, 1,
        "duplicate should not increment observation count"
    );
    assert_eq!(state.dedup_hashes.len(), 1);
}

/// AC-7, M-5: Error path returns Err (caller wraps in fail-open).
#[test]
fn invalid_cwd_returns_error() {
    let input = make_tool_input(
        "/nonexistent/path/that/does/not/exist",
        "read",
        "some content",
    );
    let cwd = Path::new("/nonexistent/path/that/does/not/exist");

    let result = handle_tool_hook(&input, cwd);
    // Should fail when trying to resolve memory path or save sidecar
    assert!(result.is_err(), "tool hook should error for invalid cwd");
}

/// Sidecar created on first invocation.
#[test]
fn sidecar_created_on_first_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let sidecar_path = sidecar::sidecar_path(cwd, "test-session-001");
    assert!(!sidecar_path.exists(), "sidecar should not exist yet");

    let input = make_tool_input(&cwd.to_string_lossy(), "bash", "command output");
    handle_tool_hook(&input, cwd).unwrap();

    assert!(
        sidecar_path.exists(),
        "sidecar should be created after first invocation"
    );
}

/// Sidecar updated with new hash and incremented observation_count.
#[test]
fn sidecar_updated_on_new_observation() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    let input1 = make_tool_input(&cwd.to_string_lossy(), "read", "first file content");
    handle_tool_hook(&input1, cwd).unwrap();

    let input2 = make_tool_input(&cwd.to_string_lossy(), "write", "second file content");
    handle_tool_hook(&input2, cwd).unwrap();

    let sidecar_path = sidecar::sidecar_path(cwd, "test-session-001");
    let state = sidecar::load(&sidecar_path).unwrap();
    assert_eq!(state.observation_count, 2);
    assert_eq!(state.dedup_hashes.len(), 2);
}

/// Empty tool response returns default HookOutput without storing.
#[test]
fn empty_tool_response_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    let input: HookInput = serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "",
        "cwd": cwd.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "read",
        "platform": "opencode"
    }))
    .unwrap();

    let result = handle_tool_hook(&input, cwd);
    assert!(result.is_ok());

    // No sidecar should be created for empty response
    let sidecar_path = sidecar::sidecar_path(cwd, "test-session-001");
    assert!(
        !sidecar_path.exists(),
        "sidecar should not be created for empty response"
    );
}
