//! Shared test helpers for adapter tests.
//!
//! Provides functions to build [`HookInput`] instances via JSON deserialization,
//! working around `#[non_exhaustive]` restrictions.

use types::HookInput;

/// Build a `HookInput` with the given event name, optional tool name, and platform.
pub fn make_input(hook_event_name: &str, tool_name: Option<&str>, platform: &str) -> HookInput {
    let tool_name_json = match tool_name {
        Some(name) => format!(r#""tool_name": "{name}","#),
        None => String::new(),
    };
    let json = format!(
        r#"{{
            "session_id": "test-session-123",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "{hook_event_name}",
            {tool_name_json}
            "platform": "{platform}"
        }}"#
    );
    serde_json::from_str(&json).expect("test HookInput JSON must parse")
}

/// Build a `HookInput` with a custom session ID (for testing empty/whitespace session IDs).
pub fn make_input_with_session_id(session_id: &str, platform: &str) -> HookInput {
    let json = format!(
        r#"{{
            "session_id": "{session_id}",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "SessionStart",
            "platform": "{platform}"
        }}"#
    );
    serde_json::from_str(&json).expect("test HookInput JSON must parse")
}
