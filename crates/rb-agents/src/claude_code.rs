//! Reference `AgentCli` adapter for Claude Code. Maps Claude Code's stdin hook
//! JSON into the canonical [`HookContext`] and renders [`HookResult`] back to
//! Claude Code's stdout JSON. Strictly fail-open: any missing/garbage field
//! degrades to a safe default and an unknown `hook_event_name` becomes
//! [`HookEvent::Other`]; parsing NEVER panics.

use std::path::PathBuf;

use serde_json::Value;

use crate::cli::{AgentCli, AgentId};
use crate::event::{HookContext, HookEvent, HookResult};

/// Claude Code hook adapter. Field names follow the Claude Code hook protocol
/// (`hook_event_name`, `tool_name`, `tool_input`, `tool_response`,
/// `last_assistant_message`, `stop_hook_active`, `custom_instructions`,
/// `source`, `cwd`, `session_id`, `transcript_path`), verified against the
/// REAL recorded payloads in `rb-hooks/tests/fixtures/claude_code/`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeCodeCli;

/// Read an optional string field; absent / non-string => `None`.
fn opt_str(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Read an object field as raw JSON; absent => `Value::Null`.
fn json_or_null(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for ClaudeCodeCli {
    fn id(&self) -> AgentId {
        AgentId::ClaudeCode
    }

    fn binary_name(&self) -> &'static str {
        "claude"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        let cwd = opt_str(raw, "cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = opt_str(raw, "session_id");
        let event_name = opt_str(raw, "hook_event_name").unwrap_or_default();
        let event = match event_name.as_str() {
            "SessionStart" => HookEvent::SessionStart {
                source: opt_str(raw, "source"),
            },
            "PostToolUse" => HookEvent::PostToolUse {
                tool_name: opt_str(raw, "tool_name").unwrap_or_default(),
                tool_input: json_or_null(raw, "tool_input"),
                tool_response: json_or_null(raw, "tool_response"),
            },
            "Stop" => HookEvent::Stop {
                last_assistant_message: opt_str(raw, "last_assistant_message"),
                // Real payloads carry this; absent / non-bool degrades to false.
                stop_hook_active: raw
                    .get("stop_hook_active")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            "PreCompact" => HookEvent::PreCompact {
                custom_instructions: opt_str(raw, "custom_instructions"),
            },
            // W3.1: the session-terminal event Claude Code emits once when a
            // session ends (real payload carries `reason` + `transcript_path`).
            "SessionEnd" => HookEvent::SessionEnd {
                reason: opt_str(raw, "reason"),
            },
            other => HookEvent::Other(other.to_string()),
        };
        HookContext {
            event,
            cwd,
            session_id,
            // Claude Code sends `transcript_path` on every hook event.
            transcript_path: opt_str(raw, "transcript_path").map(PathBuf::from),
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = serde_json::Map::new();
        // Emit the canonical `continue_execution` verbatim (the 3 sibling adapters
        // do the same); it is always `true` in P4 but the renderer must not hardcode.
        out.insert(
            "continue".to_string(),
            Value::Bool(result.continue_execution),
        );
        out.insert("suppressOutput".to_string(), Value::Bool(true));
        // SessionStart context injection rides on `hookSpecificOutput.additionalContext`
        // — the only field Claude Code feeds back into the model. `systemMessage` is
        // user-facing only and would NOT reach the model, so it is never emitted.
        // `system_message` is set solely by the SessionStart capture flow.
        if let Some(message) = &result.system_message {
            out.insert(
                "hookSpecificOutput".to_string(),
                serde_json::json!({
                    "hookEventName": "SessionStart",
                    "additionalContext": message,
                }),
            );
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::cli::AgentId;
    use crate::event::{HookEvent, HookResult};

    fn cli() -> ClaudeCodeCli {
        ClaudeCodeCli
    }

    #[test]
    fn identity_and_binary_name() {
        assert_eq!(cli().id(), AgentId::ClaudeCode);
        assert_eq!(cli().binary_name(), "claude");
    }

    #[test]
    fn parses_session_start_with_source_cwd_and_session() {
        let raw = serde_json::json!({
            "hook_event_name": "SessionStart",
            "source": "startup",
            "cwd": "/home/user/project",
            "session_id": "abc123"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
        assert_eq!(ctx.cwd, PathBuf::from("/home/user/project"));
        assert_eq!(ctx.session_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_post_tool_use_with_input_and_response() {
        let raw = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
            "tool_response": {"success": true},
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Write");
                assert_eq!(tool_input["file_path"], "/tmp/test.txt");
                assert_eq!(tool_response["success"], true);
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_stop_with_last_assistant_message() {
        let raw = serde_json::json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "done.",
            "stop_hook_active": true,
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("done.".to_string()),
                stop_hook_active: true,
            }
        );
    }

    #[test]
    fn stop_without_stop_hook_active_defaults_to_false() {
        // Synthetic edge case: a Stop missing the field (or with a non-bool
        // value) must degrade to false, never error.
        let raw = serde_json::json!({
            "hook_event_name": "Stop",
            "stop_hook_active": "yes",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: None,
                stop_hook_active: false,
            }
        );
    }

    #[test]
    fn parses_transcript_path_when_present_and_none_when_absent() {
        let raw = serde_json::json!({
            "hook_event_name": "SessionStart",
            "source": "startup",
            "cwd": "/p",
            "transcript_path": "/home/user/.claude/projects/p/abc.jsonl"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.transcript_path,
            Some(PathBuf::from("/home/user/.claude/projects/p/abc.jsonl"))
        );

        let raw = serde_json::json!({ "hook_event_name": "SessionStart", "cwd": "/p" });
        assert_eq!(cli().parse_input(&raw).transcript_path, None);
    }

    #[test]
    fn parses_precompact_with_custom_instructions() {
        let raw = serde_json::json!({
            "hook_event_name": "PreCompact",
            "custom_instructions": "keep decisions",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: Some("keep decisions".to_string())
            }
        );
    }

    #[test]
    fn parses_session_end_with_reason_and_transcript() {
        // W3.1: the real SessionEnd payload carries `reason` + `transcript_path`
        // (mirrors rb-hooks/tests/fixtures/claude_code/session_end.json).
        let raw = serde_json::json!({
            "hook_event_name": "SessionEnd",
            "reason": "other",
            "session_id": "abc123",
            "transcript_path": "/home/user/.claude/projects/p/abc.jsonl",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::SessionEnd {
                reason: Some("other".to_string())
            }
        );
        assert_eq!(
            ctx.transcript_path,
            Some(PathBuf::from("/home/user/.claude/projects/p/abc.jsonl"))
        );
        assert_eq!(ctx.session_id.as_deref(), Some("abc123"));

        // A SessionEnd missing `reason` degrades to None, never errors.
        let bare = serde_json::json!({ "hook_event_name": "SessionEnd", "cwd": "/p" });
        assert_eq!(
            cli().parse_input(&bare).event,
            HookEvent::SessionEnd { reason: None }
        );
    }

    #[test]
    fn unknown_event_name_becomes_other() {
        let raw = serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("UserPromptSubmit".to_string()));
    }

    #[test]
    fn malformed_input_degrades_to_other_with_default_cwd_never_panics() {
        // No hook_event_name, no cwd, tool fields the wrong JSON type.
        let raw = serde_json::json!({
            "tool_input": 12345,
            "session_id": false
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        // session_id was a bool, not a string -> dropped to None.
        assert_eq!(ctx.session_id, None);
    }

    #[test]
    fn non_object_input_degrades_safely() {
        // A bare JSON array is not an object; every getter returns None.
        let raw = serde_json::json!(["nope"]);
        let ctx = cli().parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert_eq!(ctx.session_id, None);
    }

    #[test]
    fn render_output_always_continues_and_suppresses() {
        let out = cli().render_output(&HookResult {
            system_message: None,
            continue_execution: true,
        });
        assert_eq!(out["continue"], true);
        assert_eq!(out["suppressOutput"], true);
        // No context to inject => no hookSpecificOutput, and never a systemMessage.
        assert!(out.get("hookSpecificOutput").is_none());
        assert!(out.get("systemMessage").is_none());
    }

    #[test]
    fn render_output_injects_context_via_hook_specific_output() {
        let out = cli().render_output(&HookResult {
            system_message: Some("injected context".to_string()),
            continue_execution: true,
        });
        assert_eq!(out["continue"], true);
        assert_eq!(out["suppressOutput"], true);
        // Context is injected via hookSpecificOutput.additionalContext (the field
        // that actually feeds the model), tagged with the SessionStart event name.
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            "injected context"
        );
        // The old user-facing systemMessage key must be gone.
        assert!(out.get("systemMessage").is_none());
    }
}
