//! Per-CLI `AgentInstaller` implementations for the four JSON-protocol CLIs.
//!
//! Each module provides a unit struct implementing
//! [`rb_agents::install::AgentInstaller`]: `detect()` (via [`crate::detect`])
//! and the pure `hook_fragment()` that produces the sentinel-marked JSON block.

mod claude_code;
mod codex;
mod gemini;
mod opencode;

use std::path::PathBuf;

use rb_agents::install::{AgentInstaller, SENTINEL};
use rb_types::Error;

pub use claude_code::ClaudeCodeInstaller;
pub use codex::CodexInstaller;
pub use gemini::GeminiInstaller;
pub use opencode::OpenCodeInstaller;

/// Every built-in installer, in display order (Claude Code first — the lead adapter).
#[must_use]
pub fn builtins() -> Vec<Box<dyn AgentInstaller>> {
    vec![
        Box::new(ClaudeCodeInstaller),
        Box::new(OpenCodeInstaller),
        Box::new(GeminiInstaller),
        Box::new(CodexInstaller),
    ]
}

/// The four hook events we register, paired with their Claude-Code event key.
pub(crate) const EVENTS: [&str; 4] = ["SessionStart", "PostToolUse", "Stop", "PreCompact"];

/// Build one Claude-Code-shaped command-hook entry for `event`, invoking
/// `rusty-brain-hooks --agent <agent_id>`, tagged with the sentinel marker.
///
/// Shape (one matcher-group): `{ "matcher": "*", "_rusty_brain": true,
/// "hooks": [ { "type": "command", "command": "<bin>",
/// "args": ["--agent", "<id>"], "_rusty_brain": true } ] }`. The `matcher` is
/// omitted for non-tool events (SessionStart/Stop/PreCompact) to match Claude
/// Code's schema.
///
/// The command is emitted in EXEC form — `command` is the raw binary path (its
/// own JSON string) and the flags live in a separate `args` array — rather than
/// a single shell-form string. A shell-form `"<bin> --agent <id>"` is
/// re-tokenized by the shell, so a binary path containing spaces (common in
/// macOS/Windows home dirs) would be split mid-path and fail to launch.
pub(crate) fn command_group(hooks_bin: &str, agent_id: &str, event: &str) -> serde_json::Value {
    let entry = serde_json::json!({
        "type": "command",
        "command": hooks_bin,
        "args": ["--agent", agent_id],
        SENTINEL: true,
    });
    if event == "PostToolUse" {
        serde_json::json!({
            "matcher": "*",
            SENTINEL: true,
            "hooks": [entry],
        })
    } else {
        serde_json::json!({
            SENTINEL: true,
            "hooks": [entry],
        })
    }
}

/// Build the full `{ "hooks": { <event>: [group], ... } }` block shared by the
/// CLIs whose config nests hooks under a top-level `hooks` key (Claude Code,
/// Gemini, Codex). OpenCode overrides with its own shape.
pub(crate) fn hooks_block(hooks_bin: &str, agent_id: &str) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for event in EVENTS {
        hooks.insert(
            event.to_string(),
            serde_json::Value::Array(vec![command_group(hooks_bin, agent_id, event)]),
        );
    }
    serde_json::json!({ "hooks": serde_json::Value::Object(hooks) })
}

/// Resolve a CLI's per-user (global) config directory, per platform.
///
/// macOS/Linux/other: `~/<rel>`; the agent owns `rel` (e.g. `.claude`).
/// Returns [`Error::Io`] when `HOME`/`USERPROFILE` is unset.
pub(crate) fn home_join(rel: &str) -> Result<PathBuf, Error> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| Error::Io("HOME/USERPROFILE not set".to_string()))?;
    Ok(PathBuf::from(home).join(rel))
}
