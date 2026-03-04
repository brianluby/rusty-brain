//! Pipeline integration tests for RB-ARCH-004 (OpenCode side).
//!
//! Proves that normalized events flow through the adapter registry + event
//! pipeline before handler logic executes. When the pipeline skips an event,
//! handlers return default (no-op) output without touching the Mind.

use opencode::bootstrap::should_process;
use opencode::chat_hook::handle_chat_hook;
use opencode::tool_hook::handle_tool_hook;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn valid_opencode_input(cwd: &str, event_name: &str) -> types::HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": event_name,
        "platform": "opencode"
    }))
    .unwrap()
}

fn whitespace_cwd_input(event_name: &str) -> types::HookInput {
    valid_opencode_input("   ", event_name)
}

// ---------------------------------------------------------------------------
// should_process unit tests
// ---------------------------------------------------------------------------

#[test]
fn should_process_returns_true_for_valid_opencode_event() {
    let input = valid_opencode_input("/tmp/project", "session_start");
    assert!(
        should_process(&input, "session_start"),
        "valid opencode input must pass pipeline"
    );
}

#[test]
fn should_process_failopen_on_unknown_event_kind() {
    let input = valid_opencode_input("/tmp/project", "session_start");
    assert!(
        should_process(&input, "completely_unknown"),
        "unknown event kind must fail-open"
    );
}

#[test]
fn should_process_skips_when_identity_unresolvable() {
    let input = whitespace_cwd_input("SessionStart");
    // Whitespace cwd → identity unresolvable → pipeline skips
    assert!(
        !should_process(&input, "session_start"),
        "whitespace-only cwd must cause pipeline to skip"
    );
}

// ---------------------------------------------------------------------------
// Handler-level pipeline integration: skip causes default output
// ---------------------------------------------------------------------------

#[test]
fn chat_hook_skipped_by_pipeline_returns_default() {
    let input = whitespace_cwd_input("SessionStart");
    let cwd = std::path::Path::new("   ");

    let output = handle_chat_hook(&input, cwd).unwrap();

    assert!(
        output.system_message.is_none(),
        "pipeline-skipped chat_hook must return no system_message"
    );
    assert!(
        output.hook_specific_output.is_none(),
        "pipeline-skipped chat_hook must return no hook_specific_output"
    );
}

#[test]
fn tool_hook_skipped_by_pipeline_returns_default() {
    let mut input = whitespace_cwd_input("PostToolUse");
    input.tool_name = Some("Read".to_string());
    input.tool_response = Some(serde_json::json!("file contents"));

    let cwd = std::path::Path::new("   ");
    let output = handle_tool_hook(&input, cwd).unwrap();

    assert!(
        output.system_message.is_none(),
        "pipeline-skipped tool_hook must return no system_message"
    );
}

// ---------------------------------------------------------------------------
// End-to-end: valid event → pipeline passes → handler processes normally
// ---------------------------------------------------------------------------

#[test]
fn chat_hook_pipeline_pass_produces_system_message() {
    let dir = tempfile::tempdir().unwrap();
    let cwd_str = dir.path().to_str().unwrap();
    let input = valid_opencode_input(cwd_str, "session_start");

    let output = handle_chat_hook(&input, dir.path()).unwrap();

    assert!(
        output.system_message.is_some(),
        "valid event passing pipeline must produce system_message from chat_hook"
    );
}
