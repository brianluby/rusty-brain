//! Canonical, CLI-agnostic hook event model. Every `AgentCli` parses its own
//! wire JSON into a [`HookContext`] carrying one [`HookEvent`]; the runtime
//! dispatches on the event and produces a [`HookResult`] which the same
//! `AgentCli` renders back to CLI-specific stdout JSON.

use std::path::PathBuf;

/// A captured hook event, normalized across every supported CLI. Unknown or
/// unparseable events MUST map to [`HookEvent::Other`] (never an error/panic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// A new agent session began. `source` is the CLI's trigger label, if any
    /// (e.g. Claude Code `"startup"`).
    SessionStart { source: Option<String> },
    /// A tool finished running. Carries the tool name and the raw input/response
    /// JSON so the runtime can decide whether to capture an observation.
    PostToolUse {
        tool_name: String,
        tool_input: serde_json::Value,
        tool_response: serde_json::Value,
    },
    /// The assistant turn is stopping. Carries the last assistant message, if any.
    Stop {
        last_assistant_message: Option<String>,
    },
    /// The context is about to be compacted. Carries any custom instructions.
    PreCompact { custom_instructions: Option<String> },
    /// An event we do not model (or could not parse). Carries the raw event name.
    Other(String),
}

/// The result of handling a hook event. In P4 `continue_execution` is ALWAYS
/// `true`: capture hooks are strictly fail-open and never block the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResult {
    /// Text to surface to the agent (e.g. injected context). `None` = nothing.
    pub system_message: Option<String>,
    /// Always `true` in P4. Kept explicit so renderers emit it verbatim.
    pub continue_execution: bool,
}

/// Fail-open default: a defaulted `HookResult` must NEVER block the CLI, so
/// `continue_execution` defaults to `true` (the derived `Default` would have made
/// it `false`, contradicting the P4 fail-open contract).
impl Default for HookResult {
    fn default() -> Self {
        Self {
            system_message: None,
            continue_execution: true,
        }
    }
}

/// A normalized hook invocation: the event plus the resolved cwd and session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookContext {
    pub event: HookEvent,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn hook_result_default_is_fail_open() {
        let r = HookResult::default();
        // Fail-open contract: a defaulted result must continue (never block).
        assert!(r.continue_execution);
        assert_eq!(r.system_message, None);
    }

    #[test]
    fn hook_context_carries_event_cwd_and_session() {
        let ctx = HookContext {
            event: HookEvent::SessionStart {
                source: Some("startup".to_string()),
            },
            cwd: PathBuf::from("/work/project"),
            session_id: Some("sess-1".to_string()),
        };
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
        assert_eq!(ctx.cwd, PathBuf::from("/work/project"));
        assert_eq!(ctx.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn post_tool_use_carries_tool_name_and_payloads() {
        let ev = HookEvent::PostToolUse {
            tool_name: "Write".to_string(),
            tool_input: serde_json::json!({"file_path": "/tmp/x"}),
            tool_response: serde_json::json!({"success": true}),
        };
        match ev {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Write");
                assert_eq!(tool_input["file_path"], "/tmp/x");
                assert_eq!(tool_response["success"], true);
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn other_event_preserves_raw_name() {
        let ev = HookEvent::Other("UserPromptSubmit".to_string());
        assert_eq!(ev, HookEvent::Other("UserPromptSubmit".to_string()));
    }
}
