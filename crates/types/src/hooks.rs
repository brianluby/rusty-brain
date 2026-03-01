//! Claude Code hook protocol request and response types.
//!
//! These types model the JSON payloads exchanged between Claude Code and hook
//! executables. [`HookInput`] is received on stdin; [`HookOutput`] is written
//! to stdout. Both are forward-compatible: unknown fields are silently ignored
//! on deserialization, and `None` fields are omitted on serialization.

use serde::{Deserialize, Serialize};

/// Represents the JSON payload delivered to a hook executable by Claude Code.
///
/// Claude Code sends hook events as JSON on stdin. This struct captures the
/// common envelope fields that appear across all event types, plus the optional
/// fields that are present only for specific events (e.g., `PostToolUse`,
/// `Stop`, `SessionStart`).
///
/// Unknown fields are silently ignored to maintain forward compatibility as the
/// Claude Code protocol evolves. Do NOT add `#[serde(deny_unknown_fields)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookInput {
    // --- Required envelope fields (present in every event) ---
    /// Unique session identifier assigned by Claude Code.
    pub session_id: String,
    /// Filesystem path to the conversation transcript JSONL file.
    pub transcript_path: String,
    /// Current working directory of the Claude Code session.
    pub cwd: String,
    /// Active permission mode (e.g. `"default"`, `"plan"`).
    pub permission_mode: String,
    /// Name of the hook event (e.g. `"PostToolUse"`, `"Stop"`, `"SessionStart"`).
    pub hook_event_name: String,

    // --- Tool-related fields (PostToolUse / PreToolUse) ---
    /// Name of the tool invoked. Present for `PreToolUse` and `PostToolUse` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// JSON input passed to the tool. Present for tool-use events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    /// JSON response returned by the tool. Present for `PostToolUse` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<serde_json::Value>,
    /// Unique identifier for this tool invocation. Present for tool-use events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,

    // --- Stop event fields ---
    /// Whether a stop hook is active for this event. Present for `Stop` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_hook_active: Option<bool>,
    /// Last message from the assistant before stopping. Present for `Stop` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,

    // --- SessionStart / informational fields ---
    /// Source that triggered the session (e.g. `"startup"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Model identifier used for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Initial user prompt, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Platform identifier (e.g. `"claude"`, `"opencode"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Represents the JSON payload a hook executable writes to stdout to control
