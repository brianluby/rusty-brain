/// Shell-quote a path so it remains a single token when parsed by the shell.
/// Uses single-quote escaping to avoid shell expansion.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\"'\"'"))
}

/// Generate the hooks.json manifest content for Claude Code hook registration.
///
/// Produces a JSON string mapping hook event types to `{binary_name} <subcommand>` commands.
/// Binary paths are shell-quoted to handle spaces and shell metacharacters.
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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // shell_quote
    // -----------------------------------------------------------------------

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        let result = shell_quote("/usr/local/bin/rusty-brain");
        assert_eq!(result, "'/usr/local/bin/rusty-brain'");
    }

    #[test]
    fn shell_quote_escapes_single_quotes_in_path() {
        let result = shell_quote("/path/with'quote");
        assert!(
            result.contains("'\"'\"'"),
            "should escape single quotes, got: {result}"
        );
    }

    #[test]
    fn shell_quote_handles_spaces() {
        let result = shell_quote("/path with spaces/bin");
        assert_eq!(result, "'/path with spaces/bin'");
    }

    // -----------------------------------------------------------------------
    // generate_manifest
    // -----------------------------------------------------------------------

    #[test]
    fn generate_manifest_contains_session_start() {
        let manifest = generate_manifest("/usr/local/bin/rusty-brain-hooks");
        assert!(
            manifest.contains("SessionStart"),
            "manifest should contain SessionStart"
        );
    }

    #[test]
    fn generate_manifest_contains_post_tool_use() {
        let manifest = generate_manifest("/usr/local/bin/rusty-brain-hooks");
        assert!(
            manifest.contains("PostToolUse"),
            "manifest should contain PostToolUse"
        );
    }

    #[test]
    fn generate_manifest_contains_stop() {
        let manifest = generate_manifest("/usr/local/bin/rusty-brain-hooks");
        assert!(manifest.contains("Stop"), "manifest should contain Stop");
    }

    #[test]
    fn generate_manifest_contains_notification_smart_install() {
        let manifest = generate_manifest("/usr/local/bin/rusty-brain-hooks");
        assert!(
            manifest.contains("Notification"),
            "manifest should contain Notification"
        );
        assert!(
            manifest.contains("smart-install"),
            "manifest should contain smart-install"
        );
    }

    #[test]
    fn generate_manifest_is_valid_json() {
        let manifest = generate_manifest("/usr/local/bin/rusty-brain-hooks");
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&manifest);
        assert!(parsed.is_ok(), "manifest should be valid JSON");
    }

    #[test]
    fn generate_manifest_quotes_binary_path() {
        let manifest = generate_manifest("/path with spaces/bin");
        assert!(
            manifest.contains("'/path with spaces/bin'"),
            "should shell-quote the binary path, got: {manifest}"
        );
    }

    #[test]
    fn generate_manifest_uses_subcommands() {
        let manifest = generate_manifest("rusty-brain-hooks");
        assert!(manifest.contains("session-start"));
        assert!(manifest.contains("post-tool-use"));
        assert!(manifest.contains("stop"));
        assert!(manifest.contains("smart-install"));
    }
}
