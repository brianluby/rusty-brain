//! Per-CLI `AgentInstaller` implementations for the four JSON-protocol CLIs.
//!
//! Each module provides a unit struct implementing
//! [`rb_agents::install::AgentInstaller`]: `detect()` (via [`crate::detect`])
//! and the pure `hook_fragment()` that produces the sentinel-marked JSON block.

mod claude_code;
mod codex;
mod gemini;

use std::path::PathBuf;

use rb_agents::install::{AgentInstaller, SENTINEL};
use rb_types::Error;

pub use claude_code::ClaudeCodeInstaller;
pub use codex::CodexInstaller;
pub use gemini::GeminiInstaller;

/// Every built-in installer, in display order (Claude Code first — the lead
/// adapter). OpenCode is intentionally absent: it loads hooks via a JS/TS plugin
/// in `.opencode/plugins/`, not a JSON hooks block, so a JSON-writing installer
/// would be inert. OpenCode install is deferred to a follow-on; the dormant
/// `rb-agents` OpenCode adapter and `--agent opencode` hook support remain so the
/// future plugin can reuse them.
#[must_use]
pub fn builtins() -> Vec<Box<dyn AgentInstaller>> {
    vec![
        Box::new(ClaudeCodeInstaller),
        Box::new(GeminiInstaller),
        Box::new(CodexInstaller),
    ]
}

/// Claude Code's hook event names. The tool event is `PostToolUse`; `SessionEnd`
/// is the W3.1 capture point (folds the per-session scratch into one summary);
/// `UserPromptSubmit` is the W3.2(a) deterministic-recall injection point.
pub(crate) const CLAUDE_EVENTS: [&str; 6] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "Stop",
    "SessionEnd",
    "PreCompact",
];
/// Gemini's hook event names. The tool event is `AfterTool`.
pub(crate) const GEMINI_EVENTS: [&str; 4] =
    ["SessionStart", "AfterTool", "SessionEnd", "PreCompress"];
/// Codex's hook event names. The tool event is `PostToolUse`.
pub(crate) const CODEX_EVENTS: [&str; 4] = ["SessionStart", "PostToolUse", "Stop", "PreCompact"];

/// Build one command-hook entry in the documented shape shared by all three
/// CLIs: `{"type": "command", "command": "<single shell string>"}`.
///
/// `command` is ONE shell-interpreted string — none of the CLIs honor a
/// separate `args` array (Claude Code's documented hooks schema has no `args`
/// field, verified against a recorded real `settings.json`; an `args` array
/// would be silently dropped and run `rusty-brain-hooks` WITHOUT `--agent`).
/// The binary path is SHELL-QUOTED via [`shell_quote`] so spaces AND
/// metacharacters survive the CLI's shell re-tokenization, followed by
/// `--agent <id>`.
///
/// No keys beyond the documented `type`/`command` are emitted — the sentinel
/// marker lives on the enclosing matcher group (see [`command_group`]), which
/// is wholly ours, so uninstall scoping is preserved without putting an
/// unknown key inside the entry the CLI validates.
fn command_entry(hooks_bin: &str, agent_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": format!("{} --agent {agent_id}", shell_quote(hooks_bin)),
    })
}

/// Quote a binary path for the single-string `command` so it survives the
/// target CLI's shell re-tokenization intact.
///
/// The `command` string embeds the path in a single shell string that
/// the CLI runs through a shell. A naive double-quote wrap tolerates spaces but
/// still lets a POSIX shell expand `$`, backticks, and embedded quotes inside the
/// path — a correctness hole and, because the string is persisted to user config
/// and later shell-evaluated, an injection risk for unusual install locations.
///
/// - POSIX: single-quote wrap, with any embedded single quote closed, escaped,
///   and reopened (`'\''`). Single quotes disable ALL shell expansion.
/// - Windows (`cmd.exe`): double-quote wrap with embedded `"` doubled (`""`).
///
/// Only the path is quoted; `agent_id` is a fixed enum value
/// (`claude-code`/`gemini`/`codex`/`opencode`) with no metacharacters and is
/// emitted unquoted by the caller.
#[cfg(not(windows))]
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// Windows (`cmd.exe`) variant of [`shell_quote`]: double-quote wrap with
/// embedded double quotes doubled. See the POSIX variant for rationale.
#[cfg(windows)]
fn shell_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\"\""))
}

