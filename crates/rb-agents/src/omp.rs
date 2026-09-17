//! Oh My Pi extension adapter.
//!
//! The installed native OMP extension serializes its lifecycle events into a
//! small JSON envelope and invokes `rusty-brain-hooks --agent omp`. This adapter
//! maps that envelope to the canonical hook model and returns the recall text in
//! an extension-consumable shape. It is deliberately not coupled to OMP's
//! internal TypeScript types, which keeps the Rust hook process portable and
//! fail-open.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::{AgentCli, AgentId};
use crate::event::{HookContext, HookEvent, HookResult};

/// OMP extension JSON adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct OmpCli;

/// Maximum inline shutdown transcript size, measured in UTF-8 bytes.
const MAX_TRANSCRIPT_BYTES: usize = 256 * 1024;

fn opt_str(raw: &Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn value_or_null(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for OmpCli {
    fn id(&self) -> AgentId {
        AgentId::Omp
    }

    fn binary_name(&self) -> &'static str {
        "omp"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        let event = match raw.get("type").and_then(Value::as_str) {
            Some("session_start") => HookEvent::SessionStart {
                source: opt_str(raw, "source"),
            },
            Some("prompt") => HookEvent::UserPromptSubmit {
                prompt: opt_str(raw, "prompt"),
            },
            Some("tool_result") => HookEvent::PostToolUse {
                tool_name: opt_str(raw, "tool_name").unwrap_or_default(),
                tool_input: value_or_null(raw, "tool_input"),
                tool_response: value_or_null(raw, "tool_response"),
            },
            Some("session_shutdown") => HookEvent::SessionEnd {
                reason: opt_str(raw, "reason"),
            },
            Some("session_checkpoint") => HookEvent::SessionCheckpoint {
                reason: opt_str(raw, "reason"),
            },
            Some("pre_compact") => HookEvent::PreCompact {
                custom_instructions: opt_str(raw, "custom_instructions"),
            },
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };
        // Reject oversized payloads before cloning; only capture boundaries may
        // carry inline prose. The runtime applies the same byte cap.
        let transcript_jsonl = if matches!(
            &event,
            HookEvent::SessionEnd { .. } | HookEvent::SessionCheckpoint { .. }
        ) {
            raw.get("transcript_jsonl")
                .and_then(Value::as_str)
                .filter(|text| text.len() <= MAX_TRANSCRIPT_BYTES)
                .map(ToString::to_string)
        } else {
            None
        };

        HookContext {
            event,
            cwd: opt_str(raw, "cwd")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
            session_id: opt_str(raw, "session_id"),
            transcript_path: opt_str(raw, "transcript_path").map(PathBuf::from),
            transcript_jsonl,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = json!({ "continue": result.continue_execution });
        if let Some(message) = &result.system_message {
            out["message"] = Value::String(message.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn identity_and_binary_name() {
        let cli = OmpCli;
        assert_eq!(cli.id(), AgentId::Omp);
        assert_eq!(cli.binary_name(), "omp");
    }

    #[test]
    fn parses_extension_lifecycle_envelope() {
        let cli = OmpCli;
        let session = cli.parse_input(&serde_json::json!({
            "type": "session_start",
            "cwd": "/work/project",
            "session_id": "omp-session",
            "source": "startup"
        }));
        assert_eq!(session.cwd, PathBuf::from("/work/project"));
        assert_eq!(session.session_id.as_deref(), Some("omp-session"));
        assert_eq!(
            session.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );

        let prompt = cli.parse_input(&serde_json::json!({
            "type": "prompt",
            "prompt": "Use the project convention"
        }));
        assert_eq!(
            prompt.event,
            HookEvent::UserPromptSubmit {
                prompt: Some("Use the project convention".to_string())
            }
        );

        let tool = cli.parse_input(&serde_json::json!({
            "type": "tool_result",
            "tool_name": "write",
            "tool_input": {"file_path": "src/lib.rs"},
            "tool_response": {"ok": true}
        }));
        assert_eq!(
            tool.event,
            HookEvent::PostToolUse {
                tool_name: "write".to_string(),
                tool_input: serde_json::json!({"file_path": "src/lib.rs"}),
                tool_response: serde_json::json!({"ok": true}),
            }
        );
    }

    #[test]
    fn inline_transcript_is_capture_only_and_bounded_in_bytes() {
        let line = r#"{"message":{"role":"user","content":"Remember the café goal"}}"#;
        let at_limit = format!("{}{line}", " ".repeat(MAX_TRANSCRIPT_BYTES - line.len()));
        let shutdown = OmpCli.parse_input(&json!({
            "type": "session_shutdown",
            "transcript_jsonl": at_limit,
        }));
        assert!(matches!(shutdown.event, HookEvent::SessionEnd { .. }));
        assert_eq!(
            shutdown.transcript_jsonl.as_deref(),
            Some(at_limit.as_str())
        );
        let checkpoint = OmpCli.parse_input(&json!({
            "type": "session_checkpoint",
            "transcript_jsonl": line,
        }));
        assert!(matches!(
            checkpoint.event,
            HookEvent::SessionCheckpoint { .. }
        ));
        assert_eq!(checkpoint.transcript_jsonl.as_deref(), Some(line));

        // One more byte exceeds the cap even though its character count fits.
        let oversized = OmpCli.parse_input(&json!({
            "type": "session_shutdown",
            "transcript_jsonl": format!("{at_limit}\n"),
        }));
        assert!(oversized.transcript_jsonl.is_none());
        assert!(matches!(oversized.event, HookEvent::SessionEnd { .. }));

        let prompt = OmpCli.parse_input(&json!({
            "type": "prompt",
            "transcript_jsonl": line,
        }));
        assert!(prompt.transcript_jsonl.is_none());
    }

    #[test]
    fn renders_recall_as_extension_message() {
        let output = OmpCli.render_output(&HookResult {
            system_message: Some("recalled memory".to_string()),
            continue_execution: true,
            ..HookResult::default()
        });
        assert_eq!(output["continue"], serde_json::json!(true));
        assert_eq!(output["message"], serde_json::json!("recalled memory"));
    }

    #[test]
    fn malformed_envelope_fails_open_to_other() {
        let cli = OmpCli;
        let context = cli.parse_input(&serde_json::json!({"type": "unknown"}));
        assert_eq!(context.cwd, PathBuf::from("."));
        assert_eq!(context.event, HookEvent::Other("unknown".to_string()));
    }
}
