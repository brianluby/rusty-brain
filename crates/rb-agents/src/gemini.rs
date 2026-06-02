//! Gemini `AgentCli` adapter.
//!
//! Gemini delivers hook events as strict JSON on stdin with an
//! `hook_event_name` discriminator (`"SessionStart"`, `"AfterTool"`,
//! `"Stop"`, `"PreCompact"`, `"BeforeTool"`, …) and flat
//! `tool_name`/`tool_input`/`tool_response` fields. This adapter normalizes
//! those into the canonical [`HookContext`]/[`HookEvent`] and renders a
//! [`HookResult`] into Gemini's strict `{ "continue", "systemMessage" }`
//! stdout shape.
//!
//! Fail-open: an unrecognized `hook_event_name` or missing fields degrade to
//! [`HookEvent::Other`]; parsing never panics.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::AgentCli;
use crate::cli::AgentId;
use crate::event::{HookContext, HookEvent, HookResult};

/// Gemini strict-JSON hook adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiCli;

/// Read a string field from `raw`, or `None` if absent/non-string.
fn str_field(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Read a JSON field from `raw`, or `Value::Null` if absent.
fn value_field(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for GeminiCli {
    fn id(&self) -> AgentId {
        AgentId::Gemini
    }

    fn binary_name(&self) -> &'static str {
        "gemini"
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
            Some("AfterTool") => HookEvent::PostToolUse {
                // DEFAULTED: tool_name absent => empty; capture proceeds fail-open.
                tool_name: str_field(raw, "tool_name").unwrap_or_default(),
                tool_input: value_field(raw, "tool_input"),
                tool_response: value_field(raw, "tool_response"),
            },
            Some("Stop") => HookEvent::Stop {
                last_assistant_message: str_field(raw, "last_assistant_message"),
            },
            Some("PreCompact") => HookEvent::PreCompact {
                custom_instructions: str_field(raw, "custom_instructions"),
            },
            // DEFAULTED: BeforeTool / unknown => not captured in P4.
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };

        HookContext {
            event,
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = json!({ "continue": result.continue_execution });
        if let Some(msg) = &result.system_message {
            out["systemMessage"] = Value::String(msg.clone());
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
        let cli = GeminiCli;
        assert_eq!(cli.id(), AgentId::Gemini);
        assert_eq!(cli.binary_name(), "gemini");
    }

    #[test]
    fn parses_session_start() {
        let raw = json!({
            "hook_event_name": "SessionStart",
            "cwd": "/work/proj",
            "session_id": "g-1",
            "source": "startup"
        });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(ctx.cwd, PathBuf::from("/work/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("g-1"));
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
    }

    #[test]
    fn parses_after_tool_as_post_tool_use() {
        let raw = json!({
            "hook_event_name": "AfterTool",
            "cwd": "/work/proj",
            "session_id": "g-2",
            "tool_name": "Edit",
            "tool_input": {"path": "x.rs"},
            "tool_response": {"ok": true}
        });
        let ctx = GeminiCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Edit");
                assert_eq!(tool_input, json!({"path": "x.rs"}));
                assert_eq!(tool_response, json!({"ok": true}));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_stop() {
        let raw = json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "finished"
        });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("finished".to_string())
            }
        );
    }

    #[test]
    fn parses_pre_compact() {
        let raw = json!({
            "hook_event_name": "PreCompact",
            "custom_instructions": "keep decisions"
        });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: Some("keep decisions".to_string())
            }
        );
    }

    #[test]
    fn before_tool_degrades_to_other() {
        let raw = json!({ "hook_event_name": "BeforeTool", "tool_name": "Write" });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("BeforeTool".to_string()));
    }

    #[test]
    fn missing_event_name_degrades_to_other_without_panic() {
        let raw = json!({ "noise": 1 });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn render_output_strict_shape() {
        let with_msg = HookResult {
            system_message: Some("ctx".to_string()),
            continue_execution: true,
        };
        let v = GeminiCli.render_output(&with_msg);
        assert_eq!(v["continue"], json!(true));
        assert_eq!(v["systemMessage"], json!("ctx"));

        let without = HookResult {
            system_message: None,
            continue_execution: true,
        };
        let v = GeminiCli.render_output(&without);
        assert_eq!(v["continue"], json!(true));
        assert!(v.get("systemMessage").is_none());
    }
}
