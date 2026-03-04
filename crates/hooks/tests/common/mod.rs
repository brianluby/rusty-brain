use types::hooks::{HookInput, HookOutput};

pub fn session_start_input() -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "/tmp/test-project",
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "source": "startup",
        "model": "claude-sonnet-4-20250514",
        "platform": "claude"
    }))
    .unwrap()
}

pub fn session_start_input_with_cwd(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "source": "startup",
        "platform": "claude"
    }))
    .unwrap()
}

pub fn post_tool_use_input(
    tool_name: &str,
    tool_input: serde_json::Value,
    tool_response: serde_json::Value,
) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "/tmp/test-project",
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_response": tool_response,
        "tool_use_id": "toolu_01TEST"
    }))
    .unwrap()
}

pub fn post_tool_use_input_with_cwd(
    cwd: &str,
    tool_name: &str,
    tool_input: serde_json::Value,
    tool_response: serde_json::Value,
) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_response": tool_response,
        "tool_use_id": "toolu_01TEST"
    }))
    .unwrap()
}

pub fn stop_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "Stop",
        "stop_hook_active": true,
        "last_assistant_message": "Done."
    }))
    .unwrap()
}

pub fn smart_install_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "Notification"
    }))
    .unwrap()
}

#[allow(dead_code)]
pub fn assert_fail_open(output: &HookOutput) {
    assert_eq!(
        output.continue_execution,
        Some(true),
        "fail-open must set continue: true"
    );
}
