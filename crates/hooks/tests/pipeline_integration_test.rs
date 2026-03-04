//! Pipeline integration tests for RB-ARCH-004.
//!
//! Proves that normalized events flow through the adapter registry + event
//! pipeline before handler logic executes. When the pipeline skips an event,
//! handlers return default (no-op) output without touching the Mind.

mod common;

use hooks::bootstrap::should_process;
use hooks::post_tool_use::handle_post_tool_use;
use hooks::session_start::handle_session_start;
use hooks::stop::handle_stop;

// ---------------------------------------------------------------------------
// should_process unit tests
// ---------------------------------------------------------------------------

#[test]
fn should_process_returns_true_for_valid_session_start() {
    let input = common::session_start_input();
    assert!(
        should_process(&input, "session_start"),
        "valid session_start input with cwd must pass pipeline"
    );
}

#[test]
fn should_process_returns_true_for_valid_post_tool_use() {
    let input = common::post_tool_use_input(
        "Read",
        serde_json::json!({"file_path": "/tmp/test.rs"}),
        serde_json::json!("file contents"),
    );
    assert!(
        should_process(&input, "PostToolUse"),
        "valid PostToolUse input must pass pipeline"
    );
}

#[test]
fn should_process_returns_true_for_valid_stop() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::stop_input(dir.path().to_str().unwrap());
    assert!(
        should_process(&input, "stop"),
        "valid stop input with cwd must pass pipeline"
    );
}

#[test]
fn should_process_returns_true_for_unrecognized_event_kind_failopen() {
    // Unrecognized event kind → adapter.normalize() returns None → fail-open
    let input = common::session_start_input();
    assert!(
        should_process(&input, "totally_unknown_event"),
        "unrecognized event kind must fail-open (return true)"
    );
}

#[test]
fn should_process_returns_true_when_session_id_empty_failopen() {
    // Empty session_id → adapter.normalize() returns None → fail-open
    let input: types::HookInput = serde_json::from_value(serde_json::json!({
        "session_id": "",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "/tmp/test-project",
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "platform": "claude"
    }))
    .unwrap();

    assert!(
        should_process(&input, "session_start"),
        "empty session_id causes normalize to return None, must fail-open"
    );
}

// ---------------------------------------------------------------------------
// Handler-level pipeline integration: skip causes default output
// ---------------------------------------------------------------------------

/// Construct a HookInput with whitespace-only cwd.
///
/// The adapter normalizes this into a PlatformEvent with cwd=" " which the
/// identity resolver treats as absent → pipeline skips the event.
fn input_with_whitespace_cwd(event_name: &str) -> types::HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "   ",
        "permission_mode": "default",
        "hook_event_name": event_name,
        "platform": "claude"
    }))
    .unwrap()
}

#[test]
fn session_start_skipped_by_pipeline_returns_default() {
    let input = input_with_whitespace_cwd("SessionStart");
    let output = handle_session_start(&input).unwrap();

    assert!(
        output.system_message.is_none(),
        "pipeline-skipped session_start must return no system_message"
    );
}

#[test]
fn post_tool_use_skipped_by_pipeline_returns_continue() {
    let mut input = input_with_whitespace_cwd("PostToolUse");
    // Add tool fields so it looks like a real post-tool-use event
    input.tool_name = Some("Read".to_string());
    input.tool_input = Some(serde_json::json!({"file_path": "/tmp/test.rs"}));
    input.tool_response = Some(serde_json::json!("contents"));

    let output = handle_post_tool_use(&input).unwrap();

    // Pipeline skip returns continue_execution: true (fail-open behavior)
    assert_eq!(
        output.continue_execution,
        Some(true),
        "pipeline-skipped post_tool_use must set continue_execution: true"
    );
    assert!(
        output.system_message.is_none(),
        "pipeline-skipped post_tool_use must have no system_message"
    );
}

#[test]
fn stop_skipped_by_pipeline_returns_default() {
    let input = input_with_whitespace_cwd("Stop");
    let output = handle_stop(&input).unwrap();

    assert!(
        output.system_message.is_none(),
        "pipeline-skipped stop must return no system_message"
    );
}

// ---------------------------------------------------------------------------
// End-to-end: valid event → pipeline passes → handler processes normally
// ---------------------------------------------------------------------------

#[test]
fn session_start_pipeline_pass_produces_system_message() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::session_start_input_with_cwd(dir.path().to_str().unwrap());

    let output = handle_session_start(&input).unwrap();

    assert!(
        output.system_message.is_some(),
        "valid event passing pipeline must produce system_message"
    );
}

#[test]
fn stop_pipeline_pass_produces_system_message() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::stop_input(dir.path().to_str().unwrap());

    let output = handle_stop(&input).unwrap();

    assert!(
        output.system_message.is_some(),
        "valid event passing pipeline must produce system_message from stop handler"
    );
}
