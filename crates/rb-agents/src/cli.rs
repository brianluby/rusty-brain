//! Per-CLI identity (`AgentId`), the `AgentCli` JSON-adapter trait, a
//! `PassthroughCli` placeholder, and the `agent_for` registry. Part V wires
//! Claude Code fully and routes the other three CLIs to `PassthroughCli`; Part X
//! replaces those arms with real adapters WITHOUT changing this signature.

use serde_json::Value;

use crate::claude_code::ClaudeCodeCli;
use crate::event::{HookContext, HookEvent, HookResult};

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

/// Placeholder adapter for CLIs not yet wired (OpenCode/Gemini/Codex in Part V).
/// Parses EVERY input into [`HookEvent::Other`] with a default cwd and renders a
/// Claude-style fail-open stdout object. Part X replaces the three registry arms
/// that use this with real adapters.
#[derive(Debug, Clone, Copy)]
pub struct PassthroughCli {
    id: AgentId,
    binary_name: &'static str,
}

impl PassthroughCli {
    fn new(id: AgentId, binary_name: &'static str) -> Self {
        Self { id, binary_name }
    }
}

impl AgentCli for PassthroughCli {
    fn id(&self) -> AgentId {
        self.id
    }

    fn binary_name(&self) -> &'static str {
        self.binary_name
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        let cwd = raw
            .get("cwd")
            .and_then(Value::as_str)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let session_id = raw
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let raw_name = raw
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        HookContext {
            event: HookEvent::Other(raw_name),
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("continue".to_string(), Value::Bool(true));
        out.insert("suppressOutput".to_string(), Value::Bool(true));
        if let Some(message) = &result.system_message {
            out.insert("systemMessage".to_string(), Value::String(message.clone()));
        }
        Value::Object(out)
    }
}

/// Registry: return the adapter for `id`. Claude Code is the real reference
/// adapter; the other three route to a [`PassthroughCli`] until Part X replaces
/// them. The signature is FINAL — Part X only swaps the three placeholder arms.
pub fn agent_for(id: AgentId) -> Box<dyn AgentCli> {
    match id {
        AgentId::ClaudeCode => Box::new(ClaudeCodeCli),
        AgentId::OpenCode => Box::new(PassthroughCli::new(AgentId::OpenCode, "opencode")),
        AgentId::Gemini => Box::new(PassthroughCli::new(AgentId::Gemini, "gemini")),
        AgentId::Codex => Box::new(PassthroughCli::new(AgentId::Codex, "codex")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::event::{HookEvent, HookResult};

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
    fn registry_returns_passthrough_for_other_three() {
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
    fn passthrough_maps_known_event_to_other_and_keeps_cwd() {
        let cli = agent_for(AgentId::OpenCode);
        let raw = serde_json::json!({
            "hook_event_name": "SessionStart",
            "cwd": "/proj",
            "session_id": "s1"
        });
        let ctx = cli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("SessionStart".to_string()));
        assert_eq!(ctx.cwd, std::path::PathBuf::from("/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn passthrough_render_is_fail_open_continue() {
        let cli = agent_for(AgentId::Codex);
        let out = cli.render_output(&HookResult {
            system_message: Some("hi".to_string()),
            continue_execution: true,
        });
        assert_eq!(out["continue"], true);
        assert_eq!(out["suppressOutput"], true);
        assert_eq!(out["systemMessage"], "hi");
    }

    #[test]
    fn agent_cli_is_object_safe() {
        // Compiles only if `AgentCli` is object-safe (used as `Box<dyn AgentCli>`).
        let cli: Box<dyn AgentCli> = agent_for(AgentId::ClaudeCode);
        assert_eq!(cli.id(), AgentId::ClaudeCode);
    }
}
