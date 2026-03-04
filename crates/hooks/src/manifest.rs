/// Generate the hooks.json manifest content for Claude Code hook registration.
///
/// Produces a JSON string mapping hook event types to `{binary_name} <subcommand>` commands.
#[must_use]
pub fn generate_manifest(binary_name: &str) -> String {
    let manifest = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "type": "command",
                    "command": format!("{binary_name} session-start")
                }
            ],
            "PostToolUse": [
                {
                    "type": "command",
                    "command": format!("{binary_name} post-tool-use")
                }
            ],
            "Stop": [
                {
                    "type": "command",
                    "command": format!("{binary_name} stop")
                }
            ],
            "Notification": [
                {
                    "type": "command",
                    "command": format!("{binary_name} smart-install"),
                    "matcher": "smart-install"
                }
            ]
        }
    });

    serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string())
}
