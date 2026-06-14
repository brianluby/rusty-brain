//! Per-CLI identity (`AgentId`), the `AgentCli` JSON-adapter trait, and the
//! `agent_for` registry. Every `AgentId` maps to a real per-CLI adapter
//! (`ClaudeCodeCli`/`OpenCodeCli`/`GeminiCli`/`CodexCli`) that normalizes that
//! CLI's hook JSON into the canonical event model.

use serde_json::Value;

use crate::claude_code::ClaudeCodeCli;
use crate::codex::CodexCli;
use crate::event::{HookContext, HookResult};
use crate::gemini::GeminiCli;
use crate::opencode::OpenCodeCli;

/// The set of CLIs the agent surface targets in P4-v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentId {
    ClaudeCode,
    OpenCode,
    Gemini,
    Codex,
}

impl AgentId {
    /// Stable lowercase wire id used on the `--agent` flag and in config.
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentId::ClaudeCode => "claude-code",
            AgentId::OpenCode => "opencode",
            AgentId::Gemini => "gemini",
            AgentId::Codex => "codex",
        }
    }

    /// Parse a wire id back into an `AgentId`. Unknown ids => `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(AgentId::ClaudeCode),
            "opencode" => Some(AgentId::OpenCode),
            "gemini" => Some(AgentId::Gemini),
            "codex" => Some(AgentId::Codex),
            _ => None,
        }
    }
}

/// A per-CLI JSON adapter: identity, the CLI binary name, stdin-JSON parsing,
/// and stdout-JSON rendering. `parse_input` MUST be fail-open (never panic):
/// unknown events => [`HookEvent::Other`], bad fields => safe defaults.
pub trait AgentCli: Send + Sync {
    fn id(&self) -> AgentId;
    fn binary_name(&self) -> &'static str;
    fn parse_input(&self, raw: &Value) -> HookContext;
    fn render_output(&self, result: &HookResult) -> Value;
}

/// Construct the [`AgentCli`] adapter for the given [`AgentId`].
///
/// Registry: one boxed adapter per supported CLI. All four are JSON-protocol
/// adapters; the returned trait object normalizes that CLI's hook JSON into the
/// canonical [`HookContext`] and renders a canonical [`HookResult`] back.
#[must_use]
pub fn agent_for(id: AgentId) -> Box<dyn AgentCli> {
    match id {
        AgentId::ClaudeCode => Box::new(ClaudeCodeCli),
        AgentId::OpenCode => Box::new(OpenCodeCli),
        AgentId::Gemini => Box::new(GeminiCli),
        AgentId::Codex => Box::new(CodexCli),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn agent_id_str_round_trips_all_variants() {
        for id in [
            AgentId::ClaudeCode,
            AgentId::OpenCode,
            AgentId::Gemini,
            AgentId::Codex,
        ] {
            let s = id.as_str();
            assert_eq!(AgentId::parse(s), Some(id));
        }
    }

    #[test]
    fn agent_id_str_values_are_stable() {
        assert_eq!(AgentId::ClaudeCode.as_str(), "claude-code");
        assert_eq!(AgentId::OpenCode.as_str(), "opencode");
        assert_eq!(AgentId::Gemini.as_str(), "gemini");
        assert_eq!(AgentId::Codex.as_str(), "codex");
    }

    #[test]
    fn agent_id_parse_rejects_unknown() {
        assert_eq!(AgentId::parse("copilot"), None);
        assert_eq!(AgentId::parse(""), None);
        assert_eq!(AgentId::parse("ClaudeCode"), None);
    }

    #[test]
    fn registry_returns_claude_code_for_claude_code() {
        let cli = agent_for(AgentId::ClaudeCode);
        assert_eq!(cli.id(), AgentId::ClaudeCode);
        assert_eq!(cli.binary_name(), "claude");
    }

    #[test]
    fn registry_returns_real_adapters_for_other_three() {
        let opencode = agent_for(AgentId::OpenCode);
        assert_eq!(opencode.id(), AgentId::OpenCode);
        assert_eq!(opencode.binary_name(), "opencode");

        let gemini = agent_for(AgentId::Gemini);
        assert_eq!(gemini.id(), AgentId::Gemini);
        assert_eq!(gemini.binary_name(), "gemini");

        let codex = agent_for(AgentId::Codex);
        assert_eq!(codex.id(), AgentId::Codex);
        assert_eq!(codex.binary_name(), "codex");
    }

    #[test]
    fn agent_cli_is_object_safe() {
        // Compiles only if `AgentCli` is object-safe (used as `Box<dyn AgentCli>`).
        let cli: Box<dyn AgentCli> = agent_for(AgentId::ClaudeCode);
        assert_eq!(cli.id(), AgentId::ClaudeCode);
    }

    #[test]
    fn agent_for_returns_matching_id_for_all_four() {
        for id in [
            AgentId::ClaudeCode,
            AgentId::OpenCode,
            AgentId::Gemini,
            AgentId::Codex,
        ] {
            let cli = agent_for(id);
            assert_eq!(cli.id(), id);
        }
    }

    #[test]
    fn agent_for_opencode_round_trips_post_tool_use() {
        let cli = agent_for(AgentId::OpenCode);
        let raw = serde_json::json!({
            "type": "tool.execute.after",
            "tool": "Write",
            "args": {},
            "output": {}
        });
        let ctx = cli.parse_input(&raw);
        assert!(matches!(
            ctx.event,
            crate::event::HookEvent::PostToolUse { .. }
        ));
        let out = cli.render_output(&crate::event::HookResult {
            system_message: None,
            continue_execution: true,
            ..crate::event::HookResult::default()
        });
        assert_eq!(out["continue"], serde_json::json!(true));
    }

    #[test]
    fn agent_for_gemini_round_trips_after_tool() {
        let cli = agent_for(AgentId::Gemini);
        let raw = serde_json::json!({
            "hook_event_name": "AfterTool",
            "tool_name": "Edit"
        });
        let ctx = cli.parse_input(&raw);
        assert!(matches!(
            ctx.event,
            crate::event::HookEvent::PostToolUse { .. }
        ));
        let out = cli.render_output(&crate::event::HookResult {
            system_message: Some("m".to_string()),
            continue_execution: true,
            ..crate::event::HookResult::default()
        });
        // Gemini injects context via hookSpecificOutput.additionalContext, not the
        // user-facing systemMessage key.
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            serde_json::json!("m")
        );
    }

    #[test]
    fn agent_for_codex_round_trips_post_tool_use() {
        let cli = agent_for(AgentId::Codex);
        let raw = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash"
        });
        let ctx = cli.parse_input(&raw);
        assert!(matches!(
            ctx.event,
            crate::event::HookEvent::PostToolUse { .. }
        ));
        let out = cli.render_output(&crate::event::HookResult {
            system_message: None,
            continue_execution: true,
            ..crate::event::HookResult::default()
        });
        assert_eq!(out["continue"], serde_json::json!(true));
    }
}
