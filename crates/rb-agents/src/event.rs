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
    /// The assistant turn is stopping. Carries the last assistant message, if
    /// any, and whether this Stop was itself caused by a Stop hook forcing
    /// continuation (Claude Code `stop_hook_active` — real payloads carry it;
    /// W3.1's retained Stop logic must check it to avoid re-capture loops).
    /// CLIs without an equivalent field parse it as `false`.
    Stop {
        last_assistant_message: Option<String>,
        stop_hook_active: bool,
    },
    /// A best-available adapter-specific boundary for CLIs that do not expose a
    /// verified session terminus. The runtime folds scratch into a live summary
    /// here without clearing scratch, so later checkpoints supersede the prior
    /// summary while preserving early observations. This is deliberately
    /// distinct from [`SessionEnd`].
    SessionCheckpoint { reason: Option<String> },
    /// The agent SESSION is ending (distinct from a per-turn [`Stop`]). This is
    /// W3.1's single capture point: the runtime folds the per-session scratch
    /// file + transcript into ONE summary memory here. `reason` is the CLI's
    /// termination label when reported (Claude Code: `clear` | `logout` |
    /// `prompt_input_exit` | `other`). CLIs without a session-end event never
    /// produce this variant.
    SessionEnd { reason: Option<String> },
    /// The context is about to be compacted. Carries any custom instructions.
    PreCompact { custom_instructions: Option<String> },
    /// The user submitted a prompt. W3.2(a)'s deterministic-recall point: the
    /// runtime recalls memories relevant to `prompt` and injects them via
    /// `additionalContext` so recall no longer depends on the model electing to
    /// call a tool. `prompt` is the raw user text (Claude Code `prompt` field);
    /// CLIs without an equivalent event never produce this variant.
    UserPromptSubmit { prompt: Option<String> },
    /// An event we do not model (or could not parse). Carries the raw event name.
    Other(String),
}

/// Which Claude Code hook event an injected [`HookResult::system_message`]
/// belongs to. Only `SessionStart` and `UserPromptSubmit` feed
/// `additionalContext` back to the model, and Claude Code requires
/// `hookSpecificOutput.hookEventName` to name the firing event — so the capture
/// flow that produced the injection declares which one here, and the Claude Code
/// adapter stamps it. The other adapters ignore it (they render `system_message`
/// their own way). Defaults to `SessionStart` (the original injection point).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InjectionEvent {
    /// Injected at session start (the W1.3/W2.5 digest).
    #[default]
    SessionStart,
    /// Injected on a user prompt (the W3.2(a) deterministic recall).
    UserPromptSubmit,
}

impl InjectionEvent {
    /// The Claude Code `hookSpecificOutput.hookEventName` value for this channel.
    #[must_use]
    pub fn hook_event_name(self) -> &'static str {
        match self {
            InjectionEvent::SessionStart => "SessionStart",
            InjectionEvent::UserPromptSubmit => "UserPromptSubmit",
        }
    }
}

/// The result of handling a hook event. In P4 `continue_execution` is ALWAYS
/// `true`: capture hooks are strictly fail-open and never block the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResult {
    /// Text to surface to the agent (e.g. injected context). `None` = nothing.
    pub system_message: Option<String>,
    /// Always `true` in P4. Kept explicit so renderers emit it verbatim.
    pub continue_execution: bool,
    /// When `system_message` is injected back to the model, which hook event it
    /// belongs to — drives `hookSpecificOutput.hookEventName` in the Claude Code
    /// adapter. Irrelevant when `system_message` is `None`; defaults to
    /// `SessionStart`. Ignored by the non-Claude adapters.
    pub injection_event: InjectionEvent,
}

/// Fail-open default: a defaulted `HookResult` must NEVER block the CLI, so
/// `continue_execution` defaults to `true` (the derived `Default` would have made
/// it `false`, contradicting the P4 fail-open contract). `injection_event` is
/// irrelevant while `system_message` is `None`.
impl Default for HookResult {
    fn default() -> Self {
        Self {
            system_message: None,
            continue_execution: true,
            injection_event: InjectionEvent::SessionStart,
        }
    }
}

/// A normalized hook invocation: the event plus the resolved cwd and session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookContext {
    pub event: HookEvent,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    /// Path to the CLI's session transcript, when the CLI reports one. Claude
    /// Code sends `transcript_path` on EVERY hook event (verified against the
    /// recorded fixtures in `rb-hooks/tests/fixtures/claude_code/`); W3.1's
    /// PreCompact redesign reads decisions out of it. CLIs without an
    /// equivalent parse it as `None`.
    pub transcript_path: Option<PathBuf>,
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
            transcript_path: Some(PathBuf::from("/work/transcript.jsonl")),
        };
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
        assert_eq!(ctx.cwd, PathBuf::from("/work/project"));
        assert_eq!(ctx.session_id.as_deref(), Some("sess-1"));
        assert_eq!(
            ctx.transcript_path,
            Some(PathBuf::from("/work/transcript.jsonl"))
        );
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
