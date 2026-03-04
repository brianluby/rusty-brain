/// Shell-quote a path so it remains a single token when parsed by the shell.
/// Wraps in double quotes and escapes embedded double quotes.
fn shell_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

/// Generate the hooks.json manifest content for Claude Code hook registration.
///
/// Produces a JSON string mapping hook event types to `{binary_name} <subcommand>` commands.
/// Binary paths are shell-quoted to handle spaces in paths.
#[must_use]
pub fn generate_manifest(binary_name: &str) -> String {
    let bin = shell_quote(binary_name);
    let manifest = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "type": "command",
                    "command": format!("{bin} session-start")
                }
            ],
            "PostToolUse": [
                {
                    "type": "command",
                    "command": format!("{bin} post-tool-use")
                }
            ],
            "Stop": [
                {
                    "type": "command",
                    "command": format!("{bin} stop")
                }
            ],
            "Notification": [
                {
                    "type": "command",
                    "command": format!("{bin} smart-install"),
                    "matcher": "smart-install"
                }
            ]
        }
    });

    serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string())
}
