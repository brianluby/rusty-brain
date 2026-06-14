//! Codex `AgentCli` adapter (JSON hook path only).
//!
//! Codex delivers hook events as JSON on stdin with an `hook_event_name`
//! discriminator (`"SessionStart"`, `"UserPromptSubmit"`, `"PreToolUse"`,
//! `"PostToolUse"`, `"PreCompact"`, `"Stop"`) and flat
//! `tool_name`/`tool_input`/`tool_response` fields. This adapter normalizes
//! those into the canonical [`HookContext`]/[`HookEvent`] and renders a
//! [`HookResult`] into Codex's `{ "continue", "hookSpecificOutput" }` stdout
//! shape (SessionStart context rides on `hookSpecificOutput.additionalContext`,
//! the field that feeds the model).
//! The Codex TOML config path is handled install-side (Part Y), NOT here.
//!
//! Fail-open: `UserPromptSubmit`/`PreToolUse`/unknown/missing degrade to
//! [`HookEvent::Other`]; parsing never panics.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::AgentCli;
use crate::cli::AgentId;
use crate::event::{HookContext, HookEvent, HookResult};

/// Codex JSON hook adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexCli;

/// Read a string field from `raw`, or `None` if absent/non-string.
fn str_field(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Read a JSON field from `raw`, or `Value::Null` if absent.
fn value_field(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for CodexCli {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn binary_name(&self) -> &'static str {
        "codex"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        // DEFAULTED: cwd absent => "." so the type stays total and never panics.
        let cwd = str_field(raw, "cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = str_field(raw, "session_id");

        let event = match raw.get("hook_event_name").and_then(Value::as_str) {
            Some("SessionStart") => HookEvent::SessionStart {
                source: str_field(raw, "source"),
            },
            Some("PostToolUse") => HookEvent::PostToolUse {
                // DEFAULTED: tool_name absent => empty; capture proceeds fail-open.
                tool_name: str_field(raw, "tool_name").unwrap_or_default(),
                tool_input: value_field(raw, "tool_input"),
                tool_response: value_field(raw, "tool_response"),
            },
            Some("Stop") => HookEvent::Stop {
                last_assistant_message: str_field(raw, "last_assistant_message"),
                // Codex has no stop-hook-forced-continuation equivalent.
                stop_hook_active: false,
            },
            Some("PreCompact") => HookEvent::PreCompact {
                custom_instructions: str_field(raw, "custom_instructions"),
            },
            // DEFAULTED: UserPromptSubmit / PreToolUse / unknown => not captured in P4.
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };

        HookContext {
            event,
            cwd,
            session_id,
            // Codex's hook payloads do not document a transcript-path field.
            transcript_path: None,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = json!({ "continue": result.continue_execution });
        // SessionStart context injection rides on `hookSpecificOutput.additionalContext`
        // — the field Codex feeds back into the model — NOT `systemMessage` (which is
        // user-facing only). `system_message` is set solely by the SessionStart flow.
        if let Some(msg) = &result.system_message {
            out["hookSpecificOutput"] = json!({
                "hookEventName": "SessionStart",
                "additionalContext": msg,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use serde_json::json;

    #[test]
    fn id_and_binary_name() {
        let cli = CodexCli;
        assert_eq!(cli.id(), AgentId::Codex);
        assert_eq!(cli.binary_name(), "codex");
    }

    #[test]
    fn parses_session_start() {
        let raw = json!({
            "hook_event_name": "SessionStart",
            "cwd": "/work/proj",
            "session_id": "c-1",
            "source": "resume"
        });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.cwd, PathBuf::from("/work/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("c-1"));
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("resume".to_string())
            }
        );
    }

    #[test]
    fn parses_post_tool_use() {
        let raw = json!({
            "hook_event_name": "PostToolUse",
            "cwd": "/work/proj",
            "session_id": "c-2",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_response": {"stdout": "a\nb"}
        });
        let ctx = CodexCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_input, json!({"command": "ls"}));
                assert_eq!(tool_response, json!({"stdout": "a\nb"}));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_stop() {
        let raw = json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "wrapped up"
        });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("wrapped up".to_string()),
                stop_hook_active: false,
            }
        );
    }

    #[test]
    fn parses_pre_compact() {
        let raw = json!({ "hook_event_name": "PreCompact" });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: None
            }
        );
    }

    #[test]
    fn user_prompt_submit_degrades_to_other() {
        let raw = json!({ "hook_event_name": "UserPromptSubmit", "prompt": "hi" });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("UserPromptSubmit".to_string()));
    }

    #[test]
    fn pre_tool_use_degrades_to_other() {
        let raw = json!({ "hook_event_name": "PreToolUse", "tool_name": "Write" });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("PreToolUse".to_string()));
    }

    #[test]
    fn missing_event_name_degrades_to_other_without_panic() {
        let raw = json!({ "unexpected": true });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn render_output_shape() {
        let with_msg = HookResult {
            system_message: Some("note".to_string()),
            continue_execution: true,
            ..HookResult::default()
        };
        let v = CodexCli.render_output(&with_msg);
        assert_eq!(v["continue"], json!(true));
        // Context is injected via hookSpecificOutput.additionalContext (the field
        // that feeds the model), tagged with the SessionStart event name.
        assert_eq!(
            v["hookSpecificOutput"]["hookEventName"],
            json!("SessionStart")
        );
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], json!("note"));
        // The old user-facing systemMessage key must be gone.
        assert!(v.get("systemMessage").is_none());

        let without = HookResult {
            system_message: None,
            continue_execution: true,
            ..HookResult::default()
        };
        let v = CodexCli.render_output(&without);
        assert_eq!(v["continue"], json!(true));
        assert!(v.get("hookSpecificOutput").is_none());
        assert!(v.get("systemMessage").is_none());
    }
}
