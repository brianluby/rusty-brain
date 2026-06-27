//! OpenCode `AgentCli` adapter.
//!
//! OpenCode delivers hook events as JSON with a `type` discriminator
//! (`"session.created"`, `"tool.execute.after"`, `"session.idle"`,
//! `"session.compacted"`, …). This adapter normalizes those into the
//! canonical [`HookContext`]/[`HookEvent`] and renders a [`HookResult`] back
//! into OpenCode's `{ "continue", "systemMessage" }` plugin-result shape.
//!
//! Fail-open: any unrecognized `type` or malformed field degrades to
//! [`HookEvent::Other`]; parsing never panics.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::AgentCli;
use crate::cli::AgentId;
use crate::event::{HookContext, HookEvent, HookResult};

/// OpenCode JSON hook adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenCodeCli;

/// Read the first present string field from `raw` among `keys`.
fn first_str(raw: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = raw.get(*key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

/// Read the first present (non-null) JSON value from `raw` among `keys`.
fn first_value(raw: &Value, keys: &[&str]) -> Value {
    for key in keys {
        if let Some(v) = raw.get(*key) {
            if !v.is_null() {
                return v.clone();
            }
        }
    }
    Value::Null
}

/// Extract the OpenCode tool name: a bare `tool` string or a nested `tool.name`.
fn opencode_tool_name(raw: &Value) -> String {
    if let Some(s) = raw.get("tool").and_then(Value::as_str) {
        return s.to_string();
    }
    if let Some(s) = raw
        .get("tool")
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
    {
        return s.to_string();
    }
    // DEFAULTED: OpenCode tool-name location is not contract-stable; fall back to
    // a flat `tool_name` field, else empty so capture still proceeds fail-open.
    raw.get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl AgentCli for OpenCodeCli {
    fn id(&self) -> AgentId {
        AgentId::OpenCode
    }

    fn binary_name(&self) -> &'static str {
        "opencode"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        // DEFAULTED: cwd absent => the running process cwd is used by the harness;
        // here we default to "." so the type stays total and never panics.
        let cwd = first_str(raw, &["directory", "cwd"])
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = first_str(raw, &["sessionID", "session_id"]);

        let event = match raw.get("type").and_then(Value::as_str) {
            Some("session.created") => HookEvent::SessionStart {
                source: first_str(raw, &["source"]),
            },
            Some("tool.execute.after") => HookEvent::PostToolUse {
                tool_name: opencode_tool_name(raw),
                tool_input: first_value(raw, &["args", "tool_input"]),
                tool_response: first_value(raw, &["output", "tool_response"]),
            },
            // OpenCode's `session.idle` is its terminal/idle boundary — the event
            // it emits to mark "the turn settled". It is NOT a clean once-per-
            // session terminus: the recorded `terminus.json` verdict is
            // `ambiguous` (a single-run observation, fired=2), so its cadence is
            // unproven. That ambiguity is exactly why it maps to the multi-fire-
            // safe SessionCheckpoint and NOT Stop: each idle folds the accumulated
            // scratch into one *superseding* summary (retaining the buffer), so
            // ANY fire count >= 1 converges to one progressively-complete memory
            // rather than N — robust to the cadence, restoring capture that
            // canonical Stop dropped. `first_str` stays forward-compatible:
            // today's idle payload carries no `reason`, so this resolves to None.
            Some("session.idle") => HookEvent::SessionCheckpoint {
                reason: first_str(raw, &["reason"]),
            },
            Some("session.compacted") => HookEvent::PreCompact {
                custom_instructions: first_str(raw, &["custom_instructions"]),
            },
            // DEFAULTED: session.deleted / tool.execute.before / unknown =>
            // not a captured event in P4; preserve the raw name for diagnostics.
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };

        HookContext {
            event,
            cwd,
            session_id,
            // OpenCode's event payloads do not document a transcript-path field.
            transcript_path: None,
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
        let cli = OpenCodeCli;
        assert_eq!(cli.id(), AgentId::OpenCode);
        assert_eq!(cli.binary_name(), "opencode");
    }

    #[test]
    fn parses_session_created_as_session_start() {
        let raw = json!({
            "type": "session.created",
            "directory": "/work/proj",
            "sessionID": "s-1",
            "source": "startup"
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.cwd, PathBuf::from("/work/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("s-1"));
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
    }

    #[test]
    fn parses_tool_execute_after_as_post_tool_use() {
        // OpenCode reports the tool name in lowercase; the adapter preserves it
        // verbatim (capture-layer normalization maps it to canonical "Write").
        let raw = json!({
            "type": "tool.execute.after",
            "directory": "/work/proj",
            "sessionID": "s-2",
            "tool": "write",
            "args": {"file_path": "/tmp/a.txt"},
            "output": {"ok": true}
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "write");
                assert_eq!(tool_input, json!({"file_path": "/tmp/a.txt"}));
                assert_eq!(tool_response, json!({"ok": true}));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_nested_tool_name_object() {
        let raw = json!({
            "type": "tool.execute.after",
            "tool": {"name": "Bash"},
            "args": {},
            "output": {}
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse { tool_name, .. } => assert_eq!(tool_name, "Bash"),
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_session_idle_as_session_checkpoint() {
        // Gap A: session.idle is the fold event. It is multi-fire (not a clean
        // terminus), so it maps to the superseding SessionCheckpoint, not Stop.
        let raw = json!({
            "type": "session.idle",
            "last_assistant_message": "done"
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::SessionCheckpoint { reason: None });
    }

    #[test]
    fn parses_session_compacted_as_pre_compact() {
        let raw = json!({ "type": "session.compacted" });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: None
            }
        );
    }

    #[test]
    fn unknown_type_degrades_to_other() {
        let raw = json!({ "type": "session.deleted" });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("session.deleted".to_string()));
    }

    #[test]
    fn malformed_payload_degrades_to_other_without_panic() {
        // No `type`, no recognizable fields: must not panic, must be Other.
        let raw = json!({ "garbage": [1, 2, 3], "nested": {"x": null} });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn render_output_includes_continue_and_optional_message() {
        let with_msg = HookResult {
            system_message: Some("hello".to_string()),
            continue_execution: true,
            ..HookResult::default()
        };
        let v = OpenCodeCli.render_output(&with_msg);
        assert_eq!(v["continue"], json!(true));
        assert_eq!(v["systemMessage"], json!("hello"));

        let without = HookResult {
            system_message: None,
            continue_execution: true,
            ..HookResult::default()
        };
        let v = OpenCodeCli.render_output(&without);
        assert_eq!(v["continue"], json!(true));
        assert!(v.get("systemMessage").is_none());
    }
}