/// Build one matcher-group for `event`, wrapping the [`command_entry`] for this
/// CLI. The group carries the sentinel marker (it is created wholly by us, so
/// stripping the group removes exactly our entries); the tool event
/// (`tool_event`) additionally carries `"matcher": "*"`, while the non-tool
/// events omit it to match each CLI's schema.
pub(crate) fn command_group(
    hooks_bin: &str,
    agent_id: &str,
    event: &str,
    tool_event: &str,
) -> serde_json::Value {
    let entry = command_entry(hooks_bin, agent_id);
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
/// `events` is that CLI's native event-name set and `tool_event` is the member
/// of `events` that gets the `"matcher": "*"` group (see [`command_entry`] for
/// the shared single-shell-string command form).
pub(crate) fn hooks_block(
    hooks_bin: &str,
    agent_id: &str,
    events: &[&str],
    tool_event: &str,
) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for event in events {
        hooks.insert(
            (*event).to_string(),
            serde_json::Value::Array(vec![command_group(hooks_bin, agent_id, event, tool_event)]),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Pull the `command` string out of a `command_entry` value.
    fn command_of(entry: &serde_json::Value) -> &str {
        entry.get("command").unwrap().as_str().unwrap()
    }

    #[test]
    fn entry_has_exactly_the_documented_keys() {
        // Documented hook-entry shape for every CLI: {"type": "command",
        // "command": "<shell string>"} — no `args` array (it would be silently
        // dropped) and no unknown keys (the sentinel lives on the group, not in
        // the entry the CLI validates).
        let entry = command_entry("/bin/rusty-brain-hooks", "claude-code");
        let keys: Vec<&str> = entry
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["command", "type"],
            "exactly type+command, nothing else"
        );
        assert_eq!(entry.get("type").unwrap(), &serde_json::json!("command"));
        assert!(command_of(&entry).ends_with("--agent claude-code"));
    }

    #[test]
    fn group_carries_sentinel_for_uninstall_scoping() {
        // The matcher group is created wholly by us, so the sentinel marker on
        // the group (not the entry) is what uninstall strips.
        let group = command_group(
            "/bin/rusty-brain-hooks",
            "claude-code",
            "Stop",
            "PostToolUse",
        );
        assert_eq!(group.get(SENTINEL).unwrap(), &serde_json::json!(true));
        let inner = group.get("hooks").unwrap().as_array().unwrap();
        assert!(
            inner[0].get(SENTINEL).is_none(),
            "entry must stay documented-shape"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn clean_path_is_single_quoted_posix() {
        let entry = command_entry("/usr/local/bin/rusty-brain-hooks", "gemini");
        assert_eq!(
            command_of(&entry),
            "'/usr/local/bin/rusty-brain-hooks' --agent gemini"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn path_with_space_is_quoted_posix() {
        let entry = command_entry("/Users/jo bloggs/.local/bin/rusty-brain-hooks", "codex");
        assert_eq!(
            command_of(&entry),
            "'/Users/jo bloggs/.local/bin/rusty-brain-hooks' --agent codex"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn path_metacharacters_are_neutralized_posix() {
        // `$(...)`, backticks, and an embedded single quote must NOT survive as
        // live shell syntax. Single-quote wrapping makes everything literal except
        // the embedded single quote, which is closed/escaped/reopened as `'\''`.
        let nasty = "/opt/$(rm -rf ~)/`whoami`/r'b/hooks";
        let entry = command_entry(nasty, "gemini");
        assert_eq!(
            command_of(&entry),
            "'/opt/$(rm -rf ~)/`whoami`/r'\\''b/hooks' --agent gemini"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_quote_roundtrips_through_sh() {
        // Strongest possible check: ask a real `/bin/sh` to echo the quoted path
        // back and confirm it equals the original, byte-for-byte — i.e. no
        // expansion, splitting, or quote-mangling occurred.
        use std::process::Command;
        for original in [
            "/usr/bin/rusty-brain-hooks",
            "/Users/jo bloggs/.local/bin/rusty-brain-hooks",
            "/opt/$HOME/`id`/r'b/hooks",
            "/a\"b/hooks",
            "/tab\there/hooks",
        ] {
            let quoted = shell_quote(original);
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {quoted}"))
                .output()
                .expect("run /bin/sh");
            assert!(out.status.success(), "sh failed for {original:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                original,
                "shell_quote must round-trip {original:?} through sh unchanged"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn path_is_double_quoted_on_windows() {
        let entry = command_entry(r"C:\Program Files\rb\rusty-brain-hooks.exe", "gemini");
        assert_eq!(
            command_of(&entry),
            "\"C:\\Program Files\\rb\\rusty-brain-hooks.exe\" --agent gemini"
        );
    }
}