/// Claude Code's behavior after the hook completes.
///
/// All fields are optional. Fields set to `None` are omitted from the
/// serialized output (`{}`). Claude Code interprets missing fields as
/// "no override" for the corresponding behavior.
///
/// Key naming follows the Claude Code hook protocol:
/// - `continue` (reserved Rust keyword → renamed via serde)
/// - `stopReason`, `suppressOutput`, `systemMessage`, `hookSpecificOutput`
///   use camelCase as required by the protocol.
/// - `decision` and `reason` use their natural names (no rename needed).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookOutput {
    /// Whether Claude Code should continue execution after this hook.
    /// Serialized as `"continue"` (camelCase rename required because `continue`
    /// is a Rust reserved keyword).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "continue")]
    pub continue_execution: Option<bool>,

    /// Human-readable reason for stopping, surfaced in the Claude Code UI.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "stopReason"
    )]
    pub stop_reason: Option<String>,

    /// When `true`, Claude Code suppresses output for this tool use.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "suppressOutput"
    )]
    pub suppress_output: Option<bool>,

    /// A message injected into the system prompt for the next assistant turn.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "systemMessage"
    )]
    pub system_message: Option<String>,

    /// Permission decision for `PreToolUse` hooks (e.g. `"allow"`, `"block"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,

    /// Human-readable reason accompanying a `decision` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Arbitrary hook-specific data passed back to Claude Code.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "hookSpecificOutput"
    )]
    pub hook_specific_output: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // T028: HookInput deserialization from real Claude Code JSON samples
    // -------------------------------------------------------------------------

    #[test]
    fn hook_input_deserializes_session_start_event() {
        let json = r#"{
            "session_id": "abc123",
            "transcript_path": "/path/to/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "SessionStart",
            "source": "startup",
            "model": "claude-sonnet-4-20250514"
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id, "abc123");
        assert_eq!(input.hook_event_name, "SessionStart");
        assert_eq!(input.source, Some("startup".to_string()));
        assert_eq!(input.model, Some("claude-sonnet-4-20250514".to_string()));
        assert!(input.tool_name.is_none());
    }

    #[test]
    fn hook_input_deserializes_post_tool_use_event() {
        let json = r#"{
            "session_id": "abc123",
            "transcript_path": "/path/to/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
            "tool_response": {"success": true},
            "tool_use_id": "toolu_01ABC"
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name, "PostToolUse");
        assert_eq!(input.tool_name, Some("Write".to_string()));
        assert!(input.tool_input.is_some());
        assert!(input.tool_response.is_some());
        assert_eq!(input.tool_use_id, Some("toolu_01ABC".to_string()));
    }

    #[test]
    fn hook_input_deserializes_stop_event() {
        let json = r#"{
            "session_id": "abc123",
            "transcript_path": "/path/to/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "Stop",
            "stop_hook_active": true,
            "last_assistant_message": "I've completed the task."
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.hook_event_name, "Stop");
        assert_eq!(input.stop_hook_active, Some(true));
        assert_eq!(
            input.last_assistant_message,
            Some("I've completed the task.".to_string())
        );
    }

    // -------------------------------------------------------------------------
    // T029: HookInput forward compatibility — unknown fields must be ignored
    // -------------------------------------------------------------------------

    #[test]
    fn hook_input_ignores_unknown_fields() {
        let json = r#"{
            "session_id": "abc123",
            "transcript_path": "/path/to/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "unknown_field_1": "value1",
            "unknown_field_2": 42,
            "unknown_field_3": true,
            "unknown_field_4": null,
            "unknown_field_5": [1, 2, 3],
            "unknown_field_6": {"nested": true},
            "unknown_field_7": "more",
            "unknown_field_8": "stuff",
            "unknown_field_9": "here",
            "unknown_field_10": "too",
            "unknown_field_11": "extra"
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id, "abc123");
        assert_eq!(input.hook_event_name, "PostToolUse");
        assert_eq!(input.tool_name, Some("Read".to_string()));
    }

    // -------------------------------------------------------------------------
    // T030: HookOutput serialization
    // -------------------------------------------------------------------------

    #[test]
    fn hook_output_serializes_continue_as_continue_key() {
        let output = HookOutput {
            continue_execution: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"continue\""));
        assert!(!json.contains("\"continueExecution\""));
        assert!(!json.contains("\"continue_execution\""));
    }

    #[test]
    fn hook_output_serializes_with_correct_mixed_case_keys() {
        let output = HookOutput {
            continue_execution: Some(true),
            stop_reason: Some("task complete".to_string()),
            suppress_output: Some(false),
            system_message: Some("All done".to_string()),
            hook_specific_output: Some(serde_json::json!({"additionalContext": "test"})),
            ..Default::default()
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"stopReason\""));
        assert!(json.contains("\"suppressOutput\""));
        assert!(json.contains("\"systemMessage\""));
        assert!(json.contains("\"hookSpecificOutput\""));
    }

    #[test]
    fn hook_output_json_round_trip() {
        let original = HookOutput {
            continue_execution: Some(false),
            stop_reason: Some("user requested".to_string()),
            suppress_output: Some(true),
            system_message: Some("Stopping now".to_string()),
            decision: Some("block".to_string()),
            reason: Some("security concern".to_string()),
            hook_specific_output: Some(serde_json::json!({"context": "details"})),
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: HookOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn hook_output_skips_none_fields() {
        let output = HookOutput::default();
        let json = serde_json::to_string(&output).unwrap();
        assert_eq!(json, "{}");
    }
}
