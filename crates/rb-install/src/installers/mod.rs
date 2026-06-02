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

/// Claude Code's hook event names. The tool event is `PostToolUse`.
pub(crate) const CLAUDE_EVENTS: [&str; 4] = ["SessionStart", "PostToolUse", "Stop", "PreCompact"];
/// Gemini's hook event names. The tool event is `AfterTool`.
pub(crate) const GEMINI_EVENTS: [&str; 4] =
    ["SessionStart", "AfterTool", "SessionEnd", "PreCompress"];
/// Codex's hook event names. The tool event is `PostToolUse`.
pub(crate) const CODEX_EVENTS: [&str; 4] = ["SessionStart", "PostToolUse", "Stop", "PreCompact"];

/// Build one command-hook entry, tagged with the sentinel marker, in the form
/// required by the target CLI.
///
/// Two command forms exist because the CLIs differ in whether they support a
/// separate `args` array:
///
/// - `exec_args == true` (Claude Code): EXEC form — `command` is the raw binary
///   path (its own JSON string) and the flags live in a separate `args` array.
///   A shell-form string would be re-tokenized by the shell, splitting a binary
///   path that contains spaces (common in macOS/Windows home dirs) mid-path and
///   failing to launch.
/// - `exec_args == false` (Gemini, Codex): INLINE form — these CLIs have **no**
///   `args` field, so a separate `args` array is silently dropped (which would
///   run `rusty-brain-hooks` WITHOUT `--agent`). The whole invocation is one
///   shell string: the binary path DOUBLE-QUOTED (to tolerate spaces) followed
///   by `--agent <id>`. No `args` key is emitted.
fn command_entry(hooks_bin: &str, agent_id: &str, exec_args: bool) -> serde_json::Value {
    if exec_args {
        serde_json::json!({
            "type": "command",
            "command": hooks_bin,
            "args": ["--agent", agent_id],
            SENTINEL: true,
        })
    } else {
        serde_json::json!({
            "type": "command",
            "command": format!("\"{hooks_bin}\" --agent {agent_id}"),
            SENTINEL: true,
        })
    }
}

/// Build one matcher-group for `event`, wrapping the [`command_entry`] for this
/// CLI. The group carries the sentinel marker; the tool event (`tool_event`)
/// additionally carries `"matcher": "*"`, while the non-tool events omit it to
/// match each CLI's schema.
pub(crate) fn command_group(
    hooks_bin: &str,
    agent_id: &str,
    event: &str,
    tool_event: &str,
    exec_args: bool,
) -> serde_json::Value {
    let entry = command_entry(hooks_bin, agent_id, exec_args);
    if event == tool_event {
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

/// Build the full `{ "hooks": { <event>: [group], ... } }` block for a CLI whose
/// config nests hooks under a top-level `hooks` key (Claude Code, Gemini, Codex).
///
/// `events` is that CLI's native event-name set, `tool_event` is the member of
/// `events` that gets the `"matcher": "*"` group, and `exec_args` selects the
/// EXEC (`true`) vs INLINE (`false`) command form (see [`command_entry`]).
pub(crate) fn hooks_block(
    hooks_bin: &str,
    agent_id: &str,
    events: &[&str],
    tool_event: &str,
    exec_args: bool,
) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for event in events {
        hooks.insert(
            (*event).to_string(),
            serde_json::Value::Array(vec![command_group(
                hooks_bin, agent_id, event, tool_event, exec_args,
            )]),
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
